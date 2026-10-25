//! Frozen storage-v6 records from finalized AGLS block 153726 (spec 119,
//! QuantumPowApi v2). This is raw-storage compatibility, not API-version dispatch.
//! Only the three sampled records are characterized; no whole-chain scan is claimed.

use codec::{Decode, DecodeAll, Encode};
use pallet_quantum_pow::{types, QBlocks};
use quip_protocol_runtime::{AccountId, Balance, BlockNumber, QuantumPow, Runtime};
use serde_json::Value;
use sp_core::{hashing::sha2_256, H256};

type Current = types::QBlock<AccountId, Balance, BlockNumber>;
type Legacy = types::v2::QBlock<AccountId, Balance, BlockNumber>;
type Stored = types::StoredQBlock<AccountId, Balance, BlockNumber>;

const FIXTURE: &[u8] = include_bytes!("fixtures/agls-qblocks-v2-153726.json");

fn fixture() -> Value {
    assert_eq!(
        hex::encode(sha2_256(FIXTURE)),
        "a22e5c00f2026309801cc05100694dcb19aadc90bd073b0ad3e7600d06ce8598"
    );
    serde_json::from_slice(FIXTURE).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap().strip_prefix("0x").unwrap()).unwrap()
}

fn array32(value: &Value) -> [u8; 32] {
    bytes(value).try_into().unwrap()
}

fn legacy_expected(entry: &Value) -> Legacy {
    // Expected fields were decoded independently by RR in Python. Do not derive
    // them from the Rust decoder or the production conversion under test.
    let d = &entry["decoded"];
    Legacy {
        miner: AccountId::from(array32(&d["miner"])),
        salt: array32(&d["salt"]),
        energy_milli: d["energy_milli"].as_i64().unwrap(),
        reward: d["reward"].as_str().unwrap().parse().unwrap(),
        submitted_at: d["submitted_at"].as_u64().unwrap().try_into().unwrap(),
        difficulty: types::v2::DifficultyConfig {
            min_solutions: d["difficulty"]["min_solutions"]
                .as_u64()
                .unwrap()
                .try_into()
                .unwrap(),
            max_energy_milli: d["difficulty"]["max_energy_milli"].as_i64().unwrap(),
            min_diversity_milli: d["difficulty"]["min_diversity_milli"]
                .as_u64()
                .unwrap()
                .try_into()
                .unwrap(),
        },
        last_proof_block_hash: H256::from(array32(&d["last_proof_block_hash"])),
        topology_hash: H256::from(array32(&d["topology_hash"])),
        device_access_time_us: d["device_access_time_us"].as_u64().unwrap(),
    }
}

fn current_expected(old: &Legacy) -> Current {
    // Spell out the compatibility contract instead of using From<v2::QBlock>.
    Current {
        miner: old.miner.clone(),
        salt: old.salt,
        energy_milli: old.energy_milli,
        reward: old.reward,
        submitted_at: old.submitted_at,
        difficulty: types::DifficultyConfig {
            max_energy_milli: old.difficulty.max_energy_milli,
        },
        last_proof_block_hash: old.last_proof_block_hash,
        topology_hash: old.topology_hash,
        device_access_time_us: old.device_access_time_us,
        // Zero means unmeasured in these historical records, not a measured
        // unfrustrated instance. The old threshold D is retained, not D-1;
        // historical admission was strict E<D, unlike current inclusive E<=B.
        frustration_milli: 0,
        instance_bar_milli: old.difficulty.max_energy_milli,
    }
}

#[test]
fn historical_qblocks_decode_with_runtime_types_and_preserve_every_field() {
    let fixture = fixture();
    let entries = fixture["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for entry in entries {
        let raw = bytes(&entry["raw_value"]);
        assert_eq!(raw.len(), 180);
        assert_eq!(entry["len"].as_u64(), Some(180));
        assert_eq!(
            hex::encode(sha2_256(&raw)),
            entry["sha256"].as_str().unwrap()
        );
        let old = legacy_expected(entry);
        assert_eq!(Legacy::decode_all(&mut &raw[..]).unwrap(), old);
        assert_eq!(old.encode(), raw);
        assert!(Current::decode_all(&mut &raw[..]).is_err());
        assert!(old.energy_milli < old.difficulty.max_energy_milli);
        assert!(old.device_access_time_us > 0);
        if old.submitted_at == 1006 {
            assert_eq!(
                old.last_proof_block_hash,
                H256::from(array32(&fixture["genesis"]))
            );
        }

        let expected = current_expected(&old);
        let stored = Stored::decode_all(&mut &raw[..]).unwrap();
        assert_eq!(stored.0, expected);
        let encoded = stored.encode();
        assert_eq!(encoded.len(), 184);
        // Independent byte layout: drop legacy min_solutions/min_diversity,
        // retain the energy bar, append unmeasured frustration and the old bar.
        let expected_bytes = [
            &raw[..92],
            &raw[96..104],
            &raw[108..],
            &[0u8; 4],
            &raw[96..104],
        ]
        .concat();
        assert_eq!(encoded, expected_bytes);
        assert_eq!(Current::decode_all(&mut &encoded[..]).unwrap(), expected);
        assert_eq!(Stored::decode_all(&mut &encoded[..]).unwrap(), stored);
        assert!(Legacy::decode_all(&mut &encoded[..]).is_err());
    }
}

#[test]
fn runtime_storage_reads_legacy_and_current_qblocks_without_rewriting_history() {
    let fixture = fixture();
    sp_io::TestExternalities::default().execute_with(|| {
        for entry in fixture["entries"].as_array().unwrap() {
            let block: BlockNumber = entry["qblock_block_number"]
                .as_u64()
                .unwrap()
                .try_into()
                .unwrap();
            let key = bytes(&entry["key"]);
            let raw = bytes(&entry["raw_value"]);
            let expected = current_expected(&legacy_expected(entry));
            assert_eq!(QBlocks::<Runtime>::hashed_key_for(block), key);
            sp_io::storage::set(&key, &raw);
            assert_eq!(QBlocks::<Runtime>::get(block).unwrap().0, expected);
            assert_eq!(QuantumPow::qblock_at(block), Some(expected.clone()));
            assert_eq!(sp_io::storage::get(&key).unwrap().as_ref(), raw.as_slice());

            // A later write uses the current encoding, including nonzero new
            // fields. It must take the current branch, not legacy backfilling.
            let mut current = expected;
            current.frustration_milli = 501;
            current.instance_bar_milli = -14_800_001;
            QBlocks::<Runtime>::insert(block, types::StoredQBlock(current.clone()));
            assert_eq!(sp_io::storage::get(&key).unwrap().len(), 184);
            assert_eq!(QuantumPow::qblock_at(block), Some(current));
        }
    });
}

#[test]
fn stored_qblocks_reject_unsupported_lengths_and_require_a_bounded_input() {
    struct UnknownLength<'a>(&'a [u8]);
    impl codec::Input for UnknownLength<'_> {
        fn remaining_len(&mut self) -> Result<Option<usize>, codec::Error> {
            Ok(None)
        }
        fn read(&mut self, into: &mut [u8]) -> Result<(), codec::Error> {
            codec::Input::read(&mut self.0, into)
        }
    }
    for entry in fixture()["entries"].as_array().unwrap() {
        let old = bytes(&entry["raw_value"]);
        let current = current_expected(&legacy_expected(entry)).encode();
        for raw in [&old, &current] {
            // 180 bytes is itself a supported legacy layout: truncating a
            // current value to that size cannot generally be detected without
            // a format tag. Do not claim integrity validation of scalar bytes.
            for len in 0..raw.len() {
                if len != 180 {
                    assert!(
                        Stored::decode_all(&mut &raw[..len]).is_err(),
                        "length {len}"
                    );
                }
            }
            let mut trailing = raw.clone();
            trailing.push(0);
            assert!(Stored::decode_all(&mut &trailing[..]).is_err());
            let mut unknown = UnknownLength(raw);
            assert!(Stored::decode(&mut unknown).is_err());
            assert_eq!(unknown.0, raw.as_slice()); // rejected before reading
        }
    }
}

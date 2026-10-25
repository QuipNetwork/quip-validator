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

#[test]
fn storage_v6_upgrade_preserves_the_pinned_graph_round_and_sampled_history() {
    use frame_support::traits::{Hooks, StorageVersion};
    use pallet_quantum_pow as pow;
    use quantum_validation::{generate_ising_model_and_frustration, AllowedValueSpec};

    let snapshot: Value =
        serde_json::from_str(include_str!("fixtures/agls-migration-153726.json")).unwrap();
    let topology_raw = include_bytes!("fixtures/agls-topology-153726.scale");
    assert_eq!(topology_raw.len(), 350_456);
    assert_eq!(
        hex::encode(sha2_256(topology_raw)),
        "d7ff6acd88cc31a81f2c17e164756b7041f8dc5fac159af1bee5e20498ba0f8d"
    );
    let history = fixture();
    assert_eq!(snapshot["at_block_hash"], history["at_block_hash"]);
    let hash = H256::from(array32(&snapshot["topology_hash"]));
    sp_io::TestExternalities::default().execute_with(|| {
        quip_protocol_runtime::System::set_block_number(153_727);
        let topology_key = bytes(&snapshot["topology_key"]);
        assert_eq!(
            pow::RegisteredTopologies::<Runtime>::hashed_key_for(hash),
            topology_key
        );
        sp_io::storage::set(&topology_key, topology_raw);
        let before = pow::RegisteredTopologies::<Runtime>::get(hash).unwrap();
        assert_eq!((before.nodes.len(), before.edges.len()), (4_577, 41_514));
        assert_eq!(before.registered_at, 87);
        assert_eq!(
            before.allowed_h_values.as_slice(),
            AllowedValueSpec::Set(&[0][..])
        );
        assert_eq!(
            before.allowed_j_values.as_slice(),
            AllowedValueSpec::Set(&[-1000, 1000][..])
        );
        assert_eq!(
            before.allowed_spin_values.as_slice(),
            AllowedValueSpec::Set(&[-1000, 1000][..])
        );
        assert_eq!(
            quantum_validation::cycle_rank(&before.nodes, &before.edges),
            36_938
        );
        assert_eq!(
            pow::topology::hash_topology(
                &before.nodes,
                &before.edges,
                &before.allowed_h_values.as_slice(),
                &before.allowed_j_values.as_slice(),
                &before.allowed_spin_values.as_slice(),
            ),
            hash
        );
        // These small values and absences were read at the same pinned block.
        for (name, key) in [
            (
                "difficulty",
                pow::Difficulties::<Runtime>::hashed_key_for(hash),
            ),
            (
                "last_proof_block",
                pow::LastProofBlock::<Runtime>::hashed_key().to_vec(),
            ),
            (
                "last_proof_block_hash",
                pow::LastProofBlockHash::<Runtime>::hashed_key().to_vec(),
            ),
            (
                "default_topology",
                pow::DefaultTopology::<Runtime>::hashed_key().to_vec(),
            ),
            (
                "topology_dims",
                pow::TopologyDims::<Runtime>::hashed_key_for(hash),
            ),
            (
                "topology_hardness",
                pow::TopologyHardnessOf::<Runtime>::hashed_key_for(hash),
            ),
            (
                "difficulty_set_at",
                pow::DifficultySetAt::<Runtime>::hashed_key().to_vec(),
            ),
        ] {
            let value = &snapshot["state"][name];
            assert_eq!(key, bytes(&value["key"]), "{name}");
            if !value["raw_value"].is_null() {
                sp_io::storage::set(&key, &bytes(&value["raw_value"]));
            } else {
                assert!(sp_io::storage::get(&key).is_none());
            }
        }
        let difficulty_key = pow::Difficulties::<Runtime>::hashed_key_for(hash);
        let old_difficulty = sp_io::storage::get(&difficulty_key).unwrap();
        assert_eq!(old_difficulty.len(), 16);
        assert_eq!(
            types::v2::DifficultyConfig::decode_all(&mut &old_difficulty[..]).unwrap(),
            types::v2::DifficultyConfig {
                min_solutions: 1,
                max_energy_milli: -14_754_648,
                min_diversity_milli: 0,
            }
        );
        let round_hash = pow::LastProofBlockHash::<Runtime>::get();
        assert_eq!(pow::LastProofBlock::<Runtime>::get(), 153_634);
        assert_eq!(pow::DefaultTopology::<Runtime>::get(), Some(hash));

        let mut entries = history["entries"]
            .as_array()
            .unwrap()
            .iter()
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry["qblock_block_number"].as_u64().unwrap());
        for (index, entry) in entries.iter().enumerate() {
            let old = legacy_expected(entry);
            sp_io::storage::set(&bytes(&entry["key"]), &bytes(&entry["raw_value"]));
            // Synthetic indices/counter for this three-record sample. These
            // are NOT claimed to be the full chain's QBlock ids or count.
            let id = index as u64 + 1;
            pow::QBlockBlockById::<Runtime>::insert(id, old.submitted_at);
            pow::QBlockIdByBlock::<Runtime>::insert(old.submitted_at, id);
        }
        pow::QBlockCount::<Runtime>::put(3);
        // Synthetic transient bytes test unconditional stale-record cleanup,
        // without pretending to have captured an in-block winning proof.
        let best_key = pow::BlockBestProof::<Runtime>::hashed_key();
        sp_io::storage::set(&best_key, &[0xff]);
        StorageVersion::new(6).put::<QuantumPow>();

        // A deterministic candidate in the pinned round, not a historical
        // submitted proof: use one sampled miner with an explicit test salt.
        let miner = legacy_expected(entries[0]).miner;
        let salt = [7; 32];
        let nonce = quantum_validation::derive_nonce(
            &round_hash.0,
            &QuantumPow::account_to_bytes(&miner),
            &salt,
        );
        let instance_before = generate_ising_model_and_frustration(
            nonce,
            &before.nodes,
            &before.edges,
            &before.allowed_h_values.as_slice(),
            &before.allowed_j_values.as_slice(),
        )
        .unwrap();
        assert_eq!(instance_before.frustration_cycles, 36_938);
        assert_eq!(
            quantum_validation::energy_bound_milli(&instance_before.h, &instance_before.j),
            41_514_000
        );

        #[cfg(feature = "try-runtime")]
        let pre = QuantumPow::pre_upgrade().unwrap();
        QuantumPow::on_runtime_upgrade();
        #[cfg(feature = "try-runtime")]
        QuantumPow::post_upgrade(pre).unwrap();

        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(7));
        assert_eq!(
            pow::Difficulties::<Runtime>::get(hash)
                .unwrap()
                .max_energy_milli,
            -14_754_648
        );
        assert_eq!(
            sp_io::storage::get(&difficulty_key).unwrap().as_ref(),
            &(-14_754_648i64).to_le_bytes()
        );
        assert!(sp_io::storage::get(&best_key).is_none());
        assert_eq!(pow::QBlockCount::<Runtime>::get(), 3);
        assert_eq!(QBlocks::<Runtime>::iter_keys().count(), 3);
        assert_eq!(QBlocks::<Runtime>::iter().count(), 3); // every value must decode
        for (index, entry) in entries.iter().enumerate() {
            let old = legacy_expected(entry);
            assert_eq!(
                QuantumPow::qblock_at(old.submitted_at),
                Some(current_expected(&old))
            );
            assert_eq!(
                sp_io::storage::get(&bytes(&entry["key"])).unwrap().as_ref(),
                bytes(&entry["raw_value"]).as_slice()
            ); // legacy 180-byte history is not rewritten
            let id = index as u64 + 1;
            assert_eq!(
                pow::QBlockBlockById::<Runtime>::get(id),
                Some(old.submitted_at)
            );
            assert_eq!(
                pow::QBlockIdByBlock::<Runtime>::get(old.submitted_at),
                Some(id)
            );
        }
        assert_eq!(
            pow::TopologyDims::<Runtime>::get(hash),
            Some(pow::TopologyDim {
                nodes: 4_577,
                edges: 41_514
            })
        );
        assert_eq!(
            pow::TopologyHardnessOf::<Runtime>::get(hash),
            Some(types::TopologyHardness {
                residual_difficulty: u32::MAX,
                core_width: u32::MAX,
                expected_frustration_milli: 500,
            })
        );
        assert_eq!(
            QuantumPow::regime_of(hash),
            Some(quantum_validation::Regime::Heuristic)
        );
        assert_eq!(pow::DefaultTopology::<Runtime>::get(), Some(hash));
        assert_eq!(pow::LastProofBlock::<Runtime>::get(), 153_634);
        assert_eq!(pow::LastProofBlockHash::<Runtime>::get(), round_hash);
        assert!(!pow::DifficultySetAt::<Runtime>::exists());
        assert!(QuantumPow::topology_dims_consistent());

        let after = pow::RegisteredTopologies::<Runtime>::get(hash).unwrap();
        assert_eq!(after, before);
        assert_eq!(
            sp_io::storage::get(&topology_key).unwrap().as_ref(),
            topology_raw
        );
        let after_nonce = quantum_validation::derive_nonce(
            &pow::LastProofBlockHash::<Runtime>::get().0,
            &QuantumPow::account_to_bytes(&miner),
            &salt,
        );
        assert_eq!(after_nonce, nonce);
        assert_eq!(
            generate_ising_model_and_frustration(
                after_nonce,
                &after.nodes,
                &after.edges,
                &after.allowed_h_values.as_slice(),
                &after.allowed_j_values.as_slice(),
            )
            .unwrap(),
            instance_before
        );

        let root = sp_io::storage::root(sp_runtime::StateVersion::V1);
        QuantumPow::on_runtime_upgrade();
        assert_eq!(sp_io::storage::root(sp_runtime::StateVersion::V1), root);
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn migration_postcheck_rejects_undecodable_history_even_when_keys_survive() {
    use frame_support::traits::{Hooks, StorageVersion};

    sp_io::TestExternalities::default().execute_with(|| {
        let entry = &fixture()["entries"][0];
        let mut raw = bytes(&entry["raw_value"]);
        raw.push(0); // unsupported 181-byte layout
        sp_io::storage::set(&bytes(&entry["key"]), &raw);
        StorageVersion::new(6).put::<QuantumPow>();
        let pre = QuantumPow::pre_upgrade().unwrap();
        QuantumPow::on_runtime_upgrade();
        assert_eq!(QBlocks::<Runtime>::iter_keys().count(), 1);
        assert!(QuantumPow::qblock_at(1006).is_none());
        assert_eq!(
            QuantumPow::post_upgrade(pre),
            Err(sp_runtime::DispatchError::Other(
                "every QBlocks entry must read in the current or the v6 layout"
            ))
        );
    });
}

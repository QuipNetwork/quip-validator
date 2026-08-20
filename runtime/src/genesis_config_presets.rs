// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    AccountId, BalancesConfig, QuantumComputeMempoolConfig, RuntimeGenesisConfig, SessionConfig,
    SessionKeys, SudoConfig, BABE_GENESIS_EPOCH_CONFIG,
};
use alloc::{vec, vec::Vec};
use frame_support::build_struct_json_patch;
use quip_crypto_primitives::substrate::ed25519_mldsa44::{
    Pair as HybridGrandpaPair, Public as HybridGrandpaPublic,
};
use quip_crypto_primitives::substrate::sr25519_mldsa44::{
    Pair as HybridBabePair, Public as HybridBabePublic,
};
use quip_transaction_crypto::{account_id_from_public, HybridPair as HybridTxPair};
use serde_json::Value;
use sp_consensus_babe::AuthorityId as BabeId;
use sp_consensus_grandpa::AuthorityId as GrandpaId;
use sp_core::crypto::ByteArray;
use sp_core::Pair as _;
use sp_genesis_builder::{self, PresetId};
use sp_keyring::Ed25519Keyring;
use sp_keyring::Sr25519Keyring;

pub const LOCAL_THREE_VALIDATOR_RUNTIME_PRESET: &str = "local_three_validator";

/// Identifier for the public quip-testnet genesis preset.
///
/// The raw chain spec exported from this preset is the canonical
/// `quip-testnet.json` published by `nodes.quip.network`. The preset itself
/// is kept in the binary so the genesis can be re-derived and audited.
pub const QUIP_TESTNET_RUNTIME_PRESET: &str = "quip_testnet";

fn babe_authority_from_seed(seed: &str) -> BabeId {
    HybridBabePair::from_string(seed, None)
        .expect("well-known dev seeds are valid for hybrid BABE authorities")
        .public()
        .into()
}

fn grandpa_authority_from_seed(seed: &str) -> GrandpaId {
    HybridGrandpaPair::from_string(seed, None)
        .expect("well-known dev seeds are valid for hybrid GRANDPA authorities")
        .public()
        .into()
}

fn tx_account_from_seed(seed: &str) -> AccountId {
    let pair = HybridTxPair::from_string(seed, None)
        .expect("well-known dev seeds are valid for hybrid transaction accounts");
    account_id_from_public(&pair.public())
}

/// Parse a hex string (with or without `0x` prefix, leading/trailing whitespace
/// from `include_str!`-loaded files is tolerated) into the raw byte vector.
///
/// `source` is the human-readable origin (e.g. the operator hex filename); it
/// is interpolated into the panic message so a malformed operator-supplied
/// file is identifiable from the runtime panic alone.
fn decode_hex(hex: &str, source: &str) -> Vec<u8> {
    sp_core::bytes::from_hex(hex.trim())
        .unwrap_or_else(|e| panic!("{source}: malformed hex: {e:?}"))
}

/// Build a BABE authority id from raw hybrid public key bytes.
///
/// The bytes must be the SCALE-encoded `sr25519_mldsa44::Public` (sr25519 32-byte
/// prefix followed by the ML-DSA-44 public key). Used by [`quip_testnet_config_genesis`]
/// to commit operator-submitted public material directly into genesis.
fn babe_authority_from_public_hex(hex: &str, source: &str) -> BabeId {
    HybridBabePublic::from_slice(&decode_hex(hex, source))
        .unwrap_or_else(|_| panic!("{source}: hybrid BABE public has wrong byte length"))
        .into()
}

/// Build a GRANDPA authority id from raw hybrid public key bytes.
fn grandpa_authority_from_public_hex(hex: &str, source: &str) -> GrandpaId {
    HybridGrandpaPublic::from_slice(&decode_hex(hex, source))
        .unwrap_or_else(|_| panic!("{source}: hybrid GRANDPA public has wrong byte length"))
        .into()
}

/// Build a transaction account id from its 32-byte raw hex (the `tx_account_hex`
/// emitted by `derive_genesis_keys`).
fn tx_account_from_hex(hex: &str, source: &str) -> AccountId {
    let bytes = decode_hex(hex, source);
    let array: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("{source}: tx account hex must decode to exactly 32 bytes"));
    AccountId::new(array)
}

// Returns the genesis config presets populated with given parameters.
//
// Each authority is a triple of `(account, babe, grandpa)`. The same account is
// used as both validator stash and controller in `pallet-session`, which is
// fine for v0.2 where staking is not wired in.
fn testnet_genesis(
    initial_authorities: Vec<(AccountId, BabeId, GrandpaId)>,
    endowed_accounts: Vec<AccountId>,
    root: AccountId,
    chain_id: u64,
) -> Value {
    build_struct_json_patch!(RuntimeGenesisConfig {
        evm_chain_id: pallet_evm_chain_id::GenesisConfig {
            chain_id,
            ..Default::default()
        },
        balances: BalancesConfig {
            balances: endowed_accounts
                .iter()
                .cloned()
                .map(|k| (k, 1u128 << 60))
                .collect::<Vec<_>>(),
        },
        // The authority sets for BABE and GRANDPA are populated by
        // `pallet_session::GenesisConfig::build` via the registered
        // `OneSessionHandler::on_genesis_session` impls (see `SessionKeys`
        // in `lib.rs` and `pallet_session::Config::SessionHandler` in
        // `configs/mod.rs`). Setting `babe.authorities` / `grandpa.authorities`
        // here as well would call `initialize_genesis_authorities` twice and
        // panic with "Authorities are already initialized!" — only the BABE
        // epoch config needs to be patched in.
        babe: pallet_babe::GenesisConfig {
            epoch_config: BABE_GENESIS_EPOCH_CONFIG,
            ..Default::default()
        },
        session: SessionConfig {
            keys: initial_authorities
                .iter()
                .map(|(account, babe, grandpa)| {
                    (
                        account.clone(),
                        account.clone(),
                        SessionKeys {
                            babe: babe.clone(),
                            grandpa: grandpa.clone(),
                        },
                    )
                })
                .collect::<Vec<_>>(),
            ..Default::default()
        },
        // Must match QUANTUM_DEFAULT_JOB_SPEC_BUILDER_SS58 on the canonical testnet
        quantum_compute_mempool: QuantumComputeMempoolConfig {
            default_ising_spec_builder: Some(root.clone()),
        },
        sudo: SudoConfig { key: Some(root) },
    })
}

/// Return the development genesis config.
pub fn development_config_genesis() -> Value {
    testnet_genesis(
        vec![(
            tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
            babe_authority_from_seed(&Sr25519Keyring::Alice.to_seed()),
            grandpa_authority_from_seed(&Ed25519Keyring::Alice.to_seed()),
        )],
        vec![
            tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
            tx_account_from_seed(&Sr25519Keyring::Bob.to_seed()),
            tx_account_from_seed(&Sr25519Keyring::AliceStash.to_seed()),
            tx_account_from_seed(&Sr25519Keyring::BobStash.to_seed()),
        ],
        tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
    )
}

/// Return the local genesis config preset.
pub fn local_config_genesis() -> Value {
    testnet_genesis(
        vec![
            (
                tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
                babe_authority_from_seed(&Sr25519Keyring::Alice.to_seed()),
                grandpa_authority_from_seed(&Ed25519Keyring::Alice.to_seed()),
            ),
            (
                tx_account_from_seed(&Sr25519Keyring::Bob.to_seed()),
                babe_authority_from_seed(&Sr25519Keyring::Bob.to_seed()),
                grandpa_authority_from_seed(&Ed25519Keyring::Bob.to_seed()),
            ),
        ],
        Sr25519Keyring::iter()
            .filter(|v| v != &Sr25519Keyring::One && v != &Sr25519Keyring::Two)
            .map(|v| tx_account_from_seed(&v.to_seed()))
            .collect::<Vec<_>>(),
        tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
    )
}

/// Return the three-validator local genesis config preset.
pub fn local_three_validator_config_genesis() -> Value {
    testnet_genesis(
        vec![
            (
                tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
                babe_authority_from_seed(&Sr25519Keyring::Alice.to_seed()),
                grandpa_authority_from_seed(&Ed25519Keyring::Alice.to_seed()),
            ),
            (
                tx_account_from_seed(&Sr25519Keyring::Bob.to_seed()),
                babe_authority_from_seed(&Sr25519Keyring::Bob.to_seed()),
                grandpa_authority_from_seed(&Ed25519Keyring::Bob.to_seed()),
            ),
            (
                tx_account_from_seed(&Sr25519Keyring::Charlie.to_seed()),
                babe_authority_from_seed(&Sr25519Keyring::Charlie.to_seed()),
                grandpa_authority_from_seed(&Ed25519Keyring::Charlie.to_seed()),
            ),
        ],
        Sr25519Keyring::iter()
            .filter(|v| v != &Sr25519Keyring::One && v != &Sr25519Keyring::Two)
            .map(|v| tx_account_from_seed(&v.to_seed()))
            .collect::<Vec<_>>(),
        tx_account_from_seed(&Sr25519Keyring::Alice.to_seed()),
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
    )
}

/// Return the public quip-testnet genesis config preset.
///
/// Each authority slot is held by an independent operator who generated their
/// own libp2p node-key and hybrid BABE/GRANDPA keys offline (see
/// [`scripts/derive-operator-keys.sh`] and [`docs/testnet-keys.md`]). Only the
/// public bytes are committed in this repository; private material lives on
/// each operator's host.
///
/// The endowed set is intentionally identical to the authority set for v0.2.0
/// — there is no separate faucet account yet. Sudo is held by operator 1; a
/// migration to a multisig is tracked for a later release.
pub fn quip_testnet_config_genesis() -> Value {
    let op1_babe = babe_authority_from_public_hex(
        include_str!("genesis_quip_testnet/operator_1_babe.hex"),
        "genesis_quip_testnet/operator_1_babe.hex",
    );
    let op1_grandpa = grandpa_authority_from_public_hex(
        include_str!("genesis_quip_testnet/operator_1_grandpa.hex"),
        "genesis_quip_testnet/operator_1_grandpa.hex",
    );
    let op2_babe = babe_authority_from_public_hex(
        include_str!("genesis_quip_testnet/operator_2_babe.hex"),
        "genesis_quip_testnet/operator_2_babe.hex",
    );
    let op2_grandpa = grandpa_authority_from_public_hex(
        include_str!("genesis_quip_testnet/operator_2_grandpa.hex"),
        "genesis_quip_testnet/operator_2_grandpa.hex",
    );
    let op3_babe = babe_authority_from_public_hex(
        include_str!("genesis_quip_testnet/operator_3_babe.hex"),
        "genesis_quip_testnet/operator_3_babe.hex",
    );
    let op3_grandpa = grandpa_authority_from_public_hex(
        include_str!("genesis_quip_testnet/operator_3_grandpa.hex"),
        "genesis_quip_testnet/operator_3_grandpa.hex",
    );

    let op1_account = tx_account_from_hex(
        "c6cb8a79a71b11347a7ce0d983104278c0682dc70b7f90be9afd92ab54f1404b",
        "operator_1 tx_account_hex literal",
    );
    let op2_account = tx_account_from_hex(
        "96ab60c5a90f6b18566155d2187fae8f52e3cd43627fb4a40d5c89f3a512bb5b",
        "operator_2 tx_account_hex literal",
    );
    let op3_account = tx_account_from_hex(
        "f8a5d50a6b32c3784b1e9fd9811e57b63524e5ec0defaafc289304bf99061db7",
        "operator_3 tx_account_hex literal",
    );

    testnet_genesis(
        vec![
            (op1_account.clone(), op1_babe, op1_grandpa),
            (op2_account.clone(), op2_babe, op2_grandpa),
            (op3_account.clone(), op3_babe, op3_grandpa),
        ],
        vec![op1_account.clone(), op2_account, op3_account],
        op1_account,
        pallet_evm_chain_id::TESTNET_CHAIN_ID,
    )
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_config_genesis(),
        LOCAL_THREE_VALIDATOR_RUNTIME_PRESET => local_three_validator_config_genesis(),
        QUIP_TESTNET_RUNTIME_PRESET => quip_testnet_config_genesis(),
        _ => return None,
    };
    Some(
        serde_json::to_string(&patch)
            .expect("serialization to json is expected to work. qed.")
            .into_bytes(),
    )
}

/// List of supported presets.
pub fn preset_names() -> Vec<PresetId> {
    vec![
        PresetId::from(sp_genesis_builder::DEV_RUNTIME_PRESET),
        PresetId::from(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET),
        PresetId::from(LOCAL_THREE_VALIDATOR_RUNTIME_PRESET),
        PresetId::from(QUIP_TESTNET_RUNTIME_PRESET),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
    use frame_support::genesis_builder_helper::build_state;

    /// Pinned hex of `tx_account_from_seed("//Alice")`. Acts as a canary for
    /// silent changes to `quip_transaction_crypto::ACCOUNT_ID_DOMAIN` or the
    /// H3 keyring derivation: any such change re-keys every account at
    /// genesis, and this constant is the cheapest grep target for catching
    /// that regression.
    const ALICE_PINNED_ACCOUNT_HEX: &str =
        "504c921d4b618d2cbb53ebebfbc98db585b325c355259545739daafb3146cdb4";

    fn hex_encode(bytes: &[u8]) -> alloc::string::String {
        const TABLE: &[u8; 16] = b"0123456789abcdef";
        let mut out = alloc::string::String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(TABLE[(b >> 4) as usize] as char);
            out.push(TABLE[(b & 0xF) as usize] as char);
        }
        out
    }

    // Recursively merges `patch` into `target` the same way `sc-chain-spec`
    // does before calling the runtime's `GenesisBuilder::build_state`. The
    // runtime presets return a *patch* (only the fields the preset touches),
    // but `build_state` needs a *full* config — without this merge, the
    // deserialise step fails with "missing field `system`" before we ever
    // reach the panic we are guarding against.
    fn merge_json(target: &mut Value, patch: Value) {
        use serde_json::map::Entry;
        match (target, patch) {
            (Value::Object(t), Value::Object(p)) => {
                for (k, v) in p {
                    match t.entry(k) {
                        Entry::Occupied(mut e) => merge_json(e.get_mut(), v),
                        Entry::Vacant(e) => {
                            e.insert(v);
                        }
                    }
                }
            }
            (t, p) => *t = p,
        }
    }

    // Exercise the same `build_state` path the runtime API uses for genesis
    // construction. The constructor-only assertions this replaces never
    // touched `BuildGenesisConfig::build`, so they happily returned valid
    // JSON while the storage build panicked on `pallet-babe` /
    // `pallet-session` double-initialisation of the authority set.
    fn assert_preset_builds_storage_with_chain_id(patch: Value, expected_chain_id: u64) {
        use frame_support::traits::Get;

        let mut full = serde_json::to_value(crate::RuntimeGenesisConfig::default())
            .expect("default runtime genesis config serialises");
        merge_json(&mut full, patch);
        let bytes = serde_json::to_vec(&full).expect("merged runtime config serialises");
        sp_io::TestExternalities::new_empty().execute_with(|| {
            build_state::<crate::RuntimeGenesisConfig>(bytes)
                .expect("genesis preset builds storage without panic");
            assert_eq!(
                <pallet_evm_chain_id::ChainId<crate::Runtime> as Get<u64>>::get(),
                expected_chain_id
            );
        });
    }

    #[test]
    fn development_preset_builds() {
        assert_preset_builds_storage_with_chain_id(
            development_config_genesis(),
            pallet_evm_chain_id::LOCAL_CHAIN_ID,
        );
    }

    #[test]
    fn local_preset_builds() {
        assert_preset_builds_storage_with_chain_id(
            local_config_genesis(),
            pallet_evm_chain_id::LOCAL_CHAIN_ID,
        );
    }

    #[test]
    fn local_three_validator_preset_builds() {
        assert_preset_builds_storage_with_chain_id(
            local_three_validator_config_genesis(),
            pallet_evm_chain_id::LOCAL_CHAIN_ID,
        );
    }

    #[test]
    fn quip_testnet_preset_builds() {
        assert_preset_builds_storage_with_chain_id(
            quip_testnet_config_genesis(),
            pallet_evm_chain_id::TESTNET_CHAIN_ID,
        );
    }

    #[test]
    fn quip_testnet_preset_is_registered() {
        let json = get_preset(&PresetId::from(QUIP_TESTNET_RUNTIME_PRESET));
        assert!(json.is_some(), "quip_testnet preset must be registered");
        let bytes = json.unwrap();
        assert!(!bytes.is_empty(), "preset must produce non-empty JSON");
    }

    /// Pinned operator-1 account hex. Catches silent breaks in either the
    /// hybrid public-key wire format or the `account_id_from_public` derivation
    /// (which would re-key every genesis account and brick the testnet).
    const OPERATOR_1_PINNED_ACCOUNT_HEX: &str =
        "c6cb8a79a71b11347a7ce0d983104278c0682dc70b7f90be9afd92ab54f1404b";

    #[test]
    fn quip_testnet_operator_1_account_is_pinned() {
        // Drive the same derivation `quip_testnet_config_genesis` uses: parse
        // the committed BABE public bytes, then derive the account id via
        // `account_id_from_public`. A regression in either step (hybrid public
        // wire format, account id domain separator) re-keys every operator at
        // genesis, so this assertion is the load-bearing canary.
        let op1_babe = HybridBabePublic::from_slice(&decode_hex(
            include_str!("genesis_quip_testnet/operator_1_babe.hex"),
            "genesis_quip_testnet/operator_1_babe.hex",
        ))
        .expect("operator_1_babe.hex must decode to a valid hybrid BABE public");
        let derived = account_id_from_public(&op1_babe);
        let derived_hex = hex_encode(&derived.encode());
        assert_eq!(
            derived_hex, OPERATOR_1_PINNED_ACCOUNT_HEX,
            "operator-1 account derivation drift. If intentional, update \
             OPERATOR_1_PINNED_ACCOUNT_HEX and the committed operator hex files \
             together: {derived_hex}",
        );
    }

    #[test]
    fn alice_account_id_is_pinned() {
        let alice = tx_account_from_seed(&Sr25519Keyring::Alice.to_seed());
        let hex = hex_encode(&alice.encode());
        assert_eq!(
            hex, ALICE_PINNED_ACCOUNT_HEX,
            "Alice's derived account id changed. If this is intentional, update \
             ALICE_PINNED_ACCOUNT_HEX above with the new value: {hex}",
        );
    }
}

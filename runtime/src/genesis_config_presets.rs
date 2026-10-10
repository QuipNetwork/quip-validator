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
use quip_crypto_primitives::substrate::ed25519_fndsa512::{
    Pair as HybridGrandpaPair, Public as HybridGrandpaPublic,
};
use quip_crypto_primitives::substrate::sr25519_fndsa512::{
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
pub const REHEARSAL_RUNTIME_PRESET: &str = "validator_rehearsal";

/// Identifier for the public quip-testnet genesis preset.
///
/// The raw chain spec exported from this preset is the canonical
/// `aglais-network.json` published by `nodes.quip.network`. The preset itself
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
/// The bytes must be the 929-byte SCALE-encoded `sr25519_fndsa512::Public`
/// (sr25519 32-byte prefix followed by the FN-DSA-512 public key). Used by
/// [`quip_testnet_config_genesis`] to commit operator-submitted public material
/// directly into genesis.
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
// used as both stash and controller in classic staking.
struct GenesisRoles {
    foundation_members: Vec<AccountId>,
    faucet_authority: AccountId,
    sudo_key: Option<AccountId>,
    ising_spec_builder: AccountId,
}

// Small local presets retain their single-key convenience. Rehearsal and the
// public testnet explicitly assign each role instead.
fn development_roles() -> GenesisRoles {
    let alice = tx_account_from_seed(&Sr25519Keyring::Alice.to_seed());
    GenesisRoles {
        foundation_members: vec![alice.clone()],
        faucet_authority: alice.clone(),
        sudo_key: Some(alice.clone()),
        ising_spec_builder: alice,
    }
}

fn testnet_genesis(
    initial_authorities: Vec<(AccountId, BabeId, GrandpaId)>,
    endowed_accounts: Vec<AccountId>,
    roles: GenesisRoles,
    chain_id: u64,
    minimum_validator_count: u32,
) -> Value {
    assert!(
        !initial_authorities.is_empty() && initial_authorities.len() <= 32,
        "authority count must be 1..=32"
    );
    assert!((1..=initial_authorities.len() as u32).contains(&minimum_validator_count));
    let approved: Vec<_> = initial_authorities
        .iter()
        .map(|(who, _, _)| who.clone())
        .collect();
    let unique: alloc::collections::BTreeSet<_> = approved.iter().collect();
    assert_eq!(unique.len(), approved.len(), "duplicate validator stash");
    assert!(
        approved.iter().all(|who| endowed_accounts.contains(who)),
        "every validator must be endowed above its bond"
    );
    assert!(
        !roles.foundation_members.is_empty(),
        "Foundation needs members"
    );
    let unique_members: alloc::collections::BTreeSet<_> = roles.foundation_members.iter().collect();
    assert_eq!(
        unique_members.len(),
        roles.foundation_members.len(),
        "duplicate Foundation member"
    );
    // SDK benchmark setups require low-bond validators and nominators. Keep
    // the production chainspec limits unchanged in every non-benchmark build.
    #[cfg(not(feature = "runtime-benchmarks"))]
    let (max_nominator_count, min_validator_bond) = (Some(0u32), 100 * crate::UNIT);
    #[cfg(feature = "runtime-benchmarks")]
    let (max_nominator_count, min_validator_bond) = (None::<u32>, 0u128);
    build_struct_json_patch!(RuntimeGenesisConfig {
        foundation_membership: pallet_membership::GenesisConfig {
            members: roles
                .foundation_members
                .try_into()
                .expect("Foundation members fit"),
            ..Default::default()
        },
        validator_admission: pallet_validator_admission::GenesisConfig {
            mode: pallet_validator_admission::AdmissionMode::FoundationOnly,
            approved: approved.clone(),
        },
        staking: pallet_staking::GenesisConfig {
            validator_count: approved.len() as u32,
            minimum_validator_count,
            max_validator_count: None,
            max_nominator_count: max_nominator_count,
            min_validator_bond: min_validator_bond,
            stakers: approved
                .iter()
                .map(|who| (
                    who.clone(),
                    who.clone(),
                    100 * crate::UNIT,
                    pallet_staking::StakerStatus::Validator
                ))
                .collect(),
            force_era: pallet_staking::Forcing::NotForcing,
            ..Default::default()
        },
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
                        }
                        .into(),
                    )
                })
                .collect::<Vec<_>>(),
            ..Default::default()
        },
        // Must match QUANTUM_DEFAULT_JOB_SPEC_BUILDER_SS58 on the canonical testnet
        quantum_compute_mempool: QuantumComputeMempoolConfig {
            default_ising_spec_builder: Some(roles.ising_spec_builder),
        },
        faucet_ops: pallet_faucet_ops::GenesisConfig {
            state: pallet_faucet_ops::FaucetState::Enabled,
            authority: Some(roles.faucet_authority),
        },
        emission_controller: pallet_emission_controller::GenesisConfig {
            enabled: true,
            start_at: None,
            routes: vec![
                (
                    pallet_emission_controller::ISING_SUBNET,
                    sp_runtime::Perbill::from_percent(50)
                ),
                (
                    pallet_emission_controller::QVRF_SUBNET,
                    sp_runtime::Perbill::from_percent(50)
                ),
            ],
            // Test currency only; mainnet MUST use zero plus the disabled fuse.
            faucet_budget: 1_000_000_000 * crate::UNIT,
            ..Default::default()
        },
        sudo: SudoConfig {
            key: roles.sudo_key
        },
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
        development_roles(),
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
        1,
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
        development_roles(),
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
        1,
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
        development_roles(),
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
        1,
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
/// Fresh spec-119 genesis gives all three operators Foundation membership.
/// Operator 1 remains sudo, faucet authority and Ising spec builder until ops
/// provisions separate role accounts; this preset does not migrate live state.
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
        "c5c1e685e181d2939fddaf43e61455c0f00a99d2716d59b1f90674e5c0292510",
        "operator_1 tx_account_hex literal",
    );
    let op2_account = tx_account_from_hex(
        "40f5f2aeab073dfa95c96e48eba57b4afbdd2d118ff4acf60987a8c8bf8bf32b",
        "operator_2 tx_account_hex literal",
    );
    let op3_account = tx_account_from_hex(
        "d88854de054c7534450cddace65332a98d12ba06ff3adb39f9ad40f129b37aa7",
        "operator_3 tx_account_hex literal",
    );

    testnet_genesis(
        vec![
            (op1_account.clone(), op1_babe, op1_grandpa),
            (op2_account.clone(), op2_babe, op2_grandpa),
            (op3_account.clone(), op3_babe, op3_grandpa),
        ],
        vec![
            op1_account.clone(),
            op2_account.clone(),
            op3_account.clone(),
        ],
        GenesisRoles {
            foundation_members: vec![op1_account.clone(), op2_account, op3_account],
            // TODO(ops): derive and commit a dedicated faucet public account and
            // coordinate sudo multisig custody before changing these genesis defaults.
            // Post-genesis, rotate via faucetOps.set_authority before faucet deployment.
            faucet_authority: op1_account.clone(),
            sudo_key: Some(op1_account.clone()),
            ising_spec_builder: op1_account,
        },
        pallet_evm_chain_id::TESTNET_CHAIN_ID,
        3,
    )
}

/// Four-validator rehearsal with a safety floor of four; dev seeds are not
/// independent production operators. Add a replacement before removing a member.
pub fn rehearsal_config_genesis() -> Value {
    let seeds = ["//Alice", "//Bob", "//Charlie", "//Dave"];
    let mut foundation_members: Vec<_> = seeds[..3]
        .iter()
        .map(|seed| tx_account_from_seed(seed))
        .collect();
    foundation_members.sort();
    let sudo = crate::Multisig::multi_account_id(&foundation_members, 2);
    let faucet_authority = tx_account_from_seed("//Eve");
    let mut endowed_accounts: Vec<_> = seeds
        .iter()
        .map(|seed| tx_account_from_seed(seed))
        .collect();
    endowed_accounts.extend([faucet_authority.clone(), sudo.clone()]);
    testnet_genesis(
        seeds
            .iter()
            .map(|seed| {
                (
                    tx_account_from_seed(seed),
                    babe_authority_from_seed(seed),
                    grandpa_authority_from_seed(seed),
                )
            })
            .collect(),
        endowed_accounts,
        GenesisRoles {
            foundation_members,
            faucet_authority,
            sudo_key: Some(sudo),
            ising_spec_builder: tx_account_from_seed("//Alice"),
        },
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
        4,
    )
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_config_genesis(),
        LOCAL_THREE_VALIDATOR_RUNTIME_PRESET => local_three_validator_config_genesis(),
        QUIP_TESTNET_RUNTIME_PRESET => quip_testnet_config_genesis(),
        REHEARSAL_RUNTIME_PRESET => rehearsal_config_genesis(),
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
        PresetId::from(REHEARSAL_RUNTIME_PRESET),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
    use frame_support::genesis_builder_helper::build_state;

    /// Pinned hex of `tx_account_from_seed("//Alice")`. Acts as a canary for
    /// silent changes to `quip_transaction_crypto::ACCOUNT_ID_DOMAIN` or the
    /// H4 keyring derivation: any such change re-keys every account at
    /// genesis, and this constant is the cheapest grep target for catching
    /// that regression.
    const ALICE_PINNED_ACCOUNT_HEX: &str =
        "4de1b06b817f61f830d5b046e4fed5c124aa52ea97ba077817c225e56e757ac9";

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
        let expected_faucet: Option<AccountId> =
            serde_json::from_value(patch["faucetOps"]["authority"].clone()).unwrap();
        let expected_sudo: Option<AccountId> =
            serde_json::from_value(patch["sudo"]["key"].clone()).unwrap();
        let mut expected_members: Vec<AccountId> =
            serde_json::from_value(patch["foundationMembership"]["members"].clone()).unwrap();
        expected_members.sort();
        merge_json(&mut full, patch);
        let bytes = serde_json::to_vec(&full).expect("merged runtime config serialises");
        sp_io::TestExternalities::new_empty().execute_with(|| {
            build_state::<crate::RuntimeGenesisConfig>(bytes)
                .expect("genesis preset builds storage without panic");
            assert_eq!(
                pallet_faucet_ops::State::<crate::Runtime>::get(),
                pallet_faucet_ops::FaucetState::Enabled
            );
            assert_eq!(
                pallet_faucet_ops::Authority::<crate::Runtime>::get(),
                expected_faucet
            );
            assert_eq!(pallet_sudo::Key::<crate::Runtime>::get(), expected_sudo);
            assert_eq!(
                pallet_collective::Members::<crate::Runtime, pallet_collective::Instance1>::get(),
                expected_members
            );
            #[cfg(not(feature = "runtime-benchmarks"))]
            {
                assert_eq!(
                    pallet_staking::MaxNominatorsCount::<crate::Runtime>::get(),
                    Some(0)
                );
                assert_eq!(
                    pallet_staking::MinValidatorBond::<crate::Runtime>::get(),
                    100 * crate::UNIT
                );
            }
            #[cfg(feature = "runtime-benchmarks")]
            {
                assert_eq!(
                    pallet_staking::MaxNominatorsCount::<crate::Runtime>::get(),
                    None
                );
                assert_eq!(pallet_staking::MinValidatorBond::<crate::Runtime>::get(), 0);
            }
            assert!(pallet_emission_controller::Enabled::<crate::Runtime>::get());
            assert_eq!(
                pallet_emission_controller::Routes::<crate::Runtime>::get().len(),
                2
            );
            assert_eq!(
                pallet_emission_controller::FaucetBudget::<crate::Runtime>::get(),
                1_000_000_000 * crate::UNIT
            );
            assert_ne!(
                crate::EmissionController::pot(0),
                crate::EmissionController::pot(1)
            );
            assert_eq!(
                <pallet_evm_chain_id::ChainId<crate::Runtime> as Get<u64>>::get(),
                expected_chain_id
            );
        });
    }

    #[test]
    fn rehearsal_preset_keeps_four_validator_floor() {
        let patch = rehearsal_config_genesis();
        assert_eq!(patch["staking"]["minimumValidatorCount"], 4);
        assert_eq!(patch["staking"]["stakers"].as_array().unwrap().len(), 4);
        let members: Vec<AccountId> =
            serde_json::from_value(patch["foundationMembership"]["members"].clone()).unwrap();
        let mut expected: Vec<_> = ["//Alice", "//Bob", "//Charlie"]
            .iter()
            .map(|seed| tx_account_from_seed(seed))
            .collect();
        expected.sort();
        assert_eq!(members, expected);
        let faucet = tx_account_from_seed("//Eve");
        assert!(!members.contains(&faucet));
        assert_eq!(
            patch["faucetOps"]["authority"],
            serde_json::to_value(&faucet).unwrap()
        );
        let sudo = crate::Multisig::multi_account_id(&expected, 2);
        assert_eq!(patch["sudo"]["key"], serde_json::to_value(&sudo).unwrap());
        assert!(!members.contains(&sudo));
        assert_ne!(sudo, faucet);
        let balances: Vec<(AccountId, crate::Balance)> =
            serde_json::from_value(patch["balances"]["balances"].clone()).unwrap();
        for account in [faucet, sudo] {
            assert!(balances
                .iter()
                .any(|(who, balance)| who == &account && *balance > 0));
        }
        assert_preset_builds_storage_with_chain_id(patch, pallet_evm_chain_id::LOCAL_CHAIN_ID);
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
        let patch = quip_testnet_config_genesis();
        let members = patch["foundationMembership"]["members"].as_array().unwrap();
        let approved = patch["validatorAdmission"]["approved"].as_array().unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members, approved);
        assert!(members[0] != members[1] && members[0] != members[2] && members[1] != members[2]);
        assert_eq!(patch["sudo"]["key"], members[0]);
        assert_eq!(patch["faucetOps"]["authority"], members[0]);
        assert_eq!(
            patch["quantumComputeMempool"]["defaultIsingSpecBuilder"],
            members[0]
        );
        assert_preset_builds_storage_with_chain_id(patch, pallet_evm_chain_id::TESTNET_CHAIN_ID);
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
        "c5c1e685e181d2939fddaf43e61455c0f00a99d2716d59b1f90674e5c0292510";

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

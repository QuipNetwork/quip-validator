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
    SessionKeys, BABE_GENESIS_EPOCH_CONFIG,
};
use alloc::{collections::BTreeSet, format, string::String, vec, vec::Vec};
use codec::Encode;
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
    ising_spec_builder: AccountId,
    /// Reject a genesis where a council seat is also a validator operator.
    /// Local presets keep the single-key convenience and leave this off; the
    /// mainnet preset must turn it on — V0 governance exists to separate the
    /// two roles, and a chain that starts with council == operators cannot
    /// reach that state without a governance change.
    require_council_disjoint: bool,
}

// Small local presets retain their single-key convenience. Rehearsal and the
// public testnet explicitly assign each role instead.
fn development_roles() -> GenesisRoles {
    let alice = tx_account_from_seed(&Sr25519Keyring::Alice.to_seed());
    GenesisRoles {
        foundation_members: vec![alice.clone()],
        faucet_authority: alice.clone(),
        ising_spec_builder: alice,
        require_council_disjoint: false,
    }
}

/// Council seats must not double as validator operators.
fn assert_council_disjoint(
    council: &[AccountId],
    operators: &[AccountId],
    require: bool,
) -> Result<(), &'static str> {
    if !require {
        return Ok(());
    }
    match council.iter().find(|who| operators.contains(who)) {
        Some(_) => Err("council member is also a validator operator"),
        None => Ok(()),
    }
}

fn testnet_genesis(
    initial_authorities: Vec<(AccountId, BabeId, GrandpaId)>,
    endowed_accounts: Vec<AccountId>,
    roles: GenesisRoles,
    chain_id: u64,
    minimum_validator_count: u32,
) -> Value {
    let funding = GenesisFunding {
        balances: endowed_accounts
            .into_iter()
            .map(|a| (a, TEST_CURRENCY_ENDOWMENT))
            .collect(),
        faucet: pallet_faucet_ops::GenesisConfig {
            state: pallet_faucet_ops::FaucetState::Enabled,
            authority: Some(roles.faucet_authority.clone()),
        },
        faucet_budget: 1_000_000_000 * crate::UNIT,
    };
    network_genesis(
        initial_authorities,
        funding,
        roles,
        chain_id,
        minimum_validator_count,
    )
}

struct GenesisFunding {
    balances: Vec<(AccountId, crate::Balance)>,
    faucet: pallet_faucet_ops::GenesisConfig<crate::Runtime>,
    faucet_budget: u128,
}

fn network_genesis(
    initial_authorities: Vec<(AccountId, BabeId, GrandpaId)>,
    funding: GenesisFunding,
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
        approved.iter().all(|who| funding
            .balances
            .iter()
            .any(|(account, balance)| account == who && *balance > VALIDATOR_BOND)),
        "every validator must be endowed above its bond"
    );
    assert_eq!(
        funding
            .balances
            .iter()
            .map(|(a, _)| a)
            .collect::<BTreeSet<_>>()
            .len(),
        funding.balances.len(),
        "duplicate endowed account"
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
    if let Err(why) = assert_council_disjoint(
        &roles.foundation_members,
        &approved,
        roles.require_council_disjoint,
    ) {
        panic!("{why}");
    }
    // SDK benchmark setups require low-bond validators and nominators. Keep
    // the production chainspec limits unchanged in every non-benchmark build.
    #[cfg(not(feature = "runtime-benchmarks"))]
    let (max_nominator_count, min_validator_bond, min_nominator_bond) = (
        Some(crate::configs::election::MaxNominators::get()),
        2_000 * crate::UNIT,
        10 * crate::UNIT,
    );
    #[cfg(feature = "runtime-benchmarks")]
    let (max_nominator_count, min_validator_bond, min_nominator_bond) = (None::<u32>, 0u128, 0u128);
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
            min_nominator_bond: min_nominator_bond,
            stakers: approved
                .iter()
                .map(|who| (
                    who.clone(),
                    who.clone(),
                    2_000 * crate::UNIT,
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
            balances: funding.balances,
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
        faucet_ops: funding.faucet,
        emission_controller: pallet_emission_controller::GenesisConfig {
            enabled: true,
            start_at: None,
            // Split the controller's 30M QUIP/year equally between these pots.
            // Staking mints its separate 10M share on claims; adding a staking
            // route here would double-count that share of the 40M envelope.
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
            faucet_budget: funding.faucet_budget,
            ..Default::default()
        },
    })
}

/// Mainnet export is deliberately blocked until QUI-2080 provisions real keys and floats.
pub const QUIP_MAINNET_RUNTIME_PRESET: &str = "quip-mainnet";
pub const MAINNET_CHAIN_ID: u64 = 20_049;
const VALIDATOR_BOND: crate::Balance = 2_000 * crate::UNIT;
const TEST_CURRENCY_ENDOWMENT: crate::Balance = 1u128 << 60;

// Canonical labels also define the reserved sentinel set. Keep this independent
// of the unresolved marker so clearing the marker cannot hide placeholder seats.
const MAINNET_COUNCIL_LABELS: [&str; 6] = ["Rick", "Colton", "Vik", "Roman", "David", "BVI Admin"];
pub const MAINNET_COUNCIL_PLACEHOLDERS: &[&str] =
    &["Rick", "Colton", "Vik", "Roman", "David", "BVI Admin"];
const MAINNET_VALIDATOR_LABELS: [&str; 5] = [
    "Validator 1",
    "Validator 2",
    "Validator 3",
    "Validator 4",
    "Validator 5",
];
/// TBD is a blocking absence, never a default or a zero allocation.
pub const MAINNET_VALIDATOR_FLOAT_TBD: Option<crate::Balance> = None;
pub const MAINNET_COUNCIL_FLOAT_TBD: Option<crate::Balance> = None;
pub const MAINNET_SPEC_BUILDER_FLOAT_TBD: Option<crate::Balance> = None;

#[derive(Clone)]
struct MainnetValidator {
    stash: AccountId,
    session_keys: Option<(BabeId, GrandpaId)>,
}

#[derive(Clone)]
struct MainnetInputs {
    council: Vec<AccountId>,
    council_placeholders: Vec<&'static str>,
    validators: Vec<MainnetValidator>,
    spec_builder: AccountId,
    validator_float: Option<crate::Balance>,
    council_float: Option<crate::Balance>,
    spec_builder_float: Option<crate::Balance>,
}

/// Hash an account ID, not a signing seed. No key pair is generated or provisioned.
fn mainnet_placeholder(role: &str, label: &str) -> AccountId {
    AccountId::new(sp_core::blake2_256(
        &("quip/genesis-placeholder/v1", "mainnet", role, label).encode(),
    ))
}

fn reserved_mainnet_accounts() -> Vec<AccountId> {
    MAINNET_COUNCIL_LABELS
        .iter()
        .map(|label| mainnet_placeholder("council", label))
        .chain(
            MAINNET_VALIDATOR_LABELS
                .iter()
                .map(|label| mainnet_placeholder("validator", label)),
        )
        .chain(core::iter::once(mainnet_placeholder(
            "spec-builder",
            "Ising",
        )))
        .collect()
}

fn mainnet_inputs() -> MainnetInputs {
    MainnetInputs {
        council: MAINNET_COUNCIL_LABELS
            .iter()
            .map(|label| mainnet_placeholder("council", label))
            .collect(),
        council_placeholders: MAINNET_COUNCIL_PLACEHOLDERS.to_vec(),
        validators: MAINNET_VALIDATOR_LABELS
            .iter()
            .map(|label| MainnetValidator {
                stash: mainnet_placeholder("validator", label),
                session_keys: None, // QUI-2080: real operator-supplied H4 BABE and H2 GRANDPA public keys.
            })
            .collect(),
        spec_builder: mainnet_placeholder("spec-builder", "Ising"),
        validator_float: MAINNET_VALIDATOR_FLOAT_TBD,
        council_float: MAINNET_COUNCIL_FLOAT_TBD,
        spec_builder_float: MAINNET_SPEC_BUILDER_FLOAT_TBD,
    }
}

fn resolved_float(value: Option<crate::Balance>, name: &str) -> Result<crate::Balance, String> {
    let value = value.ok_or_else(|| format!("mainnet genesis blocked: {name} is TBD"))?;
    if value == 0 || value < crate::EXISTENTIAL_DEPOSIT {
        return Err(format!(
            "mainnet genesis blocked: {name} must fund a usable operational account"
        ));
    }
    Ok(value)
}

fn validate_session_keys(authorities: &[(AccountId, BabeId, GrandpaId)]) -> Result<(), String> {
    let babe: BTreeSet<_> = authorities.iter().map(|(_, key, _)| key.encode()).collect();
    let grandpa: BTreeSet<_> = authorities.iter().map(|(_, _, key)| key.encode()).collect();
    if babe.len() != authorities.len() || grandpa.len() != authorities.len() {
        return Err("genesis blocked: duplicate BABE or GRANDPA session key".into());
    }
    Ok(())
}

fn validate_mainnet_funding(
    funding: &GenesisFunding,
    validators: &[AccountId],
    council: &[AccountId],
    builder: &AccountId,
) -> Result<(), String> {
    let expected: BTreeSet<_> = validators
        .iter()
        .chain(council)
        .chain(core::iter::once(builder))
        .collect();
    let actual: BTreeSet<_> = funding
        .balances
        .iter()
        .map(|(account, _)| account)
        .collect();
    if expected.len() != 12 || actual.len() != funding.balances.len() || actual != expected {
        return Err("mainnet genesis blocked: endowments must be exactly five validators, six council seats and one dedicated spec builder".into());
    }
    let reserved = reserved_mainnet_accounts();
    if funding
        .balances
        .iter()
        .any(|(account, _)| reserved.contains(account))
    {
        return Err("mainnet genesis blocked: reserved placeholder account in endowments".into());
    }
    if funding
        .balances
        .iter()
        .any(|(_, balance)| *balance == TEST_CURRENCY_ENDOWMENT)
    {
        return Err("mainnet genesis blocked: testnet-default endowment is forbidden".into());
    }
    if funding.balances.iter().any(|(account, balance)| {
        *balance < crate::EXISTENTIAL_DEPOSIT
            || (validators.contains(account) && *balance <= VALIDATOR_BOND)
    }) {
        return Err(
            "mainnet genesis blocked: unusable float or validator endowment not above its bond"
                .into(),
        );
    }
    if funding.faucet.state != pallet_faucet_ops::FaucetState::PermanentlyDisabled
        || funding.faucet.authority.is_some()
        || funding.faucet_budget != 0
    {
        return Err("mainnet genesis blocked: faucet must be PermanentlyDisabled with no authority and zero budget".into());
    }
    Ok(())
}

fn mainnet_genesis(inputs: MainnetInputs) -> Result<Value, String> {
    let actual_placeholders: BTreeSet<_> = MAINNET_COUNCIL_LABELS
        .iter()
        .copied()
        .filter(|label| {
            inputs
                .council
                .contains(&mainnet_placeholder("council", label))
        })
        .collect();
    let declared: BTreeSet<_> = inputs.council_placeholders.iter().copied().collect();
    if actual_placeholders != declared || declared.len() != inputs.council_placeholders.len() {
        return Err("mainnet genesis blocked: MAINNET_COUNCIL_PLACEHOLDERS does not match the configured placeholder seats".into());
    }
    if !declared.is_empty() {
        return Err(format!(
            "mainnet genesis blocked: unresolved council placeholders: {}",
            inputs.council_placeholders.join(", ")
        ));
    }
    let validators: Vec<_> = inputs.validators.iter().map(|v| v.stash.clone()).collect();
    if inputs.council.len() != 6
        || inputs.council.iter().collect::<BTreeSet<_>>().len() != 6
        || validators.len() != 5
        || validators.iter().collect::<BTreeSet<_>>().len() != 5
    {
        return Err("mainnet genesis blocked: exactly six distinct council seats and five distinct validators are required".into());
    }
    assert_council_disjoint(&inputs.council, &validators, true).map_err(String::from)?;
    if inputs.council.contains(&inputs.spec_builder) || validators.contains(&inputs.spec_builder) {
        return Err(
            "mainnet genesis blocked: Ising spec builder must be a dedicated account".into(),
        );
    }
    let reserved = reserved_mainnet_accounts();
    if validators
        .iter()
        .chain(&inputs.council)
        .chain(core::iter::once(&inputs.spec_builder))
        .any(|a| reserved.contains(a))
    {
        return Err("mainnet genesis blocked: unresolved validator, council or spec-builder placeholder account".into());
    }
    let authorities = inputs
        .validators
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let (babe, grandpa) = v.session_keys.ok_or_else(|| {
                format!(
                    "mainnet genesis blocked: Validator {} BABE/GRANDPA session keys are TBD",
                    i + 1
                )
            })?;
            Ok((v.stash, babe, grandpa))
        })
        .collect::<Result<Vec<_>, String>>()?;
    validate_session_keys(&authorities)?;
    let validator_float = resolved_float(inputs.validator_float, "MAINNET_VALIDATOR_FLOAT_TBD")?;
    let council_float = resolved_float(inputs.council_float, "MAINNET_COUNCIL_FLOAT_TBD")?;
    let spec_builder_float =
        resolved_float(inputs.spec_builder_float, "MAINNET_SPEC_BUILDER_FLOAT_TBD")?;
    let stash_balance = VALIDATOR_BOND.checked_add(validator_float).ok_or_else(|| {
        String::from("mainnet genesis blocked: validator bond plus float overflows")
    })?;
    let funding = GenesisFunding {
        balances: validators
            .iter()
            .cloned()
            .map(|a| (a, stash_balance))
            .chain(inputs.council.iter().cloned().map(|a| (a, council_float)))
            .chain(core::iter::once((
                inputs.spec_builder.clone(),
                spec_builder_float,
            )))
            .collect(),
        faucet: pallet_faucet_ops::GenesisConfig {
            state: pallet_faucet_ops::FaucetState::PermanentlyDisabled,
            authority: None,
        },
        faucet_budget: 0,
    };
    validate_mainnet_funding(&funding, &validators, &inputs.council, &inputs.spec_builder)?;
    Ok(network_genesis(
        authorities,
        funding,
        GenesisRoles {
            foundation_members: inputs.council,
            faucet_authority: inputs.spec_builder.clone(), // Unused: the explicit faucet config has no authority.
            ising_spec_builder: inputs.spec_builder,
            require_council_disjoint: true,
        },
        MAINNET_CHAIN_ID,
        3,
    ))
}

/// Build mainnet only after all launch inputs are resolved. Raw-spec provenance
/// and proof of key possession are separate QUI-2080 launch gates.
pub fn quip_mainnet_config_genesis() -> Result<Value, String> {
    mainnet_genesis(mainnet_inputs())
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
/// `scripts/derive-operator-keys.sh`). Only the
/// public bytes are committed in this repository; private material lives on
/// each operator's host.
///
/// Fresh spec-119 genesis gives all three operators Foundation membership.
/// Operator 1 is also faucet authority and Ising spec builder until ops
/// provisions separate role accounts; this preset does not migrate live state.
///
/// Spec 120 drops `pallet_sudo` entirely, so no preset carries a sudo key. The
/// council reaches Root through `ValidatorAdmission::dispatch_as_root` behind a
/// 2/3 collective motion instead of a single-key escape hatch.
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
            // TODO(ops): derive and commit a dedicated faucet public account before
            // changing this genesis default. Post-genesis, rotate via
            // faucetOps.set_authority before faucet deployment.
            faucet_authority: op1_account.clone(),
            ising_spec_builder: op1_account,
            // The public testnet predates V0 governance: council seats are the
            // three operators, which mainnet must not repeat (W6 / QUI-2065).
            require_council_disjoint: false,
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
    let faucet_authority = tx_account_from_seed("//Eve");
    let mut endowed_accounts: Vec<_> = seeds
        .iter()
        .map(|seed| tx_account_from_seed(seed))
        .collect();
    endowed_accounts.push(faucet_authority.clone());
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
            ising_spec_builder: tx_account_from_seed("//Alice"),
            require_council_disjoint: false,
        },
        pallet_evm_chain_id::LOCAL_CHAIN_ID,
        4,
    )
}

// The default SDK panic handler only logs its message before an opaque Wasm
// trap. Register this deliberate refusal with the host so callers receive the
// reason even when runtime logging is disabled in release builds.
fn refuse_preset_export(reason: String) -> ! {
    #[cfg(substrate_runtime)]
    {
        sp_io::panic_handler::abort_on_panic(&reason)
    }
    #[cfg(not(substrate_runtime))]
    {
        panic!("{reason}")
    }
}

/// Provides the JSON representation of predefined genesis config for given `id`.
pub fn get_preset(id: &PresetId) -> Option<Vec<u8>> {
    let patch = match id.as_ref() {
        sp_genesis_builder::DEV_RUNTIME_PRESET => development_config_genesis(),
        sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET => local_config_genesis(),
        LOCAL_THREE_VALIDATOR_RUNTIME_PRESET => local_three_validator_config_genesis(),
        QUIP_TESTNET_RUNTIME_PRESET => quip_testnet_config_genesis(),
        QUIP_MAINNET_RUNTIME_PRESET => {
            quip_mainnet_config_genesis().unwrap_or_else(|reason| refuse_preset_export(reason))
        }
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
        PresetId::from(QUIP_MAINNET_RUNTIME_PRESET),
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
        // Spec 120 removed pallet-sudo: no preset may carry a sudo key, and a
        // stray one must fail the genesis build rather than silently return.
        assert!(
            patch.get("sudo").is_none(),
            "spec 120 genesis must not configure pallet_sudo"
        );
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
            assert_eq!(
                pallet_collective::Members::<crate::Runtime, pallet_collective::Instance1>::get(),
                expected_members
            );
            #[cfg(not(feature = "runtime-benchmarks"))]
            {
                assert_eq!(
                    pallet_staking::MaxNominatorsCount::<crate::Runtime>::get(),
                    Some(1024)
                );
                assert_eq!(
                    pallet_staking::MinValidatorBond::<crate::Runtime>::get(),
                    2_000 * crate::UNIT
                );
                assert_eq!(
                    pallet_staking::MinNominatorBond::<crate::Runtime>::get(),
                    10 * crate::UNIT
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
        let balances: Vec<(AccountId, crate::Balance)> =
            serde_json::from_value(patch["balances"]["balances"].clone()).unwrap();
        assert!(balances
            .iter()
            .any(|(who, balance)| who == &faucet && *balance > 0));
        assert_preset_builds_storage_with_chain_id(patch, pallet_evm_chain_id::LOCAL_CHAIN_ID);
    }

    /// V0 governance exists to separate the council from the validator set.
    /// The mainnet preset turns the requirement on; local presets keep the
    /// single-key convenience.
    #[test]
    fn council_must_be_disjoint_from_operators_when_required() {
        let seat = AccountId::from([7; 32]);
        let other = AccountId::from([8; 32]);
        assert_eq!(
            assert_council_disjoint(&[seat.clone()], &[seat.clone()], true),
            Err("council member is also a validator operator")
        );
        assert!(assert_council_disjoint(&[seat.clone()], &[seat], false).is_ok());
        assert!(assert_council_disjoint(&[other], &[AccountId::from([9; 32])], true).is_ok());
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
    // Captured by running get_preset on unmodified 59a095c in both feature
    // configurations. Comparing serialized bytes catches drift the parity gate
    // cannot: the same accidental change in both runtime trees.
    #[test]
    fn legacy_preset_bytes_match_pre_launch_base() {
        let fixture: Value =
            serde_json::from_str(include_str!("genesis_legacy_hashes.json")).unwrap();
        let profile = if cfg!(feature = "runtime-benchmarks") {
            "benchmark"
        } else {
            "production"
        };
        let expected = fixture[profile].as_object().unwrap();
        assert_eq!(expected.len(), 5);
        for (name, hash) in expected {
            let bytes = get_preset(&PresetId::from(name.clone())).unwrap();
            assert_eq!(
                hex_encode(&sp_core::blake2_256(&bytes)),
                hash.as_str().unwrap(),
                "legacy preset changed: {name}"
            );
        }
    }

    // Fixture-only operating floats and keys. Never a production fallback.
    fn ready_mainnet_fixture() -> MainnetInputs {
        MainnetInputs {
            council: (40..46).map(|i| AccountId::new([i; 32])).collect(),
            council_placeholders: vec![],
            validators: ["//Alice", "//Bob", "//Charlie", "//Dave", "//Eve"]
                .iter()
                .map(|seed| MainnetValidator {
                    stash: tx_account_from_seed(seed),
                    session_keys: Some((
                        babe_authority_from_seed(seed),
                        grandpa_authority_from_seed(seed),
                    )),
                })
                .collect(),
            spec_builder: AccountId::new([90; 32]),
            validator_float: Some(crate::UNIT),
            council_float: Some(2 * crate::UNIT),
            spec_builder_float: Some(3 * crate::UNIT),
        }
    }

    fn funding_from_patch(patch: &Value) -> GenesisFunding {
        GenesisFunding {
            balances: serde_json::from_value(patch["balances"]["balances"].clone()).unwrap(),
            faucet: serde_json::from_value(patch["faucetOps"].clone()).unwrap(),
            faucet_budget: serde_json::from_value(
                patch["emissionController"]["faucetBudget"].clone(),
            )
            .unwrap(),
        }
    }

    fn assert_launch_storage(
        patch: Value,
        chain_id: u64,
        validator_count: u32,
        council_count: usize,
    ) {
        use frame_support::traits::Get;
        let funding = funding_from_patch(&patch);
        let mut full = serde_json::to_value(crate::RuntimeGenesisConfig::default()).unwrap();
        assert!(patch.get("sudo").is_none());
        assert_eq!(patch["staking"]["validatorCount"], validator_count);
        assert_eq!(patch["staking"]["minimumValidatorCount"], 3);
        assert_eq!(patch["emissionController"]["enabled"], true);
        assert_eq!(patch["emissionController"]["startAt"], Value::Null);
        let council: Vec<AccountId> =
            serde_json::from_value(patch["foundationMembership"]["members"].clone()).unwrap();
        let approved: Vec<AccountId> =
            serde_json::from_value(patch["validatorAdmission"]["approved"].clone()).unwrap();
        assert_eq!(council.len(), council_count);
        assert_eq!(approved.len(), validator_count as usize);
        assert!(assert_council_disjoint(&council, &approved, true).is_ok());
        let builder: AccountId = serde_json::from_value(
            patch["quantumComputeMempool"]["defaultIsingSpecBuilder"].clone(),
        )
        .unwrap();
        let keys: Vec<(AccountId, AccountId, crate::BoxedSessionKeys)> =
            serde_json::from_value(patch["session"]["keys"].clone()).unwrap();
        assert_eq!(keys.len(), validator_count as usize);
        merge_json(&mut full, patch);
        sp_io::TestExternalities::new_empty().execute_with(|| {
            build_state::<crate::RuntimeGenesisConfig>(serde_json::to_vec(&full).unwrap()).unwrap();
            assert_eq!(
                pallet_evm_chain_id::Eip155ChainId::<crate::Runtime>::get(),
                chain_id
            );
            assert_eq!(
                <pallet_evm_chain_id::ChainId<crate::Runtime> as Get<u64>>::get(),
                chain_id
            );
            assert_eq!(
                pallet_staking::ValidatorCount::<crate::Runtime>::get(),
                validator_count
            );
            assert_eq!(
                pallet_staking::MinimumValidatorCount::<crate::Runtime>::get(),
                3
            );
            #[cfg(not(feature = "runtime-benchmarks"))]
            {
                assert_eq!(
                    pallet_staking::MaxNominatorsCount::<crate::Runtime>::get(),
                    Some(1024)
                );
                assert_eq!(
                    pallet_staking::MinValidatorBond::<crate::Runtime>::get(),
                    2_000 * crate::UNIT
                );
                assert_eq!(
                    pallet_staking::MinNominatorBond::<crate::Runtime>::get(),
                    10 * crate::UNIT
                );
            }
            #[cfg(feature = "runtime-benchmarks")]
            {
                assert_eq!(
                    pallet_staking::MaxNominatorsCount::<crate::Runtime>::get(),
                    None
                );
                assert_eq!(pallet_staking::MinValidatorBond::<crate::Runtime>::get(), 0);
                assert_eq!(pallet_staking::MinNominatorBond::<crate::Runtime>::get(), 0);
            }
            let actual_council =
                pallet_collective::Members::<crate::Runtime, pallet_collective::Instance1>::get();
            assert_eq!(
                actual_council.iter().collect::<BTreeSet<_>>(),
                council.iter().collect()
            );
            assert_eq!(
                pallet_validator_admission::Mode::<crate::Runtime>::get(),
                pallet_validator_admission::AdmissionMode::FoundationOnly
            );
            assert_eq!(
                pallet_validator_admission::Approved::<crate::Runtime>::get()
                    .iter()
                    .collect::<BTreeSet<_>>(),
                approved.iter().collect()
            );
            assert_eq!(
                pallet_session::Validators::<crate::Runtime>::get()
                    .iter()
                    .collect::<BTreeSet<_>>(),
                approved.iter().collect()
            );
            for (stash, controller, session_keys) in keys {
                assert_eq!(stash, controller);
                assert_eq!(
                    pallet_session::NextKeys::<crate::Runtime>::get(&stash),
                    Some(session_keys)
                );
                let ledger = pallet_staking::Ledger::<crate::Runtime>::get(&stash).unwrap();
                assert_eq!(ledger.active, 2_000 * crate::UNIT);
                assert_eq!(ledger.total, 2_000 * crate::UNIT);
                assert_eq!(ledger.stash, stash);
            }
            for (who, amount) in &funding.balances {
                let account = frame_system::Account::<crate::Runtime>::get(who);
                assert_eq!(account.data.free + account.data.reserved, *amount);
            }
            // SDK 845eb99 frame/revive/src/lib.rs:791-800 mints one ED
            // into this named pallet account. This is the only technical
            // genesis deposit outside the explicit operational allocations.
            let revive_account = pallet_revive::Pallet::<crate::Runtime>::account_id();
            assert!(funding
                .balances
                .iter()
                .all(|(who, _)| who != &revive_account));
            let revive = frame_system::Account::<crate::Runtime>::get(&revive_account);
            assert_eq!(revive.data.free, crate::EXISTENTIAL_DEPOSIT);
            assert_eq!(revive.data.reserved, 0);
            assert_eq!(
                pallet_balances::TotalIssuance::<crate::Runtime>::get(),
                funding
                    .balances
                    .iter()
                    .map(|(_, amount)| amount)
                    .sum::<u128>()
                    .checked_add(crate::EXISTENTIAL_DEPOSIT)
                    .unwrap()
            );
            assert_eq!(
                pallet_faucet_ops::State::<crate::Runtime>::get(),
                funding.faucet.state
            );
            assert_eq!(
                pallet_faucet_ops::Authority::<crate::Runtime>::get(),
                funding.faucet.authority
            );
            assert!(pallet_emission_controller::Enabled::<crate::Runtime>::get());
            assert_eq!(
                pallet_emission_controller::StartAt::<crate::Runtime>::get(),
                None
            );
            assert_eq!(
                pallet_emission_controller::FaucetBudget::<crate::Runtime>::get(),
                funding.faucet_budget
            );
            assert_eq!(
                pallet_emission_controller::FaucetIssued::<crate::Runtime>::get(),
                0
            );
            assert_eq!(
                pallet_emission_controller::Routes::<crate::Runtime>::get().to_vec(),
                vec![
                    (
                        pallet_emission_controller::ISING_SUBNET,
                        sp_runtime::Perbill::from_percent(50)
                    ),
                    (
                        pallet_emission_controller::QVRF_SUBNET,
                        sp_runtime::Perbill::from_percent(50)
                    ),
                ]
            );
            let spec_id =
                <crate::Runtime as pallet_quantum_compute_mempool::Config>::DefaultIsingSpecId::get(
                );
            assert_eq!(
                pallet_quantum_compute_mempool::JobSpecs::<crate::Runtime>::get(spec_id)
                    .unwrap()
                    .builder,
                builder
            );
        });
    }

    #[test]
    fn mainnet_fixture_builds_the_decided_policy() {
        let inputs = ready_mainnet_fixture();
        let patch = mainnet_genesis(inputs.clone()).unwrap();
        let funding = funding_from_patch(&patch);
        let validators: Vec<_> = inputs.validators.iter().map(|v| v.stash.clone()).collect();
        validate_mainnet_funding(&funding, &validators, &inputs.council, &inputs.spec_builder)
            .unwrap();
        assert_eq!(funding.balances.len(), 12);
        for (who, amount) in &funding.balances {
            let expected = if validators.contains(who) {
                2_001 * crate::UNIT
            } else if inputs.council.contains(who) {
                2 * crate::UNIT
            } else {
                3 * crate::UNIT
            };
            assert_eq!(*amount, expected);
            assert_ne!(*amount, 1u128 << 60);
        }
        assert_eq!(
            funding.faucet.state,
            pallet_faucet_ops::FaucetState::PermanentlyDisabled
        );
        assert!(funding.faucet.authority.is_none());
        assert_eq!(funding.faucet_budget, 0);
        assert_launch_storage(patch, 20_049, 5, 6);
    }

    #[test]
    fn mainnet_placeholders_are_pinned_and_export_is_blocked() {
        assert_eq!(
            MAINNET_COUNCIL_PLACEHOLDERS,
            ["Rick", "Colton", "Vik", "Roman", "David", "BVI Admin"]
        );
        let accounts = reserved_mainnet_accounts();
        assert_eq!(accounts.iter().collect::<BTreeSet<_>>().len(), 12);
        assert_eq!(
            hex_encode(&sp_core::blake2_256(&accounts.encode())),
            "b6e46344fd96bb4deee3a025c407e7a9d502b8c40ad962a535ceb0160c2d9da2"
        );
        assert_eq!(MAINNET_VALIDATOR_FLOAT_TBD, None);
        assert_eq!(MAINNET_COUNCIL_FLOAT_TBD, None);
        assert_eq!(MAINNET_SPEC_BUILDER_FLOAT_TBD, None);
        assert!(mainnet_inputs()
            .validators
            .iter()
            .all(|v| v.session_keys.is_none()));
        assert!(preset_names().contains(&PresetId::from("quip-mainnet")));
        let reason = quip_mainnet_config_genesis().unwrap_err();
        assert!(reason.contains(
            "unresolved council placeholders: Rick, Colton, Vik, Roman, David, BVI Admin"
        ));
        let panic =
            std::panic::catch_unwind(|| get_preset(&PresetId::from("quip-mainnet"))).unwrap_err();
        assert_eq!(panic.downcast_ref::<String>().unwrap(), &reason);
    }

    #[test]
    fn mainnet_gate_rejects_inconsistent_markers_and_each_missing_input() {
        let fixture = ready_mainnet_fixture();
        let mut input = mainnet_inputs();
        input.council_placeholders.clear();
        assert!(mainnet_genesis(input)
            .unwrap_err()
            .contains("does not match"));
        let mut input = fixture.clone();
        input.council_placeholders = MAINNET_COUNCIL_PLACEHOLDERS.to_vec();
        assert!(mainnet_genesis(input)
            .unwrap_err()
            .contains("does not match"));
        for case in 0..4 {
            let mut input = fixture.clone();
            match case {
                0 => {
                    input.council.pop();
                }
                1 => {
                    input.validators.pop();
                }
                2 => input.council[1] = input.council[0].clone(),
                _ => input.validators[1].stash = input.validators[0].stash.clone(),
            }
            assert!(mainnet_genesis(input)
                .unwrap_err()
                .contains("exactly six distinct council seats and five distinct validators"));
        }
        for index in 0..5 {
            let mut input = fixture.clone();
            input.validators[index].session_keys = None;
            assert!(mainnet_genesis(input).unwrap_err().contains(&format!(
                "Validator {} BABE/GRANDPA session keys are TBD",
                index + 1
            )));
        }
        for field in 0..3 {
            let mut input = fixture.clone();
            let name = match field {
                0 => {
                    input.validator_float = None;
                    "MAINNET_VALIDATOR_FLOAT_TBD"
                }
                1 => {
                    input.council_float = None;
                    "MAINNET_COUNCIL_FLOAT_TBD"
                }
                _ => {
                    input.spec_builder_float = None;
                    "MAINNET_SPEC_BUILDER_FLOAT_TBD"
                }
            };
            assert!(mainnet_genesis(input).unwrap_err().contains(name));
        }
        for field in 0..3 {
            let mut input = fixture.clone();
            match field {
                0 => input.validator_float = Some(0),
                1 => input.council_float = Some(0),
                _ => input.spec_builder_float = Some(0),
            }
            assert!(mainnet_genesis(input)
                .unwrap_err()
                .contains("usable operational account"));
        }
        let mut input = fixture.clone();
        input.validator_float = Some(u128::MAX);
        assert!(mainnet_genesis(input).unwrap_err().contains("overflows"));
        let mut input = fixture.clone();
        input.council[0] = input.validators[0].stash.clone();
        assert!(mainnet_genesis(input)
            .unwrap_err()
            .contains("also a validator"));
        let mut input = fixture.clone();
        input.spec_builder = input.council[0].clone();
        assert!(mainnet_genesis(input)
            .unwrap_err()
            .contains("dedicated account"));
        for role in 0..3 {
            let mut input = fixture.clone();
            // Put a sentinel into a different role as well: role swapping must not bypass the gate.
            let sentinel = mainnet_placeholder("validator", "Validator 1");
            match role {
                0 => input.council[0] = sentinel,
                1 => input.validators[0].stash = sentinel,
                _ => input.spec_builder = sentinel,
            }
            assert!(mainnet_genesis(input)
                .unwrap_err()
                .contains("placeholder account"));
        }
        for key_kind in 0..2 {
            let mut input = fixture.clone();
            let duplicate = input.validators[0].session_keys.clone().unwrap();
            let keys = input.validators[1].session_keys.as_mut().unwrap();
            if key_kind == 0 {
                keys.0 = duplicate.0;
            } else {
                keys.1 = duplicate.1;
            }
            assert!(mainnet_genesis(input)
                .unwrap_err()
                .contains("duplicate BABE or GRANDPA"));
        }
    }

    #[test]
    fn mainnet_funding_rejects_wrong_allocations_and_faucet() {
        let input = ready_mainnet_fixture();
        let validators: Vec<_> = input.validators.iter().map(|v| v.stash.clone()).collect();
        let patch = mainnet_genesis(input.clone()).unwrap();
        for case in 0..9 {
            let mut funding = funding_from_patch(&patch);
            match case {
                0 => funding.balances[0].1 = VALIDATOR_BOND,
                1 => funding.balances[0].1 = VALIDATOR_BOND - 1,
                2 => funding.balances[0].1 = TEST_CURRENCY_ENDOWMENT,
                3 => funding.balances.push(funding.balances[0].clone()),
                4 => funding.balances[0].0 = AccountId::new([99; 32]),
                5 => funding.faucet_budget = 1,
                6 => funding.faucet.state = pallet_faucet_ops::FaucetState::Paused,
                7 => funding.faucet.state = pallet_faucet_ops::FaucetState::Enabled,
                _ => funding.faucet.authority = Some(input.spec_builder.clone()),
            }
            assert!(
                validate_mainnet_funding(
                    &funding,
                    &validators,
                    &input.council,
                    &input.spec_builder
                )
                .is_err(),
                "case {case}"
            );
        }
    }
    // The runtime API must preserve the descriptive refusal, not merely trap
    // opaquely or omit the preset name. Run with an actual non-benchmark Wasm.
    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn wasm_preset_api_lists_mainnet_and_reports_why_export_is_blocked() {
        use codec::Decode;
        use sp_core::traits::{CallContext, CodeExecutor, RuntimeCode, WrappedRuntimeCode};
        let wasm = crate::WASM_BINARY.expect("this test requires a real runtime Wasm build");
        let fetcher = WrappedRuntimeCode(wasm.into());
        let code = RuntimeCode {
            code_fetcher: &fetcher,
            heap_pages: None,
            hash: sp_core::blake2_256(wasm).to_vec(),
        };
        let executor =
            sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build();
        let mut ext = sp_io::TestExternalities::new_empty();
        let (names, native) = executor.call(
            &mut ext.ext(),
            &code,
            "GenesisBuilder_preset_names",
            &[],
            CallContext::Offchain,
        );
        assert!(!native);
        let names = Vec::<String>::decode(&mut &names.unwrap()[..]).unwrap();
        assert!(names.iter().any(|name| name == "quip-mainnet"));
        let args = Some(PresetId::from("quip-mainnet")).encode();
        let (result, native) = executor.call(
            &mut ext.ext(),
            &code,
            "GenesisBuilder_get_preset",
            &args,
            CallContext::Offchain,
        );
        assert!(!native);
        let reason = result.unwrap_err().to_string();
        assert!(reason.contains("mainnet genesis blocked: unresolved council placeholders: Rick, Colton, Vik, Roman, David, BVI Admin"), "{reason}");
    }
}

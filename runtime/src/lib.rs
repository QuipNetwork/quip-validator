#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub mod apis;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarks;
pub mod configs;
pub mod weights;

extern crate alloc;
use alloc::vec::Vec;
use frame_support::traits::{fungible::Mutate, OnRuntimeUpgrade};
use pallet_revive::evm::runtime::EthExtra;
use quip_transaction_crypto::HybridTxSignature;
use sp_runtime::{
    generic, impl_opaque_keys,
    traits::{BlakeTwo256, IdentifyAccount, Verify},
    MultiAddress,
};
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

pub mod genesis_config_presets;

/// Opaque types. These are used by the CLI to instantiate machinery that don't need to know
/// the specifics of the runtime. They can then be made to be agnostic over specific formats
/// of data like extrinsics, allowing for them to continue syncing the network through upgrades
/// to even the core data structures.
pub mod opaque {
    use super::*;
    use sp_runtime::{
        generic,
        traits::{BlakeTwo256, Hash as HashT},
    };

    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

    /// Opaque block header type.
    pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
    /// Opaque block type.
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    /// Opaque block identifier type.
    pub type BlockId = generic::BlockId<Block>;
    /// Opaque block hash type.
    pub type Hash = <BlakeTwo256 as HashT>::Output;
}

impl_opaque_keys! {
    pub struct SessionKeys {
        pub babe: Babe,
        pub grandpa: Grandpa,
    }
}

/// Heap-boxed session keys keep `pallet_session::Call::set_keys` from making
/// the entire `RuntimeCall` enum larger than Utility's 1 KiB batching limit.
/// `Box<T>` and this single-field wrapper are SCALE-transparent, so the
/// existing `Session.set_keys` wire encoding is unchanged.
#[derive(
    Clone,
    PartialEq,
    Eq,
    codec::Encode,
    codec::Decode,
    codec::DecodeWithMemTracking,
    scale_info::TypeInfo,
    serde::Serialize,
    serde::Deserialize,
    Debug,
)]
pub struct BoxedSessionKeys(alloc::boxed::Box<SessionKeys>);

impl From<SessionKeys> for BoxedSessionKeys {
    fn from(keys: SessionKeys) -> Self {
        Self(alloc::boxed::Box::new(keys))
    }
}

impl sp_runtime::traits::OpaqueKeys for BoxedSessionKeys {
    type KeyTypeIdProviders = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;

    fn key_ids() -> &'static [sp_core::crypto::KeyTypeId] {
        SessionKeys::key_ids()
    }

    fn get_raw(&self, key_type: sp_core::crypto::KeyTypeId) -> &[u8] {
        self.0.get_raw(key_type)
    }

    fn ownership_proof_is_valid(&self, owner: &[u8], proof: &[u8]) -> bool {
        self.0.ownership_proof_is_valid(owner, proof)
    }
}

#[cfg(test)]
mod boxed_session_keys_tests {
    use super::{BoxedSessionKeys, SessionKeys};
    use codec::Encode;
    use quip_crypto_primitives::substrate::{
        ed25519_fndsa512::Pair as HybridGrandpaPair, sr25519_fndsa512::Pair as HybridBabePair,
    };
    use sp_core::Pair as _;

    #[test]
    fn boxed_session_keys_preserve_scale_wire_encoding() {
        let keys = SessionKeys {
            babe: HybridBabePair::from_string("//Alice", None)
                .expect("Alice BABE seed is valid")
                .public()
                .into(),
            grandpa: HybridGrandpaPair::from_string("//Alice", None)
                .expect("Alice GRANDPA seed is valid")
                .public()
                .into(),
        };
        let original_encoding = keys.encode();
        let boxed_encoding = BoxedSessionKeys::from(keys).encode();

        assert_eq!(boxed_encoding, original_encoding);
    }
}

// To learn more about runtime versioning, see:
// https://docs.substrate.io/main-docs/build/upgrade#runtime-versioning
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: alloc::borrow::Cow::Borrowed("quip"),
    impl_name: alloc::borrow::Cow::Borrowed("quip"),
    authoring_version: 1,
    // The version of the runtime specification. A full node will not attempt to use its native
    //   runtime in substitute for the on-chain Wasm runtime unless all of `spec_name`,
    //   `spec_version`, and `authoring_version` are the same between Wasm and native.
    // Bumped to 101 (and `transaction_version` to 2) when the signed-extrinsic
    // wire format switched from `MultiSignature` to the hybrid envelope. Without
    // these bumps, peers/clients could treat the new format as the old one.
    // Bumped to 102 for v0.2.0: adds `pallet_faucet_ops` (idx 11) and
    // `pallet_session` (idx 12). New dispatchables, events, and storage entries
    // change the runtime metadata; the signed-extrinsic wire format is
    // unchanged, so `transaction_version` stays at 2.
    // Bumped to 103 for QUI-567: adds the canonical default plain Ising job
    // spec, root-gates `QuantumComputeMempool::register_job_spec`, and changes
    // that call's argument encoding, so `transaction_version` moves to 3.
    // Bumped to 104 for the topology-upgrade path: adds
    // `QuantumPow::set_default_topology` (call_index 5) and makes the
    // difficulty energy curve spec-aware (h/J magnitudes derived from the
    // default topology's allowed-value specs instead of hardcoded ternary-h /
    // binary-J). Existing call encodings are unchanged, so
    // `transaction_version` stays at 3.
    // Bumped to 105 for indexer-free quantum reads: adds monotonic qblock ids,
    // qblock/hardness runtime APIs, and the mempool open-order recovery index.
    // Existing call encodings are unchanged, so `transaction_version` stays at
    // 3.
    // Bumped to 106 for on-chain miner descriptors and qblock participation:
    // adds `MinerRegistry` (idx 13) with descriptor/participation calls,
    // events, and storage. Existing call encodings are unchanged, so
    // `transaction_version` stays at 3.
    // Bumped to 107 for the participants-per-qblock reverse index
    // (`ParticipantsByQBlock`, `ParticipantCountByQBlock`) and the
    // `MinerRegistryApi` runtime API. Call encodings are unchanged, so
    // `transaction_version` stays at 3.
    // Bumped to 108 for per-topology difficulty + the mineable-topology
    // whitelist: `QuantumPow.Difficulty` (global StorageValue) becomes
    // `Difficulties` (StorageMap keyed by topology hash), `MineableTopologies`
    // is added, `set_difficulty` gains a `topology_hash` argument, and
    // `add_mineable_topology`/`remove_mineable_topology` (call_index 6/7) are
    // added. `set_difficulty`'s argument encoding changed, so
    // `transaction_version` moves to 4. Pallet storage version 2 → 3 with a
    // carry-forward migration.
    // Bumped to 109 to restore on-chain `system_info`: `MinerRegistry` adds a
    // schema-v2 descriptor input (`NodeDescriptorInput::V2`) carrying an
    // optional typed hardware survey, plus a v1 → v2 storage migration that
    // drops existing descriptors (miners re-file on restart). The V1 call
    // variant keeps index 0 and encodes identically, so `transaction_version`
    // stays at 4. MinerRegistry pallet storage version 1 → 2.
    // Bumped to 110 to add the optional `runtime` block (node software identity:
    // python / quip_version / protocol_version / in_docker / docker_image) to
    // the MinerRegistry V2 descriptor. Additive trailing field on the V2 input;
    // V1 is unaffected and V2 was not yet deployed, so `transaction_version`
    // stays at 4 and no new migration is needed (the v1 → v2 migration already
    // wipes descriptors; pallet storage version stays 2).
    // Bumped to 111 (110 had already shipped in v0.2.1-rc11 when these
    // landed) for two QuantumPow changes — this is what the chain deployed:
    // - `QBlock` gains a trailing `topology_hash` so a block records which
    //   topology it was mined against. This changes the persisted `QBlocks`
    //   value layout, so QuantumPow pallet storage version goes 3 → 4 with a
    //   v3 → v4 migration that re-encodes existing entries, backfilling
    //   `topology_hash` with the default topology. Read-only runtime API
    //   shape change (`QBlock`/`QBlockWithNonce`). Includes the sudo-only
    //   per-topology curve `c` override (`set_topology_curve`, new call).
    // - `submit_proof` weight becomes dimension-scaled (QIP-03): charged
    //   weight now depends on the registered topology's node/edge counts and
    //   the proof's solution count instead of a flat 60M placeholder.
    // No call encodings changed in 111, so `transaction_version` stayed at 4.
    // Bumped to 112 (111 was already deployed when this landed):
    // `QuantumProof` gains a trailing `device_access_time_us: u64`
    // (miner-reported compute time: QPU access time for QPU wins, wall clock
    // for CPU/GPU), carried through `ProofRecord` and persisted as a trailing
    // field on `QBlock`. QuantumPow pallet storage version goes 4 → 5: the
    // deployed-v4 path appends `device_access_time_us = 0` preserving each
    // block's stored `topology_hash`; the pre-v4 path re-encodes from the
    // 7-field layout backfilling both trailing fields. Read-only runtime API
    // shape change (`QBlock`/`QBlockWithNonce`). `submit_proof`'s argument
    // encoding changed, so `transaction_version` moves to 5.
    // Bumped for pallet-revive (idx 14), its Ethereum runtime APIs, EVM-aware
    // unchecked-extrinsic wrapper, and `EthSetOrigin` transaction extension.
    // The extension set and accepted extrinsic forms change, so
    // `transaction_version` moves to 6. First shipped as 114 in the v0.2.2-rc
    // tags (113 was only an intermediate branch value and never released).
    // Bumped to 115 for the post-tag main build after the benchmark weight
    // regeneration. No call encodings changed, so `transaction_version` stays
    // at 6. 115 has not shipped. Later storage-only changes (pallet-evm-chain-id)
    // stay on 115 until a 115 runtime is live.
    // Bumped to 116 for the H2/H4 chain-wipe relaunch and to publish Metadata
    // V16 from the legacy metadata runtime API (`state_getMetadata`), which
    // previously returned the V14 inherent default. The versioned metadata API
    // keeps serving 14/15/16. The H1/H3 (ML-DSA-44) to H2/H4 (FN-DSA-512)
    // scheme change uses new public-key and signature encodings, so old signed
    // extrinsics are incompatible and `transaction_version` moves to 7.
    // Bumped to 117 to hard-invalidate 116 nodes: the crates.io pqhybridsign
    // rc5 switch and repins carry no interface change, but a spec bump makes
    // any node still on 116 refuse the new runtime outright.
    // Extended 117 with custody pallets starting at index 16. This unreleased
    // runtime keeps the existing extrinsic format, so transaction version 7
    // remains unchanged.
    // Bumped to 118: quantum-pow difficulty decay is continuous per block
    // instead of stepped per 100-block epoch, and per-win hardening is capped
    // at one epoch of decay. Same storage, same calls; a 117 node computes a
    // different threshold from identical state, so the version must move.
    // `transaction_version` stays at 7.
    spec_version: 118,
    impl_version: 1,
    apis: apis::RUNTIME_API_VERSIONS,
    transaction_version: 7,
    system_version: 1,
};

mod block_times {
    /// This determines the average expected block time that we are targeting. Blocks will be
    /// produced at a minimum duration defined by `SLOT_DURATION`. `SLOT_DURATION` is picked up by
    /// `pallet_timestamp` which is in turn picked up by `pallet_babe`.
    ///
    /// Change this to adjust the block time.
    pub const MILLI_SECS_PER_BLOCK: u64 = 6000;

    // NOTE: Currently it is not possible to change the slot duration after the chain has started.
    // Attempting to do so will brick block production.
    pub const SLOT_DURATION: u64 = MILLI_SECS_PER_BLOCK;
}
pub use block_times::*;

// Time is measured by number of blocks.
pub const MINUTES: BlockNumber = 60_000 / (MILLI_SECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

pub const BLOCK_HASH_COUNT: BlockNumber = 2400;

// Unit = the base number of indivisible units for balances
pub const UNIT: Balance = 1_000_000_000_000;
pub const MILLI_UNIT: Balance = 1_000_000_000;
pub const MICRO_UNIT: Balance = 1_000_000;

/// Existential deposit.
pub const EXISTENTIAL_DEPOSIT: Balance = MILLI_UNIT;

/// The BABE epoch configuration at genesis.
pub const BABE_GENESIS_EPOCH_CONFIG: sp_consensus_babe::BabeEpochConfiguration =
    sp_consensus_babe::BabeEpochConfiguration {
        c: (1, 4),
        allowed_slots: sp_consensus_babe::AllowedSlots::PrimaryAndSecondaryPlainSlots,
    };

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// Hybrid transaction signature used for runtime extrinsics.
pub type Signature = HybridTxSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Balance of an account.
pub type Balance = u128;

/// Index of a transaction in the chain.
pub type Nonce = u32;

/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;

/// An index to a block.
pub type BlockNumber = u32;

/// The address format for describing accounts.
pub type Address = MultiAddress<AccountId, ()>;

/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;

/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;

/// The `TransactionExtension` to the basic transaction logic.
pub type TxExtension = (
    frame_system::AuthorizeCall<Runtime>,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
    pallet_revive::evm::tx_extension::SetOrigin<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);

fn tx_extension(
    era: generic::Era,
    nonce: Nonce,
    tip: Balance,
    revive_origin: pallet_revive::evm::tx_extension::SetOrigin<Runtime>,
) -> TxExtension {
    (
        frame_system::AuthorizeCall::<Runtime>::new(),
        frame_system::CheckNonZeroSender::<Runtime>::new(),
        frame_system::CheckSpecVersion::<Runtime>::new(),
        frame_system::CheckTxVersion::<Runtime>::new(),
        frame_system::CheckGenesis::<Runtime>::new(),
        frame_system::CheckEra::<Runtime>::from(era),
        frame_system::CheckNonce::<Runtime>::from(nonce),
        frame_system::CheckWeight::<Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
        frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
        revive_origin,
        frame_system::WeightReclaim::<Runtime>::new(),
    )
}

/// Construct the extension tuple used by Quip's native signed transactions.
/// Keeping this in the runtime prevents node-side transaction builders from
/// drifting when the ordered extension set changes.
pub fn native_tx_extension(era: generic::Era, nonce: Nonce, tip: Balance) -> TxExtension {
    tx_extension(era, nonce, tip, Default::default())
}

/// Builds the normal transaction extensions used by an Ethereum transaction
/// after Revive has recovered and validated its secp256k1 signer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EthExtraImpl;

impl EthExtra for EthExtraImpl {
    type Config = Runtime;
    type Extension = TxExtension;

    fn get_eth_extension(nonce: u32, tip: Balance) -> Self::Extension {
        tx_extension(
            generic::Era::Immortal,
            nonce,
            tip,
            pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::new_from_eth_transaction(),
        )
    }
}

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
    pallet_revive::evm::runtime::UncheckedExtrinsic<Address, Signature, EthExtraImpl>;

/// The payload being signed in transactions.
pub type SignedPayload = generic::SignedPayload<RuntimeCall, TxExtension>;

/// Runtime storage migrations, run on upgrade before every pallet's
/// `on_runtime_upgrade`.
pub type Migrations = (
    pallet_miner_registry::migrations::v2::MigrateToV2<Runtime>,
    InitializeReviveAccount,
);

/// Reproduces Revive's genesis-time pallet-account initialization when the
/// pallet is introduced to an already-running chain by runtime upgrade.
///
/// The account-existence guard makes this safe and idempotent on later
/// upgrades and on chains whose genesis already included Revive.
pub struct InitializeReviveAccount;

impl OnRuntimeUpgrade for InitializeReviveAccount {
    fn on_runtime_upgrade() -> frame_support::weights::Weight {
        let account = Revive::account_id();
        let db_weight = <Runtime as frame_system::Config>::DbWeight::get();

        if System::account_exists(&account) {
            return db_weight.reads(1);
        }

        let minimum_balance =
            <Balances as frame_support::traits::fungible::Inspect<AccountId>>::minimum_balance();
        assert!(
            <Balances as Mutate<AccountId>>::mint_into(&account, minimum_balance).is_ok(),
            "Revive pallet account must be initialized during runtime upgrade",
        );

        // Account-existence check plus balance-account and total-issuance
        // reads; minting writes the latter two storage entries.
        db_weight.reads_writes(3, 2)
    }
}

/// Executive: handles dispatch to the various modules.
pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
    Migrations,
>;

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
    use quip_transaction_crypto::{account_id_from_public, HybridPair, HybridTxSignature};
    use sp_core::Pair as _;
    use sp_runtime::{traits::Checkable, transaction_validity::InvalidTransaction, BuildStorage};

    fn signed_test_extrinsic(
        sender: &HybridPair,
        address: Address,
        call: RuntimeCall,
        nonce: u32,
    ) -> UncheckedExtrinsic {
        let tx_ext = native_tx_extension(generic::Era::Immortal, nonce, 0);

        let payload = SignedPayload::new(call.clone(), tx_ext.clone()).unwrap();
        let signature = payload.using_encoded(|encoded| HybridTxSignature::sign(sender, encoded));

        generic::UncheckedExtrinsic::new_signed(call, address, signature, tx_ext).into()
    }

    #[test]
    fn hybrid_signed_extrinsic_checks_successfully() {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let sender = HybridPair::from_string("//Alice", None).unwrap();
            let account_id = account_id_from_public(&sender.public());
            let xt = signed_test_extrinsic(
                &sender,
                account_id.clone().into(),
                SystemCall::remark { remark: vec![] }.into(),
                0,
            );

            let lookup = frame_system::ChainContext::<Runtime>::default();
            let checked =
                <UncheckedExtrinsic as Checkable<frame_system::ChainContext<Runtime>>>::check(
                    xt, &lookup,
                );

            assert!(checked.is_ok());
        });
    }

    /// Confirms the runtime's `CanonicalDefaultIsingSpecId` resolves to the
    /// same hash that the pallet's mock test pins. SDKs and downstream docs
    /// embed this hash; a mock-vs-runtime divergence would silently break
    /// every client that hardcodes it.
    #[test]
    fn default_ising_spec_id_matches_pinned_hash() {
        use frame_support::traits::Get as _;
        let id = <Runtime as pallet_quantum_compute_mempool::Config>::DefaultIsingSpecId::get();
        assert_eq!(
            format!("{id:?}"),
            "0x8f46f3a31321d1d093314fc769c42cbe7a83d71a0b69e6571a0f68e2a04067f0",
        );
    }

    #[test]
    fn hybrid_signed_extrinsic_rejects_wrong_account() {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let sender = HybridPair::from_string("//Alice", None).unwrap();
            let wrong = HybridPair::from_string("//Bob", None).unwrap();
            let wrong_account = account_id_from_public(&wrong.public());
            let xt = signed_test_extrinsic(
                &sender,
                wrong_account.into(),
                SystemCall::remark { remark: vec![] }.into(),
                0,
            );

            let lookup = frame_system::ChainContext::<Runtime>::default();
            let checked =
                <UncheckedExtrinsic as Checkable<frame_system::ChainContext<Runtime>>>::check(
                    xt, &lookup,
                );

            assert_eq!(checked.unwrap_err(), InvalidTransaction::BadProof.into());
        });
    }

    #[test]
    fn balances_support_plain_reserve_round_trip() {
        use frame_support::traits::{Currency, ReservableCurrency};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            let account =
                account_id_from_public(&HybridPair::from_string("//Alice", None).unwrap().public());
            let reserve = 5 * UNIT;
            <Balances as Currency<AccountId>>::make_free_balance_be(&account, 10 * UNIT);

            assert!(Balances::reserve(&account, reserve).is_ok());
            assert_eq!(Balances::reserved_balance(&account), reserve);
            assert_eq!(Balances::unreserve(&account, reserve), 0);
            assert_eq!(Balances::reserved_balance(&account), 0);
        });
    }

    #[test]
    fn h4_multisig_uses_hash_approval_then_executes_full_call() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Currency, weights::Weight};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let target = account_id_from_public(
                &HybridPair::from_string("//Dave", None).unwrap().public(),
            );
            let mut signatories = vec![
                account_id_from_public(&alice.public()),
                account_id_from_public(&bob.public()),
                account_id_from_public(&charlie.public()),
            ];
            signatories.sort();

            for account in &signatories {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }
            let multisig = Multisig::multi_account_id(&signatories, 2);
            <Balances as Currency<AccountId>>::make_free_balance_be(&multisig, 10 * UNIT);

            let inner: RuntimeCall = BalancesCall::transfer_allow_death {
                dest: Address::Id(target.clone()),
                value: 3 * UNIT,
            }
            .into();
            let call_hash = sp_io::hashing::blake2_256(&inner.encode());

            let first_account = account_id_from_public(&alice.public());
            let mut first_others = signatories
                .iter()
                .filter(|account| **account != first_account)
                .cloned()
                .collect::<Vec<_>>();
            first_others.sort();
            let approval: RuntimeCall = pallet_multisig::Call::approve_as_multi {
                threshold: 2,
                other_signatories: first_others,
                maybe_timepoint: None,
                call_hash,
                max_weight: Weight::zero(),
            }
            .into();
            let approval_xt = signed_test_extrinsic(
                &alice,
                Address::Id(first_account),
                approval,
                0,
            );
            let approval_size = approval_xt.encode().len();
            assert!(Executive::apply_extrinsic(approval_xt).unwrap().is_ok());

            let timepoint = pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash)
                .expect("first approval creates multisig state")
                .when;
            let final_account = account_id_from_public(&bob.public());
            let mut final_others = signatories
                .iter()
                .filter(|account| **account != final_account)
                .cloned()
                .collect::<Vec<_>>();
            final_others.sort();
            let final_call: RuntimeCall = pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: final_others,
                maybe_timepoint: Some(timepoint),
                max_weight: inner.get_dispatch_info().call_weight,
                call: alloc::boxed::Box::new(inner),
            }
            .into();
            let final_xt =
                signed_test_extrinsic(&bob, Address::Id(final_account), final_call, 0);
            let final_size = final_xt.encode().len();
            assert!(Executive::apply_extrinsic(final_xt).unwrap().is_ok());

            eprintln!(
                "H4 multisig extrinsic sizes: approve_as_multi={approval_size}, as_multi={final_size}"
            );
            assert_eq!(Balances::free_balance(target), 3 * UNIT);
            assert!(pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash).is_none());
        });
    }

    #[test]
    fn h4_utility_batch_all_and_derivative_transfer_work() {
        use frame_support::traits::Currency;
        use sp_core::crypto::Ss58Codec;

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let alice_account = account_id_from_public(&alice.public());
            <Balances as Currency<AccountId>>::make_free_balance_be(&alice_account, 100 * UNIT);

            let recipients = ["//Bob", "//Charlie", "//Dave"].map(|uri| {
                account_id_from_public(&HybridPair::from_string(uri, None).unwrap().public())
            });
            let calls = recipients
                .iter()
                .enumerate()
                .map(|(index, recipient)| {
                    BalancesCall::transfer_allow_death {
                        dest: Address::Id(recipient.clone()),
                        value: (index as Balance + 1) * UNIT,
                    }
                    .into()
                })
                .collect();
            let batch: RuntimeCall = pallet_utility::Call::batch_all { calls }.into();
            let batch_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), batch, 0);
            assert!(Executive::apply_extrinsic(batch_xt).unwrap().is_ok());
            assert_eq!(Balances::free_balance(&recipients[0]), UNIT);
            assert_eq!(Balances::free_balance(&recipients[1]), 2 * UNIT);
            assert_eq!(Balances::free_balance(&recipients[2]), 3 * UNIT);

            let derivative = pallet_utility::derivative_account_id(alice_account.clone(), 7);
            <Balances as Currency<AccountId>>::make_free_balance_be(&derivative, 5 * UNIT);
            let derivative_target =
                account_id_from_public(&HybridPair::from_string("//Eve", None).unwrap().public());
            let derivative_call: RuntimeCall = pallet_utility::Call::as_derivative {
                index: 7,
                call: alloc::boxed::Box::new(
                    BalancesCall::transfer_allow_death {
                        dest: Address::Id(derivative_target.clone()),
                        value: 2 * UNIT,
                    }
                    .into(),
                ),
            }
            .into();
            let derivative_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account), derivative_call, 1);
            assert!(Executive::apply_extrinsic(derivative_xt).unwrap().is_ok());

            eprintln!(
                "utility derivative index 7 address: {}",
                derivative.to_ss58check()
            );
            assert_eq!(Balances::free_balance(derivative_target), 2 * UNIT);
        });
    }

    #[test]
    fn runtime_call_fits_utility_batching_limit() {
        assert!(core::mem::size_of::<RuntimeCall>() <= 1024);
    }

    #[test]
    fn h4_multisig_can_execute_utility_batch() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Currency, weights::Weight};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let mut signatories = vec![
                account_id_from_public(&alice.public()),
                account_id_from_public(&bob.public()),
                account_id_from_public(&charlie.public()),
            ];
            signatories.sort();
            for account in &signatories {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }
            let multisig = Multisig::multi_account_id(&signatories, 2);
            <Balances as Currency<AccountId>>::make_free_balance_be(&multisig, 10 * UNIT);

            let recipients = ["//Dave", "//Eve"].map(|uri| {
                account_id_from_public(&HybridPair::from_string(uri, None).unwrap().public())
            });
            let inner: RuntimeCall = pallet_utility::Call::batch_all {
                calls: recipients
                    .iter()
                    .map(|recipient| {
                        BalancesCall::transfer_allow_death {
                            dest: Address::Id(recipient.clone()),
                            value: 2 * UNIT,
                        }
                        .into()
                    })
                    .collect(),
            }
            .into();
            let call_hash = sp_io::hashing::blake2_256(&inner.encode());

            let alice_account = account_id_from_public(&alice.public());
            let mut alice_others = signatories
                .iter()
                .filter(|account| **account != alice_account)
                .cloned()
                .collect::<Vec<_>>();
            alice_others.sort();
            let approval: RuntimeCall = pallet_multisig::Call::approve_as_multi {
                threshold: 2,
                other_signatories: alice_others,
                maybe_timepoint: None,
                call_hash,
                max_weight: Weight::zero(),
            }
            .into();
            let approval_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account), approval, 0);
            assert!(Executive::apply_extrinsic(approval_xt).unwrap().is_ok());

            let timepoint = pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash)
                .expect("first approval creates multisig state")
                .when;
            let bob_account = account_id_from_public(&bob.public());
            let mut bob_others = signatories
                .iter()
                .filter(|account| **account != bob_account)
                .cloned()
                .collect::<Vec<_>>();
            bob_others.sort();
            let execution: RuntimeCall = pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: bob_others,
                maybe_timepoint: Some(timepoint),
                max_weight: inner.get_dispatch_info().call_weight,
                call: alloc::boxed::Box::new(inner),
            }
            .into();
            let execution_xt = signed_test_extrinsic(&bob, Address::Id(bob_account), execution, 0);
            assert!(Executive::apply_extrinsic(execution_xt).unwrap().is_ok());

            assert_eq!(Balances::free_balance(&recipients[0]), 2 * UNIT);
            assert_eq!(Balances::free_balance(&recipients[1]), 2 * UNIT);
        });
    }

    #[test]
    fn h4_proxy_filters_calls_rejects_announcements_and_creates_pure_accounts() {
        use frame_support::traits::Currency;
        use sp_runtime::traits::{BlakeTwo256, Hash as _};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let alice_account = account_id_from_public(&alice.public());
            let bob_account = account_id_from_public(&bob.public());
            let charlie_account = account_id_from_public(&charlie.public());
            for account in [&alice_account, &bob_account, &charlie_account] {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }

            let add_immediate: RuntimeCall = pallet_proxy::Call::add_proxy {
                delegate: Address::Id(bob_account.clone()),
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 0,
            }
            .into();
            let add_immediate_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), add_immediate, 0);
            assert!(Executive::apply_extrinsic(add_immediate_xt)
                .unwrap()
                .is_ok());

            let recipient =
                account_id_from_public(&HybridPair::from_string("//Dave", None).unwrap().public());
            let transfer: RuntimeCall = BalancesCall::transfer_allow_death {
                dest: Address::Id(recipient.clone()),
                value: 4 * UNIT,
            }
            .into();
            let proxied_transfer: RuntimeCall = pallet_proxy::Call::proxy {
                real: Address::Id(alice_account.clone()),
                force_proxy_type: Some(configs::ProxyType::TransferOnly),
                call: alloc::boxed::Box::new(transfer),
            }
            .into();
            let transfer_xt =
                signed_test_extrinsic(&bob, Address::Id(bob_account.clone()), proxied_transfer, 0);
            assert!(Executive::apply_extrinsic(transfer_xt).unwrap().is_ok());
            assert_eq!(Balances::free_balance(&recipient), 4 * UNIT);

            let rejected: RuntimeCall = pallet_proxy::Call::proxy {
                real: Address::Id(alice_account.clone()),
                force_proxy_type: Some(configs::ProxyType::TransferOnly),
                call: alloc::boxed::Box::new(SystemCall::remark { remark: vec![1] }.into()),
            }
            .into();
            let rejected_xt =
                signed_test_extrinsic(&bob, Address::Id(bob_account.clone()), rejected, 1);
            assert!(Executive::apply_extrinsic(rejected_xt).unwrap().is_ok());
            assert!(System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::Proxy(pallet_proxy::Event::ProxyExecuted { result: Err(_) })
            )));

            let add_delayed: RuntimeCall = pallet_proxy::Call::add_proxy {
                delegate: Address::Id(charlie_account.clone()),
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 3,
            }
            .into();
            let add_delayed_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), add_delayed, 1);
            assert!(Executive::apply_extrinsic(add_delayed_xt).unwrap().is_ok());

            let delayed_call: RuntimeCall = BalancesCall::transfer_keep_alive {
                dest: Address::Id(recipient),
                value: UNIT,
            }
            .into();
            let delayed_hash = BlakeTwo256::hash_of(&alloc::boxed::Box::new(delayed_call.clone()));
            let announce: RuntimeCall = pallet_proxy::Call::announce {
                real: Address::Id(alice_account.clone()),
                call_hash: delayed_hash,
            }
            .into();
            let announce_xt =
                signed_test_extrinsic(&charlie, Address::Id(charlie_account.clone()), announce, 0);
            assert!(Executive::apply_extrinsic(announce_xt).unwrap().is_ok());
            assert_eq!(
                pallet_proxy::Announcements::<Runtime>::get(&charlie_account)
                    .0
                    .len(),
                1
            );

            // The real account can reject the announced call immediately,
            // before the three-block execution delay has elapsed.
            let reject: RuntimeCall = pallet_proxy::Call::reject_announcement {
                delegate: Address::Id(charlie_account.clone()),
                call_hash: delayed_hash,
            }
            .into();
            let reject_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), reject, 2);
            assert!(Executive::apply_extrinsic(reject_xt).unwrap().is_ok());
            assert!(
                pallet_proxy::Announcements::<Runtime>::get(&charlie_account)
                    .0
                    .is_empty()
            );

            let create_pure: RuntimeCall = pallet_proxy::Call::create_pure {
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 0,
                index: 9,
            }
            .into();
            let create_pure_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), create_pure, 3);
            assert!(Executive::apply_extrinsic(create_pure_xt).unwrap().is_ok());
            let pure = System::events()
                .iter()
                .find_map(|record| match &record.event {
                    RuntimeEvent::Proxy(pallet_proxy::Event::PureCreated { pure, .. }) => {
                        Some(pure.clone())
                    }
                    _ => None,
                })
                .expect("create_pure emits the custody account");
            assert!(pallet_proxy::Proxies::<Runtime>::contains_key(pure));
        });
    }

    #[test]
    fn h4_multisig_account_can_delegate_to_transfer_proxy() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Currency, weights::Weight};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let delegate = HybridPair::from_string("//Dave", None).unwrap();
            let mut signatories = vec![
                account_id_from_public(&alice.public()),
                account_id_from_public(&bob.public()),
                account_id_from_public(&charlie.public()),
            ];
            signatories.sort();
            for account in &signatories {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }
            let delegate_account = account_id_from_public(&delegate.public());
            <Balances as Currency<AccountId>>::make_free_balance_be(&delegate_account, 100 * UNIT);
            let multisig = Multisig::multi_account_id(&signatories, 2);
            <Balances as Currency<AccountId>>::make_free_balance_be(&multisig, 20 * UNIT);

            let add_proxy: RuntimeCall = pallet_proxy::Call::add_proxy {
                delegate: Address::Id(delegate_account.clone()),
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 0,
            }
            .into();
            let call_hash = sp_io::hashing::blake2_256(&add_proxy.encode());
            let alice_account = account_id_from_public(&alice.public());
            let mut alice_others = signatories
                .iter()
                .filter(|account| **account != alice_account)
                .cloned()
                .collect::<Vec<_>>();
            alice_others.sort();
            let approval: RuntimeCall = pallet_multisig::Call::approve_as_multi {
                threshold: 2,
                other_signatories: alice_others,
                maybe_timepoint: None,
                call_hash,
                max_weight: Weight::zero(),
            }
            .into();
            let approval_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account), approval, 0);
            assert!(Executive::apply_extrinsic(approval_xt).unwrap().is_ok());

            let timepoint = pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash)
                .expect("first approval creates multisig state")
                .when;
            let bob_account = account_id_from_public(&bob.public());
            let mut bob_others = signatories
                .iter()
                .filter(|account| **account != bob_account)
                .cloned()
                .collect::<Vec<_>>();
            bob_others.sort();
            let execute_add: RuntimeCall = pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: bob_others,
                maybe_timepoint: Some(timepoint),
                max_weight: add_proxy.get_dispatch_info().call_weight,
                call: alloc::boxed::Box::new(add_proxy),
            }
            .into();
            let execute_add_xt =
                signed_test_extrinsic(&bob, Address::Id(bob_account), execute_add, 0);
            assert!(Executive::apply_extrinsic(execute_add_xt).unwrap().is_ok());
            assert_eq!(pallet_proxy::Proxies::<Runtime>::get(&multisig).0.len(), 1);

            let recipient =
                account_id_from_public(&HybridPair::from_string("//Eve", None).unwrap().public());
            let proxy_transfer: RuntimeCall = pallet_proxy::Call::proxy {
                real: Address::Id(multisig.clone()),
                force_proxy_type: Some(configs::ProxyType::TransferOnly),
                call: alloc::boxed::Box::new(
                    BalancesCall::transfer_allow_death {
                        dest: Address::Id(recipient.clone()),
                        value: 3 * UNIT,
                    }
                    .into(),
                ),
            }
            .into();
            let proxy_transfer_xt =
                signed_test_extrinsic(&delegate, Address::Id(delegate_account), proxy_transfer, 0);
            assert!(Executive::apply_extrinsic(proxy_transfer_xt)
                .unwrap()
                .is_ok());
            assert_eq!(Balances::free_balance(recipient), 3 * UNIT);
        });
    }

    #[test]
    fn revive_configuration_matches_network_build() {
        assert_eq!(
            <Revive as frame_support::traits::PalletInfoAccess>::index(),
            14
        );
        assert_eq!(
            <EvmChainId as frame_support::traits::PalletInfoAccess>::index(),
            15
        );

        assert_eq!(configs::ReviveDepositPerByte::get(), 10 * MICRO_UNIT);
        assert_eq!(configs::ReviveDepositPerItem::get(), 200 * MILLI_UNIT);
        assert_eq!(
            configs::ReviveDepositPerChildTrieItem::get(),
            2 * MILLI_UNIT
        );
        assert_eq!(
            configs::ReviveCodeHashLockupDepositPercent::get(),
            sp_runtime::Perbill::from_percent(30)
        );
        assert_eq!(
            <<Runtime as pallet_revive::Config>::NativeToEthRatio as frame_support::traits::Get<
                u32,
            >>::get(),
            10u32.pow(18 - 12)
        );

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| {
            assert_eq!(
                <pallet_evm_chain_id::ChainId<Runtime> as frame_support::traits::Get<u64>>::get(),
                pallet_evm_chain_id::TESTNET_CHAIN_ID
            );
            assert_eq!(
                Revive::evm_base_fee(),
                sp_core::U256::from(1_000_000_000u64)
            );
        });
    }

    #[test]
    fn revive_upgrade_initialization_is_idempotent() {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            let account = Revive::account_id();
            assert!(System::account_exists(&account));

            frame_system::Account::<Runtime>::remove(&account);
            assert!(!System::account_exists(&account));

            InitializeReviveAccount::on_runtime_upgrade();
            assert!(System::account_exists(&account));
            assert_eq!(
                frame_system::Account::<Runtime>::get(&account).data.free,
                EXISTENTIAL_DEPOSIT
            );

            InitializeReviveAccount::on_runtime_upgrade();
            assert_eq!(
                frame_system::Account::<Runtime>::get(&account).data.free,
                EXISTENTIAL_DEPOSIT
            );
        });
    }
}

// Create the runtime by composing the FRAME pallets that were previously configured.
#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Runtime;

    #[runtime::pallet_index(0)]
    pub type System = frame_system;

    #[runtime::pallet_index(1)]
    pub type Timestamp = pallet_timestamp;

    #[runtime::pallet_index(2)]
    pub type Babe = pallet_babe;

    #[runtime::pallet_index(3)]
    pub type Grandpa = pallet_grandpa;

    #[runtime::pallet_index(4)]
    pub type Balances = pallet_balances;

    #[runtime::pallet_index(5)]
    pub type TransactionPayment = pallet_transaction_payment;

    #[runtime::pallet_index(6)]
    pub type Sudo = pallet_sudo;

    // Include the custom logic from the pallet-template in the runtime.
    #[runtime::pallet_index(7)]
    pub type Template = pallet_template;

    #[runtime::pallet_index(8)]
    pub type Xqvm = pallet_xqvm;

    #[runtime::pallet_index(9)]
    pub type QuantumComputeMempool = pallet_quantum_compute_mempool;

    #[runtime::pallet_index(10)]
    pub type QuantumPow = pallet_quantum_pow;

    #[runtime::pallet_index(11)]
    pub type FaucetOps = pallet_faucet_ops;

    #[runtime::pallet_index(12)]
    pub type Session = pallet_session;

    #[runtime::pallet_index(13)]
    pub type MinerRegistry = pallet_miner_registry;

    #[runtime::pallet_index(14)]
    pub type Revive = pallet_revive;

    #[runtime::pallet_index(15)]
    pub type EvmChainId = pallet_evm_chain_id;

    #[runtime::pallet_index(16)]
    pub type Multisig = pallet_multisig;

    #[runtime::pallet_index(17)]
    pub type Utility = pallet_utility;

    #[runtime::pallet_index(18)]
    pub type Proxy = pallet_proxy;
}

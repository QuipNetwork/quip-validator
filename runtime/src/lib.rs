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

// To learn more about runtime versioning, see:
// https://docs.substrate.io/main-docs/build/upgrade#runtime-versioning
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: alloc::borrow::Cow::Borrowed("quip"),
    impl_name: alloc::borrow::Cow::Borrowed("quip"),
    authoring_version: 1,
    // A full node will not use its native runtime in place of the on-chain Wasm
    // runtime unless `spec_name`, `spec_version` and `authoring_version` match.
    //
    // DEPLOYED HISTORY.
    // 101: signed-extrinsic wire format `MultiSignature` -> hybrid envelope.
    //   `transaction_version` -> 2.
    // 102 (v0.2.0): adds `pallet_faucet_ops` (11) and `pallet_session` (12).
    //   Metadata only, so `transaction_version` stays 2.
    // 103 (QUI-567): canonical default plain Ising job spec; root-gates
    //   `QuantumComputeMempool::register_job_spec` and changes its argument
    //   encoding, so `transaction_version` -> 3.
    // 104-107 all leave `transaction_version` at 3:
    //   104: adds `QuantumPow::set_default_topology` (5); difficulty energy
    //     curve becomes spec-aware (h/J magnitudes from the default topology's
    //     allowed-value specs, not hardcoded ternary-h / binary-J).
    //   105: monotonic qblock ids, qblock/hardness runtime APIs, mempool
    //     open-order recovery index.
    //   106: adds `MinerRegistry` (13) with descriptor/participation calls.
    //   107: `ParticipantsByQBlock`/`ParticipantCountByQBlock` reverse index
    //     and `MinerRegistryApi`.
    // 108: per-topology difficulty + mineable whitelist. `Difficulty`
    //   (StorageValue) becomes `Difficulties` (StorageMap by topology hash),
    //   `MineableTopologies` added, `set_difficulty` gains `topology_hash`,
    //   `add_mineable_topology`/`remove_mineable_topology` (6/7) added.
    //   `transaction_version` -> 4. QuantumPow storage 2 -> 3, carry-forward.
    // 109-111 all leave `transaction_version` at 4:
    //   109: `MinerRegistry` gains `NodeDescriptorInput::V2` (optional typed
    //     hardware survey) and a v1 -> v2 migration that DROPS existing
    //     descriptors (miners re-file on restart). V1 keeps index 0 and encodes
    //     identically. MinerRegistry storage 1 -> 2.
    //   110: optional `runtime` block (python / quip_version /
    //     protocol_version / in_docker / docker_image) on the V2 descriptor.
    //     Additive trailing field on an input that had not shipped, so no new
    //     migration; MinerRegistry storage stays 2.
    //   111 (110 had already shipped in v0.2.1-rc11): `QBlock` gains a trailing
    //     `topology_hash`, so QuantumPow storage 3 -> 4 re-encodes existing
    //     entries, backfilling the default topology. Adds sudo-only
    //     `set_topology_curve`; `submit_proof` weight becomes dimension-scaled
    //     (QIP-03) instead of a flat 60M placeholder.
    // 112: `QuantumProof` gains a trailing `device_access_time_us: u64` (QPU
    //   access time for QPU wins, wall clock for CPU/GPU), carried on
    //   `ProofRecord` and persisted on `QBlock`. QuantumPow storage 4 -> 5: the
    //   deployed-v4 path appends `device_access_time_us = 0` keeping each
    //   block's `topology_hash`; the pre-v4 path re-encodes the 7-field layout,
    //   backfilling both. `submit_proof`'s encoding changed, so
    //   `transaction_version` -> 5.
    // 114 (v0.2.2-rc): pallet-revive EVM (idx 14), its Ethereum runtime APIs,
    //   the EVM-aware unchecked-extrinsic wrapper and the `EthSetOrigin`
    //   transaction extension. The extension set and accepted extrinsic forms
    //   change, so `transaction_version` -> 6. (113 was only an intermediate
    //   branch value and never released.)
    // 115: post-tag main build after the benchmark weight regeneration. No
    //   call encodings changed, so `transaction_version` stays 6.
    // 116: the legacy metadata runtime API (`state_getMetadata`) publishes
    //   Metadata V16 (previously the V14 inherent default); the versioned
    //   metadata API keeps serving 14/15/16. Consensus and the extrinsic wire
    //   format are unchanged, so `transaction_version` stays 6.
    // ─────────────────────────────────────────────────────────────────────
    // 117: single-dial PoUW difficulty. ONE bump covers all of
    // `v0.2.1/generic-graph-difficulty`. The branch briefly minted 113 through
    // 119 before its rebase onto main's 116; none was deployed, so they fold
    // into one step from what is actually out there. QuantumPow pallet storage
    // moves 5 -> 6 exactly once, for the same reason: 7 and 8 were minted on
    // this branch and no chain ever ran them, so the three steps are one v6
    // that chains from the deployed 5 in a single upgrade.
    // `transaction_version` moves 6 -> 7 once, covering both
    // `register_topology`'s trailing `TopologyHardness` and `submit_proof`'s
    // `solutions` field, which changes from
    // `BoundedVec<PackedSpinBytes, MaxSolutions>` to a single `PackedSpinBytes`
    // (see `QuantumProof::solutions`). Two encoding changes, one step, because
    // neither has shipped. Those entries survive below as `ALSO IN 117`.
    // ─────────────────────────────────────────────────────────────────────
    // `DifficultyConfig` sheds `min_solutions` and `min_diversity_milli`. A
    // proof carries exactly one configuration (now by type) and the energy
    // bar alone decides — one exact energy evaluation per proof.
    //
    // INSTANCE PRICING. The salt picks the instance. The load-bearing grind
    // defense is a validity gate, not interpolation: a draw more than three
    // sigma from the topology's expected frustration is refused
    // (`InstanceOutsideFrustrationBand`), as is a gauge-trivial (zero
    // frustrated cycles) draw (`InstanceIsGaugeTrivial`). Both are cheap to
    // predict off-chain — re-salt. Inside the band a token handicap of
    // `HandicapPerSigmaPermille` (default 6‰ of the curve bar per σ, withheld
    // when there is no room to move both ways) nudges the bar. An earlier
    // interpolation from the curve to `-(Σ|h|+Σ|J|)` made ordinary −1σ draws
    // unmineable; that form is not what ships. Block selection ranks by
    // `proof_quality` = (bar − energy) / (bar − anchor), not by raw margin
    // or absolute energy.
    //
    // New calls: root `set_topology_hardness` (9); permissionless
    // `prove_topology_exact` (10) — an elimination order of induced width at or
    // below `ExactSolveCeiling` ratchets the recorded width down and retires
    // the topology from mining unless it is the live default; permissionless
    // `prove_topology_planar` (11) — a zero-field planar Ising is max-cut on a
    // planar graph, polynomial-time at any width, which width alone cannot
    // detect. OPERATIONAL: `prove_topology_planar` can retire the LIVE DEFAULT
    // topology and halt qblock production until governance repoints.
    // Deliberate — paying for polynomial-time work is worse than pausing — but
    // one transaction can now stop block rewards. `register_topology` and
    // `set_topology_hardness` both derive `expected_frustration_milli` when
    // the coupling spec determines it, so a wrong declaration cannot brick
    // every honest `submit_proof`. `register_topology` gains a trailing
    // `TopologyHardness`; that encoding change is what moves
    // `transaction_version` to 7. Regime is derived from width and ceiling
    // rather than stored, so raising the ceiling re-classifies every topology
    // in one upgrade; `TopologyHardness` keeps `residual_difficulty` and
    // `core_width` separately and the regime takes the minimum.
    //
    // Difficulty: the epoch retarget (`DifficultyRetargeted`) WRITES the
    // stored baseline. Decay still EASES the bar miners see on read, one
    // step per `EpochLength` since the last qblock, without committing that
    // ease into storage — a view-only stall ease, not a second writer.
    // Per-proof adjustment, dominant-winner easing and
    // `ConsecutiveWinnerEasingThreshold` (and the `WinnerStreak` storage
    // they used) are removed. `DifficultyUpdated` no longer fires on a
    // qblock win (watch `DifficultyRetargeted`). `blocks_per_qblock` reports
    // 0 for an epoch that produced none. Only the default topology eases on
    // an empty epoch; a whitelisted incoming topology is idle, not stalled.
    // The window is twenty target intervals, not ten: arrivals are Poisson,
    // and at ten the clamp fired on ~43% of on-target windows.
    //
    // MINER-VISIBLE REFUSALS. An instance is REFUSED, not re-priced, when it
    // has no frustrated cycles (`InstanceIsGaugeTrivial`) or when its
    // frustration lands over three sigma either side of the topology's
    // expectation (`InstanceOutsideFrustrationBand`). Both are cheap to predict
    // off-chain before spending solve effort — re-salt. Registration derives
    // expected frustration for sign-symmetric coupling specs instead of
    // accepting the declared value, refuses specs that can draw a reachable
    // all-zero instance, and refuses a curve calibrated past the topology's own
    // typical weight.
    //
    // QuantumPow storage 5 -> 6, three steps in one migration.
    //
    // (a) Re-encodes `Difficulties` and `QBlocks`, backfilling each historical
    // block's `instance_bar_milli` from the bar it cleared. Kills leftover
    // `WinnerStreak`.
    //
    // (b) Canonicalizes each stored graph (nodes sorted, edges oriented then
    // sorted): `hash_topology` always hashed the canonical form while
    // `generate_ising_model` maps values POSITIONALLY, so the hash did not name
    // the instance the chain generates. Hashes are unchanged and keyed maps stay
    // valid. The same nonce against a previously non-canonical stored order
    // produces a DIFFERENT (h, j) after upgrade: miners must refresh topology
    // adjacency from chain state and discard in-flight pool proofs.
    // `prove_topology_planar` indexes its rotation into the stored edge list,
    // so an off-chain witness generator built against submission order MUST
    // be regenerated.
    //
    // (c) Adds `TopologyDims`, an eight-byte `(nodes, edges)` copy per topology
    // backfilled from `RegisteredTopologies`: pure redundancy, because the
    // weight closures for
    // `submit_proof` / `prove_topology_exact` / `prove_topology_planar` are
    // evaluated inside `validate_transaction` on every gossiped extrinsic
    // before any fee, and reading `RegisteredTopologies` there SCALE-decoded up
    // to ~370 KB per gossiped proof for two `len()`s. No call encoding change,
    // so `transaction_version` stays 7. The winning proof's difficulty also
    // rides on `ProofRecord` instead of being recomputed in `on_finalize`, so a
    // `QBlock` records the bar it was actually priced against.
    //
    // ALSO IN 117, none of which changes a call encoding:
    // (114 -> 115) the permissionless witness extrinsics report WHICH defect
    //   refused them instead of collapsing four (elimination order) and seven
    //   (rotation) failures into one error each. New variants are APPENDED, so
    //   no existing error index moves; unused `InvalidEliminationOrder` is
    //   retained for the same reason.
    // (115 -> 116) `HandicapPerSigmaPermille` (whose own measurement says the
    //   slope is not determinable over the range that matters) and
    //   `RetargetMaxStepPermille` become `#[pallet::constant]` Config values:
    //   retuning either is now a runtime upgrade, and both appear in metadata.
    //   `HANDICAP_MAX_SIGMA` gates `submit_proof` validity and IS
    //   consensus-critical, so it deliberately stays a code constant.
    // (116 -> 117) cleanup. `prove_topology_planar` no longer copies the
    //   submitted rotation (up to ~400 KB, one allocation per vertex, on a
    //   permissionless path); the dims backfill streams `iter()` instead of
    //   `iter_keys()` + per-key `get`, halving its reads; `proof_quality` takes
    //   a named struct, not three transposable `i64`s.
    // (117 -> 118) closes a chain-halt path: a negative
    //   `RetargetMaxStepPermille` made `Ord::clamp(min > max)` panic inside
    //   `on_finalize`, which cannot be refused. Guarded in `integrity_test` AND
    //   in `retarget_bar_milli`, since `integrity_test` is `#[cfg(test)]`-only
    //   and never runs on a live chain. Also splits two witness rejections that
    //   blamed the submitter for defects in the STORED topology, names
    //   `TopologyDims`' pair, and charges the dims backfill for every entry it
    //   traverses, not only those it writes.
    // (118 -> 119) `NodeIndex` resolves the LEFTMOST match on its fallback
    //   table; `binary_search_by_key` picks an arbitrary match among equal keys,
    //   so which node id a duplicate-node rejection named was an unspecified std
    //   detail a toolchain bump could flip. No caller inspects more than
    //   `is_empty()`, so nothing diverged. Also replaces a silent
    //   difficulty-controller freeze with a logged fallback to the default clamp.
    spec_version: 117,
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
}

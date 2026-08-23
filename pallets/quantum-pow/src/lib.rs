#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

mod benchmark_weights;
pub mod difficulty;
pub mod topology;
pub mod types;
pub mod weights;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use weights::*;

use frame_support::traits::Currency;

type AccountIdOf<T> = <T as frame_system::Config>::AccountId;
type BlockNumberOf<T> = frame_system::pallet_prelude::BlockNumberFor<T>;
type BalanceOf<T> =
    <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

type NodesOf<T> = frame_support::pallet_prelude::BoundedVec<u32, <T as Config>::MaxNodes>;
type EdgesOf<T> = frame_support::pallet_prelude::BoundedVec<(u32, u32), <T as Config>::MaxEdges>;
type AllowedValueSetOf<T> = frame_support::pallet_prelude::BoundedVec<
    quantum_validation::MilliValue,
    <T as Config>::MaxAllowedValues,
>;
type PackedSpinBytesOf<T> = frame_support::pallet_prelude::BoundedVec<u8, <T as Config>::MaxNodes>;
/// A proof carries exactly one configuration, so the wire type is that single packed
/// configuration rather than a collection. See `QuantumProof::solutions` for why, and
/// `runtime`'s `VERSION` for the `transaction_version` step it folds into.
type QuantumProofOf<T> = types::QuantumProof<PackedSpinBytesOf<T>>;
type TopologyMetaOf<T> =
    types::TopologyMeta<NodesOf<T>, EdgesOf<T>, AllowedValueSetOf<T>, BlockNumberOf<T>>;
type MinerInfoOf<T> = types::MinerInfo<BalanceOf<T>, BlockNumberOf<T>>;
type ProofRecordOf<T> = types::ProofRecord<AccountIdOf<T>, BlockNumberOf<T>>;
type WinnerStreakOf<T> = types::WinnerStreak<AccountIdOf<T>>;
type MiningSnapshotOf<T> = types::MiningSnapshot<NodesOf<T>, EdgesOf<T>, AllowedValueSetOf<T>>;
type QBlockOf<T> = types::QBlock<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>;
type QBlockWithNonceOf<T> = types::QBlockWithNonce<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>;

sp_api::decl_runtime_apis! {
    /// Version 2 adds the per-topology `difficulty_for` and the
    /// `mineable_topologies` whitelist query. Clients can feature-detect
    /// these via the reported API version against older runtimes.
    #[api_version(2)]
    pub trait QuantumPowApi<BlockNumber, AccountId, Balance, Nodes, Edges, AllowedValues>
    where
        BlockNumber: codec::Codec,
        AccountId: codec::Codec,
        Balance: codec::Codec,
        Nodes: codec::Codec,
        Edges: codec::Codec,
        AllowedValues: codec::Codec,
    {
        fn mining_snapshot(topology_hash: Option<sp_core::H256>) -> Option<
            crate::types::MiningSnapshot<Nodes, Edges, AllowedValues>
        >;

        /// Look up a registered topology by hash (nodes, edges, allowed value
        /// sets). Returns `None` if the hash has never been registered.
        fn topology_meta(hash: sp_core::H256) -> Option<
            crate::types::TopologyMeta<Nodes, Edges, AllowedValues, BlockNumber>
        >;

        /// Winning solution for `block_number`, augmented with its derived
        /// nonce. The nonce is reconstructed from the persisted
        /// `last_proof_block_hash`, miner, and salt — no `frame_system::block_hash`
        /// lookup needed, so this stays correct even for blocks pruned beyond
        /// `BlockHashCount`. Returns `None` if the block had no accepted
        /// proof (e.g. genesis, or any block where no `submit_proof` cleared
        /// difficulty).
        fn winning_solution(block_number: BlockNumber) -> Option<
            crate::types::QBlockWithNonce<AccountId, Balance, BlockNumber>
        >;

        /// Latest assigned monotonic qblock id, or `None` before the first
        /// qblock. qblock ids are 1-based ordinals and are distinct from
        /// substrate block numbers.
        fn latest_qblock_id() -> Option<u64>;

        /// qblock id assigned to `block_number`, or `None` if that substrate
        /// block was not a qblock.
        fn qblock_id_by_block(block_number: BlockNumber) -> Option<u64>;

        /// Winning qblock for a monotonic qblock id, augmented with its
        /// derived nonce.
        fn qblock_by_id(qblock_id: u64) -> Option<
            crate::types::QBlockWithNonce<AccountId, Balance, BlockNumber>
        >;

        /// Winning qblock for a substrate block number, augmented with its
        /// derived nonce. This is the qblock-named alias for
        /// `winning_solution`.
        fn qblock_by_block(block_number: BlockNumber) -> Option<
            crate::types::QBlockWithNonce<AccountId, Balance, BlockNumber>
        >;

        /// Live difficulty threshold a miner has to clear *right now*.
        ///
        /// Differs from `api.query.quantumPow.difficulty()` (the raw storage
        /// value): that one is the post-last-adjust baseline and does *not*
        /// reflect decay applied since the last winning proof. This API
        /// returns the decayed value, matching what `submit_proof` validation
        /// will actually require.
        fn current_difficulty() -> crate::types::DifficultyConfig;

        /// Client-facing alias for the live difficulty threshold.
        fn current_hardness() -> crate::types::DifficultyConfig;

        /// Per-topology live difficulty (decay applied), or `None` if the
        /// topology is not registered.
        fn difficulty_for(topology_hash: sp_core::H256) -> Option<crate::types::DifficultyConfig>;

        /// Hashes of every topology currently on the mineable whitelist.
        fn mineable_topologies() -> alloc::vec::Vec<sp_core::H256>;
    }
}

/// Bounds on a topology's declared `expected_frustration_milli`. Outside
/// these the σ the handicap divides by collapses and the handicap stops being
/// a gradient; `0` is accepted separately as "acyclic, no handicap".
const MIN_EXPECTED_FRUSTRATION_MILLI: u32 = 50;

/// How many independent value draws make an all-zero instance unreachable in
/// practice. With a ternary spec each draw is zero with probability at most
/// 1/2, so this puts the all-zero instance below `2^-48` — far past anything
/// a miner can grind for at `O(n + m)` per salt.
const ZERO_DRAW_SAFE_DRAWS: usize = 48;
const MAX_EXPECTED_FRUSTRATION_MILLI: u32 = 950;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;
    use codec::Encode;
    use frame_support::{
        pallet_prelude::*,
        traits::{ReservableCurrency, StorageVersion},
    };
    use frame_system::pallet_prelude::*;
    use quantum_validation::{
        derive_nonce, energy_bound_milli, energy_of_solution, induced_width_at_most,
        packed::{packed_solution_byte_len, unpack_solution},
        topology_consistency_ok, validate_spins, verify_planar_embedding, AllowedValueSpec,
        MilliValue,
    };
    use sp_core::H256;
    use sp_runtime::traits::{One, SaturatedConversion, Saturating, Zero};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(6);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_balances::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        type Currency: ReservableCurrency<Self::AccountId>;

        #[pallet::constant]
        type MaxNodes: Get<u32>;
        #[pallet::constant]
        type MaxEdges: Get<u32>;
        #[pallet::constant]
        type MaxSolutions: Get<u32>;
        #[pallet::constant]
        type MinNodes: Get<u32>;
        /// Upper bound on |allowed_h_values|, |allowed_j_values|, and
        /// |allowed_spin_values| per topology. Small (e.g., 32) is plenty —
        /// these are discrete sets like `{-1, +1}` or `{-6, 0, 6}`, not
        /// per-node arrays.
        #[pallet::constant]
        type MaxAllowedValues: Get<u32>;
        /// Decay cadence, and the target inter-qblock interval the retarget
        /// drives toward.
        #[pallet::constant]
        type EpochLength: Get<BlockNumberFor<Self>>;

        /// How many `EpochLength` intervals one retarget window spans.
        ///
        /// Must be well above 1: the error signal is a qblock *count*, so a
        /// one-interval window expects exactly one qblock and the controller
        /// degenerates to hold / slam / slam. Ten intervals expect ten, so
        /// being off by one moves the bar a tenth of the clamp. The cost is
        /// latency — the loop responds over `EpochLength x this` blocks — and
        /// that is the right trade; the decay path (still on `EpochLength`)
        /// handles an outright stall.
        #[pallet::constant]
        type RetargetWindowEpochs: Get<u32>;
        #[pallet::constant]
        type MinerDeposit: Get<BalanceOf<Self>>;
        #[pallet::constant]
        type BlockReward: Get<BalanceOf<Self>>;
        #[pallet::constant]
        type MaxProofsPerBlock: Get<u32>;

        /// Per-mille `c` value for the easiest (least-negative) end of the
        /// energy curve. The difficulty curve is calibrated against the
        /// default topology's `(num_nodes, num_edges)` and these three c
        /// values; see `crate::difficulty::EnergyCurve`.
        #[pallet::constant]
        type CurveCEasyMilli: Get<u32>;
        /// Per-mille `c` value for the curve's knee (where motion is most
        /// aggressive). Conventionally the canonical `c = 0.75` used by
        /// `quantum_validation::expected_gse`.
        #[pallet::constant]
        type CurveCKneeMilli: Get<u32>;
        /// Per-mille `c` value for the hardest (most-negative) end of the
        /// energy curve.
        #[pallet::constant]
        type CurveCHardMilli: Get<u32>;

        /// Induced width at or below which a topology's irreducible core is
        /// exactly solvable, so mining it is not useful work. `20` matches the
        /// riff toolkit's `EXACT_CEILING`: a width-20 core holds at most
        /// `2^20` colouring-DP states and finishes on a laptop.
        ///
        /// Regime is *derived* from this and `residual_difficulty`, not stored,
        /// so raising the ceiling as classical solvers improve re-classifies
        /// every registered topology in one upgrade. It also bounds the work
        /// `prove_topology_exact` can be made to do.
        #[pallet::constant]
        type ExactSolveCeiling: Get<u32>;

        /// Handicap applied per standard deviation of frustration, in
        /// per-mille of the curve bar.
        ///
        /// A `Config` constant because it is provisional: riff-morph's
        /// calibration puts the slope anywhere from 4 to 29 per-mille per sigma
        /// with correlations under 0.3, so it is expected to move. As a code
        /// constant every move cost a source edit and a spec bump, and the mock
        /// could not sweep it.
        ///
        /// NOT `HANDICAP_MAX_SIGMA`, which gates `submit_proof` validity and is
        /// consensus-critical — that one stays a code constant in
        /// `quantum-validation`, beside the band predicate.
        #[pallet::constant]
        type HandicapPerSigmaPermille: Get<i64>;

        /// Largest single-window difficulty move, in per-mille. A control-loop
        /// gain, so a tunable for the same reason as the handicap slope.
        /// `integrity_test` derives the minimum retarget window from this, so
        /// narrowing the clamp without widening `RetargetWindowEpochs` fails CI
        /// rather than silently leaving the controller in the clamp.
        #[pallet::constant]
        type RetargetMaxStepPermille: Get<i64>;

        type WeightInfo: WeightInfo;
    }

    #[pallet::storage]
    pub type RegisteredTopologies<T: Config> =
        StorageMap<_, Blake2_128Concat, H256, TopologyMetaOf<T>>;

    /// `(nodes, edges)` as a NAMED pair.
    ///
    /// SCALE encodes fields in declaration order with no name or tag bytes, so
    /// this is BYTE-IDENTICAL to the `(u32, u32)` it replaces: no migration,
    /// existing values decode unchanged, same `MaxEncodedLen` of 8. Only
    /// `TypeInfo` changes, from `Tuple` to a named `Composite`.
    ///
    /// Named because the three read sites destructure straight into weight
    /// closures that run inside `validate_transaction`, where a transposition
    /// mis-prices the exact DoS surface this item exists to close — silently,
    /// since no test asserts a weight is CORRECT, only that one is charged.
    ///
    /// NO `Default`. `(0, 0)` is the sentinel for "no dims, price at base"
    /// (see `UNREGISTERED`), and dropping the derive keeps it out of any future
    /// `ValueQuery` declaration or `..Default::default()`, where it would
    /// silently mean "free". `Default` is not part of SCALE encoding,
    /// `TypeInfo` or `MaxEncodedLen`, so no migration.
    #[derive(
        Clone,
        Copy,
        PartialEq,
        Eq,
        Debug,
        Encode,
        Decode,
        DecodeWithMemTracking,
        TypeInfo,
        MaxEncodedLen,
    )]
    pub struct TopologyDim {
        pub nodes: u32,
        pub edges: u32,
    }

    impl TopologyDim {
        /// The dims of a hash that is not registered: price the call at base.
        ///
        /// Correct because an unregistered hash is refused on the
        /// `RegisteredTopologies` lookup almost immediately, so the only work
        /// done is the decode, which base covers. NOT a neutral value: for a
        /// registered topology it undercharges the pallet's most expensive
        /// extrinsic.
        pub const UNREGISTERED: Self = Self { nodes: 0, edges: 0 };
    }

    /// `(node_count, edge_count)` for each registered topology.
    ///
    /// Redundant with `RegisteredTopologies`, deliberately. A
    /// `#[pallet::weight]` closure is evaluated by `get_dispatch_info`, which
    /// runs inside `validate_transaction` — on EVERY GOSSIPED extrinsic,
    /// unpaid, including ones that will be rejected. Reading the dims from
    /// `RegisteredTopologies` made every node SCALE-decode up to `MaxNodes`
    /// nodes plus `MaxEdges` `(u32, u32)` edges — roughly 370-420 KB at the
    /// runtime's 5_000/50_000 — for two `len()`s. A denial-of-service surface,
    /// not an inefficiency; eight bytes per topology is the right trade.
    ///
    /// Written by `register_topology` alongside the meta, backfilled by the v6
    /// migration. No drift: nothing REMOVES either map, re-registration is
    /// refused (`TopologyAlreadyRegistered`), and the one other writer of
    /// `RegisteredTopologies` — v6's canonicalization — is count-preserving
    /// (`canonical_graph` sorts and orients but does not dedupe, pinned by
    /// `canonical_graph_pins_the_consensus_visible_order`).
    #[pallet::storage]
    pub type TopologyDims<T: Config> = StorageMap<_, Blake2_128Concat, H256, TopologyDim>;

    #[pallet::storage]
    pub type DefaultTopology<T: Config> = StorageValue<_, H256>;

    /// Per-topology difficulty baseline (post-last-adjust; decay is applied
    /// on read by `current_difficulty_for`). Keyed by `topology_hash` so a
    /// `DefaultTopology` switch is clean and one topology's winners can never
    /// pin another's difficulty. Unset entries read back as
    /// `DifficultyConfig::default()`.
    #[pallet::storage]
    pub type Difficulties<T: Config> =
        StorageMap<_, Blake2_128Concat, H256, types::DifficultyConfig>;

    /// Per-topology curve `c` override, set by root via `set_topology_curve`.
    /// When present, `energy_curve_for` builds the topology's energy curve from
    /// these values instead of the runtime `CurveC*Milli` constants; an unset
    /// topology falls back to the constants (the legacy behavior). Keyed by
    /// `topology_hash` so the override is carried with the topology and a
    /// `DefaultTopology` switch resolves the matching curve.
    #[pallet::storage]
    pub type TopologyCurveC<T: Config> =
        StorageMap<_, Blake2_128Concat, H256, crate::difficulty::CurveC>;

    /// Root-controlled whitelist of topologies that may be mined: a topology
    /// must have an entry here for `submit_proof` to accept its solutions.
    /// Steady state is `{ DefaultTopology }`.
    #[pallet::storage]
    pub type MineableTopologies<T: Config> = StorageMap<_, Blake2_128Concat, H256, ()>;

    /// Structural facts per topology, set by root via `set_topology_hardness`
    /// from the riff toolkit's off-chain analysis. A topology without a
    /// record mines against the bare curve, so a missing record can never
    /// brick a live chain.
    #[pallet::storage]
    pub type TopologyHardnessOf<T: Config> =
        StorageMap<_, Blake2_128Concat, H256, types::TopologyHardness>;

    #[pallet::storage]
    pub type Miners<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, MinerInfoOf<T>>;

    #[pallet::storage]
    pub type BlockBestProof<T: Config> = StorageValue<_, ProofRecordOf<T>>;

    #[pallet::storage]
    pub type WinnerStreak<T: Config> = StorageValue<_, WinnerStreakOf<T>, OptionQuery>;

    #[pallet::storage]
    /// Block number of the last finalized winning proof.
    ///
    /// Difficulty adjustment and decay are block-based, not timestamp-based:
    /// elapsed blocks are the protocol time unit, so the difficulty path stays
    /// coherent with `EpochLength` and never mixes wall-clock moments with
    /// block numbers.
    pub type LastProofBlock<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;

    /// Hash of the block recorded in `LastProofBlock`. Captured lazily in
    /// `on_initialize` of the *next* block (when `parent_hash()` first equals
    /// `block_hash(LastProofBlock)`) and held in storage thereafter. Calling
    /// `frame_system::block_hash(LastProofBlock)` directly instead would flip
    /// the nonce seed to the zero hash mid-round once the ring buffer aged past
    /// the proof block (`BlockHashCount`, ~25 minutes at 6s blocks).
    #[pallet::storage]
    pub type LastProofBlockHash<T: Config> = StorageValue<_, H256, ValueQuery>;

    #[pallet::storage]
    pub type BlockProofCount<T: Config> = StorageValue<_, u32, ValueQuery>;

    /// First block of the current retarget epoch.
    #[pallet::storage]
    pub type EpochStart<T: Config> = StorageValue<_, BlockNumberFor<T>, ValueQuery>;

    /// qblocks won since `EpochStart`, **per topology**. With the elapsed
    /// blocks this is the cadence error the epoch retarget corrects.
    ///
    /// Keyed rather than global: during a `set_default_topology` switch — the
    /// one time two topologies are whitelisted at once — a global count would
    /// retarget the incoming topology on the outgoing one's throughput.
    #[pallet::storage]
    pub type EpochQBlocks<T: Config> = StorageMap<_, Blake2_128Concat, H256, u32, ValueQuery>;

    /// Persisted record of each qblock (PoW-won block), written in
    /// `on_finalize` alongside the `BlockWinner` event.
    ///
    /// Consumers derive the winning nonce by BLAKE3 of
    /// `(last_proof_block_hash, miner, salt)`, or call
    /// `QuantumPowApi::winning_solution`. Each entry persists the
    /// `last_proof_block_hash` its round used at submission time, so
    /// re-derivation needs no chain-state lookup.
    #[pallet::storage]
    pub type QBlocks<T: Config> = StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, QBlockOf<T>>;

    /// Number of accepted qblocks. Because qblock ids are 1-based, this is
    /// also the latest assigned qblock id when non-zero.
    #[pallet::storage]
    pub type QBlockCount<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Monotonic qblock id to substrate block number index.
    #[pallet::storage]
    pub type QBlockBlockById<T: Config> = StorageMap<_, Blake2_128Concat, u64, BlockNumberFor<T>>;

    /// Substrate block number to monotonic qblock id index.
    #[pallet::storage]
    pub type QBlockIdByBlock<T: Config> = StorageMap<_, Blake2_128Concat, BlockNumberFor<T>, u64>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        MinerRegistered {
            who: T::AccountId,
            deposit: BalanceOf<T>,
        },
        MinerDeregistered {
            who: T::AccountId,
        },
        TopologyRegistered {
            topology_hash: H256,
            node_count: u32,
            edge_count: u32,
        },
        DifficultyUpdated {
            /// The topology whose difficulty baseline changed. Difficulty is
            /// per-topology, so consumers need this to attribute the update.
            topology_hash: H256,
            difficulty: types::DifficultyConfig,
        },
        /// An epoch closed and the energy bar was retargeted to hold average
        /// block time at target.
        DifficultyRetargeted {
            topology_hash: H256,
            blocks_per_qblock: u64,
            difficulty: types::DifficultyConfig,
        },
        /// `DefaultTopology` was repointed by root. The difficulty energy
        /// curve is calibrated against this topology from now on.
        DefaultTopologySet {
            topology_hash: H256,
        },
        /// Root set a per-topology curve `c` override; the topology's energy
        /// curve is now calibrated from the stored override, not the runtime
        /// constants.
        TopologyCurveSet {
            topology_hash: H256,
        },
        TopologyMineableAdded {
            topology_hash: H256,
        },
        TopologyMineableRemoved {
            topology_hash: H256,
        },
        ProofAccepted {
            miner: T::AccountId,
            energy_milli: i64,
            /// Gauge-invariant frustration of the instance the miner's salt
            /// selected, in milli.
            frustration_milli: u32,
            /// The handicapped bar that instance had to clear, in milli.
            instance_bar_milli: i64,
        },
        /// Root registered or replaced a topology's structural record.
        TopologyHardnessSet {
            topology_hash: H256,
            hardness: types::TopologyHardness,
        },
        /// Someone exhibited an elimination order proving the topology's core
        /// is narrower than recorded — and narrow enough to be exactly
        /// solvable. Its `residual_difficulty` has been ratcheted down to the
        /// witnessed width and it is no longer useful work.
        TopologyProvenExact {
            topology_hash: H256,
            prover: T::AccountId,
            width: u32,
        },
        /// Someone exhibited a planar embedding of a zero-field topology,
        /// proving its ground state is polynomial-time regardless of width.
        /// Its recorded width is collapsed to zero and it is no longer
        /// useful work.
        TopologyProvenPlanar {
            topology_hash: H256,
            prover: T::AccountId,
        },
        BlockWinner {
            qblock_id: u64,
            block_number: BlockNumberFor<T>,
            miner: T::AccountId,
            reward: BalanceOf<T>,
            energy_milli: i64,
            submitted_at: BlockNumberFor<T>,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        MinerAlreadyRegistered,
        MinerNotRegistered,
        TopologyAlreadyRegistered,
        TopologyNotRegistered,
        /// A curve `c` override was rejected: the values are not strictly
        /// ordered `easy < knee < hard`, or they do not yield a well-ordered
        /// energy curve (`min < knee < max`) for the topology.
        InvalidCurve,
        GraphTooSmall,
        InvalidTopology,
        ProofLimitReached,
        InvalidNonce,
        NoSolutionsSubmitted,
        InvalidSpinValues,
        SolutionLengthMismatch,
        InsufficientEnergy,
        /// Dead, retained deliberately: `solutions` is a single `PackedSpinBytesOf<T>`, so
        /// this can no longer be returned. Error variant indices are CONSENSUS-VISIBLE —
        /// deleting it would renumber every variant after it, changing what already-deployed
        /// clients decode an error as. Same reason `InvalidEliminationOrder` is still here.
        /// Retiring either one means a coordinated release, not a tidy-up.
        TooManySolutions,
        ArithmeticOverflow,
        /// One of the allowed value specs is empty or has inverted bounds.
        EmptyAllowedValues,
        /// An allowed value spec requires more bits per value than the
        /// protocol supports (max 8 for indexed encodings).
        EncodingTooWide,
        /// A submitted packed solution did not have the byte length implied
        /// by the topology's allowed_spin_values spec and node count.
        PackedSolutionLengthMismatch,
        /// A submitted packed solution contained a bit pattern that does not
        /// map to any value in the allowed_spin_values spec.
        InvalidEncodedSpin,
        /// The topology's allowed_spin_values spec and node count combine to
        /// require more packed-solution bytes than the runtime's MaxNodes
        /// bound permits. Most often hit when a ContinuousRange spin spec
        /// (32 bits per spin) is paired with more than `MaxNodes / 4` nodes,
        /// which would leave the topology accepted but unmineable.
        PackedSolutionTooLarge,
        /// The proof's topology is registered but not on the mineable
        /// whitelist (`MineableTopologies`).
        TopologyNotMineable,
        /// The topology has no structural record yet, so there is no recorded
        /// width to falsify. Governance registers one first.
        TopologyNotClassified,
        /// The topology's recorded width is already at or below the exact
        /// ceiling — the claim being disproved has already been conceded.
        TopologyAlreadyExact,
        /// The topology's recorded width is at or below the exact ceiling, so
        /// its ground state is tractable and mining it would not be useful
        /// work.
        TopologyIsExact,
        /// The instance this salt drew has no frustrated cycles, so it is
        /// gauge-equivalent to a ferromagnet and its ground state is
        /// computable in `O(m)`. Re-salt.
        InstanceIsGaugeTrivial,
        /// The instance this salt drew is more than `HANDICAP_MAX_SIGMA` from
        /// the topology's expected frustration in either direction, past the
        /// point where the handicap can price it. Distinct from
        /// `InstanceIsGaugeTrivial` so an operator can tell "this draw is
        /// structurally trivial" from "this topology's declared expectation may
        /// be wrong". Re-salt.
        InstanceOutsideFrustrationBand,
        /// A hardness record would raise a topology's recorded width above the
        /// standing claim, undoing the ratchet an elimination-order witness
        /// established.
        WidthWidened,
        /// `expected_frustration_milli` exceeds `1000`; it is a per-mille
        /// fraction of the topology's fundamental cycles.
        InvalidHardnessRecord,
        /// The submitted rotation system is not a planar embedding of the
        /// topology's graph, so it witnesses nothing.
        InvalidPlanarEmbedding,
        /// The topology's `allowed_h` spec can sample a non-zero field, and
        /// planar Ising with a field is NP-hard — planarity alone does not
        /// make it tractable.
        TopologyIsFieldBearing,
        /// The submitted sequence is not a permutation of the topology's
        /// nodes, or its induced width exceeds the exact ceiling, so it
        /// witnesses nothing.
        ///
        /// NO LONGER EMITTED. Every rejection now reports which defect it was;
        /// see the `Witness*` variants and `WidthAboveCeiling`. Kept, not
        /// deleted: a FRAME error is identified by its INDEX in this enum, so
        /// removing a variant renumbers every one after it and silently
        /// re-points tooling that matched on the number.
        InvalidEliminationOrder,
        /// Refused to remove the current `DefaultTopology` from the mineable
        /// whitelist; repoint the default first.
        TopologyIsDefault,
        /// Refused to whitelist a second non-default topology while one is
        /// already mineable. The decay anchor (`LastProofBlock`) is global
        /// (model A: single active topology), so concurrent mining of two
        /// non-default topologies would let one's wins mis-drive the other's
        /// difficulty. Remove the existing one first.
        MineableTopologyConflict,
        // ── appended, deliberately ─────────────────────────────────────────
        //
        // These belong topically next to `InvalidEliminationOrder`, but a FRAME
        // error is identified by its INDEX, so inserting mid-enum renumbers
        // every later variant and re-points tooling that matched on the number.
        /// The witness is well-formed but its claim is FALSE: an elimination
        /// bag exceeded the ceiling. Retrying the same order will not help;
        /// a different order might.
        WidthAboveCeiling,
        /// The witness is well-formed but its claim is FALSE: Euler's formula
        /// puts the embedding above genus zero, so the graph is not planar in
        /// the submitted rotation.
        NotPlanar,
        // The remaining variants are MALFORMED-witness defects, one error each
        // rather than one collapsed error plus an event naming the defect: a
        // failing extrinsic rolls back its events, so the reason would be
        // discarded exactly when it is needed. Per the v6 canonicalization note the
        // likeliest cause today is a rotation generated against
        // PRE-canonicalization edge order — `WitnessEdgeNotIncident` or
        // `WitnessRotationLength`.
        /// The order or rotation does not have one entry per node.
        WitnessOrderLength,
        /// The witness names a node the topology does not have.
        WitnessUnknownNode,
        /// The witness names a node more than once, so it is not a permutation.
        WitnessDuplicateNode,
        /// An edge index in the rotation is past the end of the edge list.
        WitnessEdgeIndexOutOfRange,
        /// An edge appears in a vertex's rotation but is not incident to it,
        /// or appears twice at the same end.
        WitnessEdgeNotIncident,
        /// The stored TOPOLOGY has a self-loop. Not the submitter's fault and
        /// not fixable by resubmitting — see `WitnessError::SelfLoop`.
        WitnessSelfLoop,
        /// The stored TOPOLOGY has an edge naming a node outside its node
        /// list. Also not the submitter's fault.
        WitnessTopologyEdgeUnknownNode,
        /// A vertex's rotation length does not match its degree.
        WitnessRotationLength,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// `integrity_test` runs in test harnesses and under
        /// `frame-benchmarking-cli`, NOT during block import or node boot, so
        /// these catch a bad value in CI rather than on a live chain. That is
        /// the strongest check available for a `#[pallet::constant]`, but it is
        /// NOT a runtime guarantee.
        fn integrity_test() {
            // FIRST, above the window-bound check: that one calls
            // `min_window_for_step`, which divides by `step * step`, so a zero
            // step would panic with "attempt to divide by zero" instead of the
            // message here.
            //
            // A NEGATIVE clamp halts the chain. `retarget_bar_milli` calls
            // `clamp(-max_step, max_step)`, and `Ord::clamp` asserts
            // `min <= max`, so a negative value panics inside `on_finalize` —
            // a hook that cannot be refused. Zero does not panic; it silently
            // freezes difficulty forever. Reachable only since the value became
            // a `Config` type, so the guard arrives with it.
            assert!(
                T::RetargetMaxStepPermille::get() > 0,
                "RetargetMaxStepPermille must be positive: a negative value \
                 panics on_finalize via Ord::clamp, and zero freezes the \
                 difficulty controller"
            );
            // The slope multiplies a +/-3 sigma value in `i64` before widening,
            // and release builds do not check overflow: an absurd governance
            // value WRAPS and flips the handicap's sign, turning the equalizer
            // into the amplifier it exists to prevent. Measured scatter 4..29.
            assert!(
                (0..=crate::difficulty::HANDICAP_SLOPE_MAX_PERMILLE)
                    .contains(&T::HandicapPerSigmaPermille::get()),
                "HandicapPerSigmaPermille must be in 0..=1000 per-mille; the \
                 measured scatter is 4..29 and an unbounded value can wrap the \
                 i64 multiply in `instance_bar_milli`"
            );

            assert!(
                T::RetargetWindowEpochs::get()
                    >= crate::difficulty::min_window_for_step(T::RetargetMaxStepPermille::get()),
                "RetargetWindowEpochs must span enough target intervals for \
                 the count to carry information; qblock arrivals are Poisson, \
                 so the clamp dominates until sigma/expected <= the clamp \
                 itself. Derived from `RetargetMaxStepPermille` rather than \
                 hardcoded so that NARROWING the clamp raises this bound: a \
                 tighter clamp needs more averaging, and a literal 16 would \
                 keep passing while the controller sat in the clamp."
            );

            // `prove_topology_exact` is permissionless and its weight is
            // quadratic in the ceiling, but `PROVE_EXACT_K1_NODE` was measured
            // at a ceiling of 20 and folds that in. Raising the ceiling — which
            // is the point of it being a `Config` type — silently undercharges:
            // 24 by ~1.4x, 30 by ~2.25x. A prior 50x undercharge here was a
            // block-stuffing vector, not a rounding error. Re-benchmark and
            // re-derive the constant before moving this.
            assert!(
                T::ExactSolveCeiling::get() <= 20,
                "PROVE_EXACT_K1_NODE was benchmarked at ExactSolveCeiling = 20 \
                 and the cost is quadratic in the ceiling; raising it without \
                 re-benchmarking undercharges a permissionless extrinsic"
            );

            // `ensure_specs_cannot_draw_zero` used to carry an
            // `ensure!(i64::try_from(loosest).is_ok(), ..)`, removed as
            // unreachable — but only by an arithmetic relationship between
            // `MaxNodes`, `MaxEdges` and `MilliValue = i32` that a bounds bump
            // would erode silently. `loosest_energy_bound_milli` is
            // `nodes * min_abs(h) + edges * min_abs(j)`, and `min_abs_milli`
            // peaks on the `IntegerRange` arm (an `i32` magnitude scaled by
            // `MILLI_SCALE`). Today: 55_000 * 2.147e12 ≈ 1.2e17 against
            // `i64::MAX` ≈ 9.2e18, ~78x of headroom; the threshold is
            // `i64::MAX / widest_term` ≈ 4.29M nodes+edges.
            let widest_term = u64::from(i32::MIN.unsigned_abs())
                .saturating_mul(quantum_validation::MILLI_SCALE as u64);
            let loosest_ceiling = u64::from(T::MaxNodes::get())
                .saturating_mul(widest_term)
                .saturating_add(u64::from(T::MaxEdges::get()).saturating_mul(widest_term));
            assert!(
                loosest_ceiling <= i64::MAX as u64,
                "the loosest energy bound must stay inside i64: the try_from \
                 guard in ensure_specs_cannot_draw_zero was removed as \
                 unreachable on exactly this arithmetic, and raising MaxNodes \
                 or MaxEdges far enough makes it reachable again"
            );
        }

        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            BlockProofCount::<T>::put(0);

            // Capture `block_hash(LastProofBlock)` permanently as soon as it is
            // available. The only place it is freshly known without
            // `frame_system::block_hash` (which ages out after
            // `BlockHashCount`) is `parent_hash()` of the block right after the
            // winning proof was finalized, i.e. `LastProofBlock == n - 1`. The
            // `n == 1` arm seeds the cache with the genesis hash so pre-proof
            // nonce derivation is stable too.
            let last_proof_block = LastProofBlock::<T>::get();
            let one: BlockNumberFor<T> = One::one();
            if n == one || (n > one && last_proof_block.saturating_add(one) == n) {
                let parent = frame_system::Pallet::<T>::parent_hash();
                LastProofBlockHash::<T>::put(H256::from(Self::hash_to_bytes_32(parent)));
            }

            // Charge for BOTH hooks here: `on_finalize` returns no weight of
            // its own and the block must still pay for it. (Borrowing
            // `register_miner()` accounted for none of what `on_finalize` does
            // — reward deposit, qblock inserts, per-topology retarget scan,
            // `EpochQBlocks::clear`, `current_difficulty_for`'s decay walk.)
            //
            // Counted for the worst-case block: a win that also closes a
            // retarget window, with the whitelist at its bound of two
            // topologies. Reads: `BlockProofCount`, `LastProofBlock`,
            // `BlockBestProof`, `Miners`, `LastProofBlockHash`, `WinnerStreak`,
            // `QBlockCount`, the deposit's account and issuance,
            // `current_difficulty_for` on the winner's topology (4),
            // `EpochStart`, then per topology the whitelist key, curve pair,
            // `EpochQBlocks`, `DefaultTopology` and baseline, plus the cleared
            // epoch keys. Writes: the `BlockBestProof` kill, two balance
            // writes, `Miners`, `LastProofBlock`, `QBlockCount`, three qblock
            // records, `EpochQBlocks`, `WinnerStreak`, one `Difficulties` per
            // topology, `EpochStart`, the cleared keys, `BlockProofCount`.
            //
            // The ref_time allowance covers `expected_gse` (recomputed on every
            // `energy_curve_for`) and the decay walk, bounded because easing
            // saturates at `curve.max_milli` and the loop exits on the first
            // step that changes nothing.
            //
            // Charged on EVERY block, including the ~99.9% that neither win nor
            // close a window — the only correct direction for a hook that
            // returns no weight, but a real standing cost.
            T::DbWeight::get()
                .reads_writes(30, 18)
                .saturating_add(Weight::from_parts(300_000_000, 0))
        }

        /// Cumulative storage migration to the in-code `STORAGE_VERSION`.
        ///
        /// v2 → v3: difficulty becomes per-topology, mineable whitelist added.
        /// - `== 2`: carry the global `Difficulty` into
        ///   `Difficulties[DefaultTopology]`, whitelist the default, remove the
        ///   old global value.
        /// - `< 2`: legacy v0.2 wipe (old encodings cannot be carried).
        ///
        /// v3 → v4 (SHIPPED in spec 111): `QBlock` gained a trailing
        /// `topology_hash`; the deployed 111 chain is at v4 in that 8-field
        /// layout.
        ///
        /// v4 → v5 (spec 112): `QBlock` gains `device_access_time_us`. Entries
        /// without it fail to decode (silently reading back as `None`), so
        /// every value is re-encoded with `0` (never reported pre-112).
        /// - `== 4`: append the device time only, preserving the stored
        ///   per-block `topology_hash`.
        /// - `< 4`: 7-field pre-topology layout; append both, backfilling
        ///   `topology_hash` with the default — the only topology mineable
        ///   before per-topology binding.
        ///
        /// Both paths kill any stale `BlockBestProof` (`ProofRecord` also
        /// changed shape).
        ///
        /// Cumulative: a v2 chain runs v3 then the combined v5 backfill; a v4
        /// chain runs only the device-time append; a `< 2` chain wipes, which
        /// clears `QBlocks` and leaves nothing to translate.
        fn on_runtime_upgrade() -> Weight {
            let on_chain = Pallet::<T>::on_chain_storage_version();
            if on_chain >= STORAGE_VERSION {
                return T::DbWeight::get().reads(1);
            }

            let mut weight = Weight::zero();

            if on_chain < StorageVersion::new(3) {
                weight = weight.saturating_add(if on_chain == StorageVersion::new(2) {
                    crate::migration::v3::carry_forward::<T>()
                } else {
                    crate::migration::v3::wipe::<T>()
                });
            }

            if on_chain < StorageVersion::new(5) {
                weight = weight.saturating_add(if on_chain == StorageVersion::new(4) {
                    crate::migration::v5::append_device_time::<T>()
                } else {
                    crate::migration::v5::backfill_from_pre_topology::<T>()
                });
            }

            // Shrink `Difficulties` only where an older runtime wrote it in the
            // legacy shape and this upgrade has not already rewritten it: the
            // v3 step above leaves the map in today's shape, so re-translating
            // there would fail to decode and silently drop the default
            // topology's baseline.
            weight = weight.saturating_add(if on_chain == StorageVersion::new(5) {
                crate::migration::v6::shrink_difficulty::<T>()
            } else if on_chain >= StorageVersion::new(3) {
                crate::migration::v6::shrink_difficulties_only::<T>()
            } else {
                crate::migration::v6::kill_stale_best_proof::<T>()
            });

            // Runs from every prior version: entries written before this
            // upgrade carry submission order, and the ones the v3 wipe left
            // behind are already canonical, so the pass is a no-op for them.
            weight = weight.saturating_add(crate::migration::v6::canonicalize_topologies::<T>());

            // Backfills `TopologyDims` for every already-registered topology.
            // Runs from every prior version and is idempotent.
            weight = weight.saturating_add(crate::migration::v6::backfill_topology_dims::<T>());

            STORAGE_VERSION.put::<Pallet<T>>();
            weight.saturating_add(T::DbWeight::get().reads_writes(1, 1))
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
            // A bool, not the version: `StorageVersion` has no `Into<u16>` in
            // this SDK fork, but `==` and `bool: Encode` are available.
            let was_v2 = Pallet::<T>::on_chain_storage_version() == StorageVersion::new(2);
            // `iter_keys` decodes only the (unchanged) keys, so it is safe
            // against the old value layout. The translate must preserve this
            // count exactly; a dropped entry means an old value failed to
            // decode and was silently discarded.
            let qblocks = QBlocks::<T>::iter_keys().count() as u64;
            Ok((was_v2, DefaultTopology::<T>::get(), qblocks).encode())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            ensure!(
                Pallet::<T>::on_chain_storage_version() >= STORAGE_VERSION,
                // Not a literal version: the check is against STORAGE_VERSION,
                // and a hardcoded number here went stale the moment it moved.
                "on-chain storage version must reach STORAGE_VERSION after upgrade"
            );
            let (was_v2, default, qblocks_before): (bool, Option<H256>, u64) =
                Decode::decode(&mut &state[..]).map_err(|_| "pre_upgrade state decode failed")?;
            // Every qblock must survive the v5 re-encode and decode under the
            // new layout; a count mismatch means an entry was dropped.
            ensure!(
                QBlocks::<T>::iter().count() as u64 == qblocks_before,
                "the v5 re-encode must preserve every QBlocks entry"
            );
            // The dims invariant moved to `try_state` — the standard FRAME
            // shape, and see there for why once-per-upgrade was the wrong
            // place for it.
            Self::do_try_state()?;
            if was_v2 {
                // The old global `Difficulty` value must be gone.
                //
                // Checked on RAW bytes, not by decoding: SCALE ignores trailing
                // input, so a surviving 12-byte legacy blob decodes cleanly
                // into the 8-byte `DifficultyConfig` and a decode-based
                // `is_none()` would report success on exactly the state it is
                // meant to catch.
                ensure!(
                    frame_support::storage::unhashed::get_raw(
                        &crate::migration::v3::old_difficulty_key::<T>()
                    )
                    .is_none(),
                    "v2→v3 must remove the old global Difficulty value"
                );
                if let Some(hash) = default {
                    ensure!(
                        Difficulties::<T>::contains_key(hash),
                        "v2→v3 must seed Difficulties[DefaultTopology]"
                    );
                    ensure!(
                        MineableTopologies::<T>::contains_key(hash),
                        "v2→v3 must whitelist the default topology"
                    );
                }
            }
            Ok(())
        }

        /// Invariants that must hold on EVERY block, not once per upgrade: in
        /// `post_upgrade` alone, a bug introduced in v119 stays invisible until
        /// v120's migration, IF v120 has one.
        #[cfg(feature = "try-runtime")]
        fn try_state(_n: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            Self::do_try_state()
        }

        fn on_finalize(n: BlockNumberFor<T>) {
            let Some(record) = BlockBestProof::<T>::take() else {
                // An epoch that produced no qblock at all still has to close,
                // and reads as maximally slow — that is the recovery path
                // from a bar set too tight for anyone to clear.
                Self::maybe_retarget(n);
                return;
            };

            let reward = T::BlockReward::get();
            let _ = T::Currency::deposit_creating(&record.miner, reward);

            if let Some(mut miner) = Miners::<T>::get(&record.miner) {
                miner.proofs_won = miner.proofs_won.saturating_add(1);
                miner.rewards_earned = miner.rewards_earned.saturating_add(reward);
                Miners::<T>::insert(&record.miner, miner);
            }

            // The hash the just-won round actually used in `derive_nonce`. From
            // the cache, not `frame_system::block_hash`: on a round longer than
            // `BlockHashCount` the live lookup returns the zero hash rather
            // than the value miners derived against.
            let last_proof_block_hash = LastProofBlockHash::<T>::get();

            let topology_hash = record.topology_hash;
            // The threshold this proof had to clear, as `submit_proof` read it
            // in this same block. Carried on the record, not recomputed:
            // `current_difficulty_for` would rebuild the energy curve, decoding
            // the whole topology again. See `ProofRecord::difficulty`.
            let active = record.difficulty;
            // Streak tracking is retained for observability; it no longer
            // moves difficulty.
            let _ = Self::update_winner_streak(&record.miner);

            // No per-proof rate-band walk here: the epoch retarget is the sole
            // dial. The two fought — on a slow chain the retarget eases while
            // the band hardened ("this one took a while") — so they push
            // opposite directions in the case that matters most, a chain
            // falling behind.
            LastProofBlock::<T>::put(n);
            let qblock_id = Self::next_qblock_id();

            QBlocks::<T>::insert(
                n,
                types::QBlock {
                    miner: record.miner.clone(),
                    salt: record.salt,
                    energy_milli: record.energy_milli,
                    reward,
                    submitted_at: record.submitted_at,
                    difficulty: active,
                    last_proof_block_hash,
                    topology_hash,
                    device_access_time_us: record.device_access_time_us,
                    frustration_milli: record.frustration_milli,
                    instance_bar_milli: record.instance_bar_milli,
                },
            );
            QBlockBlockById::<T>::insert(qblock_id, n);
            QBlockIdByBlock::<T>::insert(n, qblock_id);
            EpochQBlocks::<T>::mutate(topology_hash, |count| *count = count.saturating_add(1));

            Self::deposit_event(Event::BlockWinner {
                qblock_id,
                block_number: n,
                miner: record.miner,
                reward,
                energy_milli: record.energy_milli,
                submitted_at: record.submitted_at,
            });

            Self::maybe_retarget(n);
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::register_miner())]
        pub fn register_miner(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                !Miners::<T>::contains_key(&who),
                Error::<T>::MinerAlreadyRegistered
            );

            let deposit = T::MinerDeposit::get();
            T::Currency::reserve(&who, deposit)?;

            Miners::<T>::insert(
                &who,
                MinerInfoOf::<T> {
                    registered_at: frame_system::Pallet::<T>::block_number(),
                    deposit,
                    proofs_submitted: 0,
                    proofs_won: 0,
                    rewards_earned: Zero::zero(),
                },
            );

            Self::deposit_event(Event::MinerRegistered { who, deposit });
            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::deregister_miner())]
        pub fn deregister_miner(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let miner = Miners::<T>::get(&who).ok_or(Error::<T>::MinerNotRegistered)?;

            T::Currency::unreserve(&who, miner.deposit);
            Miners::<T>::remove(&who);

            Self::deposit_event(Event::MinerDeregistered { who });
            Ok(())
        }

        /// Register a topology together with its structural record.
        ///
        /// Classification is part of registration, not a follow-up call, so
        /// "every mineable topology is classified" is a structural invariant
        /// rather than an ordering convention: the per-instance bar needs
        /// `expected_frustration_milli` to price a salt-chosen instance, and
        /// `prove_topology_exact` needs a recorded width to falsify. Registered
        /// without either, a topology mines against the bare curve and is
        /// immune to challenge. `set_topology_hardness` re-classifies.
        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::register_topology(
            nodes.len() as u32,
            edges.len() as u32,
            Pallet::<T>::allowed_value_input_count(&allowed_h_values)
                .saturating_add(Pallet::<T>::allowed_value_input_count(&allowed_j_values))
                .saturating_add(Pallet::<T>::allowed_value_input_count(&allowed_spin_values)),
        ))]
        pub fn register_topology(
            origin: OriginFor<T>,
            nodes: NodesOf<T>,
            edges: EdgesOf<T>,
            allowed_h_values: AllowedValueSpec<AllowedValueSetOf<T>>,
            allowed_j_values: AllowedValueSpec<AllowedValueSetOf<T>>,
            allowed_spin_values: AllowedValueSpec<AllowedValueSetOf<T>>,
            hardness: types::TopologyHardness,
        ) -> DispatchResult {
            ensure_root(origin)?;

            ensure!(
                nodes.len() >= T::MinNodes::get() as usize,
                Error::<T>::GraphTooSmall
            );

            // Validate each spec is non-empty and fits the protocol's bit-width
            // cap. `bits_per_value` returns the per-variant errors that the
            // pallet maps to dispatch errors.
            Self::check_spec(&allowed_h_values)?;
            Self::check_spec(&allowed_j_values)?;
            Self::check_spec(&allowed_spin_values)?;

            // Canonicalize Set ordering so the stored representation matches
            // the order-independent topology hash. Without this, registering
            // [a, b, c] and [b, a, c] in different orders hash to the same
            // value but produce different deterministic puzzles from the same
            // nonce.
            let allowed_h_values = Self::canonicalize_spec(allowed_h_values)?;
            let allowed_j_values = Self::canonicalize_spec(allowed_j_values)?;
            let allowed_spin_values = Self::canonicalize_spec(allowed_spin_values)?;

            // Same argument, applied to the graph itself. `hash_topology` sorts
            // nodes and orients-then-sorts edges, but `generate_ising_model`
            // maps `h[i]` to `nodes[i]` and `j[k]` to `edges[k]` POSITIONALLY,
            // so without this the hash is not a commitment to the instance it
            // identifies: two registrations differing only in edge order hash
            // identically and generate different puzzles from the same nonce.
            // Storing the canonical order also fixes the spanning forest the
            // frustration index builds (it walks edges in stored order), so
            // `expected_frustration_milli` is calibrated against an ordering the
            // chain pins rather than one the registrant chose.
            let (canonical_nodes, canonical_edges) =
                quantum_validation::canonical_graph(&nodes, &edges);
            let nodes =
                NodesOf::<T>::try_from(canonical_nodes).map_err(|_| Error::<T>::InvalidTopology)?;
            let edges =
                EdgesOf::<T>::try_from(canonical_edges).map_err(|_| Error::<T>::InvalidTopology)?;

            // A submitted solution is a `BoundedVec<u8, MaxNodes>`. Indexed
            // specs (Set, IntegerRange) pack <= 8 bits per spin so they always
            // fit, but ContinuousRange uses 32 bits, making any topology with
            // num_nodes > MaxNodes/4 unmineable. Reject at registration rather
            // than silently shipping a dead topology.
            let packed_bytes =
                packed_solution_byte_len(nodes.len(), &allowed_spin_values.as_slice())
                    .map_err(|_| Error::<T>::InvalidTopology)?;
            ensure!(
                packed_bytes <= T::MaxNodes::get() as usize,
                Error::<T>::PackedSolutionTooLarge
            );

            // The boolean form short-circuits on the first defect instead of
            // formatting every one into a `String` the caller discards. Matters
            // against an adversarial registration, not the happy path.
            ensure!(
                topology_consistency_ok(
                    &nodes,
                    &edges,
                    &vec![0; nodes.len()],
                    &vec![0; edges.len()],
                    None,
                    None,
                ),
                Error::<T>::InvalidTopology
            );

            let topology_hash = crate::topology::hash_topology(
                &nodes,
                &edges,
                &allowed_h_values.as_slice(),
                &allowed_j_values.as_slice(),
                &allowed_spin_values.as_slice(),
            );
            ensure!(
                !RegisteredTopologies::<T>::contains_key(topology_hash),
                Error::<T>::TopologyAlreadyRegistered
            );

            Self::check_hardness(&hardness)?;
            // A sign-symmetric coupling spec pins the expected frustration
            // exactly, so derive it — OVERWRITING, not merely checking, the
            // registrant's declaration. The frustration gate refuses draws more
            // than three σ from declared, and σ is that of a MEAN over the
            // cycle count: 2.467 milli at Z(12,4)'s 41,065 cycles. A
            // declaration off by one percentage point puts every honest draw
            // four σ out and every `submit_proof` fails forever. Rejecting a
            // wrong value still leaves the operator to get it right; computing
            // it means they cannot get it wrong. Derivation and its proof live
            // in `quantum-validation`.
            let mut hardness = hardness;
            if let Some(derived) = quantum_validation::expected_frustration_for(
                &allowed_j_values.as_slice(),
                &nodes,
                &edges,
            ) {
                hardness.expected_frustration_milli = derived;
            }
            // Specs that can draw an all-zero instance are refused outright:
            // such a draw pays full reward for no work.
            Self::ensure_specs_cannot_draw_zero(
                nodes.len(),
                edges.len(),
                &allowed_h_values.as_slice(),
                &allowed_j_values.as_slice(),
            )?;
            Self::ensure_curve_outlasts_the_typical_anchor(
                &crate::difficulty::EnergyCurve::new(
                    nodes.len() as u32,
                    edges.len() as u32,
                    crate::difficulty::CurveC {
                        easy_milli: T::CurveCEasyMilli::get(),
                        knee_milli: T::CurveCKneeMilli::get(),
                        hard_milli: T::CurveCHardMilli::get(),
                    },
                    &allowed_h_values.as_slice(),
                    &allowed_j_values.as_slice(),
                )
                .map_err(|_| Error::<T>::InvalidCurve)?,
                quantum_validation::expected_bound_milli(
                    nodes.len() as u32,
                    edges.len() as u32,
                    &allowed_h_values.as_slice(),
                    &allowed_j_values.as_slice(),
                )
                .map_err(|_| Error::<T>::InvalidCurve)?,
            )?;
            TopologyHardnessOf::<T>::insert(topology_hash, hardness);
            RegisteredTopologies::<T>::insert(
                topology_hash,
                TopologyMetaOf::<T> {
                    nodes: nodes.clone(),
                    edges: edges.clone(),
                    allowed_h_values,
                    allowed_j_values,
                    allowed_spin_values,
                    registered_at: frame_system::Pallet::<T>::block_number(),
                },
            );
            // Beside the meta so weight closures never decode it. See
            // `TopologyDims`.
            TopologyDims::<T>::insert(
                topology_hash,
                TopologyDim {
                    nodes: nodes.len() as u32,
                    edges: edges.len() as u32,
                },
            );

            // Seed the default from the first registration so a fresh chain can
            // mine immediately — but only if the topology is worth mining. An
            // exact one is left registered and unmineable rather than installed
            // as a default no proof should ever win.
            if DefaultTopology::<T>::get().is_none()
                && hardness.residual_difficulty.min(hardness.core_width)
                    > T::ExactSolveCeiling::get()
            {
                DefaultTopology::<T>::put(topology_hash);
                MineableTopologies::<T>::insert(topology_hash, ());
            }

            Self::deposit_event(Event::TopologyRegistered {
                topology_hash,
                node_count: nodes.len() as u32,
                edge_count: edges.len() as u32,
            });
            Self::deposit_event(Event::TopologyHardnessSet {
                topology_hash,
                hardness,
            });
            Ok(())
        }

        /// Repoint `DefaultTopology` to an already-registered topology.
        ///
        /// `register_topology` only seeds `DefaultTopology` on the very
        /// first registration; this call is the operator path for upgrading
        /// a live chain to a new topology (e.g. tracking a QPU's working
        /// graph across calibrations). The difficulty energy curve follows
        /// the default topology, so operators should re-baseline
        /// `set_difficulty` after repointing when the curves differ
        /// materially.
        #[pallet::call_index(5)]
        #[pallet::weight(<T as Config>::WeightInfo::set_default_topology())]
        pub fn set_default_topology(origin: OriginFor<T>, topology_hash: H256) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(
                RegisteredTopologies::<T>::contains_key(topology_hash),
                Error::<T>::TopologyNotRegistered
            );
            ensure!(
                MineableTopologies::<T>::contains_key(topology_hash),
                Error::<T>::TopologyNotMineable
            );
            // NOTE (model A): `LastProofBlock` (global decay anchor) is NOT
            // reset here. Under single-active-topology mining that is fine —
            // the new default reads through decay anchored to the previous win,
            // ~one win old on an actively-mining chain. If concurrent
            // multi-topology mining (model B) is added, reset hardness/round
            // state at the switch.
            DefaultTopology::<T>::put(topology_hash);
            Self::deposit_event(Event::DefaultTopologySet { topology_hash });
            Ok(())
        }

        /// Set the difficulty baseline for a specific registered topology.
        /// Root only. The topology must be registered so no orphan difficulty
        /// entries can be created.
        #[pallet::call_index(3)]
        #[pallet::weight(<T as Config>::WeightInfo::set_difficulty())]
        pub fn set_difficulty(
            origin: OriginFor<T>,
            topology_hash: H256,
            difficulty: types::DifficultyConfig,
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(
                RegisteredTopologies::<T>::contains_key(topology_hash),
                Error::<T>::TopologyNotRegistered
            );
            Difficulties::<T>::insert(topology_hash, difficulty);
            Self::deposit_event(Event::DifficultyUpdated {
                topology_hash,
                difficulty,
            });
            Ok(())
        }

        /// Set (or replace) a topology's curve `c` override (root only).
        ///
        /// The override is calibrated against the topology's graph and value
        /// specs and takes effect immediately for difficulty adjustment and
        /// decay; an unset topology keeps using the runtime `CurveC*Milli`
        /// constants. Changing a live topology's curve can leave its stored
        /// `Difficulty.max_energy_milli` outside the new bounds — the geometric
        /// adjustment converges it back over subsequent rounds, but operators
        /// who need an immediate baseline should follow with `set_difficulty`.
        #[pallet::call_index(8)]
        #[pallet::weight(<T as Config>::WeightInfo::set_topology_curve())]
        pub fn set_topology_curve(
            origin: OriginFor<T>,
            topology_hash: H256,
            curve_c: crate::difficulty::CurveC,
        ) -> DispatchResult {
            ensure_root(origin)?;
            let topology = RegisteredTopologies::<T>::get(topology_hash)
                .ok_or(Error::<T>::TopologyNotRegistered)?;
            // The `c` values must be strictly increasing easy -> knee -> hard,
            // and must yield a well-ordered energy curve for this topology.
            ensure!(
                curve_c.easy_milli < curve_c.knee_milli && curve_c.knee_milli < curve_c.hard_milli,
                Error::<T>::InvalidCurve
            );
            let curve = crate::difficulty::EnergyCurve::new(
                topology.nodes.len() as u32,
                topology.edges.len() as u32,
                curve_c,
                &topology.allowed_h_values.as_slice(),
                &topology.allowed_j_values.as_slice(),
            )
            .map_err(|_| Error::<T>::InvalidCurve)?;
            ensure!(
                curve.min_milli < curve.knee_milli && curve.knee_milli < curve.max_milli,
                Error::<T>::InvalidCurve
            );

            Self::ensure_curve_outlasts_the_typical_anchor(
                &curve,
                quantum_validation::expected_bound_milli(
                    topology.nodes.len() as u32,
                    topology.edges.len() as u32,
                    &topology.allowed_h_values.as_slice(),
                    &topology.allowed_j_values.as_slice(),
                )
                .map_err(|_| Error::<T>::InvalidCurve)?,
            )?;
            TopologyCurveC::<T>::insert(topology_hash, curve_c);
            Self::deposit_event(Event::TopologyCurveSet { topology_hash });
            Ok(())
        }

        /// Register or replace a topology's structural record. Root only;
        /// the topology must already be registered.
        ///
        /// The values come from the riff toolkit's off-chain analysis, trusted
        /// as the topology's nodes and specs are.
        /// `expected_frustration_milli` is load-bearing: the energy curve is
        /// calibrated at it, so the per-instance bar equalizes around it. The
        /// regime is derived, not stored (`regime_of`), and the width may only
        /// be revised downward (`WidthWidened`).
        #[pallet::call_index(9)]
        #[pallet::weight(<T as Config>::WeightInfo::set_topology_hardness())]
        pub fn set_topology_hardness(
            origin: OriginFor<T>,
            topology_hash: H256,
            hardness: types::TopologyHardness,
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(
                RegisteredTopologies::<T>::contains_key(topology_hash),
                Error::<T>::TopologyNotRegistered
            );
            Self::check_hardness(&hardness)?;
            // Widening would re-inflate a topology to look harder than a
            // witness has already shown it to be, undoing the ratchet.
            // Re-classification upward goes through a new topology, not a
            // silent overwrite — root is trusted, but not with that.
            if let Some(existing) = TopologyHardnessOf::<T>::get(topology_hash) {
                ensure!(
                    hardness.residual_difficulty <= existing.residual_difficulty
                        && hardness.core_width <= existing.core_width,
                    Error::<T>::WidthWidened
                );
            }
            TopologyHardnessOf::<T>::insert(topology_hash, hardness);
            Self::deposit_event(Event::TopologyHardnessSet {
                topology_hash,
                hardness,
            });
            Ok(())
        }

        /// Prove that a registered topology is exactly solvable by exhibiting
        /// an elimination order whose induced width is at or below
        /// `ExactSolveCeiling`, and ratchet its recorded width down to the
        /// witnessed value.
        ///
        /// **Permissionless on purpose.** `residual_difficulty` comes from an
        /// off-chain min-fill search, whose upper bound can be loose: a
        /// topology can be recorded as hard while a better order shows it is
        /// tractable. That is the misclassification that matters — the chain
        /// must never sell easy work as hard — and replaying an order bounds
        /// the width from *above*, so it is exactly the direction a witness can
        /// settle. The opposite claim has no cheap witness and stays
        /// governance's assertion, subject to this challenge.
        ///
        /// The width only ratchets down: neither this call nor
        /// `set_topology_hardness` can raise it again.
        ///
        /// A proven-exact topology is dropped from the mineable whitelist
        /// unless it is the current `DefaultTopology` — removing that would
        /// halt qblock production, so the event fires and governance repoints.
        #[pallet::call_index(10)]
        #[pallet::weight({
            // `TopologyDims`, NOT `RegisteredTopologies`: this closure runs
            // during pool validation on every gossiped extrinsic.
            let TopologyDim { nodes, edges } =
                TopologyDims::<T>::get(topology_hash).unwrap_or(TopologyDim::UNREGISTERED);
            <T as Config>::WeightInfo::prove_topology_exact(nodes, edges)
        })]
        pub fn prove_topology_exact(
            origin: OriginFor<T>,
            topology_hash: H256,
            order: NodesOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let (topology, mut hardness) = Self::witness_subject(topology_hash)?;

            // A genuine ratchet: any strictly narrower order is accepted, not
            // only the first to cross the ceiling. Verification is bounded by
            // the smaller of the ceiling and the standing claim, so a better
            // witness never costs more than the one before it. Width 0 has no
            // claim left to falsify; every other case is caught by `bound`
            // below, which caps the replay one under the standing claim, so any
            // order it accepts is already a strict improvement. (An `ensure!`
            // on the returned width would be unreachable.)
            ensure!(
                hardness.residual_difficulty > 0,
                Error::<T>::TopologyAlreadyExact
            );
            let ceiling = T::ExactSolveCeiling::get();
            let bound = ceiling.min(hardness.residual_difficulty.saturating_sub(1));

            let width = induced_width_at_most(
                topology.nodes.as_slice(),
                topology.edges.as_slice(),
                order.as_slice(),
                bound,
            )
            .map_err(Self::witness_error)?;

            hardness.residual_difficulty = width;
            TopologyHardnessOf::<T>::insert(topology_hash, hardness);
            Self::deposit_event(Event::TopologyProvenExact {
                topology_hash,
                prover: who,
                width,
            });

            Self::retire_unless_default(topology_hash);
            Ok(())
        }

        /// Prove that a registered zero-field topology is polynomial-time
        /// solvable by exhibiting a planar embedding, and retire it.
        ///
        /// **Treewidth is not the only route to tractability.** A zero-field
        /// Ising ground state is max-cut, and planar max-cut is
        /// polynomial-time at any width — a 40×40 grid has treewidth 40, far
        /// above any exactness ceiling, and is still exactly solvable. Width
        /// alone is therefore unsound, and `prove_topology_exact` cannot catch
        /// this case: no narrow elimination order exists to exhibit.
        ///
        /// Planar Ising *with* a magnetic field is NP-hard again, so the
        /// witness only applies when `allowed_h` can sample nothing but zero
        /// (`TopologyIsFieldBearing` otherwise).
        ///
        /// Permissionless and `O(m)`: the chain cannot *run* a planarity
        /// algorithm, but it can trace the faces a rotation system induces and
        /// check Euler's formula. `rotation` lists, per node in the topology's
        /// own node order, the cyclic sequence of incident edge indices.
        #[pallet::call_index(11)]
        #[pallet::weight({
            // `TopologyDims`, NOT `RegisteredTopologies`: this closure runs
            // during pool validation on every gossiped extrinsic.
            let TopologyDim { nodes, edges } =
                TopologyDims::<T>::get(topology_hash).unwrap_or(TopologyDim::UNREGISTERED);
            <T as Config>::WeightInfo::prove_topology_planar(nodes, edges)
        })]
        pub fn prove_topology_planar(
            origin: OriginFor<T>,
            topology_hash: H256,
            rotation: BoundedVec<BoundedVec<u32, T::MaxEdges>, T::MaxNodes>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let (topology, mut hardness) = Self::witness_subject(topology_hash)?;
            ensure!(
                topology.allowed_h_values.samples_only_zero(),
                Error::<T>::TopologyIsFieldBearing
            );
            ensure!(
                hardness.residual_difficulty > T::ExactSolveCeiling::get(),
                Error::<T>::TopologyAlreadyExact
            );

            // Shape-check early. Modest: SCALE has already decoded the whole
            // `BoundedVec<BoundedVec<u32>>` by the time this body runs, and
            // `verify_planar_embedding` would reject both shapes anyway — only
            // the point of rejection moves. The real exposure is that this
            // call's weight is dimensioned on the TOPOLOGY while the submission
            // is bounded by block length; tracked with the weights work.
            ensure!(
                rotation.len() == topology.nodes.len(),
                Error::<T>::InvalidPlanarEmbedding
            );
            let declared_slots: usize = rotation.iter().map(|order| order.len()).sum();
            ensure!(
                declared_slots == topology.edges.len().saturating_mul(2),
                Error::<T>::InvalidPlanarEmbedding
            );

            // No `to_vec()` copy: `verify_planar_embedding` is generic over
            // `AsRef<[u32]>`, which `BoundedVec` implements. The copy it
            // replaces was up to 2m `u32` (~400 KB at MaxEdges) plus a heap
            // allocation per vertex, on a permissionless path, of an
            // already-decoded value.
            verify_planar_embedding(
                topology.nodes.as_slice(),
                topology.edges.as_slice(),
                rotation.as_slice(),
            )
            .map_err(Self::witness_error)?;

            // Tractable by a route the width does not see. Collapse the width
            // to zero so `regime_of` reads Exact and every width-based gate
            // follows without a second concept.
            hardness.residual_difficulty = 0;
            hardness.core_width = 0;
            TopologyHardnessOf::<T>::insert(topology_hash, hardness);
            Self::deposit_event(Event::TopologyProvenPlanar {
                topology_hash,
                prover: who,
            });

            Self::retire_unless_default(topology_hash);
            Ok(())
        }

        #[pallet::call_index(4)]
        #[pallet::weight({
            // Dimensioned on the actual proof, not a fixed placeholder
            // (mitigates QIP-03):
            //   W(n, e) = BASE + Kₙ·n + Kₑ·e
            //
            // NO `s` DIMENSION, and that is the property to preserve here:
            // `solutions` is one packed configuration BY TYPE, so nothing the
            // submitter puts in the proof can scale this closure. It runs during
            // pool validation on every gossiped proof, before any fee is charged.
            //
            // An unregistered hash does O(1) work before `TopologyNotRegistered`
            // and is charged base (n = e = 0). That
            // relies on all dispatch checks before the topology lookup staying
            // O(1). A registered topology rejected later (not mineable, bad
            // nonce, …) still pays the full formula — `DispatchResult` carries
            // no `PostDispatchInfo` refund, and over-charging rejected work is
            // the safe direction.
            //
            // `TopologyDims`, NOT `RegisteredTopologies`: this closure runs
            // during pool validation on every gossiped proof, including ones
            // about to be rejected, before any fee is charged.
            let TopologyDim { nodes, edges } = TopologyDims::<T>::get(proof.topology_hash)
                .unwrap_or(TopologyDim::UNREGISTERED);
            <T as Config>::WeightInfo::submit_proof(nodes, edges)
        })]
        pub fn submit_proof(origin: OriginFor<T>, proof: QuantumProofOf<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Miners::<T>::contains_key(&who),
                Error::<T>::MinerNotRegistered
            );
            ensure!(
                BlockProofCount::<T>::get() < T::MaxProofsPerBlock::get(),
                Error::<T>::ProofLimitReached
            );
            // AN EARLY REJECT, NOT A CORRECTNESS GATE — said plainly because the
            // difference decides whether anyone may delete it. `MinNodes` is
            // enforced at registration and again below, so `num_spins >= 2`, so the
            // expected packed length is >= 1 byte, so `unpack_solution` would refuse
            // a zero-byte payload anyway with `PackedSolutionLengthMismatch`. What
            // this buys is refusing it BEFORE `generate_ising_model_and_frustration`
            // does its O(n + e) pass. Removing it costs work, not soundness.
            //
            // Covered by `submit_proof_refuses_an_empty_packed_solution`, which
            // exists because nothing else fails if this line goes.
            ensure!(
                !proof.solutions.is_empty(),
                Error::<T>::NoSolutionsSubmitted
            );

            // The stored topology is the source of truth for nodes, edges and
            // value sets; `topology_hash` is the proof's only identity claim.
            let topology = RegisteredTopologies::<T>::get(proof.topology_hash)
                .ok_or(Error::<T>::TopologyNotRegistered)?;
            ensure!(
                MineableTopologies::<T>::contains_key(proof.topology_hash),
                Error::<T>::TopologyNotMineable
            );
            // The whitelist alone is not sufficient. A successful
            // `prove_topology_exact` / `prove_topology_planar` deliberately
            // leaves the live `DefaultTopology` whitelisted so a challenge
            // cannot halt the chain outright, so the regime check is the only
            // thing between a proven-tractable default and full block reward
            // for polynomial-time work. Paying for non-work is worse than
            // pausing: governance repoints with `set_default_topology`.
            ensure!(
                Self::regime_of(proof.topology_hash) != Some(quantum_validation::Regime::Exact),
                Error::<T>::TopologyIsExact
            );
            ensure!(
                topology.nodes.len() >= T::MinNodes::get() as usize,
                Error::<T>::GraphTooSmall
            );

            // Nonce is bound to `block_hash(LastProofBlock)`, not to the
            // executing block. That value only changes when a proof wins, so a
            // submission stays valid for the whole round — no txpool-delay race
            // (the original bug). From the cache, not
            // `frame_system::block_hash`: see `LastProofBlockHash`.
            let last_proof_block_hash_bytes = LastProofBlockHash::<T>::get().0;
            let miner_bytes = Self::account_to_bytes(&who);

            let expected_nonce =
                derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &proof.salt);
            ensure!(proof.nonce == expected_nonce, Error::<T>::InvalidNonce);

            // ONE `NodeIndex` for the instance and its frustration. As two
            // calls each built its own node-position index: a discarded
            // allocation plus an O(n log n) sort per proof on the sorted-table
            // path, an extra O(n) scan on the contiguous one. Same arithmetic,
            // same error ordering.
            let quantum_validation::IsingInstance {
                h,
                j,
                frustration_milli,
                frustration_cycles,
            } = quantum_validation::generate_ising_model_and_frustration(
                proof.nonce,
                topology.nodes.as_slice(),
                topology.edges.as_slice(),
                &topology.allowed_h_values.as_slice(),
                &topology.allowed_j_values.as_slice(),
            )
            .map_err(|_| Error::<T>::InvalidTopology)?;

            let current = Self::current_difficulty_for(
                proof.topology_hash,
                frame_system::Pallet::<T>::block_number(),
            );

            // An unfrustrated draw is not hard, at any width: zero frustrated
            // cycles means the couplings gauge-transform to all-ferromagnetic,
            // so the ground state is `-(Sum|h| + Sum|J|)` and gauge propagation
            // finds it in O(m). A forest draws zero by construction and is the
            // regime gate's job (treewidth 1), so it is excluded here rather
            // than double-judged. The handicap cannot price this away: a zero
            // draw sits many sigma below expected, saturates the +/-3 sigma
            // clamp, and buys a bar only 1.8% tighter than typical. The clamp
            // is denominated in sampling noise and tractability is not noise,
            // so this must be a validity gate, not a difficulty adjustment.
            //
            // P(zero) is ~2^-cycles, so only reachable on low-cycle-rank
            // topologies — but `MinNodes` admits those, and one can carry a
            // width past `ExactSolveCeiling` and so classify Heuristic.
            ensure!(
                frustration_cycles == 0 || frustration_milli > 0,
                Error::<T>::InstanceIsGaugeTrivial
            );
            let expected_frustration_milli = TopologyHardnessOf::<T>::get(proof.topology_hash)
                .map(|hardness| hardness.expected_frustration_milli)
                .unwrap_or_default();

            // Refuse draws past the handicap's reach instead of capping them.
            // Past ±3σ the clamp saturates and hands back a bar only 1.8%
            // tighter than typical, so that is where the mechanism gives up and
            // where the instance should stop being valid. Costs honest miners
            // ~1 salt in 1000 and a re-salt; costs a grinder the only part of
            // the distribution worth grinding for.
            //
            // TWO-SIDED. Refusing only the easy tail is wrong once the handicap
            // is in play: it LOOSENS the bar for a hard draw, so if its slope
            // over-states frustration's true cost — measured scatter is 4-29
            // per-mille per σ against a deployed 6, so it might — grinding
            // toward the hard tail buys a bar looser than justified.
            //
            // A residual gradient remains INSIDE the band, worth up to
            // `HANDICAP_MAX_SIGMA` times the deployed-vs-true slope gap. It is
            // bounded by the band width and cannot be removed without a slope
            // measurement `riff-hv9.22` found unobtainable over the natural
            // draw range.
            //
            // ONE `FrustrationDraw`, read by both the gate and the price. As
            // two separate spellings each computed sigma independently, and
            // nothing made them agree — the gate only refuses draws past the
            // handicap's REACH if it measures the same reach. It also removes
            // the last transposable positional triple from the consensus path:
            // both `u32` milli in `0..=1000`, adjacent, confusable names, so
            // transposed it COMPILES, the deviation flips sign (which the
            // gate's `.abs()` HIDES) and sigma comes from the realized draw
            // instead of the topology's expectation.
            let draw = crate::difficulty::FrustrationDraw {
                realized_milli: frustration_milli,
                expected_milli: expected_frustration_milli,
                cycles: frustration_cycles,
            };
            ensure!(
                crate::difficulty::frustration_sigmas_of(draw).abs()
                    <= crate::difficulty::HANDICAP_MAX_SIGMA,
                Error::<T>::InstanceOutsideFrustrationBand
            );
            let bound_milli = energy_bound_milli(&h, &j);
            let anchor_milli = crate::difficulty::anchor_milli(bound_milli);
            let instance_bar_milli = crate::difficulty::instance_bar_milli(
                current.max_energy_milli,
                bound_milli,
                draw,
                T::HandicapPerSigmaPermille::get(),
            );

            let validation = Self::validate_proof(&proof, &topology, &h, &j)?;
            // Reaching the bar clears it. The handicap can drive the bar to the
            // instance's exact optimum, and a strict comparison would make that
            // demand unsatisfiable rather than merely maximal.
            ensure!(
                validation.energy_milli <= instance_bar_milli,
                Error::<T>::InsufficientEnergy
            );

            let mut miner = Miners::<T>::get(&who).ok_or(Error::<T>::MinerNotRegistered)?;
            miner.proofs_submitted = miner.proofs_submitted.saturating_add(1);
            Miners::<T>::insert(&who, miner);

            let record = ProofRecordOf::<T> {
                miner: who.clone(),
                submitted_at: frame_system::Pallet::<T>::block_number(),
                energy_milli: validation.energy_milli,
                salt: proof.salt,
                topology_hash: proof.topology_hash,
                device_access_time_us: proof.device_access_time_us,
                frustration_milli,
                instance_bar_milli,
                anchor_milli,
                difficulty: current,
            };

            // Rank by how far a proof beat ITS OWN bar, not by absolute energy.
            // Admission is per-instance, so selection has to be too.
            //
            // Absolute energy is not comparable across draws: reachable depth
            // scales with the instance's own `sum|h| + sum|J|`, which the salt
            // moves. Grinding for the largest-magnitude draw is O(n + m) per
            // try, so 1e5 salts on a ternary-field topology at n = 4800 buys
            // ~4 sigma of extra reach — a ~12% edge, dwarfing the 1.8% handicap
            // this file works so hard to bound.
            //
            // A raw margin `energy - bar` does NOT fix that, though it looks
            // like it should. `instance_bar_milli` is `max(curve_bar, anchor)`
            // with `curve_bar` a per-topology constant, so where the curve binds
            // the margin is the energy shifted by a constant and the ranking is
            // unchanged. Where the anchor binds it is worse: the best reachable
            // margin is `anchor - bar`, which grows with `sum|h| + sum|J|`, so
            // a strong draw beats every weak draw however well solved.
            //
            // Normalizing by the instance's own room is what makes the
            // comparison scale-free: `quality` is the fraction of the distance
            // from the bar to that instance's exact optimum, so a bigger draw's
            // extra depth and extra room cancel. A zero-room instance (bar at
            // the optimum) scores LOWEST, not full marks — see `proof_quality`.
            if Self::proof_outranks_block_best(&record) {
                BlockBestProof::<T>::put(record);
            }

            BlockProofCount::<T>::mutate(|count| {
                *count = count.saturating_add(1);
            });

            Self::deposit_event(Event::ProofAccepted {
                miner: who,
                energy_milli: validation.energy_milli,
                frustration_milli,
                instance_bar_milli,
            });

            Ok(())
        }

        /// Add a registered topology to the mineable whitelist. Root only.
        #[pallet::call_index(6)]
        #[pallet::weight(<T as Config>::WeightInfo::add_mineable_topology())]
        pub fn add_mineable_topology(origin: OriginFor<T>, topology_hash: H256) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(
                RegisteredTopologies::<T>::contains_key(topology_hash),
                Error::<T>::TopologyNotRegistered
            );
            // An unclassified topology prices its instances off the bare curve
            // and is un-challengeable. Registration supplies the record, so
            // this only trips if one was somehow removed.
            ensure!(
                TopologyHardnessOf::<T>::contains_key(topology_hash),
                Error::<T>::TopologyNotClassified
            );
            // An exactly-solvable topology is not useful work, so it never
            // enters the whitelist in the first place. `prove_topology_exact`
            // covers the other direction — a topology that turns out to be
            // exact after it was whitelisted is retired then.
            ensure!(
                Self::regime_of(topology_hash) == Some(quantum_validation::Regime::Heuristic),
                Error::<T>::TopologyIsExact
            );
            if !MineableTopologies::<T>::contains_key(topology_hash) {
                // Model A: at most one non-default mineable topology at a time,
                // so the global decay anchor stays correct. Caps the whitelist
                // at {default, one incoming}, which is also what bounds this
                // scan to <=2 keys.
                let default = DefaultTopology::<T>::get();
                if Some(topology_hash) != default {
                    let has_other_non_default =
                        MineableTopologies::<T>::iter_keys().any(|h| Some(h) != default);
                    ensure!(!has_other_non_default, Error::<T>::MineableTopologyConflict);
                }
                MineableTopologies::<T>::insert(topology_hash, ());
                Self::deposit_event(Event::TopologyMineableAdded { topology_hash });
            }
            Ok(())
        }

        /// Remove a topology from the mineable whitelist. Root only. Refuses
        /// to remove the current `DefaultTopology` so the default is always
        /// mineable.
        #[pallet::call_index(7)]
        #[pallet::weight(<T as Config>::WeightInfo::remove_mineable_topology())]
        pub fn remove_mineable_topology(
            origin: OriginFor<T>,
            topology_hash: H256,
        ) -> DispatchResult {
            ensure_root(origin)?;
            ensure!(
                DefaultTopology::<T>::get() != Some(topology_hash),
                Error::<T>::TopologyIsDefault
            );
            if MineableTopologies::<T>::contains_key(topology_hash) {
                MineableTopologies::<T>::remove(topology_hash);
                Self::deposit_event(Event::TopologyMineableRemoved { topology_hash });
            }
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Close the retarget epoch if `EpochLength` blocks have elapsed,
        /// driving the energy bar toward the target block cadence.
        ///
        /// The *only* control loop on average block time. The per-proof walk it
        /// replaced double-counted: a proof arriving early tightened the bar
        /// immediately, then the epoch containing it tightened again on the
        /// same evidence.
        ///
        /// Runs on every block, win or not: an epoch with zero qblocks is
        /// precisely the case that needs easing and has no winning proof to
        /// hang an adjustment off.
        fn maybe_retarget(n: BlockNumberFor<T>) {
            let epoch_length = T::EpochLength::get();
            let window = epoch_length.saturating_mul(T::RetargetWindowEpochs::get().max(1).into());
            let start = EpochStart::<T>::get();
            if start.is_zero() {
                EpochStart::<T>::put(n);
                return;
            }
            if n.saturating_sub(start) < window {
                return;
            }

            let epoch_blocks = n.saturating_sub(start).saturated_into::<u64>();
            let target = epoch_length.saturated_into::<u64>();

            // Model A caps the whitelist at two, so this scan is bounded.
            for topology_hash in MineableTopologies::<T>::iter_keys() {
                let Some(curve) = Self::energy_curve_for(topology_hash) else {
                    continue;
                };
                let qblocks = EpochQBlocks::<T>::get(topology_hash);
                // A zero-qblock window means "ease, the chain is stalled" — but
                // only for a topology miners were expected to be working. Two
                // cases are idle rather than stalled: not the default, and
                // freshly repointed with no stored bar yet (otherwise the
                // window right after `set_default_topology(B)` hands B a full
                // ease off its own knee before any miner has seen it).
                let is_default = DefaultTopology::<T>::get() == Some(topology_hash);
                // `chain_has_run` distinguishes "freshly repointed" from
                // genesis, where "never mined" describes the default topology
                // itself. Skipping the ease there is fatal: `Difficulties` is
                // written only by the v3 migration, `set_difficulty` and this
                // loop, so a default that has never won stays unwritten
                // forever, and decay cannot cover it either
                // (`LastProofBlock` is still the genesis zero, so
                // `current_difficulty` returns the baseline unchanged). A knee
                // bar the initial miner set cannot clear would brick the chain.
                let chain_has_run = !LastProofBlock::<T>::get().is_zero();
                let freshly_repointed =
                    chain_has_run && Difficulties::<T>::get(topology_hash).is_none();
                if qblocks == 0 && (!is_default || freshly_repointed) {
                    continue;
                }
                // Retarget the STORED baseline, deliberately, even though
                // miners face `current_difficulty_for` (the baseline eased one
                // decay step per elapsed `EpochLength` since the last proof).
                //
                // Reading the effective value instead is worse. Committing
                // decay into the baseline makes it permanent while
                // `LastProofBlock` does not advance, so the next window
                // recomputes its step count from the same anchor and applies it
                // on top: compounding, not composition. It also hands miners a
                // lever — window boundaries are public, so withholding the
                // qblock that would land in the last `EpochLength` blocks banks
                // a permanent ease every window while the controller sees an
                // on-target chain.
                //
                // Leaving decay as a pure view costs nothing: at the design
                // cadence it is a roughly CONSTANT one-step offset, and the
                // loop absorbs a constant offset — what is driven to target is
                // the cadence the effective bar produces. An offset is not a
                // drift.
                //
                // One curve lookup for the whole iteration. `baseline_for` and
                // `current_difficulty_for` each called `energy_curve_for`
                // again, so this ran three times per topology per window: six
                // storage reads and three `expected_gse` recomputations for a
                // value that cannot change mid-loop.
                let current =
                    Difficulties::<T>::get(topology_hash).unwrap_or(types::DifficultyConfig {
                        max_energy_milli: curve.knee_milli,
                    });
                let retargeted = types::DifficultyConfig {
                    max_energy_milli: crate::difficulty::retarget_bar_milli(
                        current.max_energy_milli,
                        curve,
                        crate::difficulty::RetargetWindow {
                            qblocks,
                            epoch_blocks,
                            target_blocks_per_qblock: target,
                        },
                        T::RetargetMaxStepPermille::get(),
                    ),
                };
                if retargeted != current {
                    Difficulties::<T>::insert(topology_hash, retargeted);
                    Self::deposit_event(Event::DifficultyRetargeted {
                        topology_hash,
                        // 0 is the stalled sentinel. Dividing by `max(1)`
                        // reported `epoch_length` for an empty epoch — exactly
                        // the on-target value — so an indexer could not tell a
                        // perfectly paced chain from a dead one.
                        blocks_per_qblock: if qblocks == 0 {
                            0
                        } else {
                            epoch_blocks / u64::from(qblocks)
                        },
                        difficulty: retargeted,
                    });
                }
            }

            EpochStart::<T>::put(n);
            let _ = EpochQBlocks::<T>::clear(u32::MAX, None);
        }

        /// The stored difficulty baseline for a topology, or its own curve's
        /// knee when nothing has been stored yet.
        ///
        /// `DifficultyConfig::default()` is a hardcoded `-1_200_000` predating
        /// per-topology curves, belonging to no topology in particular: a
        /// freshly whitelisted topology would mine against a bar unrelated to
        /// its own energy scale until the first retarget — trivially clearable
        /// on a large graph, unclearable on a small one. The knee is that
        /// topology's own moderate calibration point.
        fn baseline_for(topology_hash: H256) -> types::DifficultyConfig {
            Difficulties::<T>::get(topology_hash).unwrap_or_else(|| {
                Self::energy_curve_for(topology_hash)
                    .map(|curve| types::DifficultyConfig {
                        max_energy_milli: curve.knee_milli,
                    })
                    .unwrap_or_default()
            })
        }

        /// Reject a structural record that cannot describe any graph.
        /// `expected_frustration_milli` is a per-mille fraction, so a value
        /// above `1000` would silently mis-price every instance of the
        /// topology through the bar's σ.
        fn check_hardness(hardness: &types::TopologyHardness) -> DispatchResult {
            // Bounded away from BOTH endpoints. The handicap divides by
            // `σ = sqrt(p(1-p)/cycles)`, so the ends of the range make it a
            // step function rather than a gradient: at `1000` the variance is
            // zero, σ floors to one micro, and any deviation saturates the ±3σ
            // clamp; at `1` σ is ~0.155 milli at production cycle counts, the
            // same failure less obviously. Neither describes a real topology —
            // an acyclic one reports zero cycles and skips the handicap.
            ensure!(
                hardness.expected_frustration_milli == 0
                    || (MIN_EXPECTED_FRUSTRATION_MILLI..=MAX_EXPECTED_FRUSTRATION_MILLI)
                        .contains(&hardness.expected_frustration_milli),
                Error::<T>::InvalidHardnessRecord
            );
            Ok(())
        }

        /// Refuse specs that admit an all-zero draw.
        ///
        /// Such an instance has `Σ|h| + Σ|J| = 0`, so every configuration has
        /// energy `0` and any bar is cleared for no work — the gauge-trivial
        /// hole, arriving through the value specs rather than through the
        /// couplings.
        ///
        /// This says NOTHING about the energy curve. The related concern — the
        /// anchor above `curve.max_milli`, so the `max(curve_bar, anchor)`
        /// clamp disconnects the retarget — is curve calibration, needs the
        /// curve compared against the spec *distribution* rather than one
        /// worst-case draw, and is tracked as `riff-hv9.54`.
        fn ensure_specs_cannot_draw_zero(
            node_count: usize,
            edge_count: usize,
            allowed_h: &AllowedValueSpec<&[MilliValue]>,
            allowed_j: &AllowedValueSpec<&[MilliValue]>,
        ) -> DispatchResult {
            let loosest = quantum_validation::loosest_energy_bound_milli(
                node_count as u64,
                edge_count as u64,
                allowed_h,
                allowed_j,
            );
            // A zero loosest bound means SOME draw is all-zero, but on a large
            // graph that draw is unreachable (every node and edge must land on
            // zero at once), so refusing outright would bar an ordinary ternary
            // coupling spec. Scale the refusal to the graph: below
            // `ZERO_DRAW_SAFE_DRAWS` independent draws the all-zero instance is
            // reachable enough to matter; above it the realized near-zero case
            // is handled per-instance by the frustration and gauge-triviality
            // gates in `submit_proof`.
            let independent_draws = node_count.saturating_add(edge_count);
            ensure!(
                loosest > 0 || independent_draws >= ZERO_DRAW_SAFE_DRAWS,
                Error::<T>::InstanceIsGaugeTrivial
            );
            Ok(())
        }

        /// Refuse a curve whose easiest value the anchor clamp would override
        /// for the *typical* draw.
        ///
        /// The per-instance bar is `max(curve_bar, anchor)`. When the anchor
        /// sits above `curve.max_milli` that `max` picks the anchor, and no
        /// bar the retarget can produce changes the effective one — the
        /// control loop is disconnected and a chain needing easing has no way
        /// to get it.
        ///
        /// Weighed against the *expected* bound, not the loosest single draw.
        /// The loosest-draw form over-rejects: a low-magnitude draw genuinely
        /// has a shallow optimum, so pinning the bar there asks for that
        /// instance's exact ground state — attainable, merely hard, and the
        /// miner re-salts. A tail event, not a stall. The pathology this
        /// catches is the anchor above the cap for the draw the topology
        /// usually produces, a curve-calibration failure.
        ///
        /// The expected bound is an ARGUMENT, not a field on `EnergyCurve`:
        /// as a field, every difficulty read (`current_difficulty_for`, and
        /// `maybe_retarget` once per topology per window) computed a value
        /// whose only reader is this registration-time guard.
        fn ensure_curve_outlasts_the_typical_anchor(
            curve: &crate::difficulty::EnergyCurve,
            expected_bound_milli: u64,
        ) -> DispatchResult {
            // Fail CLOSED on overflow. `unwrap_or(i64::MAX)` — or the saturating
            // `difficulty::anchor_milli` — would make `typical_anchor` the most
            // negative value there is, so the `<=` below would pass vacuously
            // and the guard would check nothing.
            let bound =
                i64::try_from(expected_bound_milli).map_err(|_| Error::<T>::InvalidCurve)?;
            // Deliberately NOT `difficulty::anchor_milli`, which saturates:
            // see the fail-closed note above and on that helper.
            let typical_anchor = 0i64.saturating_sub(bound);
            ensure!(typical_anchor <= curve.max_milli, Error::<T>::InvalidCurve);
            Ok(())
        }

        fn retire_unless_default(topology_hash: H256) {
            if DefaultTopology::<T>::get() != Some(topology_hash)
                && MineableTopologies::<T>::contains_key(topology_hash)
            {
                MineableTopologies::<T>::remove(topology_hash);
                Self::deposit_event(Event::TopologyMineableRemoved { topology_hash });
            }
        }

        /// Load a registered topology and its hardness record, or fail with the
        /// error naming which is missing. The preamble every witness extrinsic
        /// opens with.
        fn witness_subject(
            topology_hash: H256,
        ) -> Result<(TopologyMetaOf<T>, types::TopologyHardness), Error<T>> {
            let topology = RegisteredTopologies::<T>::get(topology_hash)
                .ok_or(Error::<T>::TopologyNotRegistered)?;
            let hardness = TopologyHardnessOf::<T>::get(topology_hash)
                .ok_or(Error::<T>::TopologyNotClassified)?;
            Ok((topology, hardness))
        }

        /// Map a [`quantum_validation::WitnessError`] onto a pallet error.
        ///
        /// One pallet error per defect, not one collapsed error plus an event
        /// naming the reason: a failing extrinsic rolls back its events, so the
        /// reason would be discarded exactly when it is needed.
        ///
        /// The split that matters is malformed vs. false-claim.
        /// `WidthAboveCeiling` and `NotPlanar` mean the graph is not what the
        /// submitter thought and no re-encoding helps; everything else means the
        /// witness is wrong and worth resubmitting.
        /// [`quantum_validation::WitnessError::is_submitter_fixable`] draws the
        /// same line off-chain.
        ///
        /// THE FEE DELIBERATELY DOES NOT FOLLOW THE VERDICT. `SelfLoop` and
        /// `TopologyEdgeUnknownNode` name a defect in STATE THE SUBMITTER DID
        /// NOT CREATE (registration refuses both today; they survive only on
        /// topologies stored before those branches existed and carried forward
        /// by v6), so `Pays::No` for those two is tempting. It is worse than the
        /// unfairness it fixes: the state does not self-heal, this extrinsic is
        /// permissionless with a topology-dimensioned weight, and `Pays::No`
        /// refunds the fee but NOT the block weight — an attacker who finds one
        /// such topology consumes the most expensive witness path every block
        /// for free. Nor can the work be refunded as unused weight: the
        /// self-loop is discovered DURING `verify_planar_embedding`'s walk, so
        /// the O(n + m) pass has already run. A one-shot refund needs tracking
        /// state costing more than the fee. The fix is governance retiring the
        /// defective topology; revisit only with a mechanism bounding
        /// repetition.
        fn witness_error(e: quantum_validation::WitnessError) -> Error<T> {
            use quantum_validation::WitnessError as W;
            match e {
                W::WidthAboveCeiling => Error::<T>::WidthAboveCeiling,
                W::NotPlanar => Error::<T>::NotPlanar,
                W::OrderLength => Error::<T>::WitnessOrderLength,
                W::UnknownNode => Error::<T>::WitnessUnknownNode,
                W::DuplicateNode => Error::<T>::WitnessDuplicateNode,
                W::EdgeIndexOutOfRange => Error::<T>::WitnessEdgeIndexOutOfRange,
                W::EdgeNotIncident => Error::<T>::WitnessEdgeNotIncident,
                W::SelfLoop => Error::<T>::WitnessSelfLoop,
                W::TopologyEdgeUnknownNode => Error::<T>::WitnessTopologyEdgeUnknownNode,
                W::RotationLength => Error::<T>::WitnessRotationLength,
            }
        }

        /// Every registered topology must carry its dims.
        ///
        /// The three weight closures fall back to `TopologyDim::UNREGISTERED`,
        /// pricing `submit_proof` at base. Correct for an UNREGISTERED hash —
        /// the call fails immediately on the meta lookup — but for a REGISTERED
        /// one missing its dims the call SUCCEEDS at full cost for a base fee:
        /// an unmetered DoS surface on the consensus path.
        ///
        /// The invariant holds by construction (one writer each, no removers,
        /// re-registration refused, v6's canonicalization count-preserving),
        /// but that is an argument, and this is the failure it has to catch.
        ///
        /// IN `try_state`, NOT `post_upgrade`: the invariant depends on
        /// `register_topology`, which runs on any block, so a once-per-upgrade
        /// check cannot see a break introduced between upgrades. `post_upgrade`
        /// calls this — the standard FRAME shape.
        #[cfg(feature = "try-runtime")]
        pub fn do_try_state() -> Result<(), sp_runtime::TryRuntimeError> {
            ensure!(
                RegisteredTopologies::<T>::iter_keys().count()
                    == TopologyDims::<T>::iter_keys().count(),
                "every registered topology must have a TopologyDims entry"
            );
            Ok(())
        }

        pub fn proof_quality(record: &ProofRecordOf<T>) -> i128 {
            // The arithmetic lives in `difficulty` so it can be tested without a
            // `T: Config` and recomputed off-chain by a miner deciding whether
            // a proof is worth submitting.
            crate::difficulty::proof_quality(crate::difficulty::ProofDepth {
                instance_bar_milli: record.instance_bar_milli,
                anchor_milli: record.anchor_milli,
                energy_milli: record.energy_milli,
            })
        }

        /// Whether `record` should displace the block's current best.
        pub fn proof_outranks_block_best(record: &ProofRecordOf<T>) -> bool {
            match BlockBestProof::<T>::get() {
                Some(existing) => {
                    let (mine, theirs) =
                        (Self::proof_quality(record), Self::proof_quality(&existing));
                    // Ties broken on absolute energy, so equal-quality proofs
                    // are still ordered by work rather than by arrival order.
                    (mine, existing.energy_milli) > (theirs, record.energy_milli)
                }
                None => true,
            }
        }

        /// Structural regime of a registered topology, derived from its
        /// recorded width and `ExactSolveCeiling` rather than stored — so the
        /// verdict follows the ceiling, and a topology whose width has been
        /// ratcheted down by `prove_topology_exact` re-classifies itself.
        /// `None` when the topology has no structural record.
        pub fn regime_of(topology_hash: H256) -> Option<quantum_validation::Regime> {
            TopologyHardnessOf::<T>::get(topology_hash).map(|hardness| {
                // Either route being cheap makes the topology cheap: a narrow
                // raw graph is solvable outright, one the reduction stack
                // collapses is solvable through its core. The minimum is the
                // conservative direction for a useful-work gate.
                let width = hardness.residual_difficulty.min(hardness.core_width);
                if width <= T::ExactSolveCeiling::get() {
                    quantum_validation::Regime::Exact
                } else {
                    quantum_validation::Regime::Heuristic
                }
            })
        }

        pub fn default_topology() -> Option<H256> {
            DefaultTopology::<T>::get()
        }

        pub fn topology_meta(hash: H256) -> Option<TopologyMetaOf<T>> {
            RegisteredTopologies::<T>::get(hash)
        }

        pub fn default_topology_meta() -> Option<(H256, TopologyMetaOf<T>)> {
            let topology_hash = DefaultTopology::<T>::get()?;
            let topology = RegisteredTopologies::<T>::get(topology_hash)?;
            Some((topology_hash, topology))
        }

        pub fn miner_info(account: &T::AccountId) -> Option<MinerInfoOf<T>> {
            Miners::<T>::get(account)
        }

        /// Active (decay-applied) difficulty a miner must clear for
        /// `topology_hash` at `block_number`. Reads the per-topology baseline
        /// (`Difficulties[hash]`, defaulting when unset) and applies global
        /// block-based decay since the last winning proof.
        pub fn current_difficulty_for(
            topology_hash: H256,
            block_number: BlockNumberFor<T>,
        ) -> types::DifficultyConfig {
            crate::difficulty::current_difficulty(
                block_number.saturated_into::<u32>(),
                Self::baseline_for(topology_hash),
                LastProofBlock::<T>::get().saturated_into::<u32>(),
                T::EpochLength::get().saturated_into::<u32>(),
                Self::energy_curve_for(topology_hash),
            )
        }

        pub fn latest_qblock_id() -> Option<u64> {
            let count = QBlockCount::<T>::get();
            (count > 0).then_some(count)
        }

        pub fn qblock_id_by_block(block_number: BlockNumberFor<T>) -> Option<u64> {
            QBlockIdByBlock::<T>::get(block_number)
        }

        pub fn qblock_block_by_id(qblock_id: u64) -> Option<BlockNumberFor<T>> {
            QBlockBlockById::<T>::get(qblock_id)
        }

        pub fn mining_snapshot(topology_hash: Option<H256>) -> Option<MiningSnapshotOf<T>> {
            let (topology_hash, topology) = match topology_hash {
                Some(hash) => (hash, Self::topology_meta(hash)?),
                None => Self::default_topology_meta()?,
            };

            // Difficulty still tracks the current block (decay is block-based)
            // even though the nonce input no longer does. The seed comes from
            // the cache, so the snapshot stays stable across the full round.
            let block_number = frame_system::Pallet::<T>::block_number();
            let last_proof_block_hash = LastProofBlockHash::<T>::get();

            Some(types::MiningSnapshot {
                last_proof_block_hash,
                difficulty: Self::current_difficulty_for(topology_hash, block_number),
                topology_hash,
                nodes: topology.nodes,
                edges: topology.edges,
                allowed_h_values: topology.allowed_h_values,
                allowed_j_values: topology.allowed_j_values,
                allowed_spin_values: topology.allowed_spin_values,
            })
        }

        /// Per-topology live difficulty (decay applied). Returns `None` if
        /// `topology_hash` has never been registered.
        pub fn difficulty_for_api(topology_hash: H256) -> Option<types::DifficultyConfig> {
            RegisteredTopologies::<T>::contains_key(topology_hash).then(|| {
                Self::current_difficulty_for(
                    topology_hash,
                    frame_system::Pallet::<T>::block_number(),
                )
            })
        }

        /// Hashes of every topology currently on the mineable whitelist.
        pub fn mineable_topologies() -> Vec<H256> {
            MineableTopologies::<T>::iter_keys().collect()
        }

        /// 32-byte representation of an account, suitable for use as a fixed-size
        /// input to `derive_nonce`. Hashes the SCALE-encoded `AccountId` so any
        /// underlying encoding width (8-byte `u64`, 32-byte `AccountId32`, etc.)
        /// produces a deterministic 32-byte digest.
        pub fn account_to_bytes(account: &T::AccountId) -> [u8; 32] {
            sp_io::hashing::blake2_256(&account.encode())
        }

        /// Look up a persisted qblock and re-derive its nonce.
        ///
        /// Returns `None` if the block had no accepted proof (e.g. genesis
        /// where no `submit_proof` ever ran). Re-derivation reads the
        /// `last_proof_block_hash` stored alongside the qblock, so this stays
        /// correct even when `block_number` is older than `BlockHashCount`
        /// (no `frame_system::block_hash` lookup is involved).
        pub fn qblock_with_nonce(block_number: BlockNumberFor<T>) -> Option<QBlockWithNonceOf<T>> {
            let solution = QBlocks::<T>::get(block_number)?;
            let last_proof_block_hash_bytes = solution.last_proof_block_hash.0;
            let miner_bytes = Self::account_to_bytes(&solution.miner);
            let nonce = derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &solution.salt);
            Some(types::QBlockWithNonce { solution, nonce })
        }

        pub fn qblock_with_nonce_by_id(qblock_id: u64) -> Option<QBlockWithNonceOf<T>> {
            let block_number = Self::qblock_block_by_id(qblock_id)?;
            Self::qblock_with_nonce(block_number)
        }

        fn next_qblock_id() -> u64 {
            QBlockCount::<T>::mutate(|count| {
                *count = count.saturating_add(1);
                *count
            })
        }

        /// 32-byte representation of a block hash, suitable for use as a
        /// fixed-size input to `derive_nonce`. Works for any `T::Hash`
        /// whose SCALE encoding is exactly 32 bytes (the substrate default
        /// `BlakeTwo256` `H256`). Falls back to `blake2_256` of the encoded
        /// form so non-32-byte `T::Hash` configurations are also covered.
        pub fn hash_to_bytes_32(hash: <T as frame_system::Config>::Hash) -> [u8; 32] {
            let encoded = hash.encode();
            if let Ok(arr) = <[u8; 32]>::try_from(encoded.as_slice()) {
                arr
            } else {
                sp_io::hashing::blake2_256(&encoded)
            }
        }

        fn check_spec(spec: &AllowedValueSpec<AllowedValueSetOf<T>>) -> DispatchResult {
            match spec.as_slice().bits_per_value() {
                Ok(_) => Ok(()),
                Err(quantum_validation::ValidationError::EmptyAllowedValues) => {
                    Err(Error::<T>::EmptyAllowedValues.into())
                }
                Err(quantum_validation::ValidationError::EncodingTooWide { .. }) => {
                    Err(Error::<T>::EncodingTooWide.into())
                }
                Err(_) => Err(Error::<T>::InvalidTopology.into()),
            }
        }

        /// Number of variable allowed-value inputs represented by a spec.
        ///
        /// Explicit sets contribute their cardinality. Range variants have
        /// two encoded endpoints and therefore contribute two fixed inputs.
        fn allowed_value_input_count(spec: &AllowedValueSpec<AllowedValueSetOf<T>>) -> u32 {
            match spec {
                AllowedValueSpec::Set(values) => values.len() as u32,
                AllowedValueSpec::IntegerRange { .. }
                | AllowedValueSpec::ContinuousRange { .. } => 2,
            }
        }

        /// Sort the inner Set values so the stored spec matches the
        /// order-independent layout used by `canonical_bytes` / `hash_topology`.
        /// `IntegerRange` and `ContinuousRange` carry no order to canonicalize.
        fn canonicalize_spec(
            spec: AllowedValueSpec<AllowedValueSetOf<T>>,
        ) -> Result<AllowedValueSpec<AllowedValueSetOf<T>>, DispatchError> {
            match spec {
                AllowedValueSpec::Set(values) => {
                    let mut inner: alloc::vec::Vec<MilliValue> = values.into_inner();
                    inner.sort_unstable();
                    let sorted = AllowedValueSetOf::<T>::try_from(inner)
                        .map_err(|_| Error::<T>::InvalidTopology)?;
                    Ok(AllowedValueSpec::Set(sorted))
                }
                other => Ok(other),
            }
        }

        /// Build the difficulty energy curve for a specific topology.
        ///
        /// Deriving the curve from the proof's OWN topology is safe because the
        /// mineable whitelist, not a hard pin to `DefaultTopology`, is what
        /// stops a miner shifting difficulty by choosing a different registered
        /// topology: only whitelisted topologies can be mined and each owns its
        /// own root-set `Difficulties` entry, so there is no shared difficulty
        /// to pin.
        ///
        /// `None` when the topology is not registered (defensive: a proof would
        /// not validate, and `set_difficulty` requires registration).
        pub(crate) fn energy_curve_for(
            topology_hash: H256,
        ) -> Option<crate::difficulty::EnergyCurve> {
            let topology = RegisteredTopologies::<T>::get(topology_hash)?;
            // Per-topology override, falling back to the runtime constants.
            let curve_c = TopologyCurveC::<T>::get(topology_hash).unwrap_or_else(|| {
                crate::difficulty::CurveC {
                    easy_milli: T::CurveCEasyMilli::get(),
                    knee_milli: T::CurveCKneeMilli::get(),
                    hard_milli: T::CurveCHardMilli::get(),
                }
            });
            crate::difficulty::EnergyCurve::new(
                topology.nodes.len() as u32,
                topology.edges.len() as u32,
                curve_c,
                &topology.allowed_h_values.as_slice(),
                &topology.allowed_j_values.as_slice(),
            )
            .ok()
        }

        fn update_winner_streak(miner: &T::AccountId) -> WinnerStreakOf<T> {
            let next = match WinnerStreak::<T>::get() {
                Some(mut streak) if streak.miner == *miner => {
                    streak.count = streak.count.saturating_add(1);
                    streak
                }
                _ => WinnerStreakOf::<T> {
                    miner: miner.clone(),
                    count: 1,
                },
            };
            WinnerStreak::<T>::put(&next);
            next
        }

        /// Decode the proof's single configuration and return its exact energy.
        /// ONE `energy_of_solution` call — the dominant per-proof cost — where
        /// the multi-solution form paid one per submitted configuration to
        /// enforce a diversity property the bar never read.
        fn validate_proof(
            proof: &QuantumProofOf<T>,
            topology: &TopologyMetaOf<T>,
            h: &[MilliValue],
            j: &[MilliValue],
        ) -> Result<types::ProofValidation, DispatchError> {
            let spin_spec = topology.allowed_spin_values.as_slice();
            let num_spins = topology.nodes.len();
            let packed = &proof.solutions;

            let milli = unpack_solution(packed.as_slice(), num_spins, &spin_spec).map_err(
                |err| match err {
                    quantum_validation::ValidationError::PackedSolutionLengthMismatch {
                        ..
                    } => DispatchError::from(Error::<T>::PackedSolutionLengthMismatch),
                    quantum_validation::ValidationError::InvalidEncodedValue { .. } => {
                        DispatchError::from(Error::<T>::InvalidEncodedSpin)
                    }
                    _ => DispatchError::from(Error::<T>::InvalidTopology),
                },
            )?;
            let mut spins = Vec::with_capacity(milli.len());
            for value in milli {
                let sign = value.signum();
                ensure!(sign == -1 || sign == 1, Error::<T>::InvalidSpinValues);
                spins.push(sign as i8);
            }
            ensure!(validate_spins(&spins), Error::<T>::InvalidSpinValues);

            let energy_milli = energy_of_solution(
                &spins,
                h,
                topology.edges.as_slice(),
                j,
                topology.nodes.as_slice(),
            )
            .map_err(|err| match err {
                quantum_validation::ValidationError::SolutionLengthMismatch { .. } => {
                    DispatchError::from(Error::<T>::SolutionLengthMismatch)
                }
                quantum_validation::ValidationError::InvalidSpinValue { .. } => {
                    DispatchError::from(Error::<T>::InvalidSpinValues)
                }
                _ => DispatchError::from(Error::<T>::InvalidTopology),
            })?;

            Ok(types::ProofValidation { energy_milli })
        }
    }
}

pub(crate) mod migration {
    use crate::types;
    use codec::{Decode, Encode};

    /// `DifficultyConfig` as encoded through storage v5: the energy bar plus
    /// the `min_solutions` / `min_diversity_milli` pair that v6 drops. Kept
    /// so both the v3 carry-forward and the v6 translate can decode values
    /// written before the shrink.
    #[derive(Decode, Encode)]
    pub(crate) struct LegacyDifficultyConfig {
        pub(crate) min_solutions: u32,
        pub(crate) max_energy_milli: i64,
        pub(crate) min_diversity_milli: u32,
    }

    impl From<LegacyDifficultyConfig> for types::DifficultyConfig {
        fn from(old: LegacyDifficultyConfig) -> Self {
            Self {
                max_energy_milli: old.max_energy_milli,
            }
        }
    }

    pub(crate) mod v3 {
        use crate::pallet::{Config, Difficulties, MineableTopologies, Pallet};
        use crate::{types, BlockBestProof, DefaultTopology};
        use frame_support::traits::{Get, PalletInfoAccess};
        use frame_support::weights::Weight;
        use frame_support::{StorageHasher, Twox128};

        /// Raw storage key of the pre-v3 global `Difficulty` StorageValue:
        /// `twox128(pallet_name) ++ twox128("Difficulty")`.
        pub(crate) fn old_difficulty_key<T: Config>() -> [u8; 32] {
            let mut key = [0u8; 32];
            key[..16].copy_from_slice(&Twox128::hash(
                <Pallet<T> as PalletInfoAccess>::name().as_bytes(),
            ));
            key[16..].copy_from_slice(&Twox128::hash(b"Difficulty"));
            key
        }

        /// 2 → 3: carry the global difficulty into the per-topology map keyed
        /// by the default topology, whitelist the default, drop the old value.
        pub(crate) fn carry_forward<T: Config>() -> Weight {
            let key = old_difficulty_key::<T>();
            // The pre-v3 value is in the three-field layout; decoding it as
            // today's single-field `DifficultyConfig` would read
            // `min_solutions` as the leading bytes of `max_energy_milli`.
            let old: types::DifficultyConfig =
                frame_support::storage::unhashed::get::<super::LegacyDifficultyConfig>(&key)
                    .map(Into::into)
                    .unwrap_or_default();
            let reads = 2u64; // DefaultTopology + old Difficulty
            let mut writes = 0u64;
            if let Some(default_hash) = DefaultTopology::<T>::get() {
                Difficulties::<T>::insert(default_hash, old);
                MineableTopologies::<T>::insert(default_hash, ());
                writes = writes.saturating_add(2);
            }
            // Drop the old global value, and kill any transient pre-v3
            // `BlockBestProof`: `ProofRecord` gains `topology_hash` in v3, so a
            // stale entry is a different shape. It is always empty across an
            // upgrade boundary (`on_finalize` take()s it every block) and
            // `OptionQuery` decode failure reads as `None`, but `kill()`
            // removes all doubt for free.
            frame_support::storage::unhashed::kill(&key);
            BlockBestProof::<T>::kill();
            writes = writes.saturating_add(2);
            T::DbWeight::get().reads_writes(reads, writes)
        }

        /// `< 2`: clear the whole pallet prefix (legacy v0.2 encodings).
        pub(crate) fn wipe<T: Config>() -> Weight {
            let pallet_prefix = Twox128::hash(<Pallet<T> as PalletInfoAccess>::name().as_bytes());
            let cleared =
                frame_support::storage::unhashed::clear_prefix(&pallet_prefix, None, None).backend;
            T::DbWeight::get().reads_writes(1, u64::from(cleared))
        }
    }

    pub(crate) mod v6 {
        use super::LegacyDifficultyConfig;
        use crate::pallet::{
            Config, Difficulties, QBlocks, RegisteredTopologies, TopologyDim, TopologyDims,
        };
        use crate::{
            types, AccountIdOf, BalanceOf, BlockNumberOf, EdgesOf, NodesOf, TopologyMetaOf,
        };
        use codec::Decode;
        use frame_support::traits::Get;
        use frame_support::weights::Weight;
        use sp_core::H256;

        /// The v5 `QBlock` layout: three-field `difficulty`, and no record of
        /// which instance was solved.
        #[derive(Decode)]
        struct V5QBlock<AccountId, Balance, BlockNumber> {
            miner: AccountId,
            salt: [u8; 32],
            energy_milli: i64,
            reward: Balance,
            submitted_at: BlockNumber,
            difficulty: LegacyDifficultyConfig,
            last_proof_block_hash: H256,
            topology_hash: H256,
            device_access_time_us: u64,
        }

        /// 5 → 6: `DifficultyConfig` loses `min_solutions` and
        /// `min_diversity_milli`, and `QBlock` gains the instance the proof
        /// solved. Both the `Difficulties` map and every `QBlocks` entry
        /// embed the struct, so both are re-encoded.
        ///
        /// Historical blocks predate the per-instance handicap, so their bar
        /// *was* the topology-level `max_energy_milli`: backfilling
        /// `instance_bar_milli` with it is exact, not a placeholder.
        /// `frustration_milli` was never measured then, so it backfills to `0`.
        pub(crate) fn shrink_difficulty<T: Config>() -> Weight {
            let difficulties = shrink_map::<T>();

            let mut qblocks = 0u64;
            QBlocks::<T>::translate::<V5QBlock<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>, _>(
                |_block, old| {
                    qblocks = qblocks.saturating_add(1);
                    Some(types::QBlock {
                        miner: old.miner,
                        salt: old.salt,
                        energy_milli: old.energy_milli,
                        reward: old.reward,
                        submitted_at: old.submitted_at,
                        instance_bar_milli: old.difficulty.max_energy_milli,
                        difficulty: old.difficulty.into(),
                        last_proof_block_hash: old.last_proof_block_hash,
                        topology_hash: old.topology_hash,
                        device_access_time_us: old.device_access_time_us,
                        frustration_milli: 0,
                    })
                },
            );

            // `ProofRecord` gained trailing fields — same reasoning as v3.
            crate::BlockBestProof::<T>::kill();
            let touched = difficulties.saturating_add(qblocks);
            T::DbWeight::get().reads_writes(touched, touched.saturating_add(1))
        }

        /// Re-encode `Difficulties` alone. Chains arriving from below v5 have
        /// already had their `QBlocks` written in the final shape by the v5
        /// step, so only the map is left in the legacy layout — re-translating
        /// `QBlocks` there would fail to decode and silently drop every entry.
        pub(crate) fn shrink_difficulties_only<T: Config>() -> Weight {
            let difficulties = shrink_map::<T>();
            crate::BlockBestProof::<T>::kill();
            T::DbWeight::get().reads_writes(difficulties, difficulties.saturating_add(1))
        }

        /// Nothing of the pre-v6 shape survives (the v3 step wiped or already
        /// rewrote it), so only the transient best-proof needs clearing.
        pub(crate) fn kill_stale_best_proof<T: Config>() -> Weight {
            crate::BlockBestProof::<T>::kill();
            T::DbWeight::get().writes(1)
        }

        fn shrink_map<T: Config>() -> u64 {
            let mut count = 0u64;
            Difficulties::<T>::translate::<LegacyDifficultyConfig, _>(|_hash, old| {
                count = count.saturating_add(1);
                Some(old.into())
            });
            count
        }

        /// Backfill [`TopologyDims`].
        ///
        /// The dims are derivable from `RegisteredTopologies`; the copy exists
        /// because that derivation costs a ~370 KB SCALE decode and the weight
        /// closures needing it run inside `validate_transaction`, unpaid, on
        /// every gossiped extrinsic.
        ///
        /// Uses `iter()`, which yields `(key, value)` and decodes each meta
        /// ONCE — `iter_keys()` plus a per-key `get()` costs two storage
        /// accesses per entry plus a `Vec` of every key. Writes to a DIFFERENT
        /// map than the one iterated, so mutation-during-iteration is not a
        /// concern. Idempotent.
        pub(crate) fn backfill_topology_dims<T: Config>() -> Weight {
            let mut touched = 0u64;
            let mut visited = 0u64;
            for (hash, meta) in RegisteredTopologies::<T>::iter() {
                visited = visited.saturating_add(1);
                if TopologyDims::<T>::contains_key(hash) {
                    continue;
                }
                touched = touched.saturating_add(1);
                TopologyDims::<T>::insert(
                    hash,
                    TopologyDim {
                        nodes: meta.nodes.len() as u32,
                        edges: meta.edges.len() as u32,
                    },
                );
            }
            // Charged on VISITED, not touched. Every entry costs two reads
            // (`iter()`'s key+value and the `contains_key` probe) whether or not
            // it needs backfilling, and this runs unconditionally from every
            // prior version. Charging `touched` returns `Weight::zero()` on the
            // idempotent re-run after doing all that work — the wrong direction
            // on a consensus-visible weight.
            T::DbWeight::get().reads_writes(visited.saturating_mul(2), touched)
        }

        /// Canonicalize the stored graph of every registered topology.
        ///
        /// `hash_topology` sorts nodes and orients-then-sorts edges before
        /// hashing, but `generate_ising_model` maps `h[i]` to `nodes[i]` and
        /// `j[k]` to `edges[k]` positionally. Registration now stores the
        /// canonical order so the two agree; until an existing entry is brought
        /// into line its hash does not name the instance the chain generates.
        ///
        /// Hashes are unchanged — they were always computed over the canonical
        /// form — so every keyed map stays valid, and nothing stores node or
        /// edge indices. The one live consequence is `prove_topology_planar`'s
        /// rotation, which indexes the stored edge list: a witness generator
        /// computing against submission order must be regenerated.
        pub(crate) fn canonicalize_topologies<T: Config>() -> Weight {
            let mut touched = 0u64;
            RegisteredTopologies::<T>::translate::<TopologyMetaOf<T>, _>(|_hash, mut meta| {
                touched = touched.saturating_add(1);
                let (nodes, edges) = quantum_validation::canonical_graph(&meta.nodes, &meta.edges);
                // Bound violations are impossible: `canonical_graph` sorts and
                // orients, so both outputs are permutations of values already
                // stored under the same bounds, and it is count-preserving —
                // which the `TopologyDims` backfill also relies on.
                //
                // SAY SO LOUDLY IF IT EVER IS NOT. An `if let Ok` with no
                // `else` would, on failure, write back the UN-canonicalized
                // meta and report success — precisely the hash/instance
                // divergence this migration exists to REPAIR, now silent and
                // permanent.
                match (NodesOf::<T>::try_from(nodes), EdgesOf::<T>::try_from(edges)) {
                    (Ok(n), Ok(e)) => {
                        meta.nodes = n;
                        meta.edges = e;
                    }
                    _ => {
                        log::error!(
                            target: "runtime::quantum-pow",
                            "v6: canonicalizing a stored topology overflowed \
                             MaxNodes/MaxEdges, which cannot happen for a \
                             permutation of already-stored values. The topology \
                             is left UN-canonicalized, so its hash no longer \
                             names the instance the chain generates.",
                        );
                        frame_support::defensive!("v6 canonicalization exceeded the stored bounds");
                    }
                }
                Some(meta)
            });
            T::DbWeight::get().reads_writes(touched, touched)
        }
    }

    pub(crate) mod v5 {
        use crate::pallet::{Config, QBlocks};
        use crate::{types, AccountIdOf, BalanceOf, BlockNumberOf, DefaultTopology};
        use codec::Decode;
        use frame_support::traits::Get;
        use frame_support::weights::Weight;
        use sp_core::H256;

        /// The pre-v4 (pre-topology) `QBlock` layout: [`types::QBlock`] minus
        /// the trailing `topology_hash` and `device_access_time_us`. Kept so
        /// entries from chains that never ran the v4 step can be re-encoded.
        #[derive(Decode)]
        struct PreTopologyQBlock<AccountId, Balance, BlockNumber> {
            miner: AccountId,
            salt: [u8; 32],
            energy_milli: i64,
            reward: Balance,
            submitted_at: BlockNumber,
            difficulty: super::LegacyDifficultyConfig,
            last_proof_block_hash: H256,
        }

        /// The v4 `QBlock` layout as SHIPPED in spec 111 (the deployed
        /// chain): carries `topology_hash` but not `device_access_time_us`.
        #[derive(Decode)]
        struct V4QBlock<AccountId, Balance, BlockNumber> {
            miner: AccountId,
            salt: [u8; 32],
            energy_milli: i64,
            reward: Balance,
            submitted_at: BlockNumber,
            difficulty: super::LegacyDifficultyConfig,
            last_proof_block_hash: H256,
            topology_hash: H256,
        }

        /// Kill any stale `BlockBestProof`: `ProofRecord` gained a trailing
        /// field. Same reasoning as the v3 step.
        fn kill_stale_best_proof<T: Config>() {
            crate::BlockBestProof::<T>::kill();
        }

        /// `< 4` → 5: re-encode every `QBlocks` entry from the 7-field
        /// pre-topology layout, backfilling `topology_hash` with the default
        /// topology (`H256::zero()` when none is set — blocks won before
        /// per-topology binding were all mined against the default, so this
        /// is the historically-correct value) and `device_access_time_us`
        /// with 0 (never reported before spec 112).
        pub(crate) fn backfill_from_pre_topology<T: Config>() -> Weight {
            let backfill = DefaultTopology::<T>::get().unwrap_or_default();
            let mut count = 0u64;
            QBlocks::<T>::translate::<
                PreTopologyQBlock<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>,
                _,
            >(|_block, old| {
                count = count.saturating_add(1);
                Some(types::QBlock {
                    miner: old.miner,
                    salt: old.salt,
                    energy_milli: old.energy_milli,
                    reward: old.reward,
                    submitted_at: old.submitted_at,
                    instance_bar_milli: old.difficulty.max_energy_milli,
                    difficulty: old.difficulty.into(),
                    last_proof_block_hash: old.last_proof_block_hash,
                    topology_hash: backfill,
                    device_access_time_us: 0,
                    frustration_milli: 0,
                })
            });
            kill_stale_best_proof::<T>();
            // One read + one write per entry, plus the `DefaultTopology`
            // read and the `BlockBestProof` kill.
            T::DbWeight::get().reads_writes(count.saturating_add(1), count.saturating_add(1))
        }

        /// 4 → 5: re-encode every `QBlocks` entry from the shipped v4 layout,
        /// appending `device_access_time_us = 0` and PRESERVING the stored
        /// `topology_hash` — post-binding blocks may record non-default
        /// topologies, so no backfill here.
        pub(crate) fn append_device_time<T: Config>() -> Weight {
            let mut count = 0u64;
            QBlocks::<T>::translate::<V4QBlock<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>, _>(
                |_block, old| {
                    count = count.saturating_add(1);
                    Some(types::QBlock {
                        miner: old.miner,
                        salt: old.salt,
                        energy_milli: old.energy_milli,
                        reward: old.reward,
                        submitted_at: old.submitted_at,
                        instance_bar_milli: old.difficulty.max_energy_milli,
                        difficulty: old.difficulty.into(),
                        last_proof_block_hash: old.last_proof_block_hash,
                        topology_hash: old.topology_hash,
                        device_access_time_us: 0,
                        frustration_milli: 0,
                    })
                },
            );
            kill_stale_best_proof::<T>();
            // One read + one write per entry, plus the `BlockBestProof` kill.
            T::DbWeight::get().reads_writes(count, count.saturating_add(1))
        }
    }
}

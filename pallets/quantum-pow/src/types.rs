use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use quantum_validation::AllowedValueSpec;
use scale_info::TypeInfo;
use sp_core::{H256, U256};

/// Structural facts about a topology.
///
/// Every field is a function of the interaction graph and its value specs alone
/// — identical for every nonce — so no salt can move any of them. Computed
/// off-chain and registered by governance; `residual_difficulty` is also
/// falsifiable on-chain via `Pallet::prove_topology_exact`.
#[derive(
    Clone,
    Copy,
    Debug,
    Encode,
    Decode,
    DecodeWithMemTracking,
    Eq,
    PartialEq,
    TypeInfo,
    MaxEncodedLen,
)]
pub struct TopologyHardness {
    /// A chain-verified **upper bound** on the induced width of the topology's
    /// **raw interaction graph** — what an elimination-order witness is
    /// replayed against, and the exact-solve exponent: width `w` costs `2^w`.
    ///
    /// Deliberately *separate* from `core_width`. Conflating them — the toolkit
    /// reports post-reduction core width, the challenge replays an order over
    /// the raw graph — left a wide raw graph with a collapsing reduction stack
    /// unchallengeable, the case governance is most likely to misjudge.
    ///
    /// Not merely trusted: it only ratchets *down*, anyone can drive it there
    /// by exhibiting a narrower elimination order (`prove_topology_exact`), and
    /// not even root may raise it. The regime is derived from it and
    /// `ExactSolveCeiling` rather than stored, so one constant change
    /// re-classifies every topology as classical solvers improve.
    pub residual_difficulty: u32,
    /// Induced width of the **irreducible core**, after the toolkit's exact
    /// reduction stack (tropical spin elimination, Δ-Y/SP, roof-duality
    /// QPBO, virtualization).
    ///
    /// This is what actually decides tractability, and it is *not*
    /// `residual_difficulty`: a graph can be wide while the stack collapses it
    /// to a trivial core, and solving the core solves the original. So the
    /// regime takes the **minimum** of the two — either cheap route makes the
    /// topology cheap, the conservative direction for a mineability gate.
    ///
    /// Governance-trusted, unlike `residual_difficulty`: no witness for "the
    /// reduction stack collapses this" is as cheap as an elimination order.
    /// Falsifying it needs a replayable reduction certificate — the remaining
    /// half of `riff-hv9.16`.
    ///
    /// **Zero is irreversible.** Widths only ratchet down and not even root may
    /// raise one, so registering `0` — or a planarity proof collapsing it —
    /// retires the topology permanently. Intended for a proven tractable graph,
    /// but a fat-fingered `0` then needs re-registration under a new hash.
    pub core_width: u32,
    /// Frustration, in milli, an honestly-sampled instance is expected to show:
    /// where the mean-field curve is calibrated and the reference of the
    /// per-instance bar (`crate::difficulty::instance_bar_milli`). `500` for a
    /// symmetric ±J spec on a graph with cycles; `0` disables the handicap and
    /// falls back to the bare curve.
    pub expected_frustration_milli: u32,
}

/// A submitted proof-of-work payload.
///
/// The proof carries only the strictly-non-derivable inputs to validation:
///
/// - `topology_hash` identifies the registered puzzle definition. The
///   pallet looks up nodes, edges, and the allowed value sets from
///   `RegisteredTopologies`.
/// - `nonce` is the full 256-bit BLAKE3 digest of
///   `(last_proof_block_hash, miner, salt)`, where `last_proof_block_hash =
///   block_hash(LastProofBlock)` is the header hash of the most recent
///   winning block (stable across an entire round). The verifier
///   re-derives it for free; carrying it in the proof lets `submit_proof`
///   reject mismatched salts before doing any topology work.
/// - `salt` is the only freely-chosen miner input. Fixed at 32 bytes so the
///   PoW search space is statically known and identical across every call.
/// - `solutions` is one bit-packed spin vector, decoded under the registered
///   topology's `allowed_spin_values` spec, so the wire-format width per spin
///   matches the on-chain spec (e.g., 1 bit per spin for the default binary
///   Ising topology).
#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct QuantumProof<PackedSolution> {
    pub topology_hash: H256,
    pub nonce: U256,
    pub salt: [u8; 32],
    /// The submitted configuration — a single packed value, not a collection.
    ///
    /// It was `BoundedVec<PackedSpinBytesOf<T>, MaxSolutions>` with the extrinsic refusing
    /// `len() != 1`. That refusal fired only after SCALE had decoded up to `MaxSolutions`
    /// (32) configurations, on the unpaid pool-validation path, so the weight formula had to
    /// carry solution-scaled terms purely to price rejected work. Encoding one configuration
    /// makes `TooManySolutions` unrepresentable and removes the decode instead of charging
    /// for it. The name stays plural only to keep this change to the type: SCALE encodes
    /// structs positionally, so renaming to `solution` would cost no wire bytes — it would
    /// move only the metadata hash, which this type change already moves.
    pub solutions: PackedSolution,
    /// Miner-reported compute time spent producing this proof, in
    /// microseconds. QPU miners report the summed D-Wave QPU access time
    /// across the solution's attempts; CPU/GPU miners report wall-clock
    /// mining time. Self-reported observability (same trust model as
    /// `MinerRegistry.participate`'s `budget_seconds`) — the chain cannot
    /// verify it and consensus never reads it. `0` = unreported.
    pub device_access_time_us: u64,
}

/// The topology-level difficulty baseline: one dial, the energy bar.
///
/// `min_solutions` and `min_diversity_milli` are gone: they charged the
/// verifier an exact energy evaluation per solution — the dominant per-proof
/// cost — for a property that is not difficulty, since a miner near a good
/// basin emits distant near-ties as cheaply as one. One configuration and one
/// bar leaves a single monotone control the epoch retarget can drive.
#[derive(
    Clone,
    Copy,
    Debug,
    Encode,
    Decode,
    DecodeWithMemTracking,
    Eq,
    PartialEq,
    TypeInfo,
    MaxEncodedLen,
)]
pub struct DifficultyConfig {
    /// Topology-level bar before the per-instance handicap (see
    /// `crate::difficulty::instance_bar_milli`). Negative.
    pub max_energy_milli: i64,
}

impl Default for DifficultyConfig {
    fn default() -> Self {
        Self {
            max_energy_milli: -1_200_000,
        }
    }
}

/// On-chain record of a registered topology.
///
/// A `topology_hash` uniquely identifies the full puzzle definition: graph
/// structure plus the allowed h, j, and spin value sets. The hash is computed
/// by [`crate::topology::hash_topology`] over all five inputs so two
/// topologies that differ only in their allowed value sets get distinct hashes.
#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct TopologyMeta<Nodes, Edges, AllowedValues, BlockNumber> {
    pub nodes: Nodes,
    pub edges: Edges,
    /// How the nonce-seeded RNG selects per-node h field values.
    pub allowed_h_values: AllowedValueSpec<AllowedValues>,
    /// How the nonce-seeded RNG selects per-edge j coupling values.
    /// Replaces the historical hardcoded `±MILLI_SCALE` magnitude.
    pub allowed_j_values: AllowedValueSpec<AllowedValues>,
    /// Which milli values a spin in a submitted solution may take. The
    /// variant also implies the bit-width used to encode each spin in the
    /// `QuantumProof::solutions` payload.
    pub allowed_spin_values: AllowedValueSpec<AllowedValues>,
    pub registered_at: BlockNumber,
}

#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct MinerInfo<Balance, BlockNumber> {
    pub registered_at: BlockNumber,
    pub deposit: Balance,
    pub proofs_submitted: u32,
    /// Cached win count for cheap miner stats.
    ///
    /// This is not required for protocol correctness and can be dropped later
    /// in favor of deriving the same view from emitted reward/winner events.
    pub proofs_won: u32,
    pub rewards_earned: Balance,
}

#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct ProofRecord<AccountId, BlockNumber> {
    pub miner: AccountId,
    pub submitted_at: BlockNumber,
    /// Exact energy of the proof's single submitted configuration.
    pub energy_milli: i64,
    /// A LOWER BOUND on the instance's optimum, `-(Σ|h| + Σ|J|)`, in milli.
    /// Exact only for an unfrustrated draw — frustration lifts the true optimum
    /// above it, so the normalized room over-estimates reachable depth.
    ///
    /// Fields here need no storage migration only because `BlockBestProof` is
    /// `take()`n unconditionally at the top of `on_finalize`: a record never
    /// survives a block boundary, so it cannot be decoded under an older
    /// layout.
    pub anchor_milli: i64,
    /// Salt of the submitted proof. Copied here so `on_finalize` can persist
    /// it into `QBlocks` without re-reading the (PQ-signed)
    /// extrinsic body.
    pub salt: [u8; 32],
    /// Topology the winning proof was mined against. `on_finalize` adjusts
    /// the difficulty entry for *this* topology only — never another's.
    pub topology_hash: H256,
    /// Miner-reported compute time from the accepted proof, in microseconds
    /// (see `QuantumProof::device_access_time_us`). Carried here so
    /// `on_finalize` can persist it into `QBlocks` without re-reading the
    /// extrinsic body.
    pub device_access_time_us: u64,
    /// The live difficulty this proof was priced against, as `submit_proof`
    /// read it.
    ///
    /// Do not recompute this at finalize with `current_difficulty_for`: that
    /// re-reads two storages and rebuilds the energy curve (a full topology
    /// decode) for a value the accepting extrinsic already had in the same
    /// block, and it would pick up any later `Difficulties` write, recording a
    /// threshold the miner was never held to.
    pub difficulty: DifficultyConfig,
    /// Gauge-invariant frustration of the instance this proof solved, in
    /// milli. Carried so `on_finalize` can persist the bar's provenance
    /// without regenerating the instance.
    pub frustration_milli: u32,
    /// The handicapped bar this proof actually cleared, in milli.
    pub instance_bar_milli: i64,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Encode,
    Decode,
    DecodeWithMemTracking,
    Eq,
    PartialEq,
    TypeInfo,
    MaxEncodedLen,
)]
pub struct ProofValidation {
    /// Exact energy of the proof's single submitted configuration.
    pub energy_milli: i64,
}

#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct MiningSnapshot<Nodes, Edges, AllowedValues> {
    /// `block_hash(LastProofBlock)` — the header hash of the most recent
    /// winning block. The only "time" input the miner needs: it's stable
    /// for the whole round and feeds straight into `derive_nonce`. Both
    /// `block_number` and `parent_hash` were dropped from this snapshot
    /// because each existed only to feed the old block-number-bound nonce
    /// derivation; the new contract has neither in its input set.
    pub last_proof_block_hash: H256,
    pub difficulty: DifficultyConfig,
    pub topology_hash: H256,
    pub nodes: Nodes,
    pub edges: Edges,
    /// Same value sets as the registered topology; miners need these to know
    /// what h/j the verifier will reconstruct and what spin encoding to use.
    pub allowed_h_values: AllowedValueSpec<AllowedValues>,
    pub allowed_j_values: AllowedValueSpec<AllowedValues>,
    pub allowed_spin_values: AllowedValueSpec<AllowedValues>,
}

/// Persisted record of a qblock — a chain block won by a quantum PoW proof
/// (formerly "winning solution" / "solution #N"), written in `on_finalize`
/// alongside the `BlockWinner` event. The nonce is not stored directly —
/// consumers derive it from `(last_proof_block_hash, miner, salt)`, or call
/// the `winning_solution` runtime API which does it server-side. (The
/// chain-facing API name keeps the legacy term until the API-rename ticket
/// lands.)
///
/// `last_proof_block_hash` is the value the proof actually used at submission
/// time (i.e. `block_hash(previous qblock)`). Storing it makes
/// `qblock_with_nonce` self-contained — no `frame_system::block_hash`
/// lookup is needed at re-derivation time, and re-derivation stays correct
/// even after the original block is pruned beyond `BlockHashCount`.
///
/// `difficulty` captures the *active* threshold the proof actually had to
/// clear (decay applied on read). The next block's threshold is the stored
/// baseline — possibly retargeted at the epoch boundary — plus decay on
/// the next read. That value is *not* duplicated here.
#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct QBlock<AccountId, Balance, BlockNumber> {
    pub miner: AccountId,
    pub salt: [u8; 32],
    pub energy_milli: i64,
    pub reward: Balance,
    pub submitted_at: BlockNumber,
    pub difficulty: DifficultyConfig,
    pub last_proof_block_hash: H256,
    /// Topology the winning proof was mined against, copied from the
    /// accepted [`ProofRecord`]. Persisting it makes each block's
    /// topology provenance queryable from state rather than only from the
    /// (prunable) `DifficultyUpdated` event, and keeps `difficulty` above
    /// interpretable against the curve it was computed from.
    pub topology_hash: H256,
    /// Miner-reported compute time spent producing the winning proof, in
    /// microseconds — QPU access time for QPU wins, wall clock for CPU/GPU
    /// wins. Copied from the accepted [`ProofRecord`]. Self-reported and
    /// unverifiable; `0` = unreported (including all pre-111 blocks, which
    /// the v4 migration backfills with 0). Wall-clock mining duration
    /// remains derivable from block spacing (`LastProofBlock` deltas), so
    /// no information is lost by carrying compute time here instead.
    pub device_access_time_us: u64,
    /// Frustration of the instance that was solved, in milli. With
    /// `difficulty` above and the topology's registered
    /// `expected_frustration_milli`, this makes the bar the block actually
    /// had to clear reproducible from state alone.
    pub frustration_milli: u32,
    /// The handicapped bar the winning proof cleared, in milli.
    pub instance_bar_milli: i64,
}

/// Runtime-API view augmenting [`QBlock`] with the derived nonce.
/// Saves consumers from running BLAKE3 client-side. The `solution` field
/// name is part of the decoded runtime-API shape — it keeps the legacy
/// name until the API-rename ticket lands.
#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct QBlockWithNonce<AccountId, Balance, BlockNumber> {
    pub solution: QBlock<AccountId, Balance, BlockNumber>,
    pub nonce: U256,
}

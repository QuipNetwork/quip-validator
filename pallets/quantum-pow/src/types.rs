use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use quantum_validation::AllowedValueSpec;
use scale_info::TypeInfo;
use sp_core::{H256, U256};

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
/// - `solutions` is a list of bit-packed spin vectors. Each entry is decoded
///   under the registered topology's `allowed_spin_values` spec, so the
///   wire-format width per spin matches the on-chain spec (e.g., 1 bit per
///   spin for the default binary Ising topology).
#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub struct QuantumProof<PackedSolutions> {
    pub topology_hash: H256,
    pub nonce: U256,
    pub salt: [u8; 32],
    pub solutions: PackedSolutions,
    /// Miner-reported compute time spent producing this proof, in
    /// microseconds. QPU miners report the summed D-Wave QPU access time
    /// across the solution's attempts; CPU/GPU miners report wall-clock
    /// mining time. Self-reported observability (same trust model as
    /// `MinerRegistry.participate`'s `budget_seconds`) — the chain cannot
    /// verify it and consensus never reads it. `0` = unreported.
    pub device_access_time_us: u64,
}

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
    pub min_solutions: u32,
    pub max_energy_milli: i64,
    pub min_diversity_milli: u32,
}

impl Default for DifficultyConfig {
    fn default() -> Self {
        Self {
            min_solutions: 5,
            max_energy_milli: -1_200_000,
            min_diversity_milli: 200,
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
    /// Best energy found within the submitted proof.
    ///
    /// The doc uses `energy_milli`; this field keeps that meaning while making
    /// the "best among submitted solutions" interpretation explicit in the
    /// record documentation rather than in the field name.
    pub energy_milli: i64,
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
    pub best_energy_milli: i64,
    pub diversity_milli: u32,
    pub valid_solution_count: u32,
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
/// clear (i.e. decay applied, but before the post-win adjustment). The next
/// block's threshold is whatever `Difficulty<T>` storage now holds, which is
/// `adjust_on_proof(difficulty, ...)` — that value is *not* duplicated here.
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

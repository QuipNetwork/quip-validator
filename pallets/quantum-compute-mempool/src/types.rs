use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::pallet_prelude::BoundedVec;
use scale_info::TypeInfo;

/// Maximum number of solvers that can share one settled reward.
pub const MAX_REWARD_WINNERS: u32 = 32;

/// Maximum number of orders `on_initialize` finalises in one block. Orders
/// beyond this bound carry over to the following blocks in bucket order.
pub const MAX_FINALISED_ORDERS_PER_BLOCK: u32 = 8;

/// Registered problem families. v0 supports Ising only.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum Formulation {
    /// User-submitted Ising problem.
    Ising,
}

/// Registered solver hardware families.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum MinerType {
    Cpu,
    Gpu,
    QpuDwave,
    QpuIbm,
    QpuIonq,
    QpuPasqal,
    Asic,
}

/// Reward distribution strategy for a job order.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum RewardResolution {
    /// Winner-take-all.
    SingleBest,
    /// Split proportionally among the top N solvers.
    TopNWeighted { n: u32 },
    /// Split equally among the top N solvers.
    TopNEqual { n: u32 },
}

/// Order lifecycle status.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum OrderStatus {
    /// Accepting solutions.
    Opened,
    /// No longer accepts solutions.
    Expired,
    /// Fully settled.
    Closed,
}

/// How the proposer expects to consume final results.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub enum ResultDelivery {
    OnChainOnly,
    Callback {
        endpoint: BoundedVec<u8, frame_support::traits::ConstU32<256>>,
    },
    CallbackWithPoll {
        endpoint: BoundedVec<u8, frame_support::traits::ConstU32<256>>,
    },
}

/// Ising problem parameters and optional acceptance thresholds.
///
/// `max_energy_milli` is an energy ceiling: `submit_solution` drops every
/// vector whose energy is above it. Lower energy is better, so the ceiling
/// is a quality floor expressed on the energy axis, the same rule as
/// `DifficultyConfig::max_energy_milli` in `pallet-quantum-pow`.
/// `min_diversity_milli` and `min_solutions` are floors on the selected set.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct IsingParams<Nodes, Edges, Fields, Couplings> {
    pub nodes: Nodes,
    pub edges: Edges,
    pub h_values: Fields,
    pub j_values: Couplings,
    /// Energy ceiling in milli units. Vectors with energy above it are dropped.
    pub max_energy_milli: Option<i64>,
    pub min_diversity_milli: Option<u32>,
    pub min_solutions: Option<u32>,
}

/// Solver access policy for a job order.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub enum JobMode<Accounts, MinerTypes> {
    Open,
    Bid {
        miners: Option<Accounts>,
        miner_types: Option<MinerTypes>,
    },
}

/// Registered job specification template.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct JobSpec<AccountId, BlockNumber, Hash> {
    pub builder: AccountId,
    pub name: BoundedVec<u8, frame_support::traits::ConstU32<128>>,
    pub formulation: Formulation,
    pub validation_program: Option<Hash>,
    pub transform_program: Option<Hash>,
    pub registered_at: BlockNumber,
    pub total_orders: u64,
    pub successful_orders: u64,
}

/// Order timing parameters.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct OrderTiming<BlockNumber> {
    pub deadline_blocks: BlockNumber,
    pub block_wait: BlockNumber,
}

/// Specific job instance to be solved.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct JobOrder<AccountId, Balance, BlockNumber, Hash, Params, Mode> {
    pub spec_id: Hash,
    pub proposer: AccountId,
    pub ising_params: Params,
    pub reward: Balance,
    pub mode: Mode,
    pub resolution: RewardResolution,
    pub timing: OrderTiming<BlockNumber>,
    pub delivery: ResultDelivery,
    pub status: OrderStatus,
    pub created_at: BlockNumber,
    pub first_solution_at: Option<BlockNumber>,
    pub solution_count: u32,
}

/// Accepted solver submission for an order.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct JobSolution<AccountId, BlockNumber, Solutions> {
    pub solver: AccountId,
    pub solver_type: MinerType,
    pub solutions: Solutions,
    pub best_energy_milli: i64,
    pub diversity_milli: u32,
    pub num_valid: u32,
    pub submitted_at: BlockNumber,
}

/// Registered solver metadata.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct SolverInfo<AccountId, Balance, BlockNumber> {
    pub account: AccountId,
    pub solver_type: MinerType,
    pub registered_at: BlockNumber,
    pub solutions_submitted: u64,
    pub rewards_earned: Balance,
}

/// Simple ranked-solver entry used for Top-N order tracking.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct RankedSolver<AccountId> {
    pub solver: AccountId,
    pub energy_milli: i64,
}

/// SingleBest frontrunner state.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct FrontRunner<AccountId> {
    pub solver: AccountId,
    pub energy_milli: i64,
}

/// Summary of a settled winner used for off-chain result delivery.
///
/// Delivery always uses a homogeneous winners list. Single-winner settlement is
/// represented as a one-element list instead of a separate enum variant, which
/// keeps callback/event consumers on one shape across `SingleBest`,
/// `TopNEqual`, and `TopNWeighted`.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct WinnerSummary<AccountId, Balance> {
    pub solver: AccountId,
    pub energy_milli: i64,
    pub amount: Balance,
}

/// Persisted settlement payload for poll-based result retrieval.
///
/// This is written only for `CallbackWithPoll`, where off-chain consumers need
/// a canonical on-chain payload to fetch after the settlement event fires.
#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
pub struct StoredResult<AccountId, Balance, BlockNumber> {
    pub endpoint: BoundedVec<u8, frame_support::traits::ConstU32<256>>,
    pub resolution: RewardResolution,
    pub settled_at: BlockNumber,
    pub winners: BoundedVec<
        WinnerSummary<AccountId, Balance>,
        frame_support::traits::ConstU32<MAX_REWARD_WINNERS>,
    >,
}

/// Signed destination metadata only; this does not execute or promise payment.
/// V1 supports syntactically valid nonzero u64 chain IDs and nonzero H160 addresses.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum WinnerPayee {
    #[codec(index = 0)]
    V1 {
        chain_id: u64,
        address: sp_core::H160,
    },
}

impl WinnerPayee {
    pub fn chain_id(&self) -> u64 {
        match self {
            Self::V1 { chain_id, .. } => *chain_id,
        }
    }
    pub fn is_valid(&self) -> bool {
        match self {
            Self::V1 { chain_id, address } => *chain_id != 0 && !address.is_zero(),
        }
    }
}

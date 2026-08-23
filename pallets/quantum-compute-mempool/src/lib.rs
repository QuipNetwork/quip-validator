#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

mod delivery;
mod lifecycle;
pub mod rewards;
pub mod types;
pub mod weights;
pub mod xqvm;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub use weights::*;

use alloc::vec::Vec;
use core::marker::PhantomData;
use frame_support::traits::{Currency, Get};

type AccountIdOf<T> = <T as frame_system::Config>::AccountId;
type BlockNumberOf<T> = frame_system::pallet_prelude::BlockNumberFor<T>;
type BalanceOf<T> =
    <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

type NodesOf<T> = frame_support::pallet_prelude::BoundedVec<u32, <T as Config>::MaxNodes>;
type EdgesOf<T> = frame_support::pallet_prelude::BoundedVec<(u32, u32), <T as Config>::MaxEdges>;
type FieldsOf<T> = frame_support::pallet_prelude::BoundedVec<i32, <T as Config>::MaxNodes>;
type CouplingsOf<T> = frame_support::pallet_prelude::BoundedVec<i32, <T as Config>::MaxEdges>;
type MinerAccountsOf<T> =
    frame_support::pallet_prelude::BoundedVec<AccountIdOf<T>, <T as Config>::MaxBidMiners>;
type MinerTypesOf =
    frame_support::pallet_prelude::BoundedVec<types::MinerType, frame_support::traits::ConstU32<8>>;
type SolutionsOf<T> = frame_support::pallet_prelude::BoundedVec<
    frame_support::pallet_prelude::BoundedVec<i8, <T as Config>::MaxNodes>,
    <T as Config>::MaxSolutions,
>;
type ProposerOrdersOf<T> =
    frame_support::pallet_prelude::BoundedVec<u64, <T as Config>::MaxOrdersPerProposer>;
type TopSolversOf<T> = frame_support::pallet_prelude::BoundedVec<
    types::RankedSolver<AccountIdOf<T>>,
    frame_support::traits::ConstU32<{ types::MAX_REWARD_WINNERS }>,
>;
type IsingParamsOf<T> = types::IsingParams<NodesOf<T>, EdgesOf<T>, FieldsOf<T>, CouplingsOf<T>>;
type JobModeOf<T> = types::JobMode<MinerAccountsOf<T>, MinerTypesOf>;
type JobSpecOf<T> =
    types::JobSpec<AccountIdOf<T>, BlockNumberOf<T>, <T as frame_system::Config>::Hash>;
type JobOrderOf<T> = types::JobOrder<
    AccountIdOf<T>,
    BalanceOf<T>,
    BlockNumberOf<T>,
    <T as frame_system::Config>::Hash,
    IsingParamsOf<T>,
    JobModeOf<T>,
>;
type JobSolutionOf<T> = types::JobSolution<AccountIdOf<T>, BlockNumberOf<T>, SolutionsOf<T>>;
type SolverInfoOf<T> = types::SolverInfo<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>;
type FrontRunnerOf<T> = types::FrontRunner<AccountIdOf<T>>;
type WinnerSummariesOf<T> = frame_support::pallet_prelude::BoundedVec<
    types::WinnerSummary<AccountIdOf<T>, BalanceOf<T>>,
    frame_support::traits::ConstU32<{ types::MAX_REWARD_WINNERS }>,
>;
type StoredResultOf<T> = types::StoredResult<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>;

/// Canonical SDK-facing plain Ising job spec name.
///
/// The full canonical spec tuple is:
///
/// - `name = b"plain-ising-v1"`
/// - `formulation = Formulation::Ising`
/// - `validation_program = None`
/// - `transform_program = None`
///
/// `DefaultIsingSpecId` is the runtime hash of the SCALE-encoded tuple
/// `(name, formulation, validation_program, transform_program)`. If any
/// tuple field changes, update the pinned hash test and SDK docs together.
pub const DEFAULT_ISING_SPEC_NAME: &[u8] = b"plain-ising-v1";

// Compile-time check that `DEFAULT_ISING_SPEC_NAME` fits the bound used by
// `JobSpec::name`. Surfaces a build failure rather than a runtime panic if a
// future rename pushes the name past the cap.
const _: () = assert!(
    DEFAULT_ISING_SPEC_NAME.len() <= 128,
    "DEFAULT_ISING_SPEC_NAME must fit BoundedVec<u8, ConstU32<128>>",
);

/// Pallet-constant provider for the canonical plain Ising `spec_id`.
///
/// This is intentionally generic over the runtime because the concrete hash
/// type and hasher come from `frame_system::Config`.
pub struct CanonicalDefaultIsingSpecId<T>(PhantomData<T>);

impl<T: Config> Get<<T as frame_system::Config>::Hash> for CanonicalDefaultIsingSpecId<T> {
    fn get() -> <T as frame_system::Config>::Hash {
        Pallet::<T>::default_ising_spec_id()
    }
}

sp_api::decl_runtime_apis! {
    pub trait QuantumComputeMempoolApi<
        AccountId,
        Balance,
        BlockNumber,
        Hash,
        Nodes,
        Edges,
        Fields,
        Couplings,
        MinerAccounts,
        MinerTypes,
    >
    where
        AccountId: codec::Codec,
        Balance: codec::Codec,
        BlockNumber: codec::Codec,
        Hash: codec::Codec,
        Nodes: codec::Codec,
        Edges: codec::Codec,
        Fields: codec::Codec,
        Couplings: codec::Codec,
        MinerAccounts: codec::Codec,
        MinerTypes: codec::Codec,
    {
        /// Open order ids after `start_after`, capped by `limit`.
        ///
        /// The pallet maintains `OpenOrders` as the recovery index. This API
        /// additionally filters lazily expired orders against the current
        /// block without mutating state.
        fn open_order_ids(start_after: Option<u64>, limit: u32) -> Vec<u64>;

        /// Full order payload for a known order id.
        fn job_order(order_id: u64) -> Option<
            crate::types::JobOrder<
                AccountId,
                Balance,
                BlockNumber,
                Hash,
                crate::types::IsingParams<Nodes, Edges, Fields, Couplings>,
                crate::types::JobMode<MinerAccounts, MinerTypes>,
            >
        >;

        /// Stored poll result for a settled order, if retained.
        fn order_result(order_id: u64) -> Option<
            crate::types::StoredResult<AccountId, Balance, BlockNumber>
        >;

        /// Current ranked solver list for Top-N settlement modes. Empty for
        /// unknown orders and for SingleBest orders.
        fn order_top_solvers(order_id: u64) -> Vec<crate::types::RankedSolver<AccountId>>;
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use crate::xqvm::QuantumVm;
    use alloc::vec::Vec;
    use frame_support::{
        pallet_prelude::*,
        traits::{ExistenceRequirement::AllowDeath, ReservableCurrency, StorageVersion},
    };
    use frame_system::pallet_prelude::*;
    use quantum_validation::{
        calculate_diversity, energy_of_solution_indexed, select_diverse, TopologyIndex,
        ValidationError,
    };
    use sp_runtime::traits::{Hash as _, SaturatedConversion, Saturating, Zero};

    /// v2 drops all v0.2 mempool data on the network upgrade and reseeds the
    /// canonical default plain Ising spec from scratch.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(2);

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
        type MaxBidMiners: Get<u32>;
        #[pallet::constant]
        type MaxOrdersPerProposer: Get<u32>;
        #[pallet::constant]
        type MaxDeadlineBlocks: Get<BlockNumberFor<Self>>;
        #[pallet::constant]
        type MaxBlockWait: Get<BlockNumberFor<Self>>;
        #[pallet::constant]
        type MinReward: Get<BalanceOf<Self>>;

        /// Retention window for pollable settled results.
        ///
        /// v0 keeps `OrderResults` only for a bounded period so settled payloads
        /// do not grow unboundedly in state. After the TTL elapses, any caller
        /// may purge the stored result. Revisit this if the protocol later
        /// needs stronger archival or callback accountability guarantees.
        #[pallet::constant]
        type ResultTtlBlocks: Get<BlockNumberFor<Self>>;

        /// Canonical plain Ising job spec id exposed through metadata for SDKs.
        #[pallet::constant]
        type DefaultIsingSpecId: Get<Self::Hash>;

        /// Account recorded as the builder when the runtime migration backfills
        /// the canonical default spec on already-running chains.
        type DefaultJobSpecBuilder: Get<Self::AccountId>;

        type VM: xqvm::QuantumVm<Self::AccountId, BalanceOf<Self>, Self::Hash>;
        type WeightInfo: WeightInfo;
    }

    /// Genesis inputs for seeding team-controlled job specs.
    ///
    /// v1 only seeds the canonical default plain Ising spec. Additional
    /// blessed specs should be registered by root after genesis.
    #[pallet::genesis_config]
    #[derive(frame_support::DefaultNoBound)]
    pub struct GenesisConfig<T: Config> {
        /// Builder account recorded on the canonical default plain Ising spec.
        ///
        /// `None` keeps genesis empty, which is useful for tests that exercise
        /// registration or migration paths directly.
        pub default_ising_spec_builder: Option<T::AccountId>,
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            if let Some(builder) = &self.default_ising_spec_builder {
                if let Err(err) = Pallet::<T>::insert_default_ising_spec(builder.clone(), false) {
                    // Failing here means the chain spec is broken (e.g. another
                    // preset already inserted the canonical spec, or the wired
                    // VM rejects empty validation/transform programs). Genesis
                    // panics make the node refuse to start — surface enough
                    // context to make the misconfiguration debuggable.
                    panic!(
                        "quantum_compute_mempool genesis: failed to insert default \
                         plain Ising spec: {err:?}",
                    );
                }
            }
        }
    }

    #[pallet::storage]
    pub type JobSpecs<T: Config> = StorageMap<_, Blake2_128Concat, T::Hash, JobSpecOf<T>>;

    #[pallet::storage]
    pub type JobOrders<T: Config> = StorageMap<_, Blake2_128Concat, u64, JobOrderOf<T>>;

    #[pallet::storage]
    pub type NextOrderId<T: Config> = StorageValue<_, u64, ValueQuery>;

    /// Recovery index for orders that are still considered open by stored
    /// lifecycle state. Expired orders are removed lazily when the pallet next
    /// touches them; runtime APIs filter against the current block so callers
    /// do not have to treat this index as authoritative for time-based expiry.
    #[pallet::storage]
    pub type OpenOrders<T: Config> = StorageMap<_, Blake2_128Concat, u64, (), OptionQuery>;

    #[pallet::storage]
    pub type OrderSolutions<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u64,
        Blake2_128Concat,
        T::AccountId,
        JobSolutionOf<T>,
    >;

    #[pallet::storage]
    pub type OrderFrontRunner<T: Config> = StorageMap<_, Blake2_128Concat, u64, FrontRunnerOf<T>>;

    #[pallet::storage]
    pub type OrderTopSolvers<T: Config> =
        StorageMap<_, Blake2_128Concat, u64, TopSolversOf<T>, ValueQuery>;

    #[pallet::storage]
    pub type Solvers<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, SolverInfoOf<T>>;

    #[pallet::storage]
    pub type ProposerOrders<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ProposerOrdersOf<T>, ValueQuery>;

    #[pallet::storage]
    /// Persisted settlement payloads for `CallbackWithPoll`.
    ///
    /// These are intentionally short-lived and are cleaned up via `purge_result`
    /// after `ResultTtlBlocks` elapses.
    // TODO: Investigate whether stored results should also be cleaned up
    // automatically once the job has been finalized, instead of relying only on
    // TTL-based explicit purging.
    pub type OrderResults<T: Config> = StorageMap<_, Blake2_128Concat, u64, StoredResultOf<T>>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        SolverRegistered {
            who: T::AccountId,
            solver_type: types::MinerType,
        },
        SolverDeregistered {
            who: T::AccountId,
        },
        JobSpecRegistered {
            spec_id: T::Hash,
            builder: T::AccountId,
        },
        JobProposed {
            order_id: u64,
            spec_id: T::Hash,
            proposer: T::AccountId,
            reward: BalanceOf<T>,
            deadline_blocks: BlockNumberFor<T>,
            block_wait: BlockNumberFor<T>,
        },
        SolutionAccepted {
            order_id: u64,
            solver: T::AccountId,
            energy_milli: i64,
            diversity_milli: u32,
        },
        FirstSolutionReceived {
            order_id: u64,
            solver: T::AccountId,
            effective_expiry: BlockNumberFor<T>,
        },
        BlockWaitStarted {
            order_id: u64,
            first_solution_at: BlockNumberFor<T>,
            closes_at: BlockNumberFor<T>,
        },
        FrontRunnerChanged {
            order_id: u64,
            solver: T::AccountId,
            energy_milli: i64,
        },
        OrderExpired {
            order_id: u64,
        },
        RewardClaimed {
            order_id: u64,
            solver: T::AccountId,
            amount: BalanceOf<T>,
        },
        RewardReclaimed {
            order_id: u64,
            proposer: T::AccountId,
            amount: BalanceOf<T>,
        },
        OrderClosed {
            order_id: u64,
            successful: bool,
        },
        ResultPurged {
            order_id: u64,
        },
        /// Final delivery payload for off-chain consumers.
        ///
        /// The event always emits a winners list rather than a single-vs-multi
        /// enum. A single winner is encoded as `winners.len() == 1`, which
        /// keeps the external contract stable across all reward-resolution
        /// modes.
        ResultReady {
            order_id: u64,
            endpoint: BoundedVec<u8, ConstU32<256>>,
            winners: WinnerSummariesOf<T>,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        SolverAlreadyRegistered,
        SolverNotRegistered,
        JobSpecAlreadyExists,
        JobSpecNotFound,
        OrderNotFound,
        OrderNotOpen,
        NotEligibleSolver,
        InvalidSpinValues,
        SolutionLengthMismatch,
        InsufficientEnergy,
        InsufficientDiversity,
        InsufficientSolutions,
        NoSolutionsSubmitted,
        RewardTooLow,
        TooManySolutions,
        OrderLimitReached,
        InvalidDeliveryMode,
        InvalidTopology,
        InvalidRewardResolution,
        EmptyBidCriteria,
        DeadlineTooLong,
        BlockWaitTooLong,
        OrderNotExpired,
        NotProposer,
        NotWinner,
        AlreadyClaimed,
        NoSolutionsAccepted,
        ResultNotFound,
        ResultTtlNotElapsed,
        /// A positive diversity floor cannot be paired with an explicit
        /// solution minimum below two. `None` remains valid because it
        /// selects every valid submitted solution.
        InvalidDiversityConfig,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::register_solver())]
        pub fn register_solver(
            origin: OriginFor<T>,
            solver_type: types::MinerType,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                !Solvers::<T>::contains_key(&who),
                Error::<T>::SolverAlreadyRegistered
            );

            Solvers::<T>::insert(
                &who,
                SolverInfoOf::<T> {
                    account: who.clone(),
                    solver_type,
                    registered_at: frame_system::Pallet::<T>::block_number(),
                    solutions_submitted: 0,
                    rewards_earned: BalanceOf::<T>::default(),
                },
            );

            Self::deposit_event(Event::SolverRegistered { who, solver_type });
            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::deregister_solver())]
        pub fn deregister_solver(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                Solvers::<T>::contains_key(&who),
                Error::<T>::SolverNotRegistered
            );
            Solvers::<T>::remove(&who);
            Self::deposit_event(Event::SolverDeregistered { who });
            Ok(())
        }

        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::register_job_spec())]
        /// Register a team-controlled job spec.
        ///
        /// This is root-only because registered specs are intended to be
        /// blessed protocol/team templates. The builder is explicit because a
        /// sudo-dispatched call reaches this pallet as `Root`, not as the sudo
        /// account's signed origin.
        pub fn register_job_spec(
            origin: OriginFor<T>,
            builder: T::AccountId,
            name: BoundedVec<u8, ConstU32<128>>,
            formulation: types::Formulation,
            validation_program: Option<T::Hash>,
            transform_program: Option<T::Hash>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            Self::insert_job_spec(
                builder,
                name,
                formulation,
                validation_program,
                transform_program,
                frame_system::Pallet::<T>::block_number(),
                true,
            )?;
            Ok(())
        }

        #[pallet::call_index(3)]
        #[pallet::weight({
            let (bid_miners, bid_types) = Pallet::<T>::bid_dimensions(&mode);
            <T as Config>::WeightInfo::propose_job(
                ising_params.nodes.len() as u32,
                ising_params.edges.len() as u32,
                bid_miners,
                bid_types,
            )
        })]
        pub fn propose_job(
            origin: OriginFor<T>,
            spec_id: T::Hash,
            ising_params: IsingParamsOf<T>,
            reward: BalanceOf<T>,
            mode: JobModeOf<T>,
            resolution: types::RewardResolution,
            deadline_blocks: BlockNumberFor<T>,
            block_wait: BlockNumberFor<T>,
            delivery: types::ResultDelivery,
        ) -> DispatchResult {
            let proposer = ensure_signed(origin)?;
            ensure!(reward >= T::MinReward::get(), Error::<T>::RewardTooLow);
            ensure!(
                deadline_blocks <= T::MaxDeadlineBlocks::get(),
                Error::<T>::DeadlineTooLong
            );
            ensure!(
                block_wait <= T::MaxBlockWait::get(),
                Error::<T>::BlockWaitTooLong
            );
            ensure!(
                JobSpecs::<T>::contains_key(&spec_id),
                Error::<T>::JobSpecNotFound
            );

            if let Some(min_solutions) = ising_params.min_solutions {
                ensure!(
                    min_solutions <= T::MaxSolutions::get(),
                    Error::<T>::TooManySolutions
                );
            }
            if ising_params.min_diversity_milli.unwrap_or(0) > 0 {
                ensure!(
                    ising_params
                        .min_solutions
                        .is_none_or(|min_solutions| min_solutions >= 2),
                    Error::<T>::InvalidDiversityConfig
                );
            }

            ensure!(
                // The boolean form: the same single scan, short-circuiting on
                // the first defect. `propose_job` is SIGNED and flat-weighted,
                // so the reporting variant let one fee buy tens of thousands of
                // `String`s that this caller discards on its way to a single
                // `InvalidTopology`.
                quantum_validation::topology_consistency_ok(
                    ising_params.nodes.as_slice(),
                    ising_params.edges.as_slice(),
                    ising_params.h_values.as_slice(),
                    ising_params.j_values.as_slice(),
                    None,
                    None,
                ),
                Error::<T>::InvalidTopology
            );

            ensure!(
                delivery::validate_delivery_mode(&delivery, &mode),
                Error::<T>::InvalidDeliveryMode
            );
            Self::ensure_valid_mode(&mode)?;
            Self::ensure_valid_resolution(&resolution)?;

            let mut proposer_orders = ProposerOrders::<T>::get(&proposer);
            proposer_orders
                .try_push(NextOrderId::<T>::get())
                .map_err(|_| Error::<T>::OrderLimitReached)?;

            T::Currency::reserve(&proposer, reward)?;

            let order_id = NextOrderId::<T>::get();
            NextOrderId::<T>::put(order_id.saturating_add(1));

            let created_at = frame_system::Pallet::<T>::block_number();
            JobOrders::<T>::insert(
                order_id,
                JobOrderOf::<T> {
                    spec_id,
                    proposer: proposer.clone(),
                    ising_params,
                    reward,
                    mode,
                    resolution,
                    timing: types::OrderTiming {
                        deadline_blocks,
                        block_wait,
                    },
                    delivery,
                    status: types::OrderStatus::Opened,
                    created_at,
                    first_solution_at: None,
                    solution_count: 0,
                },
            );
            OpenOrders::<T>::insert(order_id, ());
            ProposerOrders::<T>::insert(&proposer, proposer_orders);
            JobSpecs::<T>::mutate(spec_id, |maybe_spec| {
                if let Some(spec) = maybe_spec {
                    spec.total_orders = spec.total_orders.saturating_add(1);
                }
            });

            Self::deposit_event(Event::JobProposed {
                order_id,
                spec_id,
                proposer,
                reward,
                deadline_blocks,
                block_wait,
            });
            Ok(())
        }

        #[pallet::call_index(4)]
        #[pallet::weight(
            // The stored topology and VM-transformed output dimensions are not
            // safely available from the encoded call before dispatch. Charge
            // the configured maxima; the benchmark still exposes all three
            // dimensions so a future call format or VM weight contract can
            // pass exact values without changing WeightInfo again.
            // TODO(benchmarking): Return DispatchResultWithPostInfo and refund
            // to the actual topology and transformed-solution dimensions once
            // they are known; calibrate that path on node02.
            <T as Config>::WeightInfo::submit_solution(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxSolutions::get(),
            )
        )]
        pub fn submit_solution(
            origin: OriginFor<T>,
            order_id: u64,
            solutions: SolutionsOf<T>,
        ) -> DispatchResult {
            let solver = ensure_signed(origin)?;
            let mut solver_info =
                Solvers::<T>::get(&solver).ok_or(Error::<T>::SolverNotRegistered)?;
            ensure!(!solutions.is_empty(), Error::<T>::NoSolutionsSubmitted);

            let mut order = JobOrders::<T>::get(order_id).ok_or(Error::<T>::OrderNotFound)?;
            let spec = JobSpecs::<T>::get(order.spec_id).ok_or(Error::<T>::JobSpecNotFound)?;
            Self::expire_order_if_needed(order_id, &mut order);
            ensure!(
                order.status == types::OrderStatus::Opened,
                Error::<T>::OrderNotOpen
            );

            ensure!(
                Self::solver_is_eligible(&solver, solver_info.solver_type, &order.mode),
                Error::<T>::NotEligibleSolver
            );

            let nodes = order.ising_params.nodes.as_slice();
            let edges = order.ising_params.edges.as_slice();
            let h_values = order.ising_params.h_values.as_slice();
            let j_values = order.ising_params.j_values.as_slice();
            let topology = TopologyIndex::new(nodes, edges).map_err(Self::map_validation_error)?;
            let transformed_solutions = T::VM::transform_solutions(
                &order.spec_id,
                spec.validation_program.as_ref(),
                spec.transform_program.as_ref(),
                &solver,
                solutions
                    .iter()
                    .map(|solution| solution.as_slice().to_vec())
                    .collect(),
            )?;
            ensure!(
                !transformed_solutions.is_empty(),
                Error::<T>::NoSolutionsSubmitted
            );
            ensure!(
                transformed_solutions.len() <= T::MaxSolutions::get() as usize,
                Error::<T>::TooManySolutions
            );

            let mut valid_solutions: Vec<Vec<i8>> = Vec::new();
            let mut best_energy = i64::MAX;

            for solution in &transformed_solutions {
                let energy = energy_of_solution_indexed(
                    solution.as_slice(),
                    h_values,
                    edges,
                    j_values,
                    &topology,
                )
                .map_err(Self::map_validation_error)?;
                if let Some(min_energy) = order.ising_params.min_energy_milli {
                    if energy > min_energy {
                        continue;
                    }
                }

                best_energy = best_energy.min(energy);
                valid_solutions.push(solution.as_slice().to_vec());
            }

            ensure!(!valid_solutions.is_empty(), Error::<T>::InsufficientEnergy);

            let target_count = order
                .ising_params
                .min_solutions
                .unwrap_or(valid_solutions.len() as u32)
                .min(valid_solutions.len() as u32) as usize;

            ensure!(
                valid_solutions.len() >= target_count.max(1),
                Error::<T>::InsufficientSolutions
            );

            let selected_indices = select_diverse(&valid_solutions, target_count)
                .map_err(Self::map_validation_error)?;
            let selected_solutions: Vec<Vec<i8>> = selected_indices
                .into_iter()
                .map(|index| valid_solutions[index].clone())
                .collect();

            if let Some(min_solutions) = order.ising_params.min_solutions {
                ensure!(
                    (selected_solutions.len() as u32) >= min_solutions,
                    Error::<T>::InsufficientSolutions
                );
            }

            let diversity_milli =
                calculate_diversity(&selected_solutions).map_err(Self::map_validation_error)?;
            if let Some(min_diversity) = order.ising_params.min_diversity_milli {
                ensure!(
                    diversity_milli >= min_diversity,
                    Error::<T>::InsufficientDiversity
                );
            }

            let bounded_solutions: SolutionsOf<T> = selected_solutions
                .iter()
                .map(|solution| {
                    BoundedVec::<i8, T::MaxNodes>::try_from(solution.clone())
                        .expect("validated against MaxNodes")
                })
                .collect::<Vec<_>>()
                .try_into()
                .expect("selected subset cannot exceed MaxSolutions");

            let is_new_solver = !OrderSolutions::<T>::contains_key(order_id, &solver);
            let now = frame_system::Pallet::<T>::block_number();
            OrderSolutions::<T>::insert(
                order_id,
                &solver,
                JobSolutionOf::<T> {
                    solver: solver.clone(),
                    solver_type: solver_info.solver_type,
                    solutions: bounded_solutions,
                    best_energy_milli: best_energy,
                    diversity_milli,
                    num_valid: selected_solutions.len() as u32,
                    submitted_at: now,
                },
            );

            if is_new_solver {
                order.solution_count = order.solution_count.saturating_add(1);
            }

            if order.first_solution_at.is_none() {
                order.first_solution_at = Some(now);
                let closes_at = lifecycle::effective_expiry(
                    order.created_at,
                    order.first_solution_at,
                    &order.timing,
                );
                Self::deposit_event(Event::FirstSolutionReceived {
                    order_id,
                    solver: solver.clone(),
                    effective_expiry: closes_at,
                });
                Self::deposit_event(Event::BlockWaitStarted {
                    order_id,
                    first_solution_at: now,
                    closes_at,
                });
            }

            Self::update_ranking(order_id, &order.resolution, &solver, best_energy)?;
            solver_info.solutions_submitted = solver_info.solutions_submitted.saturating_add(1);
            Solvers::<T>::insert(&solver, solver_info);
            JobOrders::<T>::insert(order_id, order);

            Self::deposit_event(Event::SolutionAccepted {
                order_id,
                solver,
                energy_milli: best_energy,
                diversity_milli,
            });
            Ok(())
        }

        #[pallet::call_index(5)]
        #[pallet::weight(<T as Config>::WeightInfo::claim_reward())]
        pub fn claim_reward(origin: OriginFor<T>, order_id: u64) -> DispatchResult {
            let caller = ensure_signed(origin)?;
            let mut order = JobOrders::<T>::get(order_id).ok_or(Error::<T>::OrderNotFound)?;
            let spec = JobSpecs::<T>::get(order.spec_id).ok_or(Error::<T>::JobSpecNotFound)?;
            Self::expire_order_if_needed(order_id, &mut order);
            ensure!(
                order.status != types::OrderStatus::Closed,
                Error::<T>::AlreadyClaimed
            );
            ensure!(
                order.status == types::OrderStatus::Expired,
                Error::<T>::OrderNotExpired
            );
            ensure!(order.solution_count > 0, Error::<T>::NoSolutionsAccepted);

            let payouts = Self::compute_payouts(order_id, &order)?;
            ensure!(
                payouts.iter().any(|(winner, _, _)| winner == &caller),
                Error::<T>::NotWinner
            );
            let winners = Self::winner_summaries_from_payouts(&payouts);
            T::VM::validate_result(
                &order.spec_id,
                spec.validation_program.as_ref(),
                spec.transform_program.as_ref(),
                winners.as_slice(),
            )?;

            let unreserved = T::Currency::unreserve(&order.proposer, order.reward);
            debug_assert!(unreserved.is_zero());

            for (winner, payout_u128, _) in &payouts {
                let payout: BalanceOf<T> = (*payout_u128).saturated_into();
                if payout.is_zero() {
                    continue;
                }
                T::Currency::transfer(&order.proposer, winner, payout, AllowDeath)?;
                Solvers::<T>::mutate(winner, |maybe_solver| {
                    if let Some(solver) = maybe_solver {
                        solver.rewards_earned = solver.rewards_earned.saturating_add(payout);
                    }
                });
                Self::deposit_event(Event::RewardClaimed {
                    order_id,
                    solver: winner.clone(),
                    amount: payout,
                });
            }

            order.status = types::OrderStatus::Closed;
            JobOrders::<T>::insert(order_id, &order);
            OpenOrders::<T>::remove(order_id);
            JobSpecs::<T>::mutate(order.spec_id, |maybe_spec| {
                if let Some(spec) = maybe_spec {
                    spec.successful_orders = spec.successful_orders.saturating_add(1);
                }
            });

            Self::persist_result_if_needed(order_id, &order, winners.clone());
            Self::emit_result_ready(order_id, &order.delivery, winners);
            Self::deposit_event(Event::OrderClosed {
                order_id,
                successful: true,
            });
            Ok(())
        }

        #[pallet::call_index(6)]
        #[pallet::weight(<T as Config>::WeightInfo::reclaim_order())]
        pub fn reclaim_order(origin: OriginFor<T>, order_id: u64) -> DispatchResult {
            let caller = ensure_signed(origin)?;
            let mut order = JobOrders::<T>::get(order_id).ok_or(Error::<T>::OrderNotFound)?;
            ensure!(order.proposer == caller, Error::<T>::NotProposer);
            Self::expire_order_if_needed(order_id, &mut order);
            ensure!(
                order.status != types::OrderStatus::Closed,
                Error::<T>::AlreadyClaimed
            );
            ensure!(
                order.status == types::OrderStatus::Expired,
                Error::<T>::OrderNotExpired
            );
            ensure!(order.solution_count == 0, Error::<T>::NoSolutionsAccepted);

            let unreserved = T::Currency::unreserve(&caller, order.reward);
            debug_assert!(unreserved.is_zero());

            order.status = types::OrderStatus::Closed;
            JobOrders::<T>::insert(order_id, &order);
            OpenOrders::<T>::remove(order_id);

            Self::deposit_event(Event::RewardReclaimed {
                order_id,
                proposer: caller,
                amount: order.reward,
            });
            Self::deposit_event(Event::OrderClosed {
                order_id,
                successful: false,
            });
            Ok(())
        }

        #[pallet::call_index(7)]
        #[pallet::weight(<T as Config>::WeightInfo::purge_result())]
        pub fn purge_result(origin: OriginFor<T>, order_id: u64) -> DispatchResult {
            let _caller = ensure_signed(origin)?;
            // TODO: Revisit whether result cleanup should stay permissionless or
            // be restricted to the proposer and/or another explicit role.
            let stored = OrderResults::<T>::get(order_id).ok_or(Error::<T>::ResultNotFound)?;
            let now = frame_system::Pallet::<T>::block_number();
            let purge_at = stored.settled_at.saturating_add(T::ResultTtlBlocks::get());
            ensure!(now >= purge_at, Error::<T>::ResultTtlNotElapsed);

            OrderResults::<T>::remove(order_id);
            Self::deposit_event(Event::ResultPurged { order_id });
            Ok(())
        }
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Network upgrade (v0.2 → v2): drop all mempool state instead of
        /// carrying it forward.
        ///
        /// The v0.2 layout (orders, solutions, solver/proposer records, job
        /// specs, counters) is intentionally not migrated. We clear the entire
        /// pallet prefix and then reseed the canonical default plain Ising spec
        /// exactly as genesis would on a fresh chain. The reseed is best-effort:
        /// a failure (e.g. a future VM that rejects empty programs) emits a
        /// defensive log and still bumps the version so this never retries every
        /// block; operators can backfill via root `register_job_spec`.
        ///
        /// The version guard makes this run exactly once: v0.2 chains sit at
        /// version 1, fresh chains are seeded at version 2 by genesis and skip.
        fn on_runtime_upgrade() -> Weight {
            use frame_support::{traits::PalletInfoAccess, StorageHasher};

            let on_chain = Pallet::<T>::on_chain_storage_version();
            if on_chain >= STORAGE_VERSION {
                return T::DbWeight::get().reads(1);
            }

            let pallet_prefix =
                frame_support::Twox128::hash(<Pallet<T> as PalletInfoAccess>::name().as_bytes());
            let cleared =
                frame_support::storage::unhashed::clear_prefix(&pallet_prefix, None, None).backend;

            if let Err(err) =
                Self::insert_default_ising_spec(T::DefaultJobSpecBuilder::get(), false)
            {
                frame_support::defensive!(
                    "on_runtime_upgrade: failed to reseed default Ising spec",
                    err
                );
            }

            STORAGE_VERSION.put::<Pallet<T>>();

            // 1 read (version) + 1 write per cleared key + reseed (~2 writes)
            // + 1 write (version).
            T::DbWeight::get().reads_writes(1, u64::from(cleared).saturating_add(3))
        }

        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            Ok(alloc::vec::Vec::new())
        }

        #[cfg(feature = "try-runtime")]
        fn post_upgrade(_state: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            ensure!(
                Pallet::<T>::on_chain_storage_version() >= STORAGE_VERSION,
                sp_runtime::TryRuntimeError::Other("storage version not bumped"),
            );
            let spec_id = T::DefaultIsingSpecId::get();
            let spec = JobSpecs::<T>::get(spec_id).ok_or(sp_runtime::TryRuntimeError::Other(
                "default Ising spec missing after network upgrade",
            ))?;
            ensure!(
                spec.builder == T::DefaultJobSpecBuilder::get(),
                sp_runtime::TryRuntimeError::Other("default Ising spec builder mismatch"),
            );
            ensure!(
                NextOrderId::<T>::get() == 0,
                sp_runtime::TryRuntimeError::Other("network upgrade must leave orders wiped"),
            );
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Return the bounded canonical default plain Ising spec name.
        ///
        /// This is the `name` component of the canonical tuple documented on
        /// `DEFAULT_ISING_SPEC_NAME`.
        pub fn default_ising_spec_name() -> BoundedVec<u8, ConstU32<128>> {
            BoundedVec::try_from(DEFAULT_ISING_SPEC_NAME.to_vec())
                .expect("default Ising spec name is within bound")
        }

        /// Derive the canonical default plain Ising job spec id.
        ///
        /// The hash input is exactly the SCALE-encoded tuple:
        /// `(b"plain-ising-v1", Formulation::Ising, None, None)`.
        pub fn default_ising_spec_id() -> T::Hash {
            Self::job_spec_id(
                &Self::default_ising_spec_name(),
                types::Formulation::Ising,
                None,
                None,
            )
        }

        /// Read a persisted poll result without exposing raw storage access as
        /// the pallet’s external contract.
        pub fn result_for_order(order_id: u64) -> Option<StoredResultOf<T>> {
            OrderResults::<T>::get(order_id)
        }

        pub fn job_order(order_id: u64) -> Option<JobOrderOf<T>> {
            JobOrders::<T>::get(order_id)
        }

        pub fn order_top_solvers(order_id: u64) -> Vec<types::RankedSolver<T::AccountId>> {
            OrderTopSolvers::<T>::get(order_id).to_vec()
        }

        pub fn open_order_ids(start_after: Option<u64>, limit: u32) -> Vec<u64> {
            let limit = limit.min(1_000) as usize;
            if limit == 0 {
                return Vec::new();
            }

            let now = frame_system::Pallet::<T>::block_number();
            let mut ids: Vec<u64> = OpenOrders::<T>::iter_keys()
                .filter(|order_id| start_after.map(|cursor| *order_id > cursor).unwrap_or(true))
                .filter(|order_id| {
                    let Some(order) = JobOrders::<T>::get(order_id) else {
                        return false;
                    };
                    order.status == types::OrderStatus::Opened
                        && !lifecycle::is_expired(
                            now,
                            order.created_at,
                            order.first_solution_at,
                            &order.timing,
                        )
                })
                .collect();

            ids.sort_unstable();
            ids.truncate(limit);
            ids
        }

        /// Derive the storage key hash for a job spec tuple.
        ///
        /// All spec registration paths use this helper so genesis, migration,
        /// and the root extrinsic agree on `spec_id` derivation.
        fn job_spec_id(
            name: &BoundedVec<u8, ConstU32<128>>,
            formulation: types::Formulation,
            validation_program: Option<T::Hash>,
            transform_program: Option<T::Hash>,
        ) -> T::Hash {
            T::Hashing::hash_of(&(
                name.clone(),
                formulation,
                validation_program,
                transform_program,
            ))
        }

        /// Insert the canonical default plain Ising spec.
        ///
        /// `builder` is stored as attribution on the resulting `JobSpec`.
        /// `emit_event` is `false` for genesis/migration and `true` only when
        /// this helper is reused by an extrinsic path.
        fn insert_default_ising_spec(
            builder: T::AccountId,
            emit_event: bool,
        ) -> Result<T::Hash, DispatchError> {
            Self::insert_job_spec(
                builder,
                Self::default_ising_spec_name(),
                types::Formulation::Ising,
                None,
                None,
                frame_system::Pallet::<T>::block_number(),
                emit_event,
            )
        }

        /// Validate and insert a job spec with consistent duplicate handling.
        ///
        /// This is the single write path for job specs. It computes the
        /// `spec_id`, rejects duplicates, asks the VM to validate referenced
        /// programs, writes `JobSpecs`, and optionally emits
        /// `JobSpecRegistered`.
        fn insert_job_spec(
            builder: T::AccountId,
            name: BoundedVec<u8, ConstU32<128>>,
            formulation: types::Formulation,
            validation_program: Option<T::Hash>,
            transform_program: Option<T::Hash>,
            registered_at: BlockNumberFor<T>,
            emit_event: bool,
        ) -> Result<T::Hash, DispatchError> {
            let spec_id =
                Self::job_spec_id(&name, formulation, validation_program, transform_program);
            ensure!(
                !JobSpecs::<T>::contains_key(&spec_id),
                Error::<T>::JobSpecAlreadyExists
            );
            T::VM::validate_programs(validation_program.as_ref(), transform_program.as_ref())?;

            JobSpecs::<T>::insert(
                spec_id,
                JobSpecOf::<T> {
                    builder: builder.clone(),
                    name,
                    formulation,
                    validation_program,
                    transform_program,
                    registered_at,
                    total_orders: 0,
                    successful_orders: 0,
                },
            );

            if emit_event {
                Self::deposit_event(Event::JobSpecRegistered { spec_id, builder });
            }
            Ok(spec_id)
        }

        fn ensure_valid_mode(mode: &JobModeOf<T>) -> Result<(), DispatchError> {
            if let types::JobMode::Bid {
                miners,
                miner_types,
            } = mode
            {
                ensure!(
                    miners.is_some() || miner_types.is_some(),
                    Error::<T>::EmptyBidCriteria
                );
            }
            Ok(())
        }

        fn bid_dimensions(mode: &JobModeOf<T>) -> (u32, u32) {
            match mode {
                types::JobMode::Open => (0, 0),
                types::JobMode::Bid {
                    miners,
                    miner_types,
                } => (
                    miners
                        .as_ref()
                        .map(|accounts| accounts.len() as u32)
                        .unwrap_or(0),
                    miner_types
                        .as_ref()
                        .map(|types| types.len() as u32)
                        .unwrap_or(0),
                ),
            }
        }

        fn ensure_valid_resolution(
            resolution: &types::RewardResolution,
        ) -> Result<(), DispatchError> {
            match resolution {
                types::RewardResolution::SingleBest => Ok(()),
                types::RewardResolution::TopNWeighted { n }
                | types::RewardResolution::TopNEqual { n } => {
                    ensure!(
                        *n > 0 && *n <= types::MAX_REWARD_WINNERS,
                        Error::<T>::InvalidRewardResolution
                    );
                    Ok(())
                }
            }
        }

        fn solver_is_eligible(
            solver: &T::AccountId,
            solver_type: types::MinerType,
            mode: &JobModeOf<T>,
        ) -> bool {
            match mode {
                types::JobMode::Open => true,
                types::JobMode::Bid {
                    miners,
                    miner_types,
                } => {
                    let account_match = miners
                        .as_ref()
                        .map(|allowed| allowed.iter().any(|candidate| candidate == solver))
                        .unwrap_or(false);
                    let type_match = miner_types
                        .as_ref()
                        .map(|allowed| allowed.iter().any(|candidate| *candidate == solver_type))
                        .unwrap_or(false);
                    account_match || type_match
                }
            }
        }

        fn expire_order_if_needed(order_id: u64, order: &mut JobOrderOf<T>) {
            let now = frame_system::Pallet::<T>::block_number();
            if order.status == types::OrderStatus::Opened
                && lifecycle::is_expired(
                    now,
                    order.created_at,
                    order.first_solution_at,
                    &order.timing,
                )
            {
                order.status = types::OrderStatus::Expired;
                JobOrders::<T>::insert(order_id, order.clone());
                OpenOrders::<T>::remove(order_id);
                Self::deposit_event(Event::OrderExpired { order_id });
            }
        }

        fn update_ranking(
            order_id: u64,
            resolution: &types::RewardResolution,
            solver: &T::AccountId,
            best_energy: i64,
        ) -> Result<(), DispatchError> {
            match resolution {
                types::RewardResolution::SingleBest => {
                    let should_replace = OrderFrontRunner::<T>::get(order_id)
                        .map(|entry| best_energy < entry.energy_milli)
                        .unwrap_or(true);

                    if should_replace {
                        OrderFrontRunner::<T>::insert(
                            order_id,
                            FrontRunnerOf::<T> {
                                solver: solver.clone(),
                                energy_milli: best_energy,
                            },
                        );
                        Self::deposit_event(Event::FrontRunnerChanged {
                            order_id,
                            solver: solver.clone(),
                            energy_milli: best_energy,
                        });
                    }
                }
                types::RewardResolution::TopNWeighted { n }
                | types::RewardResolution::TopNEqual { n } => {
                    let current = OrderTopSolvers::<T>::get(order_id);
                    let previous_front = current
                        .first()
                        .map(|entry| (entry.solver.clone(), entry.energy_milli));
                    let updated = rewards::update_ranked_solvers(
                        current.as_slice(),
                        types::RankedSolver {
                            solver: solver.clone(),
                            energy_milli: best_energy,
                        },
                        *n as usize,
                    );
                    let updated_bounded: TopSolversOf<T> = updated
                        .try_into()
                        .map_err(|_| Error::<T>::InvalidRewardResolution)?;
                    let next_front = updated_bounded
                        .first()
                        .map(|entry| (entry.solver.clone(), entry.energy_milli));
                    OrderTopSolvers::<T>::insert(order_id, updated_bounded);

                    if previous_front != next_front {
                        if let Some((leader, energy)) = next_front {
                            Self::deposit_event(Event::FrontRunnerChanged {
                                order_id,
                                solver: leader,
                                energy_milli: energy,
                            });
                        }
                    }
                }
            }
            Ok(())
        }

        fn compute_payouts(
            order_id: u64,
            order: &JobOrderOf<T>,
        ) -> Result<Vec<(T::AccountId, u128, i64)>, DispatchError> {
            let reward_u128: u128 = order.reward.saturated_into();
            let payouts = match order.resolution {
                types::RewardResolution::SingleBest => {
                    let front = OrderFrontRunner::<T>::get(order_id)
                        .ok_or(Error::<T>::NoSolutionsAccepted)?;
                    rewards::single_best_payouts(&front, reward_u128)
                }
                types::RewardResolution::TopNEqual { .. } => {
                    let ranked = OrderTopSolvers::<T>::get(order_id);
                    ensure!(!ranked.is_empty(), Error::<T>::NoSolutionsAccepted);
                    rewards::top_n_equal_payouts(ranked.as_slice(), reward_u128)
                }
                types::RewardResolution::TopNWeighted { .. } => {
                    let ranked = OrderTopSolvers::<T>::get(order_id);
                    ensure!(!ranked.is_empty(), Error::<T>::NoSolutionsAccepted);
                    rewards::top_n_weighted_payouts(ranked.as_slice(), reward_u128)
                }
            };
            Ok(payouts)
        }

        fn emit_result_ready(
            order_id: u64,
            delivery: &types::ResultDelivery,
            winners: WinnerSummariesOf<T>,
        ) {
            match delivery {
                types::ResultDelivery::OnChainOnly => {}
                types::ResultDelivery::Callback { endpoint }
                | types::ResultDelivery::CallbackWithPoll { endpoint } => {
                    Self::deposit_event(Event::ResultReady {
                        order_id,
                        endpoint: endpoint.clone(),
                        winners,
                    });
                }
            }
        }

        fn persist_result_if_needed(
            order_id: u64,
            order: &JobOrderOf<T>,
            winners: WinnerSummariesOf<T>,
        ) {
            match &order.delivery {
                types::ResultDelivery::CallbackWithPoll { endpoint } => {
                    OrderResults::<T>::insert(
                        order_id,
                        StoredResultOf::<T> {
                            endpoint: endpoint.clone(),
                            resolution: order.resolution,
                            settled_at: frame_system::Pallet::<T>::block_number(),
                            winners,
                        },
                    );
                }
                types::ResultDelivery::OnChainOnly | types::ResultDelivery::Callback { .. } => {}
            }
        }

        fn winner_summaries_from_payouts(
            payouts: &[(T::AccountId, u128, i64)],
        ) -> WinnerSummariesOf<T> {
            payouts
                .iter()
                .map(|(solver, amount, energy_milli)| types::WinnerSummary {
                    solver: solver.clone(),
                    energy_milli: *energy_milli,
                    amount: (*amount).saturated_into(),
                })
                .collect::<Vec<_>>()
                .try_into()
                .expect("reward resolution limits payouts to MAX_REWARD_WINNERS")
        }

        fn map_validation_error(error: ValidationError) -> Error<T> {
            match error {
                ValidationError::InvalidSpinValue { .. } => Error::<T>::InvalidSpinValues,
                ValidationError::SolutionLengthMismatch { .. } => {
                    Error::<T>::SolutionLengthMismatch
                }
                ValidationError::EmptyNodes
                | ValidationError::FieldLengthMismatch { .. }
                | ValidationError::EdgeWeightLengthMismatch { .. }
                | ValidationError::DuplicateNode { .. }
                | ValidationError::UnknownNodeInEdge { .. }
                | ValidationError::EmptyFieldValues
                | ValidationError::EmptyAllowedValues
                | ValidationError::InvalidEncodedValue { .. }
                | ValidationError::EncodingTooWide { .. }
                | ValidationError::PackedSolutionLengthMismatch { .. }
                | ValidationError::ArithmeticOverflow => Error::<T>::InvalidTopology,
            }
        }
    }
}

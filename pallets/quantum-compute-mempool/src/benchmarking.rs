//! Benchmarking setup for pallet-quantum-compute-mempool.

use super::*;

#[allow(unused)]
use crate::Pallet as QuantumComputeMempool;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
    pallet_prelude::ConstU32,
    traits::{Currency, Get, Hooks, ReservableCurrency},
    BoundedVec,
};
use frame_system::RawOrigin;
use sp_runtime::traits::{Hash as _, SaturatedConversion, Saturating, Zero};

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: frame_support::traits::Get<u32>,
{
    items
        .try_into()
        .ok()
        .expect("benchmark input fits within bounds")
}

fn benchmark_reward<T: Config>() -> BalanceOf<T> {
    T::MinReward::get()
}

fn benchmark_funding<T: Config>() -> BalanceOf<T> {
    benchmark_reward::<T>().saturating_mul(10u32.saturated_into())
}

fn fund_account<T: Config>(who: &T::AccountId, amount: BalanceOf<T>) {
    let _ = T::Currency::make_free_balance_be(who, amount);
}

fn sample_spec<T: Config>() -> (
    BoundedVec<u8, ConstU32<128>>,
    types::Formulation,
    Option<T::Hash>,
    Option<T::Hash>,
) {
    (
        bounded(b"max-cut".to_vec()),
        types::Formulation::Ising,
        None,
        None,
    )
}

fn sample_spec_id<T: Config>(
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

fn sample_params<T: Config>() -> IsingParamsOf<T> {
    types::IsingParams {
        nodes: bounded(vec![0, 1]),
        edges: bounded(vec![(0, 1)]),
        h_values: bounded(vec![0, 0]),
        j_values: bounded(vec![-1_000]),
        max_energy_milli: None,
        min_diversity_milli: None,
        min_solutions: None,
    }
}

fn sample_solution<T: Config>() -> SolutionsOf<T> {
    bounded(vec![bounded(vec![1, 1])])
}

fn params_with_dimensions<T: Config>(
    node_count: u32,
    edge_count: u32,
    min_solutions: Option<u32>,
) -> IsingParamsOf<T> {
    assert!(node_count >= 2);
    let nodes: Vec<u32> = (0..node_count).collect();
    let edges: Vec<(u32, u32)> = (0..edge_count)
        .map(|index| {
            let source = index % node_count;
            (source, (source + 1) % node_count)
        })
        .collect();

    types::IsingParams {
        nodes: bounded(nodes),
        edges: bounded(edges),
        h_values: bounded(vec![0; node_count as usize]),
        j_values: bounded(vec![-1_000; edge_count as usize]),
        max_energy_milli: None,
        min_diversity_milli: None,
        min_solutions,
    }
}

/// `solution_count` rows of `node_count` spins that are pairwise distinct
/// answers (W1-004) whenever enough spins exist: spin 0 is `first_spin` in every
/// row, so no row is a global flip of another, and spin `i >= 1` is negated when
/// bit `i - 1` of the row index is set. The sets for `first_spin = 1` and `-1`
/// are exact negations of each other, row by row.
fn distinct_rows<T: Config>(
    node_count: u32,
    solution_count: u32,
    first_spin: i8,
) -> SolutionsOf<T> {
    bounded(
        (0..solution_count)
            .map(|solution_index| {
                bounded(
                    (0..node_count)
                        .map(|node_index| {
                            let flipped = node_index >= 1
                                && node_index <= 32
                                && (solution_index >> (node_index - 1)) & 1 == 1;
                            if flipped {
                                -first_spin
                            } else {
                                first_spin
                            }
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

/// The worst case for the W1-004 dedupe: every row a distinct answer.
fn solutions_with_dimensions<T: Config>(node_count: u32, solution_count: u32) -> SolutionsOf<T> {
    distinct_rows::<T>(node_count, solution_count, 1)
}

/// How many distinct answers `distinct_rows` can hold: `node_count - 1` free
/// spins give at most `2^(node_count - 1)` flip classes.
fn achievable_distinct(node_count: u32, solution_count: u32) -> u32 {
    let free = node_count.saturating_sub(1);
    if free >= 32 {
        solution_count
    } else {
        solution_count.min(1u32 << free)
    }
}

fn register_spec_for<T: Config>(builder: &T::AccountId) -> T::Hash {
    let (name, formulation, validation_program, transform_program) = sample_spec::<T>();
    let spec_id = sample_spec_id::<T>(&name, formulation, validation_program, transform_program);
    assert!(QuantumComputeMempool::<T>::register_job_spec(
        RawOrigin::Root.into(),
        builder.clone(),
        name,
        formulation,
        validation_program,
        transform_program,
    )
    .is_ok());
    spec_id
}

fn register_solver_for<T: Config>(solver: &T::AccountId) {
    fund_account::<T>(
        solver,
        T::Currency::minimum_balance().saturating_mul(2u32.into()),
    );
    assert!(QuantumComputeMempool::<T>::register_solver(
        RawOrigin::Signed(solver.clone()).into(),
        types::MinerType::Cpu,
    )
    .is_ok());
}

fn propose_open_order_for<T: Config>(
    proposer: &T::AccountId,
    reward: BalanceOf<T>,
    resolution: types::RewardResolution,
    deadline_blocks: BlockNumberOf<T>,
    block_wait: BlockNumberOf<T>,
    delivery: types::ResultDelivery,
) -> u64 {
    fund_account::<T>(proposer, reward.saturating_mul(10u32.saturated_into()));
    let spec_id = register_spec_for::<T>(proposer);
    let order_id = NextOrderId::<T>::get();
    assert!(QuantumComputeMempool::<T>::propose_job(
        RawOrigin::Signed(proposer.clone()).into(),
        spec_id,
        sample_params::<T>(),
        reward,
        types::JobMode::Open,
        resolution,
        deadline_blocks,
        block_wait,
        delivery,
    )
    .is_ok());
    order_id
}

fn propose_order_for<T: Config>(
    proposer: &T::AccountId,
    params: IsingParamsOf<T>,
    reward: BalanceOf<T>,
    mode: JobModeOf<T>,
    resolution: types::RewardResolution,
    delivery: types::ResultDelivery,
) -> u64 {
    fund_account::<T>(proposer, reward.saturating_mul(10u32.saturated_into()));
    let spec_id = register_spec_for::<T>(proposer);
    let order_id = NextOrderId::<T>::get();
    assert!(QuantumComputeMempool::<T>::propose_job(
        RawOrigin::Signed(proposer.clone()).into(),
        spec_id,
        params,
        reward,
        mode,
        resolution,
        10u32.into(),
        5u32.into(),
        delivery,
    )
    .is_ok());
    order_id
}

// The bounded closure scan must see a full index with the closing id last.
fn fill_proposer_index<T: Config>(proposer: &T::AccountId, order_id: u64) {
    let mut ids: Vec<u64> = (1..T::MaxOrdersPerProposer::get())
        .map(|i| order_id + u64::from(i))
        .collect();
    ids.push(order_id);
    ProposerOrders::<T>::insert(proposer, bounded::<_, T::MaxOrdersPerProposer>(ids));
}

// Shared maximal settlement setup; lifecycle variants are measured separately.
fn setup_claim_reward<T: Config>() -> (T::AccountId, T::AccountId, Vec<T::AccountId>, u64) {
    let caller: T::AccountId = whitelisted_caller();
    let proposer: T::AccountId = account("proposer", 0, 0);
    fund_account::<T>(&caller, benchmark_funding::<T>());
    let winner_count = types::MAX_REWARD_WINNERS;
    // Keep every weighted payout above the existential deposit, so all
    // 32 transfers and solver-counter writes execute even in the mock.
    let payout_floor = benchmark_reward::<T>().max(T::Currency::minimum_balance());
    let reward = payout_floor.saturating_mul((winner_count * winner_count).saturated_into());
    let winners: Vec<T::AccountId> = (0..winner_count)
        .map(|index| account("winner", index, 0))
        .collect();
    for winner in &winners {
        register_solver_for::<T>(winner);
    }
    let order_id = propose_order_for::<T>(
        &proposer,
        params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None),
        reward,
        types::JobMode::Bid {
            miners: Some(bounded(
                winners
                    .iter()
                    .take(T::MaxBidMiners::get() as usize)
                    .cloned()
                    .collect(),
            )),
            miner_types: None,
        },
        types::RewardResolution::TopNWeighted { n: winner_count },
        types::ResultDelivery::CallbackWithPoll {
            endpoint: bounded(vec![b'x'; 256]),
        },
    );
    let ranked: TopSolversOf<T> = bounded(
        winners
            .iter()
            .enumerate()
            .map(|(index, winner)| types::RankedSolver {
                solver: winner.clone(),
                energy_milli: -i64::from(winner_count) + index as i64,
            })
            .collect(),
    );
    OrderTopSolvers::<T>::insert(order_id, ranked);
    JobOrders::<T>::mutate(order_id, |maybe_order| {
        let order = maybe_order.as_mut().expect("benchmark order exists");
        order.solution_count = winner_count;
        order.first_solution_at = Some(order.created_at);
        assert_eq!(order.status, types::OrderStatus::Opened);
    });

    // Mirror the first-solution reschedule after seeding the maximal ranking.
    let created_at = JobOrders::<T>::get(order_id).unwrap().created_at;
    OrdersExpiringAt::<T>::remove(created_at.saturating_add(10u32.into()), order_id);
    OrdersExpiringAt::<T>::insert(created_at.saturating_add(5u32.into()), order_id, ());
    // Expiry/result publication is hook work, outside the measured settlement.
    // Advance every block: the hook intentionally bounds empty-bucket scanning.
    for offset in 1u32..=5 {
        let now = created_at.saturating_add(offset.into());
        frame_system::Pallet::<T>::set_block_number(now);
        QuantumComputeMempool::<T>::on_initialize(now);
    }
    assert_eq!(
        JobOrders::<T>::get(order_id).unwrap().status,
        types::OrderStatus::Expired
    );
    assert!(!OpenOrders::<T>::contains_key(order_id));
    assert!(OrderResults::<T>::contains_key(order_id));
    assert!(!Solvers::<T>::contains_key(&caller));

    fill_proposer_index::<T>(&proposer, order_id);
    (caller, proposer, winners, order_id)
}

fn verify_claim_reward<T: Config>(order_id: u64, winners: &[T::AccountId]) {
    let winner_count = types::MAX_REWARD_WINNERS;
    let order = JobOrders::<T>::get(order_id).expect("order exists");
    assert_eq!(order.status, types::OrderStatus::Closed);
    // W1-006: settlement empties the order's escrow account.
    assert!(
        T::Currency::total_balance(&QuantumComputeMempool::<T>::escrow_account(order_id)).is_zero()
    );
    assert!(!ProposerOrders::<T>::get(&order.proposer).contains(&order_id));
    assert!(!OpenOrders::<T>::contains_key(order_id));
    for winner in winners {
        assert!(Solvers::<T>::get(winner).unwrap().rewards_earned > 0u32.into());
    }
    assert_eq!(
        OrderResults::<T>::get(order_id)
            .expect("poll result is stored")
            .winners
            .len(),
        winner_count as usize
    );
}

// Seed a real accepted maximal payload and a full ranking outside measurement.
fn setup_solution_replacement<T: Config>(n: u32, e: u32, s: u32) -> (T::AccountId, u64, i64) {
    let proposer: T::AccountId = whitelisted_caller();
    let solver: T::AccountId = account("solver", 0, 0);
    register_solver_for::<T>(&solver);
    let mut params = params_with_dimensions::<T>(n, e, Some(achievable_distinct(n, s)));
    // An isolated local field makes every first-spin-minus row strictly worse
    // than its negation (the first-spin-plus row) at every dimension, without
    // relying on a fabricated stored score.
    params.h_values[0] = -1_000;
    let order_id = propose_order_for::<T>(
        &proposer,
        params,
        benchmark_reward::<T>(),
        types::JobMode::Open,
        types::RewardResolution::TopNEqual {
            n: types::MAX_REWARD_WINNERS,
        },
        types::ResultDelivery::OnChainOnly,
    );
    let payee = types::WinnerPayee::V1 {
        chain_id: 20033,
        address: sp_core::H160::repeat_byte(1),
    };
    assert!(QuantumComputeMempool::<T>::submit_solution_with_payee(
        RawOrigin::Signed(solver.clone()).into(),
        order_id,
        distinct_rows::<T>(n, s, -1),
        payee,
    )
    .is_ok());
    let old_energy = OrderSolutions::<T>::get(order_id, &solver)
        .unwrap()
        .best_energy_milli;
    let ranked: TopSolversOf<T> = bounded(
        (0..types::MAX_REWARD_WINNERS)
            .map(|index| types::RankedSolver {
                solver: account("ranked-solver", index, 0),
                energy_milli: old_energy + i64::from(index),
            })
            .collect(),
    );
    OrderTopSolvers::<T>::insert(order_id, ranked);

    (solver, order_id, old_energy)
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn register_solver() {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(
            &caller,
            T::Currency::minimum_balance().saturating_mul(2u32.into()),
        );

        #[extrinsic_call]
        QuantumComputeMempool::register_solver(
            RawOrigin::Signed(caller.clone()),
            types::MinerType::Cpu,
        );

        assert!(Solvers::<T>::contains_key(&caller));
        assert_eq!(
            T::Currency::reserved_balance(&caller),
            T::Currency::minimum_balance()
        );
    }

    #[benchmark]
    fn deregister_solver() {
        let caller: T::AccountId = whitelisted_caller();
        register_solver_for::<T>(&caller);

        #[extrinsic_call]
        QuantumComputeMempool::deregister_solver(RawOrigin::Signed(caller.clone()));

        assert!(!Solvers::<T>::contains_key(&caller));
        assert_eq!(T::Currency::reserved_balance(&caller), 0u32.into());
        assert_eq!(
            T::Currency::free_balance(&caller),
            T::Currency::minimum_balance().saturating_mul(2u32.into())
        );
    }

    #[benchmark]
    fn register_job_spec() {
        let caller: T::AccountId = whitelisted_caller();
        let (name, formulation, validation_program, transform_program) = sample_spec::<T>();
        let spec_id =
            sample_spec_id::<T>(&name, formulation, validation_program, transform_program);

        #[extrinsic_call]
        QuantumComputeMempool::register_job_spec(
            RawOrigin::Root,
            caller.clone(),
            name,
            formulation,
            validation_program,
            transform_program,
        );

        assert!(JobSpecs::<T>::contains_key(spec_id));
    }

    #[benchmark]
    fn propose_job(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        b: Linear<1, { T::MaxBidMiners::get() }>,
        t: Linear<0, { MinerTypesOf::bound() as u32 }>,
    ) {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(&caller, benchmark_funding::<T>());
        let spec_id = register_spec_for::<T>(&caller);
        let reward = benchmark_reward::<T>();
        let miners = bounded((0..b).map(|index| account("bid-miner", index, 0)).collect());
        let miner_types = if t == 0 {
            None
        } else {
            Some(bounded(vec![types::MinerType::Cpu; t as usize]))
        };

        #[extrinsic_call]
        QuantumComputeMempool::propose_job(
            RawOrigin::Signed(caller.clone()),
            spec_id,
            params_with_dimensions::<T>(n, e, None),
            reward,
            types::JobMode::Bid {
                miners: Some(miners),
                miner_types,
            },
            types::RewardResolution::SingleBest,
            10u32.into(),
            5u32.into(),
            types::ResultDelivery::OnChainOnly,
        );

        assert!(JobOrders::<T>::contains_key(0));
        // W1-006: the reward moved into the order's escrow account.
        assert_eq!(
            T::Currency::free_balance(&QuantumComputeMempool::<T>::escrow_account(0)),
            JobOrders::<T>::get(0).expect("order stored").reward
        );
    }

    #[benchmark]
    fn submit_solution(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let proposer: T::AccountId = whitelisted_caller();
        let solver: T::AccountId = account("solver", 0, 0);
        register_solver_for::<T>(&solver);
        // Only distinct answers count (W1-004); cap at what n spins can hold.
        let min_solutions = s
            .saturating_sub(1)
            .max(1)
            .min(achievable_distinct(n, s))
            .max(1);
        let order_id = propose_order_for::<T>(
            &proposer,
            params_with_dimensions::<T>(n, e, Some(min_solutions)),
            benchmark_reward::<T>(),
            types::JobMode::Open,
            types::RewardResolution::TopNEqual {
                n: types::MAX_REWARD_WINNERS,
            },
            types::ResultDelivery::OnChainOnly,
        );
        let ranked: TopSolversOf<T> = bounded(
            (0..types::MAX_REWARD_WINNERS)
                .map(|index| types::RankedSolver {
                    solver: account("ranked-solver", index, 0),
                    energy_milli: 1_000_000_000_i64.saturating_add(i64::from(index)),
                })
                .collect(),
        );
        OrderTopSolvers::<T>::insert(order_id, ranked);

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution(
            RawOrigin::Signed(solver.clone()),
            order_id,
            solutions_with_dimensions::<T>(n, s),
        );

        assert!(OrderSolutions::<T>::contains_key(order_id, solver));
    }

    #[benchmark]
    fn propose_job_with_payee_requirement(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        b: Linear<1, { T::MaxBidMiners::get() }>,
        t: Linear<0, { MinerTypesOf::bound() as u32 }>,
    ) {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(&caller, benchmark_funding::<T>());
        let spec_id = register_spec_for::<T>(&caller);
        let reward = benchmark_reward::<T>();
        let miners = bounded((0..b).map(|index| account("bid-miner", index, 0)).collect());
        let miner_types = if t == 0 {
            None
        } else {
            Some(bounded(vec![types::MinerType::Cpu; t as usize]))
        };

        #[extrinsic_call]
        QuantumComputeMempool::propose_job_with_payee_requirement(
            RawOrigin::Signed(caller.clone()),
            spec_id,
            params_with_dimensions::<T>(n, e, None),
            reward,
            types::JobMode::Bid {
                miners: Some(miners),
                miner_types,
            },
            types::RewardResolution::SingleBest,
            10u32.into(),
            5u32.into(),
            types::ResultDelivery::OnChainOnly,
            20033,
        );

        assert!(JobOrders::<T>::contains_key(0));
        assert_eq!(OrderPayeeRequirements::<T>::get(0), Some(20033));
    }

    #[benchmark]
    fn submit_solution_with_payee(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let proposer: T::AccountId = whitelisted_caller();
        let solver: T::AccountId = account("solver", 0, 0);
        register_solver_for::<T>(&solver);
        // Only distinct answers count (W1-004); cap at what n spins can hold.
        let min_solutions = s
            .saturating_sub(1)
            .max(1)
            .min(achievable_distinct(n, s))
            .max(1);
        let order_id = propose_order_for::<T>(
            &proposer,
            params_with_dimensions::<T>(n, e, Some(min_solutions)),
            benchmark_reward::<T>(),
            types::JobMode::Open,
            types::RewardResolution::TopNEqual {
                n: types::MAX_REWARD_WINNERS,
            },
            types::ResultDelivery::OnChainOnly,
        );
        let ranked: TopSolversOf<T> = bounded(
            (0..types::MAX_REWARD_WINNERS)
                .map(|index| types::RankedSolver {
                    solver: account("ranked-solver", index, 0),
                    energy_milli: 1_000_000_000_i64.saturating_add(i64::from(index)),
                })
                .collect(),
        );
        OrderTopSolvers::<T>::insert(order_id, ranked);

        OrderPayeeRequirements::<T>::insert(order_id, 20033);
        let payee = types::WinnerPayee::V1 {
            chain_id: 20033,
            address: sp_core::H160::repeat_byte(1),
        };

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution_with_payee(
            RawOrigin::Signed(solver.clone()),
            order_id,
            solutions_with_dimensions::<T>(n, s),
            payee,
        );

        assert!(OrderSolutions::<T>::contains_key(order_id, &solver));
        assert_eq!(OrderPayees::<T>::get(order_id, &solver), Some(payee));
    }

    // Reading/replacing the largest previously accepted payload is a separate
    // timing case. Compare this with first-submission rescheduling before
    // generating the public submission weights.
    #[benchmark(extra)]
    fn submit_solution_replacement(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let (solver, order_id, old_energy) = setup_solution_replacement::<T>(n, e, s);
        OrderPayeeRequirements::<T>::insert(order_id, 20033);
        let payee = types::WinnerPayee::V1 {
            chain_id: 20033,
            address: sp_core::H160::repeat_byte(2),
        };

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution_with_payee(
            RawOrigin::Signed(solver.clone()),
            order_id,
            distinct_rows::<T>(n, s, 1),
            payee,
        );

        assert!(
            OrderSolutions::<T>::get(order_id, &solver)
                .unwrap()
                .best_energy_milli
                < old_energy
        );
        assert_eq!(OrderPayees::<T>::get(order_id, &solver), Some(payee));
    }

    // Legacy call clears an optional destination while replacing the full payload.
    #[benchmark(extra)]
    fn submit_solution_clear_payee(
        n: Linear<2, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let (solver, order_id, old_energy) = setup_solution_replacement::<T>(n, e, s);
        assert!(OrderPayees::<T>::contains_key(order_id, &solver));

        #[extrinsic_call]
        QuantumComputeMempool::submit_solution(
            RawOrigin::Signed(solver.clone()),
            order_id,
            distinct_rows::<T>(n, s, 1),
        );

        assert!(
            OrderSolutions::<T>::get(order_id, &solver)
                .unwrap()
                .best_energy_milli
                < old_energy
        );
        assert!(!OrderPayees::<T>::contains_key(order_id, &solver));
        assert!(frame_system::Pallet::<T>::events()
            .iter()
            .any(|record| record.event
                == Event::<T>::SolutionPayeeSet {
                    order_id,
                    solver: solver.clone(),
                    payee: None
                }
                .into()));
    }

    #[benchmark]
    fn claim_reward() {
        let (caller, proposer, winners, order_id) = setup_claim_reward::<T>();

        #[extrinsic_call]
        QuantumComputeMempool::claim_reward(RawOrigin::Signed(caller), order_id);

        verify_claim_reward::<T>(order_id, &winners);
        assert!(T::Currency::free_balance(&proposer) >= T::Currency::minimum_balance());
    }

    // The public claim weight takes the maximum of both measured lifecycle cases.
    #[benchmark(extra)]
    fn claim_reward_dust() {
        let (caller, proposer, winners, order_id) = setup_claim_reward::<T>();
        let dust = T::Currency::minimum_balance().saturating_sub(1u32.into());
        // A proposer left holding only dust (an independent provider keeps the
        // account in System). Since W1-006 the reward sits in the order's
        // escrow account, so settlement pays from there and must not touch the
        // proposer's own balance at all: the dust survives, nothing is burned,
        // and the escrow ends empty.
        frame_system::Pallet::<T>::inc_providers(&proposer);
        fund_account::<T>(&proposer, dust);
        assert_eq!(T::Currency::free_balance(&proposer), dust);
        let issuance = T::Currency::total_issuance();

        #[extrinsic_call]
        QuantumComputeMempool::claim_reward(RawOrigin::Signed(caller), order_id);

        verify_claim_reward::<T>(order_id, &winners);
        assert_eq!(T::Currency::total_balance(&proposer), dust);
        assert_eq!(T::Currency::total_issuance(), issuance);
    }

    #[benchmark]
    fn reclaim_order() {
        let proposer: T::AccountId = whitelisted_caller();
        let order_id = propose_open_order_for::<T>(
            &proposer,
            benchmark_reward::<T>(),
            types::RewardResolution::SingleBest,
            1u32.into(),
            1u32.into(),
            types::ResultDelivery::OnChainOnly,
        );
        JobOrders::<T>::mutate(order_id, |maybe_order| {
            let order = maybe_order.as_mut().expect("benchmark order exists");
            order.ising_params =
                params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None);
        });

        fill_proposer_index::<T>(&proposer, order_id);
        let now = frame_system::Pallet::<T>::block_number().saturating_add(1u32.into());
        frame_system::Pallet::<T>::set_block_number(now);
        QuantumComputeMempool::<T>::on_initialize(now);

        #[extrinsic_call]
        QuantumComputeMempool::reclaim_order(RawOrigin::Signed(proposer.clone()), order_id);

        let order = JobOrders::<T>::get(order_id).expect("order exists");
        assert_eq!(order.status, types::OrderStatus::Closed);
        assert!(!ProposerOrders::<T>::get(&proposer).contains(&order_id));
        // W1-006: reclaim empties the order's escrow account.
        assert!(
            T::Currency::total_balance(&QuantumComputeMempool::<T>::escrow_account(order_id))
                .is_zero()
        );
    }

    #[benchmark]
    fn purge_result() {
        let proposer: T::AccountId = whitelisted_caller();
        let solver: T::AccountId = account("solver", 0, 0);
        let cleaner: T::AccountId = account("cleaner", 0, 0);
        register_solver_for::<T>(&solver);
        fund_account::<T>(&cleaner, benchmark_funding::<T>());
        let spec_id = register_spec_for::<T>(&proposer);
        fund_account::<T>(&proposer, benchmark_funding::<T>());
        let order_id = NextOrderId::<T>::get();
        assert!(QuantumComputeMempool::<T>::propose_job(
            RawOrigin::Signed(proposer.clone()).into(),
            spec_id,
            sample_params::<T>(),
            benchmark_reward::<T>(),
            types::JobMode::Bid {
                miners: Some(bounded(vec![solver.clone()])),
                miner_types: None,
            },
            types::RewardResolution::SingleBest,
            2u32.into(),
            1u32.into(),
            types::ResultDelivery::CallbackWithPoll {
                endpoint: bounded(b"https://solver.example/poll".to_vec()),
            },
        )
        .is_ok());
        assert!(QuantumComputeMempool::<T>::submit_solution(
            RawOrigin::Signed(solver.clone()).into(),
            order_id,
            sample_solution::<T>(),
        )
        .is_ok());
        frame_system::Pallet::<T>::set_block_number(2u32.into());
        QuantumComputeMempool::<T>::on_initialize(2u32.into());
        assert!(OrderResults::<T>::get(order_id).is_some());
        frame_system::Pallet::<T>::set_block_number(
            T::ResultTtlBlocks::get().saturating_add(2u32.into()),
        );

        #[extrinsic_call]
        QuantumComputeMempool::purge_result(RawOrigin::Signed(cleaner), order_id);

        assert!(OrderResults::<T>::get(order_id).is_none());
    }

    /// Measures the worst-case order: maximum nodes and edges, 32 weighted
    /// winners with full-size solution vectors, and `CallbackWithPoll` with
    /// a maximum-length endpoint, including the `OrderResults` write.
    #[benchmark]
    fn finalise_expired_orders(k: Linear<0, { types::MAX_FINALISED_ORDERS_PER_BLOCK }>) {
        let proposer: T::AccountId = whitelisted_caller();
        let winner_count = types::MAX_REWARD_WINNERS;
        let reward = benchmark_reward::<T>().saturating_mul(winner_count.saturated_into());
        fund_account::<T>(
            &proposer,
            reward
                .saturating_mul(k.saturating_add(1).saturated_into())
                .saturating_mul(2u32.saturated_into()),
        );
        let spec_id = register_spec_for::<T>(&proposer);
        let winners: Vec<T::AccountId> = (0..winner_count)
            .map(|index| account("winner", index, 0))
            .collect();
        for winner in &winners {
            register_solver_for::<T>(winner);
        }
        let bid_miner = winners.first().expect("winner set is nonempty").clone();

        let mut order_ids = Vec::new();
        for _ in 0..k {
            let order_id = NextOrderId::<T>::get();
            assert!(QuantumComputeMempool::<T>::propose_job(
                RawOrigin::Signed(proposer.clone()).into(),
                spec_id,
                params_with_dimensions::<T>(T::MaxNodes::get(), T::MaxEdges::get(), None),
                reward,
                types::JobMode::Bid {
                    miners: Some(bounded(vec![bid_miner.clone()])),
                    miner_types: None,
                },
                types::RewardResolution::TopNWeighted { n: winner_count },
                1u32.into(),
                1u32.into(),
                types::ResultDelivery::CallbackWithPoll {
                    endpoint: bounded(vec![b'x'; <ConstU32<256> as Get<u32>>::get() as usize]),
                },
            )
            .is_ok());
            for winner in &winners {
                // Rotate the fixture's bid allow-list because the mock's
                // MaxBidMiners is smaller than the 32-winner settlement bound.
                JobOrders::<T>::mutate(order_id, |maybe_order| {
                    let order = maybe_order.as_mut().expect("benchmark order exists");
                    order.mode = types::JobMode::Bid {
                        miners: Some(bounded(vec![winner.clone()])),
                        miner_types: None,
                    };
                });
                assert!(QuantumComputeMempool::<T>::submit_solution(
                    RawOrigin::Signed(winner.clone()).into(),
                    order_id,
                    solutions_with_dimensions::<T>(T::MaxNodes::get(), 1),
                )
                .is_ok());
            }
            // Pruning must measure maximum encoded payloads for every slot,
            // not only the one-solution setup needed to establish rankings.
            for winner in &winners {
                OrderSolutions::<T>::mutate(order_id, winner, |entry| {
                    let entry = entry.as_mut().expect("accepted solver payload");
                    entry.solutions =
                        solutions_with_dimensions::<T>(T::MaxNodes::get(), T::MaxSolutions::get());
                    entry.num_valid = T::MaxSolutions::get();
                });
            }
            order_ids.push(order_id);
        }

        let expiry_block = frame_system::Pallet::<T>::block_number().saturating_add(1u32.into());
        frame_system::Pallet::<T>::set_block_number(expiry_block);
        // Measure one full bucket, not the empty buckets before it.
        NextExpiryBucket::<T>::put(expiry_block);
        OldestExpiryBucket::<T>::put(expiry_block);

        #[block]
        {
            QuantumComputeMempool::<T>::on_initialize(expiry_block);
        }

        for order_id in order_ids {
            let order = JobOrders::<T>::get(order_id).expect("order exists");
            assert_eq!(order.status, types::OrderStatus::Expired);
            assert!(OrderSolutions::<T>::iter_key_prefix(order_id)
                .next()
                .is_none());
            assert_eq!(
                OrderResults::<T>::get(order_id)
                    .expect("poll result is stored")
                    .winners
                    .len(),
                winner_count as usize
            );
        }
    }

    /// Maximum closed record and all sparse auxiliary prefixes, including
    /// defensive cleanup of 32 maximum-size orphaned solution payloads.
    #[benchmark]
    fn prune_closed_orders(k: Linear<0, { types::MAX_PRUNED_ORDERS_PER_BLOCK }>) {
        let (caller, _, winners, template_id) = setup_claim_reward::<T>();
        assert!(QuantumComputeMempool::<T>::claim_reward(
            RawOrigin::Signed(caller).into(),
            template_id,
        )
        .is_ok());
        let order = JobOrders::<T>::take(template_id).expect("closed template");
        let result = OrderResults::<T>::take(template_id).expect("poll template");
        let ranking = OrderTopSolvers::<T>::take(template_id);
        let now = frame_system::Pallet::<T>::block_number()
            .saturating_add(T::OrderRetentionBlocks::get());
        OrdersPrunableAt::<T>::remove(now, template_id);
        NextPruneBucket::<T>::put(now);
        let first = winners.first().expect("nonempty winners");
        for id in 0..u64::from(k) {
            JobOrders::<T>::insert(id, &order);
            OrderResults::<T>::insert(id, &result);
            OrderTopSolvers::<T>::insert(id, &ranking);
            OrderFrontRunner::<T>::insert(
                id,
                types::FrontRunner {
                    solver: first.clone(),
                    energy_milli: -1,
                },
            );
            OrderPayeeRequirements::<T>::insert(id, 20033);
            OpenOrders::<T>::insert(id, ());
            OrdersPrunableAt::<T>::insert(now, id, ());
            for winner in &winners {
                OrderPayees::<T>::insert(
                    id,
                    winner,
                    types::WinnerPayee::V1 {
                        chain_id: 20033,
                        address: sp_core::H160::repeat_byte(255),
                    },
                );
                OrderSolutions::<T>::insert(
                    id,
                    winner,
                    types::JobSolution {
                        solver: winner.clone(),
                        solver_type: types::MinerType::Cpu,
                        solutions: solutions_with_dimensions::<T>(
                            T::MaxNodes::get(),
                            T::MaxSolutions::get(),
                        ),
                        best_energy_milli: -1,
                        diversity_milli: 0,
                        num_valid: T::MaxSolutions::get(),
                        submitted_at: order.created_at,
                    },
                );
            }
        }
        frame_system::Pallet::<T>::set_block_number(now);
        #[block]
        {
            QuantumComputeMempool::<T>::prune_closed_orders(now);
        }
        for id in 0..u64::from(k) {
            assert!(!JobOrders::<T>::contains_key(id));
            assert!(!OrderResults::<T>::contains_key(id));
            assert!(!OrderTopSolvers::<T>::contains_key(id));
            assert!(!OrderFrontRunner::<T>::contains_key(id));
            assert!(!OrderPayeeRequirements::<T>::contains_key(id));
            assert!(!OpenOrders::<T>::contains_key(id));
            assert!(OrderPayees::<T>::iter_key_prefix(id).next().is_none());
            assert!(OrderSolutions::<T>::iter_key_prefix(id).next().is_none());
        }
        assert!(!NextPruneBucket::<T>::exists());
    }

    impl_benchmark_test_suite!(
        QuantumComputeMempool,
        crate::mock::new_test_ext(),
        crate::mock::Test
    );
}

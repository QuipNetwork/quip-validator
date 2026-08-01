//! Benchmarking setup for pallet-quantum-compute-mempool.

use super::*;

#[allow(unused)]
use crate::Pallet as QuantumComputeMempool;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
    pallet_prelude::ConstU32,
    traits::{Currency, Get},
    BoundedVec,
};
use frame_system::RawOrigin;
use sp_runtime::traits::{Hash as _, SaturatedConversion, Saturating};

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
        min_energy_milli: None,
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
        min_energy_milli: None,
        min_diversity_milli: None,
        min_solutions,
    }
}

fn solutions_with_dimensions<T: Config>(node_count: u32, solution_count: u32) -> SolutionsOf<T> {
    bounded(
        (0..solution_count)
            .map(|solution_index| {
                bounded(
                    (0..node_count)
                        .map(|node_index| {
                            if (node_index + solution_index) % 3 == 0 {
                                -1
                            } else {
                                1
                            }
                        })
                        .collect(),
                )
            })
            .collect(),
    )
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

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn register_solver() {
        let caller: T::AccountId = whitelisted_caller();

        #[extrinsic_call]
        QuantumComputeMempool::register_solver(
            RawOrigin::Signed(caller.clone()),
            types::MinerType::Cpu,
        );

        assert!(Solvers::<T>::contains_key(caller));
    }

    #[benchmark]
    fn deregister_solver() {
        let caller: T::AccountId = whitelisted_caller();
        register_solver_for::<T>(&caller);

        #[extrinsic_call]
        QuantumComputeMempool::deregister_solver(RawOrigin::Signed(caller.clone()));

        assert!(!Solvers::<T>::contains_key(caller));
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
        t: Linear<0, 8>,
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
        let min_solutions = if s > 1 { s - 1 } else { 1 };
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
    fn claim_reward() {
        let proposer: T::AccountId = whitelisted_caller();
        let winner_count = types::MAX_REWARD_WINNERS;
        let reward = benchmark_reward::<T>().saturating_mul(winner_count.saturated_into());
        let winners: Vec<T::AccountId> = (0..winner_count)
            .map(|index| account("winner", index, 0))
            .collect();
        for winner in &winners {
            register_solver_for::<T>(winner);
        }
        let caller = winners.first().expect("winner set is nonempty").clone();
        let order_id = propose_order_for::<T>(
            &proposer,
            sample_params::<T>(),
            reward,
            types::JobMode::Bid {
                miners: Some(bounded(vec![caller.clone()])),
                miner_types: None,
            },
            types::RewardResolution::TopNEqual { n: winner_count },
            types::ResultDelivery::CallbackWithPoll {
                endpoint: bounded(b"https://solver.example/result".to_vec()),
            },
        );
        let ranked: TopSolversOf<T> = bounded(
            winners
                .iter()
                .enumerate()
                .map(|(index, winner)| types::RankedSolver {
                    solver: winner.clone(),
                    energy_milli: -(index as i64) - 1,
                })
                .collect(),
        );
        OrderTopSolvers::<T>::insert(order_id, ranked);
        JobOrders::<T>::mutate(order_id, |maybe_order| {
            let order = maybe_order.as_mut().expect("benchmark order exists");
            order.solution_count = winner_count;
            order.status = types::OrderStatus::Expired;
        });

        #[extrinsic_call]
        QuantumComputeMempool::claim_reward(RawOrigin::Signed(caller), order_id);

        let order = JobOrders::<T>::get(order_id).expect("order exists");
        assert_eq!(order.status, types::OrderStatus::Closed);
        assert_eq!(
            OrderResults::<T>::get(order_id)
                .expect("poll result is stored")
                .winners
                .len(),
            winner_count as usize
        );
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
        frame_system::Pallet::<T>::set_block_number(2u32.into());

        #[extrinsic_call]
        QuantumComputeMempool::reclaim_order(RawOrigin::Signed(proposer.clone()), order_id);

        let order = JobOrders::<T>::get(order_id).expect("order exists");
        assert_eq!(order.status, types::OrderStatus::Closed);
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
        assert!(QuantumComputeMempool::<T>::claim_reward(
            RawOrigin::Signed(solver.clone()).into(),
            order_id,
        )
        .is_ok());
        frame_system::Pallet::<T>::set_block_number(
            T::ResultTtlBlocks::get().saturating_add(2u32.into()),
        );

        #[extrinsic_call]
        QuantumComputeMempool::purge_result(RawOrigin::Signed(cleaner), order_id);

        assert!(OrderResults::<T>::get(order_id).is_none());
    }

    impl_benchmark_test_suite!(
        QuantumComputeMempool,
        crate::mock::new_test_ext(),
        crate::mock::Test
    );
}

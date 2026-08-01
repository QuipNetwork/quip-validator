use super::mock::*;
use crate::types::{Formulation, JobMode, MinerType, ResultDelivery, RewardResolution};
use crate::{
    Event as QuantumComputeMempoolEvent, JobOrders, JobSpecs, NextOrderId, OpenOrders,
    OrderFrontRunner, Solvers,
};
use frame_support::{
    assert_noop, assert_ok,
    traits::{Hooks, StorageVersion},
    BoundedVec,
};
use sp_runtime::traits::Hash;

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: frame_support::traits::Get<u32>,
{
    items.try_into().ok().unwrap()
}

fn sample_spec() -> (
    BoundedVec<u8, frame_support::traits::ConstU32<128>>,
    Formulation,
    Option<<Test as frame_system::Config>::Hash>,
    Option<<Test as frame_system::Config>::Hash>,
) {
    (bounded(b"max-cut".to_vec()), Formulation::Ising, None, None)
}

fn sample_params() -> crate::IsingParamsOf<Test> {
    crate::types::IsingParams {
        nodes: bounded(vec![0, 1]),
        edges: bounded(vec![(0, 1)]),
        h_values: bounded(vec![0, 0]),
        j_values: bounded(vec![-1_000]),
        min_energy_milli: None,
        min_diversity_milli: None,
        min_solutions: None,
    }
}

fn sample_solution() -> crate::SolutionsOf<Test> {
    bounded(vec![bounded(vec![1, 1])])
}

fn alternate_solution() -> crate::SolutionsOf<Test> {
    bounded(vec![bounded(vec![1, -1])])
}

fn weighted_params() -> crate::IsingParamsOf<Test> {
    crate::types::IsingParams {
        nodes: bounded(vec![0, 1]),
        edges: bounded(vec![(0, 1)]),
        h_values: bounded(vec![-500, 0]),
        j_values: bounded(vec![-1_000]),
        min_energy_milli: None,
        min_diversity_milli: None,
        min_solutions: None,
    }
}

fn register_spec() -> <Test as frame_system::Config>::Hash {
    let (name, formulation, validation_program, transform_program) = sample_spec();
    assert_ok!(QuantumComputeMempool::register_job_spec(
        RuntimeOrigin::root(),
        1,
        name.clone(),
        formulation,
        validation_program,
        transform_program
    ));
    <Test as frame_system::Config>::Hashing::hash_of(&(
        name,
        formulation,
        validation_program,
        transform_program,
    ))
}

#[test]
fn default_ising_spec_id_is_pinned() {
    new_test_ext().execute_with(|| {
        let spec_id = QuantumComputeMempool::default_ising_spec_id();
        assert_eq!(
            format!("{spec_id:?}"),
            "0x8f46f3a31321d1d093314fc769c42cbe7a83d71a0b69e6571a0f68e2a04067f0",
        );
    });
}

#[test]
fn register_solver_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));

        let solver = Solvers::<Test>::get(2).unwrap();
        assert_eq!(solver.solver_type, MinerType::Cpu);
    });
}

#[test]
fn register_job_spec_invokes_vm_validate_programs() {
    new_test_ext().execute_with(|| {
        let (name, formulation, validation_program, transform_program) = sample_spec();

        assert_ok!(QuantumComputeMempool::register_job_spec(
            RuntimeOrigin::root(),
            1,
            name,
            formulation,
            validation_program,
            transform_program,
        ));

        assert_eq!(mock_vm_state().validate_programs_calls, 1);
    });
}

#[test]
fn signed_register_job_spec_is_rejected() {
    new_test_ext().execute_with(|| {
        let (name, formulation, validation_program, transform_program) = sample_spec();

        assert_noop!(
            QuantumComputeMempool::register_job_spec(
                RuntimeOrigin::signed(1),
                1,
                name,
                formulation,
                validation_program,
                transform_program,
            ),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn register_job_spec_propagates_vm_validation_error() {
    new_test_ext().execute_with(|| {
        set_vm_fail_validate_programs(true);
        let (name, formulation, validation_program, transform_program) = sample_spec();

        assert_noop!(
            QuantumComputeMempool::register_job_spec(
                RuntimeOrigin::root(),
                1,
                name,
                formulation,
                validation_program,
                transform_program,
            ),
            sp_runtime::DispatchError::Other("vm-validate-programs")
        );
    });
}

#[test]
fn genesis_seeds_default_ising_spec() {
    new_test_ext_with_default_ising_spec(true).execute_with(|| {
        let spec_id = QuantumComputeMempool::default_ising_spec_id();
        let spec = JobSpecs::<Test>::get(spec_id).unwrap();
        assert_eq!(spec.builder, 1);
        assert_eq!(spec.name, QuantumComputeMempool::default_ising_spec_name());
        assert_eq!(spec.formulation, Formulation::Ising);
        assert_eq!(spec.validation_program, None);
        assert_eq!(spec.transform_program, None);
    });
}

#[test]
fn propose_job_works_with_genesis_default_ising_spec() {
    new_test_ext_with_default_ising_spec(true).execute_with(|| {
        let spec_id = QuantumComputeMempool::default_ising_spec_id();

        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.spec_id, spec_id);
        assert_eq!(order.proposer, 1);
    });
}

#[test]
fn propose_job_rejects_positive_diversity_with_explicitly_fewer_than_two_solutions() {
    for min_solutions in [0, 1] {
        new_test_ext().execute_with(|| {
            let spec_id = register_spec();
            let mut params = sample_params();
            params.min_diversity_milli = Some(1);
            params.min_solutions = Some(min_solutions);

            assert_noop!(
                QuantumComputeMempool::propose_job(
                    RuntimeOrigin::signed(1),
                    spec_id,
                    params,
                    100,
                    JobMode::Open,
                    RewardResolution::SingleBest,
                    10,
                    5,
                    ResultDelivery::OnChainOnly,
                ),
                crate::Error::<Test>::InvalidDiversityConfig
            );
            assert_eq!(NextOrderId::<Test>::get(), 0);
        });
    }
}

#[test]
fn propose_job_allows_positive_diversity_without_explicit_solution_minimum() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let mut params = sample_params();
        params.min_diversity_milli = Some(500);

        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            params,
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));
        assert_eq!(
            JobOrders::<Test>::get(0)
                .expect("order was stored")
                .ising_params
                .min_solutions,
            None
        );
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            bounded(vec![bounded(vec![1, 1]), bounded(vec![1, -1])]),
        ));
    });
}

#[test]
fn proposed_jobs_are_discoverable_through_open_order_index() {
    new_test_ext().execute_with(|| {
        let spec_id = register_spec();

        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));

        assert!(OpenOrders::<Test>::contains_key(0));
        assert_eq!(QuantumComputeMempool::open_order_ids(None, 10), vec![0]);
        assert_eq!(
            QuantumComputeMempool::open_order_ids(Some(0), 10),
            Vec::<u64>::new()
        );
        assert_eq!(QuantumComputeMempool::job_order(0).unwrap().proposer, 1);
    });
}

#[test]
fn open_order_ids_filter_lazily_expired_orders() {
    new_test_ext().execute_with(|| {
        let spec_id = register_spec();

        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            1,
            1,
            ResultDelivery::OnChainOnly,
        ));
        assert!(OpenOrders::<Test>::contains_key(0));

        System::set_block_number(2);

        assert!(OpenOrders::<Test>::contains_key(0));
        assert_eq!(
            QuantumComputeMempool::open_order_ids(None, 10),
            Vec::<u64>::new()
        );
    });
}

#[test]
fn network_upgrade_wipes_mempool_and_reseeds_default_spec() {
    new_test_ext().execute_with(|| {
        // v0.2-style state: a registered (non-default) spec plus an order id.
        let spec_id = register_spec();
        assert!(JobSpecs::<Test>::contains_key(spec_id));
        NextOrderId::<Test>::put(7);
        // Simulate the on-chain v0.2 storage version so the guard fires.
        StorageVersion::new(1).put::<QuantumComputeMempool>();

        QuantumComputeMempool::on_runtime_upgrade();

        // Old data is dropped, not carried forward...
        assert!(
            !JobSpecs::<Test>::contains_key(spec_id),
            "v0.2 job specs must be wiped"
        );
        assert_eq!(NextOrderId::<Test>::get(), 0, "order counter must reset");
        // ...and only the canonical default Ising spec is reseeded.
        let default_id = QuantumComputeMempool::default_ising_spec_id();
        assert!(
            JobSpecs::<Test>::contains_key(default_id),
            "default Ising spec must be reseeded"
        );
        assert_eq!(
            StorageVersion::get::<QuantumComputeMempool>(),
            StorageVersion::new(2)
        );
    });
}

#[test]
fn network_upgrade_is_noop_once_at_v2() {
    new_test_ext_with_default_ising_spec(true).execute_with(|| {
        // Fresh chain already at the post-upgrade version: the guard must
        // short-circuit so live state is neither wiped nor re-seeded.
        StorageVersion::new(2).put::<QuantumComputeMempool>();
        let spec_id = QuantumComputeMempool::default_ising_spec_id();
        let pre = JobSpecs::<Test>::get(spec_id).unwrap();

        QuantumComputeMempool::on_runtime_upgrade();

        // The version guard short-circuits: nothing is wiped or re-seeded.
        let post = JobSpecs::<Test>::get(spec_id).unwrap();
        assert_eq!(post.builder, pre.builder);
        assert_eq!(post.registered_at, pre.registered_at);
        assert_eq!(
            StorageVersion::get::<QuantumComputeMempool>(),
            StorageVersion::new(2)
        );
    });
}

#[test]
fn root_register_job_spec_emits_event_with_provided_builder() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let (name, formulation, validation_program, transform_program) = sample_spec();
        let spec_id = <Test as frame_system::Config>::Hashing::hash_of(&(
            name.clone(),
            formulation,
            validation_program,
            transform_program,
        ));

        assert_ok!(QuantumComputeMempool::register_job_spec(
            RuntimeOrigin::root(),
            42,
            name,
            formulation,
            validation_program,
            transform_program,
        ));

        let found = System::events().into_iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::QuantumComputeMempool(
                    QuantumComputeMempoolEvent::JobSpecRegistered { spec_id: emitted_id, builder },
                ) if emitted_id == spec_id && builder == 42,
            )
        });
        assert!(found, "root register_job_spec must emit JobSpecRegistered");
    });
}

#[test]
fn network_upgrade_does_not_emit_job_spec_registered_event() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        StorageVersion::new(1).put::<QuantumComputeMempool>();

        let _ = QuantumComputeMempool::on_runtime_upgrade();

        let emitted = System::events().into_iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::QuantumComputeMempool(
                    QuantumComputeMempoolEvent::JobSpecRegistered { .. },
                ),
            )
        });
        assert!(
            !emitted,
            "network-upgrade reseed must be silent (no JobSpecRegistered)",
        );
    });
}

#[test]
fn propose_job_stores_order_and_reserves_reward() {
    new_test_ext().execute_with(|| {
        let spec_id = register_spec();

        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.reward, 100);
        assert_eq!(pallet_balances::Pallet::<Test>::reserved_balance(1), 100);
    });
}

#[test]
fn submit_solution_tracks_front_runner_and_first_solution() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));

        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.first_solution_at, Some(1));
        let leader = OrderFrontRunner::<Test>::get(0).unwrap();
        assert_eq!(leader.solver, 2);
        assert_eq!(leader.energy_milli, -1_000);
    });
}

#[test]
fn submit_solution_uses_vm_transformed_solutions_for_scoring() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));
        set_vm_transformed_solutions(vec![vec![1, 1]]);

        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            alternate_solution(),
        ));

        let leader = OrderFrontRunner::<Test>::get(0).unwrap();
        assert_eq!(leader.energy_milli, -1_000);
        assert_eq!(mock_vm_state().transform_solutions_calls, 1);
    });
}

#[test]
fn submit_solution_propagates_vm_transform_error() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));
        set_vm_fail_transform_solutions(true);

        assert_noop!(
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution()),
            sp_runtime::DispatchError::Other("vm-transform-solutions")
        );
    });
}

#[test]
fn submit_solution_rejects_ineligible_bid_solver() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Bid {
                miners: Some(bounded(vec![3])),
                miner_types: None,
            },
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));

        assert_noop!(
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution()),
            crate::Error::<Test>::NotEligibleSolver
        );
    });
}

#[test]
fn submit_solution_rejects_invalid_spin_values() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));

        let bad_solution = bounded(vec![bounded(vec![1, 0])]);
        assert_noop!(
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, bad_solution),
            crate::Error::<Test>::InvalidSpinValues
        );
    });
}

#[test]
fn claim_reward_rejects_before_expiry() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(2), 0),
            crate::Error::<Test>::OrderNotExpired
        );
    });
}

#[test]
fn claim_reward_single_best_transfers_and_closes_order() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.status, crate::types::OrderStatus::Closed);
        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(1), 999_900);
        assert_eq!(Balances::free_balance(2), 1_000_100);
    });
}

#[test]
fn claim_reward_invokes_vm_validate_result() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        let state = mock_vm_state();
        assert_eq!(state.validate_result_calls, 1);
        assert_eq!(state.last_result_winner_count, Some(1));
    });
}

#[test]
fn claim_reward_propagates_vm_result_validation_error() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));
        set_vm_fail_validate_result(true);

        System::set_block_number(2);
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(2), 0),
            sp_runtime::DispatchError::Other("vm-validate-result")
        );

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.status, crate::types::OrderStatus::Opened);
        assert_eq!(Balances::reserved_balance(1), 100);
    });
}

#[test]
fn reclaim_order_returns_reserved_reward_when_no_solutions() {
    new_test_ext().execute_with(|| {
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            1,
            1,
            ResultDelivery::OnChainOnly,
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::reclaim_order(
            RuntimeOrigin::signed(1),
            0
        ));

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.status, crate::types::OrderStatus::Closed);
        assert!(!OpenOrders::<Test>::contains_key(0));
        assert_eq!(
            QuantumComputeMempool::open_order_ids(None, 10),
            Vec::<u64>::new()
        );
        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(1), 1_000_000);
    });
}

#[test]
fn claim_reward_top_n_equal_splits_remainder_to_best() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(3),
            MinerType::Gpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            101,
            JobMode::Open,
            RewardResolution::TopNEqual { n: 2 },
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));

        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(3),
            0,
            alternate_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(2), 1_000_051);
        assert_eq!(Balances::free_balance(3), 1_000_050);
    });
}

#[test]
fn claim_reward_top_n_weighted_splits_by_absolute_energy() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(3),
            MinerType::Gpu
        ));
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            weighted_params(),
            100,
            JobMode::Open,
            RewardResolution::TopNWeighted { n: 2 },
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));

        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(3),
            0,
            alternate_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(2), 1_000_075);
        assert_eq!(Balances::free_balance(3), 1_000_025);
    });
}

#[test]
fn claim_reward_emits_result_ready_for_callback_delivery() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let endpoint = bounded::<u8, frame_support::traits::ConstU32<256>>(
            b"https://solver.example/callback".to_vec(),
        );
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::Callback {
                endpoint: endpoint.clone(),
            },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        let found = System::events().into_iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::QuantumComputeMempool(
                    QuantumComputeMempoolEvent::ResultReady {
                        order_id: 0,
                        winners: ref summaries,
                        endpoint: ref emitted,
                    }
                ) if emitted == &endpoint
                    && summaries.len() == 1
                    && summaries[0].solver == 2
                    && summaries[0].energy_milli == -1_000
                    && summaries[0].amount == 100
            )
        });

        assert!(found);
    });
}

#[test]
fn claim_reward_emits_all_winners_for_callback_delivery() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(3),
            MinerType::Gpu
        ));
        let spec_id = register_spec();
        let endpoint = bounded::<u8, frame_support::traits::ConstU32<256>>(
            b"https://solver.example/callback".to_vec(),
        );
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            101,
            JobMode::Open,
            RewardResolution::TopNEqual { n: 2 },
            2,
            1,
            ResultDelivery::Callback {
                endpoint: endpoint.clone(),
            },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(3),
            0,
            alternate_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        let found = System::events().into_iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::QuantumComputeMempool(
                    QuantumComputeMempoolEvent::ResultReady {
                        order_id: 0,
                        winners: ref summaries,
                        endpoint: ref emitted,
                    }
                ) if emitted == &endpoint
                    && summaries.len() == 2
                    && summaries[0].solver == 2
                    && summaries[0].energy_milli == -1_000
                    && summaries[0].amount == 51
                    && summaries[1].solver == 3
                    && summaries[1].energy_milli == 1_000
                    && summaries[1].amount == 50
            )
        });

        assert!(found);
    });
}

#[test]
fn claim_reward_persists_result_for_callback_with_poll() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let endpoint = bounded::<u8, frame_support::traits::ConstU32<256>>(
            b"https://solver.example/poll".to_vec(),
        );
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Bid {
                miners: Some(bounded(vec![2])),
                miner_types: None,
            },
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::CallbackWithPoll {
                endpoint: endpoint.clone(),
            },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        let result = QuantumComputeMempool::result_for_order(0).unwrap();
        assert_eq!(result.endpoint, endpoint);
        assert_eq!(result.resolution, RewardResolution::SingleBest);
        assert_eq!(result.settled_at, 2);
        assert_eq!(result.winners.len(), 1);
        assert_eq!(result.winners[0].solver, 2);
        assert_eq!(result.winners[0].energy_milli, -1_000);
        assert_eq!(result.winners[0].amount, 100);
    });
}

#[test]
fn claim_reward_does_not_persist_result_for_callback_only() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let endpoint = bounded::<u8, frame_support::traits::ConstU32<256>>(
            b"https://solver.example/callback".to_vec(),
        );
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::Callback { endpoint },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        assert!(QuantumComputeMempool::result_for_order(0).is_none());
    });
}

#[test]
fn purge_result_rejects_before_ttl() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let endpoint = bounded::<u8, frame_support::traits::ConstU32<256>>(
            b"https://solver.example/poll".to_vec(),
        );
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Bid {
                miners: Some(bounded(vec![2])),
                miner_types: None,
            },
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::CallbackWithPoll { endpoint },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        assert_noop!(
            QuantumComputeMempool::purge_result(RuntimeOrigin::signed(3), 0),
            crate::Error::<Test>::ResultTtlNotElapsed
        );
    });
}

#[test]
fn purge_result_is_permissionless_after_ttl() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let endpoint = bounded::<u8, frame_support::traits::ConstU32<256>>(
            b"https://solver.example/poll".to_vec(),
        );
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Bid {
                miners: Some(bounded(vec![2])),
                miner_types: None,
            },
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::CallbackWithPoll { endpoint },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));

        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        System::set_block_number(12);
        assert_ok!(QuantumComputeMempool::purge_result(
            RuntimeOrigin::signed(4),
            0
        ));

        assert!(QuantumComputeMempool::result_for_order(0).is_none());
    });
}

#[test]
fn propose_job_dispatch_info_uses_call_dimensions() {
    use crate::weights::WeightInfo;
    use frame_support::dispatch::GetDispatchInfo;

    new_test_ext().execute_with(|| {
        let mode = JobMode::Bid {
            miners: Some(bounded(vec![2, 3])),
            miner_types: Some(bounded(vec![
                MinerType::Cpu,
                MinerType::Gpu,
                MinerType::QpuDwave,
            ])),
        };
        let call = crate::Call::<Test>::propose_job {
            spec_id: QuantumComputeMempool::default_ising_spec_id(),
            ising_params: sample_params(),
            reward: MinReward::get(),
            mode,
            resolution: RewardResolution::SingleBest,
            deadline_blocks: 2,
            block_wait: 1,
            delivery: ResultDelivery::OnChainOnly,
        };

        assert_eq!(
            call.get_dispatch_info().call_weight,
            <() as WeightInfo>::propose_job(2, 1, 2, 3),
        );
    });
}

#[test]
fn submit_solution_dispatch_info_charges_configured_maxima() {
    use crate::weights::WeightInfo;
    use frame_support::dispatch::GetDispatchInfo;

    new_test_ext().execute_with(|| {
        let call = crate::Call::<Test>::submit_solution {
            order_id: 0,
            solutions: sample_solution(),
        };

        assert_eq!(
            call.get_dispatch_info().call_weight,
            <() as WeightInfo>::submit_solution(
                MaxNodes::get(),
                MaxEdges::get(),
                MaxSolutions::get(),
            ),
        );
    });
}

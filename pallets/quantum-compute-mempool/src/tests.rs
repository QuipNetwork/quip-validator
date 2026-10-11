use super::mock::*;
use crate::types::{
    Formulation, JobMode, MinerType, OrderStatus, ResultDelivery, RewardResolution,
};
use crate::{
    Event as QuantumComputeMempoolEvent, JobOrders, JobSpecs, NextExpiryBucket, NextOrderId,
    OpenOrders, OrderFrontRunner, OrderSolutions, OrdersExpiringAt, Solvers,
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
        max_energy_milli: None,
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
        max_energy_milli: None,
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
            StorageVersion::new(3)
        );
    });
}

#[test]
fn network_upgrade_is_noop_once_at_v3() {
    new_test_ext_with_default_ising_spec(true).execute_with(|| {
        // Fresh chain already at the post-upgrade version: the guard must
        // short-circuit so live state is neither wiped nor re-seeded.
        StorageVersion::new(3).put::<QuantumComputeMempool>();
        let spec_id = QuantumComputeMempool::default_ising_spec_id();
        let pre = JobSpecs::<Test>::get(spec_id).unwrap();

        QuantumComputeMempool::on_runtime_upgrade();

        // The version guard short-circuits: nothing is wiped or re-seeded.
        let post = JobSpecs::<Test>::get(spec_id).unwrap();
        assert_eq!(post.builder, pre.builder);
        assert_eq!(post.registered_at, pre.registered_at);
        assert_eq!(
            StorageVersion::get::<QuantumComputeMempool>(),
            StorageVersion::new(3)
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
fn claim_reward_rejects_unsigned_and_missing_order() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::none(), 0),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
            crate::Error::<Test>::OrderNotFound
        );
    });
}

#[test]
fn claim_reward_rejects_unsolved_expired_order() {
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
        run_to_block(2);
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
            crate::Error::<Test>::NoSolutionsAccepted
        );
        assert_eq!(
            JobOrders::<Test>::get(0).unwrap().status,
            crate::types::OrderStatus::Expired
        );
        assert!(!OpenOrders::<Test>::contains_key(0));
        assert_eq!(Balances::reserved_balance(1), 100);
    });
}

#[test]
fn claim_reward_later_transfer_failure_rolls_back_entire_settlement() {
    use frame_support::traits::{LockableCurrency, WithdrawReasons};

    new_test_ext().execute_with(|| {
        for solver in [2, 3] {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(solver),
                MinerType::Cpu
            ));
        }
        let spec_id = register_spec();
        let endpoint = bounded(b"https://solver.example/poll".to_vec());
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            101,
            JobMode::Bid {
                miners: Some(bounded(vec![2, 3])),
                miner_types: None
            },
            RewardResolution::TopNEqual { n: 2 },
            2,
            1,
            ResultDelivery::CallbackWithPoll { endpoint },
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution()
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(3),
            0,
            alternate_solution()
        ));
        // Unreserving restores 1,000,000. The first payout (51) fits within
        // the 75 transferable units; the second payout (50) then fails.
        Balances::set_lock(*b"settle!!", &1, 999_925, WithdrawReasons::TRANSFER);
        run_to_block(2);
        let result_before = crate::OrderResults::<Test>::get(0).expect("hook published result");
        let index_before = crate::ProposerOrders::<Test>::get(1);
        let events_before = System::events();
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
            sp_runtime::TokenError::Frozen
        );
        // assert_noop also checks the entire storage root, including the
        // first transfer and any events emitted by the failed settlement.
        // Hook expiry and its prepublished result remain committed.
        assert_eq!(Balances::free_balance(1), 999_899);
        assert_eq!(Balances::reserved_balance(1), 101);
        assert_eq!(Balances::free_balance(2), 1_000_000);
        assert_eq!(Balances::free_balance(3), 1_000_000);
        assert_eq!(Balances::free_balance(4), 1_000_000);
        for solver in [2, 3] {
            assert_eq!(Solvers::<Test>::get(solver).unwrap().rewards_earned, 0);
        }
        assert_eq!(JobSpecs::<Test>::get(spec_id).unwrap().successful_orders, 0);
        assert_eq!(
            JobOrders::<Test>::get(0).unwrap().status,
            crate::types::OrderStatus::Expired
        );
        assert!(!OpenOrders::<Test>::contains_key(0));
        assert_eq!(crate::OrderResults::<Test>::get(0), Some(result_before));
        assert_eq!(System::events(), events_before);
        assert_eq!(crate::ProposerOrders::<Test>::get(1), index_before);

        Balances::remove_lock(*b"settle!!", &1);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(Balances::free_balance(2), 1_000_051);
        assert_eq!(Balances::free_balance(3), 1_000_050);
        assert_eq!(Solvers::<Test>::get(2).unwrap().rewards_earned, 51);
        assert_eq!(Solvers::<Test>::get(3).unwrap().rewards_earned, 50);
        assert_eq!(JobSpecs::<Test>::get(spec_id).unwrap().successful_orders, 1);
        assert_eq!(
            crate::OrderResults::<Test>::get(0).unwrap().winners.len(),
            2
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
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
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

        run_to_block(2);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        // Direct pallet calls do not charge transaction fees; the caller gets no reward.
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.status, crate::types::OrderStatus::Closed);
        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(1), 999_900);
        assert_eq!(Balances::free_balance(2), 1_000_100);
        assert_eq!(Solvers::<Test>::get(2).unwrap().rewards_earned, 100);
        assert_eq!(JobSpecs::<Test>::get(spec_id).unwrap().successful_orders, 1);
        assert!(!OpenOrders::<Test>::contains_key(0));
        assert!(!crate::OrderResults::<Test>::contains_key(0));
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
            crate::Error::<Test>::AlreadyClaimed
        );
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

        run_to_block(2);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        // Direct pallet calls do not charge transaction fees; the caller gets no reward.
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));

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

        run_to_block(2);
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
            sp_runtime::DispatchError::Other("vm-validate-result")
        );

        let order = JobOrders::<Test>::get(0).unwrap();
        assert_eq!(order.status, crate::types::OrderStatus::Expired);
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

        run_to_block(2);
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

        run_to_block(2);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        // Direct pallet calls do not charge transaction fees; the caller gets no reward.
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));

        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(2), 1_000_051);
        assert_eq!(Balances::free_balance(3), 1_000_050);
    });
}

#[test]
fn claim_reward_top_n_weighted_splits_by_rank() {
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

        run_to_block(2);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        // Direct pallet calls do not charge transaction fees; the caller gets no reward.
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));

        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(2), 1_000_067);
        assert_eq!(Balances::free_balance(3), 1_000_033);
    });
}

#[test]
fn expiry_emits_result_ready_for_callback_delivery() {
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

        run_to_block(2);

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

        let result_before = crate::OrderResults::<Test>::get(0);
        System::reset_events();
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_eq!(crate::OrderResults::<Test>::get(0), result_before);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
    });
}

#[test]
fn expiry_emits_all_winners_for_callback_delivery() {
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

        run_to_block(2);

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

        let result_before = crate::OrderResults::<Test>::get(0);
        System::reset_events();
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_eq!(crate::OrderResults::<Test>::get(0), result_before);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
    });
}

#[test]
fn expiry_persists_result_for_callback_with_poll() {
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

        run_to_block(2);

        let result = QuantumComputeMempool::result_for_order(0).unwrap();
        assert_eq!(result.endpoint, endpoint);
        assert_eq!(result.resolution, RewardResolution::SingleBest);
        assert_eq!(result.settled_at, 2);
        assert_eq!(result.winners.len(), 1);
        assert_eq!(result.winners[0].solver, 2);
        assert_eq!(result.winners[0].energy_milli, -1_000);
        assert_eq!(result.winners[0].amount, 100);

        let result_before = crate::OrderResults::<Test>::get(0);
        System::reset_events();
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_eq!(crate::OrderResults::<Test>::get(0), result_before);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
    });
}

#[test]
fn expiry_does_not_persist_result_for_callback_only() {
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

        run_to_block(2);

        assert!(QuantumComputeMempool::result_for_order(0).is_none());

        let result_before = crate::OrderResults::<Test>::get(0);
        System::reset_events();
        assert!(!Solvers::<Test>::contains_key(4));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(Balances::free_balance(4), 1_000_000);
        assert!(!Solvers::<Test>::contains_key(4));
        assert_eq!(crate::OrderResults::<Test>::get(0), result_before);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
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

        run_to_block(2);
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

        run_to_block(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        run_to_block(12);
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

#[test]
fn submit_solution_drops_vectors_above_max_energy() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let mut params = sample_params();
        // sample_solution() scores -1_000; alternate_solution() scores +1_000.
        params.max_energy_milli = Some(0);
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            params,
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            5,
            2,
            ResultDelivery::OnChainOnly,
        ));

        assert_noop!(
            QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                alternate_solution(),
            ),
            crate::Error::<Test>::InsufficientEnergy
        );
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));
    });
}

#[test]
fn submit_solution_keeps_vectors_at_the_energy_ceiling() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let mut params = sample_params();
        params.max_energy_milli = Some(-1_000);
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            params,
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            5,
            2,
            ResultDelivery::OnChainOnly,
        ));

        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
        ));
        assert_eq!(
            OrderSolutions::<Test>::get(0, 2).unwrap().solutions,
            sample_solution()
        );
    });
}

#[test]
fn submit_solution_keeps_only_vectors_within_the_ceiling_from_a_mixed_submission() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let spec_id = register_spec();
        let mut params = sample_params();
        params.max_energy_milli = Some(0);
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            params,
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            5,
            2,
            ResultDelivery::OnChainOnly,
        ));
        let solutions: crate::SolutionsOf<Test> = bounded(vec![
            alternate_solution()[0].clone(),
            sample_solution()[0].clone(),
        ]);

        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            solutions,
        ));
        assert_eq!(
            OrderSolutions::<Test>::get(0, 2).unwrap().solutions,
            sample_solution()
        );
    });
}

fn propose_single_best(proposer: u64, deadline_blocks: u64, block_wait: u64) -> u64 {
    let spec_id = match JobSpecs::<Test>::iter_keys().next() {
        Some(spec_id) => spec_id,
        None => register_spec(),
    };
    let order_id = NextOrderId::<Test>::get();
    assert_ok!(QuantumComputeMempool::propose_job(
        RuntimeOrigin::signed(proposer),
        spec_id,
        sample_params(),
        100,
        JobMode::Open,
        RewardResolution::SingleBest,
        deadline_blocks,
        block_wait,
        ResultDelivery::OnChainOnly,
    ));
    order_id
}

fn bucket_ids(block: u64) -> Vec<u64> {
    let mut ids: Vec<u64> = OrdersExpiringAt::<Test>::iter_key_prefix(block).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn propose_job_schedules_expiry_at_hard_deadline() {
    new_test_ext().execute_with(|| {
        let order_id = propose_single_best(1, 5, 2);
        assert_eq!(bucket_ids(6), vec![order_id]);
    });
}

#[test]
fn propose_job_with_zero_deadline_schedules_next_block() {
    new_test_ext().execute_with(|| {
        let order_id = propose_single_best(1, 0, 0);
        assert_eq!(bucket_ids(1), Vec::<u64>::new());
        assert_eq!(bucket_ids(2), vec![order_id]);
    });
}

#[test]
fn first_solution_moves_expiry_to_block_wait() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let order_id = propose_single_best(1, 5, 1);
        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            order_id,
            sample_solution(),
        ));
        assert_eq!(bucket_ids(6), Vec::<u64>::new());
        assert_eq!(bucket_ids(3), vec![order_id]);
    });
}

#[test]
fn first_solution_with_zero_block_wait_schedules_next_block() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let order_id = propose_single_best(1, 5, 0);
        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            order_id,
            sample_solution(),
        ));
        assert_eq!(bucket_ids(6), Vec::<u64>::new());
        assert_eq!(bucket_ids(2), Vec::<u64>::new());
        assert_eq!(bucket_ids(3), vec![order_id]);
    });
}

#[test]
fn first_solution_keeps_hard_deadline_when_block_wait_is_not_sooner() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let order_id = propose_single_best(1, 2, 5);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            order_id,
            sample_solution(),
        ));
        assert_eq!(bucket_ids(3), vec![order_id]);
        assert_eq!(OrdersExpiringAt::<Test>::iter().count(), 1);
    });
}

#[test]
fn second_solution_does_not_reschedule() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(3),
            MinerType::Gpu
        ));
        let order_id = propose_single_best(1, 5, 2);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            order_id,
            sample_solution(),
        ));
        System::set_block_number(2);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(3),
            order_id,
            alternate_solution(),
        ));
        assert_eq!(bucket_ids(3), vec![order_id]);
        assert_eq!(OrdersExpiringAt::<Test>::iter().count(), 1);
    });
}

#[test]
fn on_initialize_charges_proof_weight_when_no_order_expires() {
    new_test_ext().execute_with(|| {
        let weight = QuantumComputeMempool::on_initialize(1);
        assert!(weight.proof_size() > 0);
    });
}

#[test]
fn on_initialize_expires_order_at_hard_deadline_without_a_touching_extrinsic() {
    new_test_ext().execute_with(|| {
        let order_id = propose_single_best(1, 2, 5);

        run_to_block(2);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Opened
        );
        assert!(OpenOrders::<Test>::contains_key(order_id));

        run_to_block(3);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Expired
        );
        assert!(!OpenOrders::<Test>::contains_key(order_id));
        assert_eq!(bucket_ids(3), Vec::<u64>::new());
        assert_eq!(NextExpiryBucket::<Test>::get(), 4);
        let expired = System::events().into_iter().any(|record| {
            matches!(
                record.event,
                RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::OrderExpired {
                    order_id: emitted
                }) if emitted == order_id
            )
        });
        assert!(expired);
    });
}

#[test]
fn on_initialize_expires_order_after_block_wait() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let order_id = propose_single_best(1, 10, 1);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            order_id,
            sample_solution(),
        ));

        run_to_block(2);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Expired
        );
    });
}

/// Nine orders expiring in one block with a cap of eight.
fn propose_nine_orders_expiring_at_block_2() -> Vec<u64> {
    let mut ids = Vec::new();
    // MaxOrdersPerProposer is 8 in the mock, so split across two proposers.
    for _ in 0..8 {
        ids.push(propose_single_best(1, 1, 1));
    }
    ids.push(propose_single_best(2, 1, 1));
    ids
}

fn status_count(ids: &[u64], status: OrderStatus) -> usize {
    ids.iter()
        .filter(|id| JobOrders::<Test>::get(**id).unwrap().status == status)
        .count()
}

#[test]
fn on_initialize_finalises_at_most_the_per_block_cap_and_carries_overflow() {
    new_test_ext().execute_with(|| {
        let ids = propose_nine_orders_expiring_at_block_2();
        assert_eq!(bucket_ids(2).len(), 9);

        run_to_block(2);
        assert_eq!(status_count(&ids, OrderStatus::Expired), 8);
        assert_eq!(status_count(&ids, OrderStatus::Opened), 1);
        assert_eq!(bucket_ids(2).len(), 1);
        assert_eq!(NextExpiryBucket::<Test>::get(), 2);

        run_to_block(3);
        assert_eq!(status_count(&ids, OrderStatus::Expired), 9);
        assert_eq!(bucket_ids(2), Vec::<u64>::new());
        assert_eq!(NextExpiryBucket::<Test>::get(), 4);
    });
}

#[test]
fn on_initialize_visits_at_most_eight_buckets_per_block_and_resumes() {
    new_test_ext().execute_with(|| {
        let order_id = propose_single_best(1, 20, 20);
        assert_eq!(bucket_ids(21), vec![order_id]);
        assert_eq!(NextExpiryBucket::<Test>::get(), 0);
        System::set_block_number(21);

        QuantumComputeMempool::on_initialize(21);
        assert_eq!(NextExpiryBucket::<Test>::get(), 8);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Opened
        );

        QuantumComputeMempool::on_initialize(21);
        assert_eq!(NextExpiryBucket::<Test>::get(), 16);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Opened
        );

        QuantumComputeMempool::on_initialize(21);
        assert_eq!(NextExpiryBucket::<Test>::get(), 22);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Expired
        );
    });
}

#[test]
fn reclaim_order_waits_for_finalisation_under_backlog() {
    new_test_ext().execute_with(|| {
        let ids = propose_nine_orders_expiring_at_block_2();
        run_to_block(2);
        let lagging = *ids
            .iter()
            .find(|id| JobOrders::<Test>::get(**id).unwrap().status == OrderStatus::Opened)
            .unwrap();
        let proposer = JobOrders::<Test>::get(lagging).unwrap().proposer;

        assert_noop!(
            QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(proposer), lagging),
            crate::Error::<Test>::OrderNotExpired
        );

        run_to_block(3);
        assert_ok!(QuantumComputeMempool::reclaim_order(
            RuntimeOrigin::signed(proposer),
            lagging
        ));
    });
}

#[test]
fn submit_solution_is_rejected_at_expiry_even_when_finalisation_lags() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(3),
            MinerType::Cpu
        ));
        let ids = propose_nine_orders_expiring_at_block_2();
        run_to_block(2);
        let lagging = *ids
            .iter()
            .find(|id| JobOrders::<Test>::get(**id).unwrap().status == OrderStatus::Opened)
            .unwrap();

        assert_noop!(
            QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(3),
                lagging,
                sample_solution()
            ),
            crate::Error::<Test>::OrderNotOpen
        );
    });
}

#[test]
fn claim_reward_pays_without_emitting_result_ready() {
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
        run_to_block(2);
        System::reset_events();

        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(2),
            0
        ));

        let events: Vec<_> = System::events()
            .into_iter()
            .map(|record| record.event)
            .collect();
        assert!(!events.iter().any(|event| matches!(
            event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::RewardClaimed {
                order_id: 0,
                solver: 2,
                amount: 100,
            })
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::OrderClosed {
                order_id: 0,
                successful: true,
            })
        )));
        assert_eq!(Balances::free_balance(2), 1_000_100);
    });
}

#[test]
fn expiry_without_solutions_emits_no_result_ready() {
    new_test_ext().execute_with(|| {
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
            1,
            1,
            ResultDelivery::CallbackWithPoll { endpoint },
        ));

        run_to_block(2);

        assert_eq!(
            JobOrders::<Test>::get(0).unwrap().status,
            OrderStatus::Expired
        );
        assert!(QuantumComputeMempool::result_for_order(0).is_none());
        assert!(!System::events().into_iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
    });
}

#[test]
fn network_upgrade_v2_to_v3_indexes_open_orders() {
    new_test_ext_with_default_ising_spec(true).execute_with(|| {
        // Two v2-style open orders at block 1: one expiring at block 2, one
        // at block 6. v2 had no expiry index, so clear what propose_job wrote.
        let past = propose_single_best(1, 1, 1);
        let future = propose_single_best(1, 5, 1);
        let _ = OrdersExpiringAt::<Test>::clear(u32::MAX, None);
        NextExpiryBucket::<Test>::kill();
        StorageVersion::new(2).put::<QuantumComputeMempool>();
        System::set_block_number(3);

        QuantumComputeMempool::on_runtime_upgrade();

        assert_eq!(
            StorageVersion::get::<QuantumComputeMempool>(),
            StorageVersion::new(3)
        );
        assert_eq!(NextOrderId::<Test>::get(), 2, "v2 orders are kept");
        assert_eq!(
            bucket_ids(4),
            vec![past],
            "past expiry lands in the next block"
        );
        assert_eq!(bucket_ids(6), vec![future]);
        assert_eq!(NextExpiryBucket::<Test>::get(), 4);
        assert_eq!(
            JobOrders::<Test>::get(past).unwrap().status,
            OrderStatus::Opened
        );

        run_to_block(4);
        assert_eq!(
            JobOrders::<Test>::get(past).unwrap().status,
            OrderStatus::Expired
        );
        assert_eq!(
            JobOrders::<Test>::get(future).unwrap().status,
            OrderStatus::Opened
        );
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn post_upgrade_rejects_an_open_order_missing_from_its_bucket() {
    new_test_ext_with_default_ising_spec(true).execute_with(|| {
        StorageVersion::new(2).put::<QuantumComputeMempool>();
        let order_id = propose_single_best(1, 1, 1);
        let _ = OrdersExpiringAt::<Test>::clear(u32::MAX, None);
        NextExpiryBucket::<Test>::kill();
        System::set_block_number(3);
        let state = QuantumComputeMempool::pre_upgrade().unwrap();

        QuantumComputeMempool::on_runtime_upgrade();
        assert!(OrdersExpiringAt::<Test>::contains_key(4, order_id));
        OrdersExpiringAt::<Test>::remove(4, order_id);

        assert!(QuantumComputeMempool::post_upgrade(state).is_err());
    });
}

#[test]
fn settlement_wrapper_preserves_reference_time_and_floors_both_weight_providers() {
    use crate::{benchmark_weights, weights};
    use frame_support::{traits::Get, weights::constants::RocksDbWeight};
    use weights::WeightInfo;

    let generated = <() as benchmark_weights::WeightInfo>::claim_reward();
    let generic =
        <benchmark_weights::SubstrateWeight<Test> as benchmark_weights::WeightInfo>::claim_reward();
    let test_db: frame_support::weights::RuntimeDbWeight =
        <Test as frame_system::Config>::DbWeight::get();
    // Both generated implementations must have the same measured base once
    // the modeled 69/69 DB accesses are removed. Reference regeneration may
    // replace that base without changing this accounting contract.
    assert_eq!(
        generated.ref_time() - RocksDbWeight::get().reads_writes(69, 69).ref_time(),
        generic.ref_time() - test_db.reads_writes(69, 69).ref_time(),
    );
    assert_eq!(
        <() as WeightInfo>::claim_reward().ref_time(),
        generated.ref_time()
    );
    assert!(<() as WeightInfo>::claim_reward().proof_size() >= weights::CLAIM_REWARD_PROOF_FLOOR);
    let composed = <weights::SubstrateWeight<Test> as WeightInfo>::claim_reward();
    assert_eq!(composed.ref_time(), generic.ref_time());
    assert!(composed.proof_size() >= weights::CLAIM_REWARD_PROOF_FLOOR);
    assert!(weights::claim_reward_proof_envelope::<Test>() <= weights::CLAIM_REWARD_PROOF_FLOOR);
}

#[test]
fn only_strict_improvements_replace_payloads_for_every_payout_mode() {
    for resolution in [
        RewardResolution::SingleBest,
        RewardResolution::TopNEqual { n: 2 },
        RewardResolution::TopNWeighted { n: 2 },
    ] {
        new_test_ext().execute_with(|| {
            let spec = register_spec();
            for solver in [2, 3] {
                assert_ok!(QuantumComputeMempool::register_solver(
                    RuntimeOrigin::signed(solver),
                    MinerType::Cpu
                ));
            }
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                weighted_params(),
                100,
                JobMode::Open,
                resolution,
                10,
                5,
                ResultDelivery::OnChainOnly
            ));
            // Initial +500, then strictly better -1500. A third vector has +1500.
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                alternate_solution()
            ));
            for spins in [vec![1, -1], vec![-1, 1]] {
                assert_noop!(
                    QuantumComputeMempool::submit_solution(
                        RuntimeOrigin::signed(2),
                        0,
                        bounded(vec![bounded(spins)])
                    ),
                    crate::Error::<Test>::SolutionNotImproved
                );
            }
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                sample_solution()
            ));
            let accepted = OrderSolutions::<Test>::get(0, 2).unwrap();
            assert_eq!(accepted.best_energy_milli, -1500);
            assert_eq!(accepted.solutions, sample_solution());
            assert_eq!(JobOrders::<Test>::get(0).unwrap().solution_count, 1);
            assert_eq!(Solvers::<Test>::get(2).unwrap().solutions_submitted, 2);
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    alternate_solution()
                ),
                crate::Error::<Test>::SolutionNotImproved
            );
            // Equal energy from a different solver does not displace the incumbent.
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(3),
                0,
                sample_solution()
            ));
            match resolution {
                RewardResolution::SingleBest => {
                    assert_eq!(OrderFrontRunner::<Test>::get(0).unwrap().solver, 2)
                }
                _ => assert_eq!(
                    crate::OrderTopSolvers::<Test>::get(0)
                        .iter()
                        .map(|s| s.solver)
                        .collect::<Vec<_>>(),
                    vec![2, 3]
                ),
            }
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(3),
                    0,
                    sample_solution()
                ),
                crate::Error::<Test>::SolutionNotImproved
            );
        });
    }
}

#[test]
fn stored_subset_always_contains_its_ranked_energy_vector() {
    for count in [None, Some(0), Some(1), Some(2), Some(3)] {
        new_test_ext().execute_with(|| {
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let mut params = weighted_params();
            params.min_solutions = count;
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                params,
                100,
                JobMode::Open,
                RewardResolution::SingleBest,
                10,
                5,
                ResultDelivery::OnChainOnly
            ));
            let inputs = bounded(vec![
                bounded(vec![1, -1]),
                bounded(vec![1, 1]),
                bounded(vec![-1, 1]),
            ]);
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                inputs
            ));
            let stored = OrderSolutions::<Test>::get(0, 2).unwrap();
            assert_eq!(stored.solutions[0].as_slice(), &[1, 1]);
            assert_eq!(stored.solutions.len(), count.unwrap_or(3).max(1) as usize);
            let order = JobOrders::<Test>::get(0).unwrap();
            let p = order.ising_params;
            let minimum = stored
                .solutions
                .iter()
                .map(|v| {
                    quantum_validation::energy_of_solution(
                        v.as_slice(),
                        p.h_values.as_slice(),
                        p.edges.as_slice(),
                        p.j_values.as_slice(),
                        p.nodes.as_slice(),
                    )
                    .unwrap()
                })
                .min()
                .unwrap();
            assert_eq!(minimum, stored.best_energy_milli);
            assert_eq!(
                minimum,
                OrderFrontRunner::<Test>::get(0).unwrap().energy_milli
            );
            // Deterministic distance tie chooses the earlier input after the best.
            if stored.solutions.len() > 1 {
                assert_eq!(stored.solutions[1].as_slice(), &[1, -1]);
            }
        });
    }
}

#[test]
fn best_anchoring_does_not_waive_diversity_or_count_requirements() {
    for (count, diversity, expected) in [
        (
            Some(2),
            Some(500),
            crate::Error::<Test>::InsufficientDiversity,
        ),
        (Some(3), None, crate::Error::<Test>::InsufficientSolutions),
    ] {
        new_test_ext().execute_with(|| {
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let mut params = weighted_params();
            params.min_solutions = count;
            params.min_diversity_milli = diversity;
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                params,
                100,
                JobMode::Open,
                RewardResolution::SingleBest,
                10,
                5,
                ResultDelivery::OnChainOnly
            ));
            // Global inverses have zero symmetric diversity; best is not first.
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    bounded(vec![bounded(vec![-1, -1]), bounded(vec![1, 1])])
                ),
                expected
            );
        });
    }
}

#[test]
fn outstanding_index_frees_slots_only_after_successful_closure() {
    new_test_ext().execute_with(|| {
        assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
        for _ in 0..MaxOrdersPerProposer::get() {
            propose_single_best(1, 1, 1);
        }
        let spec = JobSpecs::<Test>::iter_keys().next().unwrap();
        assert_noop!(
            QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                sample_params(),
                100,
                JobMode::Open,
                RewardResolution::SingleBest,
                1,
                1,
                ResultDelivery::OnChainOnly
            ),
            crate::Error::<Test>::OrderLimitReached
        );
        run_to_block(3);
        assert_eq!(
            crate::ProposerOrders::<Test>::get(1).len(),
            MaxOrdersPerProposer::get() as usize
        );
        assert_noop!(
            QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(4), 0),
            crate::Error::<Test>::NotProposer
        );
        assert_ok!(QuantumComputeMempool::reclaim_order(
            RuntimeOrigin::signed(1),
            0
        ));
        assert!(!crate::ProposerOrders::<Test>::get(1).contains(&0));
        assert!(JobOrders::<Test>::contains_key(0));
        let new_id = propose_single_best(1, 1, 1);
        assert!(crate::ProposerOrders::<Test>::get(1).contains(&new_id));
    });
}

#[test]
fn proposer_can_complete_more_than_a_lifetime_cap() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        for i in 0..40 {
            let id = propose_single_best(1, 1, 1);
            if i % 2 == 0 {
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    sample_solution()
                ));
            }
            run_to_block(System::block_number() + 1);
            if i % 2 == 0 {
                assert_ok!(QuantumComputeMempool::claim_reward(
                    RuntimeOrigin::signed(4),
                    id
                ));
            } else {
                assert_ok!(QuantumComputeMempool::reclaim_order(
                    RuntimeOrigin::signed(1),
                    id
                ));
            }
            assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Closed
            );
        }
        assert_eq!(JobOrders::<Test>::iter().count(), 40);
    });
}

#[test]
fn missing_reserve_cannot_close_or_release_a_slot() {
    use frame_support::traits::ReservableCurrency;
    for solved in [false, true] {
        new_test_ext().execute_with(|| {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let id = propose_single_best(1, 1, 1);
            if solved {
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    sample_solution()
                ));
            }
            run_to_block(2);
            let _ = Balances::slash_reserved(&1, 1);
            if solved {
                assert_noop!(
                    QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), id),
                    crate::Error::<Test>::RewardReserveMissing
                );
            } else {
                assert_noop!(
                    QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(1), id),
                    crate::Error::<Test>::RewardReserveMissing
                );
            }
            assert_eq!(Balances::reserved_balance(1), 99);
            assert!(crate::ProposerOrders::<Test>::get(1).contains(&id));
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Expired
            );
            assert_ok!(Balances::reserve(&1, 1));
            if solved {
                assert_ok!(QuantumComputeMempool::claim_reward(
                    RuntimeOrigin::signed(4),
                    id
                ));
            } else {
                assert_ok!(QuantumComputeMempool::reclaim_order(
                    RuntimeOrigin::signed(1),
                    id
                ));
            }
            assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
        });
    }
}

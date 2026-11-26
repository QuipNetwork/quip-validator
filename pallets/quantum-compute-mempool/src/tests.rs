use super::mock::*;
use crate::types::{
    Formulation, JobMode, MinerType, OrderStatus, ResultDelivery, RewardResolution,
};
use crate::{
    Event as QuantumComputeMempoolEvent, JobOrders, JobSpecs, NextExpiryBucket, NextOrderId,
    OpenOrders, OrderFrontRunner, OrderSolutions, OrdersExpiringAt, Solvers,
};
use frame_support::{assert_noop, assert_ok, traits::Hooks, BoundedVec};
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

/// A ferromagnetic chain of `n` nodes: every spin is coupled, so no spin is
/// isolated and each flip class of a row is its own answer (W1-012).
fn chain_params(n: u32) -> crate::IsingParamsOf<Test> {
    crate::types::IsingParams {
        nodes: bounded((0..n).collect()),
        edges: bounded((0..n - 1).map(|i| (i, i + 1)).collect()),
        h_values: bounded(vec![0; n as usize]),
        j_values: bounded(vec![-1_000; (n - 1) as usize]),
        max_energy_milli: None,
        min_diversity_milli: None,
        min_solutions: None,
    }
}

/// A one-row submission on `chain_params(n)`: spin 0 is +1 and spin `i >= 1`
/// is -1 when bit `i - 1` of `k` is set, so each `k < 2^(n-1)` is a distinct
/// answer. `k = 0` is the unique ground state; every other `k` is worse.
fn chain_row(n: u32, k: u32) -> crate::SolutionsOf<Test> {
    bounded(vec![bounded(
        (0..n)
            .map(|i| {
                if i >= 1 && (k >> (i - 1)) & 1 == 1 {
                    -1
                } else {
                    1
                }
            })
            .collect(),
    )])
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
        // W1-006: the reward sits in the order's own escrow account, not in a
        // reserve on the proposer.
        assert_eq!(escrow_balance(0), 100);
        assert_eq!(pallet_balances::Pallet::<Test>::reserved_balance(1), 0);
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
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution())
                .map_err(|error| error.error),
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
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution())
                .map_err(|error| error.error),
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
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, bad_solution)
                .map_err(|error| error.error),
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
        // W3-006: an unsolved order is refunded and closed at expiry, so there
        // is nothing left to claim.
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), 0),
            crate::Error::<Test>::AlreadyClaimed
        );
        assert_eq!(
            JobOrders::<Test>::get(0).unwrap().status,
            crate::types::OrderStatus::Closed
        );
        assert!(!OpenOrders::<Test>::contains_key(0));
        assert_eq!(escrow_balance(0), 0);
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
        // The escrow (101) never carries a lock in production; one is placed
        // here only to make a later payout fail. The first payout (51) leaves
        // 50, which still covers the 50 lock; the second (50) then fails.
        Balances::set_lock(
            *b"settle!!",
            &QuantumComputeMempool::escrow_account(0),
            50,
            WithdrawReasons::TRANSFER,
        );
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
        assert_eq!(escrow_balance(0), 101);
        assert_eq!(
            Balances::free_balance(2),
            1_000_000 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
        assert_eq!(
            Balances::free_balance(3),
            1_000_000 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
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
        assert!(crate::ProposerOrders::<Test>::contains_key(1));
        assert_eq!(crate::ProposerOrders::<Test>::get(1), index_before);

        Balances::remove_lock(*b"settle!!", &QuantumComputeMempool::escrow_account(0));
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(
            Balances::free_balance(2),
            1_000_051 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
        assert_eq!(
            Balances::free_balance(3),
            1_000_050 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
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
        assert_eq!(
            Balances::free_balance(2),
            1_000_100 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
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
        assert_eq!(escrow_balance(0), 100);
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
        // W3-006: the refund happens at expiry; `reclaim_order` finds the
        // order already closed.
        assert_noop!(
            QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(1), 0),
            crate::Error::<Test>::AlreadyClaimed
        );

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
        assert_eq!(
            Balances::free_balance(2),
            1_000_051 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
        assert_eq!(
            Balances::free_balance(3),
            1_000_050 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
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
        assert_eq!(
            Balances::free_balance(2),
            1_000_067 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
        assert_eq!(
            Balances::free_balance(3),
            1_000_033 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
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
        assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
            RuntimeOrigin::signed(2),
            0,
            sample_solution(),
            payee(20033, 2),
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
        assert_eq!(crate::OrderPayees::<Test>::get(0, 2), Some(payee(20033, 2)));
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
            )
            .saturating_add(QuantumComputeMempool::transform_weight(
                MaxTransformSteps::get(),
                true
            )),
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
            )
            .map_err(|error| error.error),
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

/// Free balance of order `order_id`'s reward escrow account (W1-006).
fn escrow_balance(order_id: u64) -> u128 {
    Balances::free_balance(QuantumComputeMempool::escrow_account(order_id))
}

fn propose_single_best(proposer: AccountId, deadline_blocks: u64, block_wait: u64) -> u64 {
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

/// W3-006: `deadline_blocks` is at least 1, so an order is never due in the
/// block it was proposed in; the minimum lands in the next block's bucket.
#[test]
fn propose_job_rejects_a_zero_deadline_and_schedules_the_minimum_next_block() {
    new_test_ext().execute_with(|| {
        let spec_id = register_spec();
        assert_noop!(
            QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec_id,
                sample_params(),
                100,
                JobMode::Open,
                RewardResolution::SingleBest,
                0,
                0,
                ResultDelivery::OnChainOnly,
            ),
            crate::Error::<Test>::DeadlineTooShort
        );
        let order_id = propose_single_best(1, 1, 0);
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
        // W3-006: unsolved, so finalisation also closes it.
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Closed
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
        assert_eq!(status_count(&ids, OrderStatus::Closed), 8);
        assert_eq!(status_count(&ids, OrderStatus::Opened), 1);
        assert_eq!(bucket_ids(2).len(), 1);
        assert_eq!(crate::OldestExpiryBucket::<Test>::get(), 2);
        assert_eq!(NextExpiryBucket::<Test>::get(), 3);

        run_to_block(3);
        assert_eq!(status_count(&ids, OrderStatus::Closed), 9);
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
            OrderStatus::Closed
        );
    });
}

/// W3-006 (design §B4 v2 item 3): a due order the backlogged queue has not
/// reached is finalised by `reclaim_order` itself -- auto-closed, refunded,
/// `Ok` -- and the queue does not touch it again.
#[test]
fn reclaim_order_finalises_a_due_order_lazily_under_backlog() {
    new_test_ext().execute_with(|| {
        let ids = propose_nine_orders_expiring_at_block_2();
        run_to_block(2);
        let lagging = *ids
            .iter()
            .find(|id| JobOrders::<Test>::get(**id).unwrap().status == OrderStatus::Opened)
            .unwrap();
        let order = JobOrders::<Test>::get(lagging).unwrap();
        let bucket = QuantumComputeMempool::bucket_of(&order);
        assert!(
            OrdersExpiringAt::<Test>::contains_key(bucket, lagging),
            "queued"
        );

        System::reset_events();
        assert_ok!(QuantumComputeMempool::reclaim_order(
            RuntimeOrigin::signed(order.proposer),
            lagging
        ));
        assert_eq!(
            JobOrders::<Test>::get(lagging).unwrap().status,
            OrderStatus::Closed
        );
        assert_eq!(escrow_balance(lagging), 0);
        assert!(!OrdersExpiringAt::<Test>::contains_key(bucket, lagging));
        let reclaimed = || {
            System::events()
                .into_iter()
                .filter(|record| {
                    matches!(
                        record.event,
                        RuntimeEvent::QuantumComputeMempool(
                            QuantumComputeMempoolEvent::RewardReclaimed { order_id, .. }
                        ) if order_id == lagging
                    )
                })
                .count()
        };
        assert_eq!(reclaimed(), 1);

        run_to_block(3);
        assert_eq!(reclaimed(), 1, "the queue does not finalise it again");
        assert_eq!(status_count(&ids, OrderStatus::Closed), 9);
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
            )
            .map_err(|error| error.error),
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
        assert_eq!(
            Balances::free_balance(2),
            1_000_100 - <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
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
            OrderStatus::Closed
        );
        assert!(QuantumComputeMempool::result_for_order(0).is_none());
        assert!(!System::events().into_iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::ResultReady { .. })
        )));
    });
}

#[test]
fn settlement_wrapper_maximizes_complete_weights_and_floors_both_providers() {
    use crate::{benchmark_weights, weights};
    use frame_support::{traits::Get, weights::constants::RocksDbWeight};
    use weights::WeightInfo;

    let generated = <() as benchmark_weights::WeightInfo>::claim_reward();
    let generic =
        <benchmark_weights::SubstrateWeight<Test> as benchmark_weights::WeightInfo>::claim_reward();
    let test_db: frame_support::weights::RuntimeDbWeight =
        <Test as frame_system::Config>::DbWeight::get();
    // Both generated implementations must have the same measured base once
    // the modeled DB accesses are removed. Reference regeneration may replace
    // that base without changing this accounting contract.
    //
    // The accesses are the benchmark's `r:`/`w:` totals, pinned so that a
    // regeneration that changes them is reviewed against the code path. At
    // the d102f1a reference run they are 70/71: 69/69 before, plus the
    // pruning schedule a settled order now writes -- `NextPruneBucket`
    // (r:1 w:1) and `OrdersPrunableAt` (w:1). The call also adds
    // `schedule_order_pruning_weight` on top, so those are counted twice.
    const CLAIM_REWARD_READS_WRITES: (u64, u64) = (70, 71);
    let (reads, writes) = CLAIM_REWARD_READS_WRITES;
    assert_eq!(
        generated.ref_time() - RocksDbWeight::get().reads_writes(reads, writes).ref_time(),
        generic.ref_time() - test_db.reads_writes(reads, writes).ref_time(),
    );
    assert_eq!(
        <() as WeightInfo>::claim_reward().ref_time(),
        generated
            .max(<() as benchmark_weights::WeightInfo>::claim_reward_dust())
            .ref_time()
    );
    assert!(<() as WeightInfo>::claim_reward().proof_size() >= weights::CLAIM_REWARD_PROOF_FLOOR);
    let composed = <weights::SubstrateWeight<Test> as WeightInfo>::claim_reward();
    assert_eq!(composed, generic
        .max(<benchmark_weights::SubstrateWeight<Test> as benchmark_weights::WeightInfo>::claim_reward_dust())
        .max(frame_support::weights::Weight::from_parts(0, weights::CLAIM_REWARD_PROOF_FLOOR)));
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
                    )
                    .map_err(|error| error.error),
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
                )
                .map_err(|error| error.error),
                crate::Error::<Test>::SolutionNotImproved
            );
            // Another solver cannot take the incumbent's answer (W1-012); in
            // this two-spin topology every other equal-energy row is that
            // answer. Equal energy across distinct answers is covered by
            // w1_012::equal_energy_distinct_answer_does_not_displace_the_incumbent.
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(3),
                    0,
                    sample_solution()
                )
                .map_err(|error| error.error),
                crate::Error::<Test>::DuplicateAnswer
            );
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(3),
                0,
                alternate_solution()
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
            // The same solver resubmitting its own answer meets the
            // must-improve rule, not DuplicateAnswer (W1-012).
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(3),
                    0,
                    alternate_solution()
                )
                .map_err(|error| error.error),
                crate::Error::<Test>::SolutionNotImproved
            );
        });
    }
}

#[test]
fn stored_subset_always_contains_its_ranked_energy_vector() {
    // [1, -1] and [-1, 1] are global flips (energies 500 and 1500 under
    // weighted_params), so they are one answer (W1-004) and only the
    // lower-energy [1, -1] is kept: three rows give two distinct answers.
    for count in [None, Some(0), Some(1), Some(2)] {
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
            assert_eq!(stored.solutions.len(), count.unwrap_or(2).max(1) as usize);
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
            // The flip class keeps its lowest-energy member, [1, -1].
            if stored.solutions.len() > 1 {
                assert_eq!(stored.solutions[1].as_slice(), &[1, -1]);
            }
        });
    }
}

#[test]
fn copies_and_flips_do_not_count_as_distinct_answers() {
    // W1-004. Requiring three answers from rows that hold only two distinct
    // ones fails, and one answer submitted four times (with flips) is one.
    for (rows, count) in [
        (vec![vec![1, -1], vec![1, 1], vec![-1, 1]], 3),
        (vec![vec![1, 1], vec![-1, -1], vec![1, 1], vec![-1, -1]], 2),
    ] {
        new_test_ext().execute_with(|| {
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let mut params = weighted_params();
            params.min_solutions = Some(count);
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
            let rows = bounded(rows.into_iter().map(bounded).collect());
            assert_noop!(
                QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, rows)
                    .map_err(|error| error.error),
                crate::Error::<Test>::InsufficientSolutions
            );
        });
    }
}

#[test]
fn isolated_spin_variants_count_as_one_answer() {
    // W1-004 with isolated spins (claude's R8 repro; owner decision
    // 2026-11-22): on a chain 0-1-2 plus nodes 3 and 4 with no field and no
    // coupling, the four (s3, s4) variants of one chain assignment are ONE
    // answer. They used to count as four, meeting a four-solution gate alone.
    for (count, accepted) in [(4, false), (1, true)] {
        new_test_ext().execute_with(|| {
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let params = crate::types::IsingParams {
                nodes: bounded(vec![0, 1, 2, 3, 4]),
                edges: bounded(vec![(0, 1), (1, 2)]),
                h_values: bounded(vec![0; 5]),
                j_values: bounded(vec![-1_000, -1_000]),
                max_energy_milli: None,
                min_diversity_milli: None,
                min_solutions: Some(count),
            };
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
            let rows = bounded(
                [
                    [1i8, 1, 1, 1, 1],
                    [1, 1, 1, 1, -1],
                    [1, 1, 1, -1, 1],
                    [1, 1, 1, -1, -1],
                ]
                .iter()
                .map(|row| bounded(row.to_vec()))
                .collect(),
            );
            if accepted {
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    rows
                ));
            } else {
                assert_noop!(
                    QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, rows)
                        .map_err(|error| error.error),
                    crate::Error::<Test>::InsufficientSolutions
                );
            }
        });
    }
}

#[test]
fn best_anchoring_does_not_waive_diversity_or_count_requirements() {
    // [1, -1] and [1, 1] are distinct answers at symmetric distance 1 of 2
    // (diversity 500); the best, [1, 1], is not first. [-1, -1] and [1, 1] are
    // global flips, so they are one answer and fail any count above 1 (W1-004).
    for (rows, count, diversity, expected) in [
        (
            vec![vec![1, -1], vec![1, 1]],
            Some(2),
            Some(501),
            crate::Error::<Test>::InsufficientDiversity,
        ),
        (
            vec![vec![-1, -1], vec![1, 1]],
            Some(2),
            None,
            crate::Error::<Test>::InsufficientSolutions,
        ),
        (
            vec![vec![1, -1], vec![1, 1]],
            Some(3),
            None,
            crate::Error::<Test>::InsufficientSolutions,
        ),
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
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    bounded(rows.into_iter().map(bounded).collect())
                )
                .map_err(|error| error.error),
                expected
            );
        });
    }
}

#[test]
fn outstanding_index_frees_slots_only_after_successful_closure() {
    new_test_ext().execute_with(|| {
        assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
        // Solved orders: they stay `Expired` until settled (W3-006 closes
        // only unsolved orders at expiry).
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        for _ in 0..MaxOrdersPerProposer::get() {
            let id = propose_single_best(1, 1, 1);
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                id,
                sample_solution()
            ));
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
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
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
            }
            // W3-006: an unsolved order was closed at expiry, without a call.
            assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
            assert!(!crate::ProposerOrders::<Test>::contains_key(1));
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Closed
            );
        }
        assert_eq!(NextOrderId::<Test>::get(), 40);
        assert_eq!(
            JobOrders::<Test>::iter().count(),
            OrderRetentionBlocks::get() as usize
        );
    });
}

#[test]
fn missing_reserve_cannot_close_or_release_a_slot() {
    // Solved orders only: the unsolved case fails inside finalisation instead
    // (W3-006): `unsolved_refund_failure_leaves_the_order_expired_for_reclaim`.
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let id = propose_single_best(1, 1, 1);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            id,
            sample_solution()
        ));
        run_to_block(2);
        // An escrow that no longer holds the full reward (here, slashed by
        // 1) must fail closed: nothing is paid and the order stays open.
        let escrow = QuantumComputeMempool::escrow_account(id);
        let _ = <Balances as frame_support::traits::Currency<AccountId>>::slash(&escrow, 1);
        assert_noop!(
            QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), id),
            crate::Error::<Test>::RewardReserveMissing
        );
        assert_eq!(escrow_balance(id), 99);
        assert!(crate::ProposerOrders::<Test>::get(1).contains(&id));
        assert_eq!(
            JobOrders::<Test>::get(id).unwrap().status,
            OrderStatus::Expired
        );
        assert_ok!(
            <Balances as frame_support::traits::Currency<AccountId>>::transfer(
                &4,
                &escrow,
                1,
                frame_support::traits::ExistenceRequirement::AllowDeath
            )
        );
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            id
        ));
        assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
        assert!(!crate::ProposerOrders::<Test>::contains_key(1));
    });
}

fn payee(chain_id: u64, byte: u8) -> crate::types::WinnerPayee {
    crate::types::WinnerPayee::V1 {
        chain_id,
        address: sp_core::H160::repeat_byte(byte),
    }
}

fn propose_required_payee(resolution: RewardResolution) {
    let spec = register_spec();
    assert_ok!(QuantumComputeMempool::propose_job_with_payee_requirement(
        RuntimeOrigin::signed(1),
        spec,
        weighted_params(),
        100,
        JobMode::Open,
        resolution,
        10,
        5,
        ResultDelivery::OnChainOnly,
        20033,
    ));
    assert_eq!(crate::OrderPayeeRequirements::<Test>::get(0), Some(20033));
    assert_eq!(escrow_balance(0), 100);
}

#[test]
fn required_payees_bind_each_signed_winner_and_survive_native_closure() {
    for resolution in [
        RewardResolution::SingleBest,
        RewardResolution::TopNEqual { n: 2 },
        RewardResolution::TopNWeighted { n: 2 },
    ] {
        new_test_ext().execute_with(|| {
            propose_required_payee(resolution);
            for solver in [2, 3] {
                assert_ok!(QuantumComputeMempool::register_solver(
                    RuntimeOrigin::signed(solver),
                    MinerType::Cpu
                ));
                assert_noop!(
                    QuantumComputeMempool::submit_solution(
                        RuntimeOrigin::signed(solver),
                        0,
                        sample_solution()
                    )
                    .map_err(|error| error.error),
                    crate::Error::<Test>::PayeeRequired
                );
                assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(solver),
                    0,
                    // One answer per solver (W1-012).
                    if solver == 2 {
                        sample_solution()
                    } else {
                        alternate_solution()
                    },
                    payee(20033, solver as u8)
                ));
                // Even an existing destination does not authorize a legacy submission.
                assert_noop!(
                    QuantumComputeMempool::submit_solution(
                        RuntimeOrigin::signed(solver),
                        0,
                        alternate_solution()
                    )
                    .map_err(|error| error.error),
                    crate::Error::<Test>::PayeeRequired
                );
            }
            assert!(crate::OrderPayees::<Test>::get(0, 1).is_none());
            let before = Balances::free_balance(2) + Balances::free_balance(3);
            run_to_block(6);
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                0
            ));
            assert_eq!(
                Balances::free_balance(2) + Balances::free_balance(3),
                before + 100
            );
            assert_eq!(Balances::reserved_balance(1), 0);
            assert_eq!(
                JobOrders::<Test>::get(0).unwrap().status,
                OrderStatus::Closed
            );
            assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
            assert!(!crate::ProposerOrders::<Test>::contains_key(1));
            for solver in [2, 3] {
                assert_eq!(
                    crate::OrderPayees::<Test>::get(0, solver),
                    Some(payee(20033, solver as u8))
                );
                assert_noop!(
                    QuantumComputeMempool::submit_solution_with_payee(
                        RuntimeOrigin::signed(solver),
                        0,
                        sample_solution(),
                        payee(20033, 9)
                    )
                    .map_err(|error| error.error),
                    crate::Error::<Test>::OrderNotOpen
                );
            }
            assert_eq!(crate::OrderPayeeRequirements::<Test>::get(0), Some(20033));
        });
    }
}

#[test]
fn invalid_or_non_improving_payee_submission_is_atomic_and_expiry_freezes_metadata() {
    new_test_ext().execute_with(|| {
        propose_required_payee(RewardResolution::SingleBest);
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_noop!(
            QuantumComputeMempool::submit_solution_with_payee(
                RuntimeOrigin::root(),
                0,
                sample_solution(),
                payee(20033, 2)
            )
            .map_err(|error| error.error),
            sp_runtime::DispatchError::BadOrigin
        );
        for (destination, error) in [
            (payee(0, 2), crate::Error::<Test>::InvalidPayee),
            (payee(20033, 0), crate::Error::<Test>::InvalidPayee),
            (payee(1, 2), crate::Error::<Test>::PayeeChainMismatch),
        ] {
            assert_noop!(
                QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(2),
                    0,
                    sample_solution(),
                    destination
                )
                .map_err(|error| error.error),
                error
            );
        }
        assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
            RuntimeOrigin::signed(2),
            0,
            alternate_solution(),
            payee(20033, 2)
        ));
        assert_noop!(
            QuantumComputeMempool::submit_solution_with_payee(
                RuntimeOrigin::signed(2),
                0,
                alternate_solution(),
                payee(20033, 9)
            )
            .map_err(|error| error.error),
            crate::Error::<Test>::SolutionNotImproved
        );
        assert_noop!(
            QuantumComputeMempool::submit_solution_with_payee(
                RuntimeOrigin::signed(2),
                0,
                sample_solution(),
                payee(1, 9)
            )
            .map_err(|error| error.error),
            crate::Error::<Test>::PayeeChainMismatch
        );
        assert_noop!(
            QuantumComputeMempool::submit_solution_with_payee(
                RuntimeOrigin::signed(2),
                0,
                bounded(vec![bounded(vec![0, 1])]),
                payee(20033, 9)
            )
            .map_err(|error| error.error),
            crate::Error::<Test>::InvalidSpinValues
        );
        assert_eq!(crate::OrderPayees::<Test>::get(0, 2), Some(payee(20033, 2)));
        assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
            RuntimeOrigin::signed(2),
            0,
            bounded(vec![bounded(vec![-1, -1])]),
            payee(20033, 3)
        ));
        assert_eq!(crate::OrderPayees::<Test>::get(0, 2), Some(payee(20033, 3)));
        assert_eq!(
            OrderSolutions::<Test>::get(0, 2).unwrap().best_energy_milli,
            -500
        );
        // Leave the hook behind: expiry is enforced from timing, not stored status.
        System::set_block_number(6);
        assert_eq!(
            JobOrders::<Test>::get(0).unwrap().status,
            OrderStatus::Opened
        );
        assert_noop!(
            QuantumComputeMempool::submit_solution_with_payee(
                RuntimeOrigin::signed(2),
                0,
                sample_solution(),
                payee(20033, 4)
            )
            .map_err(|error| error.error),
            crate::Error::<Test>::OrderNotOpen
        );
        assert_eq!(crate::OrderPayees::<Test>::get(0, 2), Some(payee(20033, 3)));
    });
}

#[test]
fn optional_payee_is_replaced_or_cleared_only_with_an_accepted_payload() {
    new_test_ext().execute_with(|| {
        let id = propose_single_best(1, 10, 5);
        assert!(crate::OrderPayeeRequirements::<Test>::get(id).is_none());
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
            RuntimeOrigin::signed(2),
            id,
            alternate_solution(),
            payee(u64::MAX, 2)
        ));
        assert_noop!(
            QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                id,
                alternate_solution()
            )
            .map_err(|error| error.error),
            crate::Error::<Test>::SolutionNotImproved
        );
        assert_eq!(
            crate::OrderPayees::<Test>::get(id, 2),
            Some(payee(u64::MAX, 2))
        );
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            id,
            sample_solution()
        ));
        assert!(crate::OrderPayees::<Test>::get(id, 2).is_none());
    });
}

#[test]
fn required_payee_proposal_reuses_native_checks_and_has_no_partial_writes() {
    new_test_ext().execute_with(|| {
        let spec = register_spec();
        for (chain, reward, expected) in [
            (0, 100, crate::Error::<Test>::InvalidPayeeChain),
            (1, 0, crate::Error::<Test>::RewardTooLow),
        ] {
            assert_noop!(
                QuantumComputeMempool::propose_job_with_payee_requirement(
                    RuntimeOrigin::signed(1),
                    spec,
                    sample_params(),
                    reward,
                    JobMode::Open,
                    RewardResolution::SingleBest,
                    10,
                    5,
                    ResultDelivery::OnChainOnly,
                    chain
                ),
                expected
            );
        }
        assert_eq!(NextOrderId::<Test>::get(), 0);
        assert_eq!(crate::OrderPayeeRequirements::<Test>::iter().count(), 0);
        assert_eq!(crate::OrderPayees::<Test>::iter().count(), 0);
        assert_eq!(Balances::reserved_balance(1), 0);
    });
}

#[test]
fn payee_calls_append_indices_and_keep_legacy_submission_bytes() {
    use codec::Encode;
    let old = crate::Call::<Test>::submit_solution {
        order_id: 42,
        solutions: sample_solution(),
    }
    .encode();
    let mut expected = vec![4];
    expected.extend(42u64.encode());
    expected.extend(sample_solution().encode());
    assert_eq!(old, expected);
    let new = crate::Call::<Test>::submit_solution_with_payee {
        order_id: 42,
        solutions: sample_solution(),
        payee: payee(20033, 2),
    }
    .encode();
    assert_eq!(new[0], 9);
    assert_eq!(&new[1..old.len()], &old[1..]);
    assert_eq!(payee(20033, 2).encode().len(), 29);
    assert_eq!(payee(20033, 2).encode()[0], 0);
}

#[test]
fn closure_preserves_other_slots_and_deletes_only_the_last_index_key() {
    for solved in [false, true] {
        new_test_ext().execute_with(|| {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let first = propose_single_best(1, 1, 1);
            // W3-006: unsolved orders close at expiry, so stagger them to
            // observe each closure.
            let second = propose_single_best(1, if solved { 1 } else { 2 }, 1);
            if solved {
                for id in [first, second] {
                    assert_ok!(QuantumComputeMempool::submit_solution(
                        RuntimeOrigin::signed(2),
                        id,
                        sample_solution()
                    ));
                }
            }
            if !solved {
                run_to_block(2);
                assert_eq!(crate::ProposerOrders::<Test>::get(1).as_slice(), &[second]);
                run_to_block(3);
                assert!(!crate::ProposerOrders::<Test>::contains_key(1));
                assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
                return;
            }
            run_to_block(2);
            let index_before = crate::ProposerOrders::<Test>::get(1);
            assert_noop!(
                QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(4), first),
                crate::Error::<Test>::NotProposer
            );
            assert!(crate::ProposerOrders::<Test>::contains_key(1));
            assert_eq!(crate::ProposerOrders::<Test>::get(1), index_before);
            for id in [first, second] {
                assert_ok!(QuantumComputeMempool::claim_reward(
                    RuntimeOrigin::signed(4),
                    id
                ));
                if id == first {
                    assert!(crate::ProposerOrders::<Test>::contains_key(1));
                    assert_eq!(crate::ProposerOrders::<Test>::get(1).as_slice(), &[second]);
                } else {
                    assert!(!crate::ProposerOrders::<Test>::contains_key(1));
                    assert!(crate::ProposerOrders::<Test>::get(1).is_empty());
                }
            }
        });
    }
}

#[test]
fn accepted_payee_events_report_only_metadata_transitions() {
    new_test_ext().execute_with(|| {
        let spec = register_spec();
        let params = crate::types::IsingParams {
            nodes: bounded(vec![0, 1, 2]),
            edges: bounded(vec![(0, 1)]),
            h_values: bounded(vec![1_000, 2_000, 4_000]),
            j_values: bounded(vec![0]),
            max_energy_milli: None,
            min_diversity_milli: None,
            min_solutions: None,
        };
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
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let mut previous = None;
        for (step, destination) in [
            None,
            None,
            Some(payee(20033, 2)),
            Some(payee(20033, 2)),
            Some(payee(20033, 3)),
            None,
        ]
        .into_iter()
        .enumerate()
        {
            // Binary-weighted local fields give a strictly decreasing energy each step.
            let solutions = bounded(vec![bounded(
                (0..3)
                    .map(|bit| if step & (1 << bit) == 0 { 1 } else { -1 })
                    .collect(),
            )]);
            let start = System::events().len();
            if let Some(payee) = destination {
                assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(2),
                    0,
                    solutions,
                    payee
                ));
            } else {
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    solutions
                ));
            }
            let events = System::events();
            let emitted: Vec<_> = events[start..]
                .iter()
                .filter_map(|record| match record.event {
                    RuntimeEvent::QuantumComputeMempool(
                        QuantumComputeMempoolEvent::SolutionPayeeSet {
                            order_id,
                            solver,
                            payee,
                        },
                    ) => Some((order_id, solver, payee)),
                    _ => None,
                })
                .collect();
            if previous == destination {
                assert!(emitted.is_empty());
            } else {
                assert_eq!(emitted, vec![(0, 2, destination)]);
            }
            assert_eq!(crate::OrderPayees::<Test>::get(0, 2), destination);
            assert_eq!(
                crate::OrderPayees::<Test>::contains_key(0, 2),
                destination.is_some()
            );
            let accepted = OrderSolutions::<Test>::get(0, 2).unwrap();
            assert_noop!(
                QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(2),
                    0,
                    accepted.solutions,
                    payee(20033, 9)
                )
                .map_err(|error| error.error),
                crate::Error::<Test>::SolutionNotImproved
            );
            previous = destination;
        }
    });
}

#[test]
fn submission_wrapper_covers_every_full_measured_case_without_double_charging_db() {
    use crate::{benchmark_weights::WeightInfo as Measured, weights::WeightInfo as Public};
    use frame_support::weights::Weight;
    fn check<W: Measured, P: Public>() {
        for (n, e, s) in [(2, 1, 1), (5_000, 50_000, 20)] {
            let expected = W::submit_solution(n, e, s)
                .max(W::submit_solution_with_payee(n, e, s))
                .max(W::submit_solution_replacement(n, e, s))
                .max(W::submit_solution_clear_payee(n, e, s))
                .max(Weight::from_parts(
                    0,
                    crate::weights::SUBMIT_SOLUTION_PROOF_FLOOR,
                ));
            assert_eq!(P::submit_solution(n, e, s), expected);
            assert_eq!(P::submit_solution_with_payee(n, e, s), expected);
        }
    }
    check::<(), ()>();
    check::<crate::benchmark_weights::SubstrateWeight<Test>, crate::weights::SubstrateWeight<Test>>(
    );
}

#[path = "vm_tests.rs"]
mod vm_tests;

#[test]
fn vv_poc_open_order_ids_limit_one_decodes_one_order() {
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
        let template = JobOrders::<Test>::get(0).unwrap();
        for id in 1..=10_000 {
            JobOrders::<Test>::insert(id, template.clone());
            OpenOrders::<Test>::insert(id, ());
        }

        crate::OPEN_ORDER_DECODES.with(|count| count.set(0));
        let ids = QuantumComputeMempool::open_order_ids(None, 1);
        let decodes = crate::OPEN_ORDER_DECODES.with(|count| count.get());
        if decodes > 1 {
            eprintln!("VV-BUG-REPRODUCED: limit=1 decoded {decodes} full orders");
        }
        assert_eq!(ids, vec![0]);
        assert_eq!(decodes, 1, "limit=1 should decode only the returned order");
    });
}

#[test]
fn a6f_open_order_caps_solvers_and_prunes() {
    new_test_ext().execute_with(|| {
        let cap: AccountId = 32; // keep in sync with types::MAX_SOLVERS_PER_OPEN_ORDER
        let first_solver: AccountId = 10;
        // Fund and register cap + 1 distinct solvers (the mock only endows
        // accounts 1..=4, so top up the rest to cover the deposit + fees).
        for who in first_solver..=first_solver + cap {
            let _ = <Balances as frame_support::traits::Currency<AccountId>>::make_free_balance_be(
                &who, 1_000_000,
            );
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(who),
                MinerType::Cpu
            ));
        }
        let spec_id = register_spec();
        // Seven chained spins hold 64 answers, one per solver (W1-012).
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            chain_params(7),
            100,
            JobMode::Open,
            RewardResolution::TopNEqual { n: 3 },
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));
        // The first `cap` distinct solvers are admitted.
        for who in first_solver..first_solver + cap {
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(who),
                0,
                chain_row(7, (who - first_solver) as u32)
            ));
        }
        assert_eq!(
            OrderSolutions::<Test>::iter_prefix(0).count() as AccountId,
            cap,
            "every distinct solver up to the cap should be stored"
        );
        // The (cap + 1)-th distinct solver is rejected once the cap exists.
        // On pinned source there is no cap, so this submission succeeds and the
        // assertion below fails.
        let over = QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(first_solver + cap),
            0,
            chain_row(7, cap as u32),
        );
        assert!(
            over.is_err(),
            "the (cap + 1)-th distinct solver must be rejected by the per-order cap"
        );
        // Settlement at expiry prunes every stored submission. On pinned source
        // nothing prunes OrderSolutions, so the prefix is non-empty here.
        run_to_block(2);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(9),
            0
        ));
        assert_eq!(
            OrderSolutions::<Test>::iter_prefix(0).count(),
            0,
            "settlement must prune the per-order submission map"
        );
    });
}

#[test]
fn full_solver_cap_allows_improvements_without_new_payload_slots() {
    use frame_support::traits::Currency;
    new_test_ext().execute_with(|| {
        let id = NextOrderId::<Test>::get();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            register_spec(),
            chain_params(7),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));
        // 32 distinct non-ground answers (W1-012), then solver 10 improves to
        // the ground state.
        for who in 10..42 {
            let _ = Balances::make_free_balance_be(&who, 1_000_000);
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(who),
                MinerType::Cpu
            ));
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(who),
                id,
                chain_row(7, (who - 9) as u32)
            ));
        }
        assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 32);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(10),
            id,
            chain_row(7, 0)
        ));
        assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 32);
        assert_eq!(OrderSolutions::<Test>::iter_key_prefix(id).count(), 32);
        run_to_block(2);
        assert!(OrderSolutions::<Test>::iter_key_prefix(id).next().is_none());
        assert_eq!(crate::OrderFrontRunner::<Test>::get(id).unwrap().solver, 10);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            id
        ));
        assert_eq!(
            JobOrders::<Test>::get(id).unwrap().status,
            OrderStatus::Closed
        );
    });
}

#[test]
fn a6f_solver_registration_reserves_deposit() {
    new_test_ext().execute_with(|| {
        let who: AccountId = 2;
        let ed = <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance();
        assert!(ed > 0);
        let before = Balances::reserved_balance(who);
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(who),
            MinerType::Cpu
        ));
        assert_eq!(
            Balances::reserved_balance(who),
            before + ed,
            "register_solver did not reserve the registration deposit"
        );
        assert!(Solvers::<Test>::contains_key(who));
    });
}

#[test]
fn solver_deposit_failure_and_duplicates_are_atomic_and_refund_is_once() {
    new_test_ext().execute_with(|| {
        let initial = Balances::free_balance(2);
        assert_noop!(
            QuantumComputeMempool::register_solver(RuntimeOrigin::signed(99), MinerType::Cpu),
            pallet_balances::Error::<Test>::InsufficientBalance
        );
        assert!(!Solvers::<Test>::contains_key(99));
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let reserved = Balances::reserved_balance(2);
        assert_eq!(
            reserved,
            <Balances as frame_support::traits::Currency<AccountId>>::minimum_balance()
        );
        assert_noop!(
            QuantumComputeMempool::register_solver(RuntimeOrigin::signed(2), MinerType::Cpu),
            crate::Error::<Test>::SolverAlreadyRegistered
        );
        assert_eq!(Balances::reserved_balance(2), reserved);
        assert_ok!(QuantumComputeMempool::deregister_solver(
            RuntimeOrigin::signed(2)
        ));
        assert_eq!(Balances::free_balance(2), initial);
        assert_eq!(Balances::reserved_balance(2), 0);
        assert_noop!(
            QuantumComputeMempool::deregister_solver(RuntimeOrigin::signed(2)),
            crate::Error::<Test>::SolverNotRegistered
        );
    });
}

// QUIP-26 handover, with a thread-local ED override and an above-ED leader
// control. Deregistration keeps the reaped-winner case valid after QUIP-29.
#[test]
fn poc_reaped_sub_ed_winner_locks_order_reward() {
    new_test_ext().execute_with(|| {
        ExistentialDeposit::set(50);
        for solver in [2, 3] {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(solver),
                MinerType::Cpu
            ));
        }
        let spec_id = register_spec();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            101,
            JobMode::Bid {
                miners: Some(bounded(vec![2, 3])),
                miner_types: None
            },
            RewardResolution::TopNWeighted { n: 2 },
            2,
            1,
            ResultDelivery::OnChainOnly
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
        assert_ok!(QuantumComputeMempool::deregister_solver(
            RuntimeOrigin::signed(3)
        ));
        let bal3 = Balances::free_balance(3);
        assert_ok!(Balances::transfer_allow_death(
            RuntimeOrigin::signed(3),
            4,
            bal3
        ));
        assert_eq!(Balances::free_balance(3), 0);
        run_to_block(2);
        let leader_before = Balances::free_balance(2);
        let proposer_before = Balances::free_balance(1);
        let issuance = Balances::total_issuance();
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(
            JobOrders::<Test>::get(0).unwrap().status,
            OrderStatus::Closed
        );
        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(Balances::free_balance(2), leader_before + 68);
        assert_eq!(Balances::free_balance(3), 0);
        assert_eq!(Balances::free_balance(1), proposer_before + 33);
        assert_eq!(Balances::total_issuance(), issuance);
        assert_eq!(Solvers::<Test>::get(2).unwrap().rewards_earned, 68);
        assert!(!System::events().iter().any(|record| matches!(
            record.event,
            RuntimeEvent::QuantumComputeMempool(QuantumComputeMempoolEvent::RewardClaimed {
                solver: 3,
                ..
            })
        )));
    });
}

#[test]
fn overflowing_expiry_head_and_later_bucket_both_progress_within_budget() {
    new_test_ext().execute_with(|| {
        let mut head = Vec::new();
        for proposer in [1, 2, 3] {
            for _ in 0..8 {
                head.push(propose_single_best(proposer, 1, 1));
            }
        }
        let later = propose_single_best(4, 2, 1);
        run_to_block(2);
        assert_eq!(status_count(&head, OrderStatus::Closed), 8);
        let before = status_count(&head, OrderStatus::Closed);
        run_to_block(3);
        assert_eq!(
            JobOrders::<Test>::get(later).unwrap().status,
            OrderStatus::Closed,
            "later due bucket must receive work before the head drains"
        );
        let after = status_count(&head, OrderStatus::Closed);
        assert!(after >= before + 4 && after < head.len());
        assert!(after - before < crate::types::MAX_FINALISED_ORDERS_PER_BLOCK as usize);
        let mut previous = after;
        for now in 4..=10 {
            run_to_block(now);
            let current = status_count(&head, OrderStatus::Closed);
            assert!(current - previous <= crate::types::MAX_FINALISED_ORDERS_PER_BLOCK as usize);
            assert!(
                current > previous || current == head.len(),
                "head must not starve"
            );
            previous = current;
        }
        assert_eq!(previous, head.len());
        assert!(bucket_ids(2).is_empty() && bucket_ids(3).is_empty());
    });
}

#[test]
fn small_expiry_buckets_keep_fifo_order_with_the_same_total_budget() {
    new_test_ext().execute_with(|| {
        let early: Vec<_> = (0..3).map(|_| propose_single_best(1, 1, 1)).collect();
        let later: Vec<_> = (0..6).map(|_| propose_single_best(2, 2, 1)).collect();
        // A skipped hook models catching up to two due buckets at once.
        System::set_block_number(3);
        QuantumComputeMempool::on_initialize(3);
        assert_eq!(status_count(&early, OrderStatus::Closed), 3);
        assert_eq!(status_count(&later, OrderStatus::Closed), 5);
        run_to_block(4);
        assert_eq!(status_count(&later, OrderStatus::Closed), 6);
    });
}

// QUIP-30 handover's six closed records now disappear after the retention
// window, while the seventh unpaid record and its escrow remain claimable.
#[test]
fn closed_orders_are_retained_then_pruned_without_touching_unpaid_orders() {
    new_test_ext().execute_with(|| {
        // Six unsolved orders close at expiry (W3-006); the seventh is solved
        // and stays unpaid until claimed.
        for _ in 0..7 {
            propose_single_best(1, 1, 1);
        }
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            6,
            sample_solution()
        ));
        run_to_block(21);
        assert_eq!(JobOrders::<Test>::iter_keys().count(), 7);
        // Only the unclosed seventh order still holds its reward.
        assert_eq!(escrow_balance(6), 100);
        assert!((0..6).all(|id| escrow_balance(id) == 0));
        run_to_block(22);
        assert_eq!(JobOrders::<Test>::iter_keys().collect::<Vec<_>>(), vec![6]);
        assert_eq!(escrow_balance(6), 100);
        assert_eq!(NextOrderId::<Test>::get(), 7);
        assert!(!crate::NextPruneBucket::<Test>::exists());
        assert_noop!(
            QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(1), 0),
            crate::Error::<Test>::OrderNotFound
        );
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            6
        ));
        assert_eq!(Balances::reserved_balance(1), 0);
        assert_eq!(propose_single_best(1, 1, 1), 7);
    });
}

#[test]
fn retention_sweep_limits_work_and_resumes_the_bucket() {
    new_test_ext().execute_with(|| {
        // Solved orders, so all nine close in the same block and share one
        // pruning bucket (unsolved ones would close at expiry, W3-006).
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        for _ in 0..8 {
            propose_single_best(1, 1, 1);
        }
        propose_single_best(4, 1, 1);
        for id in 0..9 {
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                id,
                sample_solution()
            ));
        }
        run_to_block(3);
        for id in 0..9 {
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                id
            ));
        }
        run_to_block(22);
        assert_eq!(JobOrders::<Test>::iter_keys().count(), 9);
        run_to_block(23);
        assert_eq!(JobOrders::<Test>::iter_keys().count(), 1);
        assert_eq!(crate::NextPruneBucket::<Test>::get(), Some(23));
        run_to_block(24);
        assert_eq!(JobOrders::<Test>::iter_keys().count(), 0);
        assert!(!crate::NextPruneBucket::<Test>::exists());
    });
}

#[test]
fn retention_sweep_preserves_poll_ttl_then_removes_all_auxiliary_records() {
    new_test_ext().execute_with(|| {
        propose_required_payee(RewardResolution::TopNEqual { n: 2 });
        JobOrders::<Test>::mutate(0, |order| {
            order.as_mut().unwrap().delivery = ResultDelivery::CallbackWithPoll {
                endpoint: bounded(b"https://example.invalid/result".to_vec()),
            };
        });
        for solver in [2, 3] {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(solver),
                MinerType::Cpu
            ));
            assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
                RuntimeOrigin::signed(solver),
                0,
                // One answer per solver (W1-012).
                if solver == 2 {
                    sample_solution()
                } else {
                    alternate_solution()
                },
                payee(20033, solver as u8)
            ));
        }
        run_to_block(6);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            0
        ));
        assert_eq!(crate::OrderPayees::<Test>::iter_prefix(0).count(), 2);
        assert!(crate::OrderPayeeRequirements::<Test>::contains_key(0));
        assert!(!OrderFrontRunner::<Test>::contains_key(0));
        // Exercise defensive cleanup of sparse stale data as well.
        OrderFrontRunner::<Test>::insert(
            0,
            crate::types::FrontRunner {
                solver: 2,
                energy_milli: -1_000,
                effective_at: 0,
                seq: 0,
            },
        );
        assert!(crate::OrderTopSolvers::<Test>::contains_key(0));
        run_to_block(15);
        assert!(crate::OrderResults::<Test>::contains_key(0));
        assert!(JobOrders::<Test>::contains_key(0));
        run_to_block(25);
        assert!(JobOrders::<Test>::contains_key(0));
        run_to_block(26);
        assert!(!JobOrders::<Test>::contains_key(0));
        assert_eq!(crate::OrderPayees::<Test>::iter_prefix(0).count(), 0);
        assert!(!crate::OrderPayeeRequirements::<Test>::contains_key(0));
        assert!(!OrderFrontRunner::<Test>::contains_key(0));
        assert!(!crate::OrderTopSolvers::<Test>::contains_key(0));
        assert!(!crate::OrderResults::<Test>::contains_key(0));
        assert_eq!(OrderSolutions::<Test>::iter_prefix(0).count(), 0);
        assert!(!OpenOrders::<Test>::contains_key(0));
    });
}

#[test]
fn retention_sweep_bounds_empty_buckets_and_fails_closed_on_unpaid_queue_entry() {
    new_test_ext().execute_with(|| {
        // Solved and unclaimed, so unpaid (an unsolved order is refunded and
        // queued at expiry, W3-006).
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
        let id = propose_single_best(1, 1, 1);
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            id,
            sample_solution()
        ));
        run_to_block(2);
        crate::OrdersPrunableAt::<Test>::insert(12, id, ());
        crate::NextPruneBucket::<Test>::put(0);
        System::set_block_number(12);
        let charged = QuantumComputeMempool::on_initialize(12);
        assert!(charged.all_gte(crate::weights::closed_order_pruning_weight::<Test>(0, 8)));
        assert!(charged.all_gte(
            <<Test as crate::Config>::WeightInfo as crate::weights::WeightInfo>::prune_closed_orders(0)
        ));
        assert_eq!(crate::NextPruneBucket::<Test>::get(), Some(8));
        assert_eq!(
            JobOrders::<Test>::get(id).unwrap().status,
            OrderStatus::Expired
        );
        run_to_block(13);
        assert!(!crate::NextPruneBucket::<Test>::exists());
        assert_eq!(
            JobOrders::<Test>::get(id).unwrap().status,
            OrderStatus::Expired
        );
        assert_eq!(escrow_balance(id), 100);
        assert_ok!(QuantumComputeMempool::claim_reward(
            RuntimeOrigin::signed(4),
            id
        ));
    });
}

/// The composed pruning providers cover the hand floor at every bounded
/// cleanup shape (A12). The measured `prune_closed_orders(k)` varies orders
/// only, with one bucket visit, so it does not cover the bucket dimension by
/// itself (at 0 orders its proof size is under one bucket's): the providers
/// floor it at the maximum bucket visits. The generated functions alone are
/// deliberately not asserted here.
#[test]
fn pruning_weight_providers_cover_every_bounded_cleanup_shape() {
    use crate::{types, weights};
    for orders in 0..=types::MAX_PRUNED_ORDERS_PER_BLOCK {
        for buckets in 0..=u64::from(types::MAX_PRUNED_ORDERS_PER_BLOCK) {
            let floor = weights::closed_order_pruning_weight::<Test>(orders, buckets);
            for bound in [
                <weights::SubstrateWeight<Test> as weights::WeightInfo>::prune_closed_orders(
                    orders,
                ),
                <() as weights::WeightInfo>::prune_closed_orders(orders),
            ] {
                assert!(
                    bound.all_gte(floor),
                    "cleanup bound below existing floor: {orders} orders, {buckets} buckets"
                );
            }
        }
    }
}

/// The retention sweep's charge (A12). `on_initialize` charges
/// `prune_closed_orders(pruned).max(closed_order_pruning_weight(pruned,
/// buckets))` componentwise: the provider for the orders pruned and the hand
/// floor for the shape actually visited. The bucket dimension is priced by
/// the hand floor alone (benchmarking bucket visits is a follow-up). Driven
/// at no orders in one bucket, no orders across the eight-bucket bound, and
/// the most orders across the most buckets.
#[test]
fn the_retention_sweep_charges_the_provider_and_the_floor_at_the_visited_shape() {
    use crate::weights::{closed_order_pruning_weight, WeightInfo};
    type Provider = <Test as crate::Config>::WeightInfo;
    let max = crate::types::MAX_PRUNED_ORDERS_PER_BLOCK;
    let check = |now: u64, orders: u32, buckets: u64| {
        let charged = QuantumComputeMempool::on_initialize(now);
        let expected = Provider::prune_closed_orders(orders)
            .max(closed_order_pruning_weight::<Test>(orders, buckets));
        assert!(
            charged.all_gte(expected),
            "{orders} orders, {buckets} buckets: charged {charged:?} < {expected:?}"
        );
    };
    // No orders, one empty bucket at the cursor.
    new_test_ext().execute_with(|| {
        crate::NextPruneBucket::<Test>::put(12);
        System::set_block_number(12);
        check(12, 0, 1);
        assert!(!crate::NextPruneBucket::<Test>::exists());
    });
    // No orders, eight empty buckets: the visit bound stops the sweep.
    new_test_ext().execute_with(|| {
        let id = propose_single_best(1, 1, 1);
        run_to_block(2);
        crate::OrdersPrunableAt::<Test>::insert(12, id, ());
        crate::NextPruneBucket::<Test>::put(0);
        System::set_block_number(12);
        check(12, 0, u64::from(max));
        assert_eq!(crate::NextPruneBucket::<Test>::get(), Some(u64::from(max)));
    });
    // The most orders across the most buckets: one closed order per bucket.
    // Unsolved orders close themselves at expiry (W3-006), so one proposed
    // per block closes, and schedules its pruning, one block apart.
    new_test_ext().execute_with(|| {
        for id in 0..u64::from(max) {
            run_to_block(1 + id);
            propose_single_best(1, 1, 1);
        }
        run_to_block(1 + u64::from(max));
        assert!((0..u64::from(max))
            .all(|id| JobOrders::<Test>::get(id).unwrap().status == OrderStatus::Closed));
        let first = 2 + <Test as crate::Config>::OrderRetentionBlocks::get();
        let last = first + u64::from(max) - 1;
        assert_eq!(crate::NextPruneBucket::<Test>::get(), Some(first));
        System::set_block_number(last);
        check(last, max, u64::from(max));
        assert_eq!(JobOrders::<Test>::iter_keys().count(), 0);
    });
}

// --- W1-006: per-order reward escrow ---

mod w1_006 {
    use super::*;
    use frame_support::traits::{
        Currency, ExistenceRequirement, LockableCurrency, WithdrawReasons,
    };

    fn spend(from: AccountId, to: AccountId, amount: u128) {
        assert_ok!(<Balances as Currency<AccountId>>::transfer(
            &from,
            &to,
            amount,
            ExistenceRequirement::KeepAlive
        ));
    }

    fn solve_and_expire(order_id: u64, solver: AccountId) {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(solver),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(solver),
            order_id,
            sample_solution()
        ));
        run_to_block(System::block_number() + 20);
        assert_eq!(
            JobOrders::<Test>::get(order_id).unwrap().status,
            OrderStatus::Expired
        );
    }

    fn escrow_exists(order_id: u64) -> bool {
        System::account_exists(&QuantumComputeMempool::escrow_account(order_id))
    }

    #[test]
    fn reward_must_be_spendable_from_the_proposer() {
        new_test_ext().execute_with(|| {
            let spec = register_spec();
            let propose = |who: AccountId| {
                QuantumComputeMempool::propose_job(
                    RuntimeOrigin::signed(who),
                    spec,
                    sample_params(),
                    100,
                    JobMode::Open,
                    RewardResolution::SingleBest,
                    10,
                    5,
                    ResultDelivery::OnChainOnly,
                )
            };
            // Frozen: a vesting-style lock that the reward would breach.
            Balances::set_lock(*b"vesting ", &1, 999_950, WithdrawReasons::all());
            assert_noop!(propose(1), crate::Error::<Test>::RewardNotSpendable);
            // Unavailable: not enough funds at all.
            let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&7, 50);
            assert_noop!(propose(7), crate::Error::<Test>::RewardNotSpendable);
            // Not expendable: paying it would reap the proposer.
            let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&8, 100);
            assert_noop!(propose(8), crate::Error::<Test>::RewardNotSpendable);
        });
    }

    #[test]
    fn pre_existing_freeze_then_spending_after_posting_still_pays_the_solver() {
        // Under the old reserve escrow the reserve backed this freeze, so the
        // proposer could spend after posting and the claim then failed with
        // TokenError::Frozen. The escrow account holds the reward instead.
        new_test_ext().execute_with(|| {
            Balances::set_lock(*b"vesting ", &1, 999_800, WithdrawReasons::all());
            let id = propose_single_best(1, 1, 1);
            spend(1, 4, 100);
            let before = Balances::free_balance(2);
            solve_and_expire(id, 2);
            let after_registration = Balances::free_balance(2);
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                id
            ));
            assert_eq!(Balances::free_balance(2), after_registration + 100);
            assert!(Balances::free_balance(2) > before);
            assert_eq!(escrow_balance(id), 0);
            assert!(!escrow_exists(id));
        });
    }

    #[test]
    fn freeze_created_after_posting_then_spending_still_pays_the_solver() {
        new_test_ext().execute_with(|| {
            let id = propose_single_best(1, 1, 1);
            // A freeze arrives after posting (as a vested transfer would add
            // one) and the proposer spends down to it.
            Balances::set_lock(*b"vesting ", &1, 999_850, WithdrawReasons::all());
            spend(1, 4, 50);
            solve_and_expire(id, 2);
            let before = Balances::free_balance(2);
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                id
            ));
            assert_eq!(Balances::free_balance(2), before + 100);
            assert!(!escrow_exists(id));
        });
    }

    #[test]
    fn reclaim_returns_the_whole_reward_and_reaps_the_escrow() {
        new_test_ext().execute_with(|| {
            let id = propose_single_best(1, 1, 1);
            let after_posting = Balances::free_balance(1);
            assert_eq!(escrow_balance(id), 100);
            // W3-006: refunded by finalisation at expiry, with no call.
            run_to_block(2);
            assert_eq!(Balances::free_balance(1), after_posting + 100);
            assert_eq!(escrow_balance(id), 0);
            assert!(!escrow_exists(id));
        });
    }

    #[test]
    fn escrow_holds_exactly_the_reward_until_close_and_nothing_after() {
        new_test_ext().execute_with(|| {
            // Solved, so they stay `Expired` until claimed (W3-006).
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let ids: Vec<u64> = (0..3).map(|_| propose_single_best(1, 1, 1)).collect();
            for &id in &ids {
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    sample_solution()
                ));
            }
            let check = || {
                for &id in &ids {
                    let order = JobOrders::<Test>::get(id).unwrap();
                    let expected = if order.status == OrderStatus::Closed {
                        0
                    } else {
                        order.reward
                    };
                    assert_eq!(escrow_balance(id), expected, "order {id}");
                }
            };
            check(); // all Open
            run_to_block(2);
            assert!(ids
                .iter()
                .all(|&id| JobOrders::<Test>::get(id).unwrap().status == OrderStatus::Expired));
            check(); // all Expired, unclaimed
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                ids[1]
            ));
            check(); // one Closed
                     // Distinct accounts per order.
            let accounts: alloc::collections::BTreeSet<_> = ids
                .iter()
                .map(|&id| QuantumComputeMempool::escrow_account(id))
                .collect();
            assert_eq!(accounts.len(), ids.len());
        });
    }

    #[test]
    fn a_skipped_sub_ed_share_returns_to_the_proposer_without_dust_loss() {
        new_test_ext().execute_with(|| {
            // ED 40: a 2-winner weighted split of 100 pays 67 and 33, so the
            // 33 share is below ED and skipped. It must reach the proposer,
            // and the escrow must end at exactly 0 with no dust burned.
            ExistentialDeposit::set(40);
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                sample_params(),
                100,
                JobMode::Open,
                RewardResolution::TopNWeighted { n: 2 },
                1,
                1,
                ResultDelivery::OnChainOnly,
            ));
            for (solver, rows) in [(2, sample_solution()), (3, alternate_solution())] {
                assert_ok!(QuantumComputeMempool::register_solver(
                    RuntimeOrigin::signed(solver),
                    MinerType::Cpu
                ));
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(solver),
                    0,
                    rows
                ));
            }
            run_to_block(2);
            let proposer_before = Balances::free_balance(1);
            let solvers_before = Balances::free_balance(2) + Balances::free_balance(3);
            let issuance = Balances::total_issuance();
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                0
            ));
            assert_eq!(Balances::free_balance(1), proposer_before + 33);
            assert_eq!(
                Balances::free_balance(2) + Balances::free_balance(3),
                solvers_before + 67
            );
            assert_eq!(Balances::total_issuance(), issuance, "no dust was burned");
            assert_eq!(escrow_balance(0), 0);
            assert!(!escrow_exists(0));
        });
    }

    #[test]
    fn with_every_share_below_ed_the_whole_reward_returns_and_the_escrow_is_reaped() {
        new_test_ext().execute_with(|| {
            // ED 60: a 2-winner equal split of 100 is 50 and 50, both below
            // ED, so no winner is paid (`paid` is empty). The whole reward goes
            // back to the proposer in one transfer that must be AllowDeath:
            // KeepAlive would refuse to empty the escrow.
            ExistentialDeposit::set(60);
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                sample_params(),
                100,
                JobMode::Open,
                RewardResolution::TopNEqual { n: 2 },
                1,
                1,
                ResultDelivery::OnChainOnly,
            ));
            for (solver, rows) in [(2, sample_solution()), (3, alternate_solution())] {
                assert_ok!(QuantumComputeMempool::register_solver(
                    RuntimeOrigin::signed(solver),
                    MinerType::Cpu
                ));
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(solver),
                    0,
                    rows
                ));
            }
            run_to_block(2);
            let proposer_before = Balances::free_balance(1);
            let solvers_before = Balances::free_balance(2) + Balances::free_balance(3);
            let issuance = Balances::total_issuance();
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                0
            ));
            assert_eq!(Balances::free_balance(1), proposer_before + 100);
            assert_eq!(
                Balances::free_balance(2) + Balances::free_balance(3),
                solvers_before,
                "no winner share is paid"
            );
            assert_eq!(Balances::total_issuance(), issuance, "nothing was burned");
            assert_eq!(escrow_balance(0), 0);
            assert!(!escrow_exists(0), "the escrow is reaped");
        });
    }

    #[test]
    #[should_panic(expected = "MinReward must be at least the existential deposit")]
    fn integrity_test_requires_min_reward_at_least_ed() {
        new_test_ext().execute_with(|| {
            // The mock's MinReward is 10; an ED above it must fail the check.
            ExistentialDeposit::set(11);
            <QuantumComputeMempool as Hooks<u64>>::integrity_test();
        });
    }

    fn dust_lost_events() -> usize {
        System::events()
            .iter()
            .filter(|record| {
                matches!(
                    record.event,
                    RuntimeEvent::Balances(pallet_balances::Event::DustLost { .. })
                )
            })
            .count()
    }

    fn reward_claimed(solver: AccountId) -> Option<u128> {
        System::events()
            .iter()
            .find_map(|record| match &record.event {
                RuntimeEvent::QuantumComputeMempool(crate::Event::RewardClaimed {
                    solver: who,
                    amount,
                    ..
                }) if *who == solver => Some(*amount),
                _ => None,
            })
    }

    /// Reap the proposer's own account (it holds no reserve since W1-006).
    fn reap_proposer() {
        let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&1, 0);
        assert!(!System::account_exists(&1));
    }

    #[test]
    fn a_sub_ed_remainder_the_reaped_proposer_cannot_take_goes_to_rank_zero() {
        new_test_ext().execute_with(|| {
            // ED 40: a 2-winner weighted split of 100 pays 67 and 33; the 33
            // share is skipped. The proposer reaped itself, so it cannot take a
            // 33 remainder: rank 0 gets it, and nothing is burned.
            ExistentialDeposit::set(40);
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                sample_params(),
                100,
                JobMode::Open,
                RewardResolution::TopNWeighted { n: 2 },
                1,
                1,
                ResultDelivery::OnChainOnly,
            ));
            for (solver, rows) in [(2, sample_solution()), (3, alternate_solution())] {
                assert_ok!(QuantumComputeMempool::register_solver(
                    RuntimeOrigin::signed(solver),
                    MinerType::Cpu
                ));
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(solver),
                    0,
                    rows
                ));
            }
            run_to_block(2);
            reap_proposer();
            let (before_2, before_3) = (Balances::free_balance(2), Balances::free_balance(3));
            let issuance = Balances::total_issuance();
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                0
            ));
            assert_eq!(
                Balances::free_balance(2),
                before_2 + 100,
                "rank 0 takes 67 + 33"
            );
            assert_eq!(
                Balances::free_balance(3),
                before_3,
                "the sub-ED share is skipped"
            );
            assert_eq!(Balances::total_balance(&1), 0, "the proposer stays reaped");
            assert_eq!(Balances::total_issuance(), issuance, "no dust was burned");
            assert_eq!(dust_lost_events(), 0);
            assert_eq!(reward_claimed(2), Some(100));
            assert_eq!(Solvers::<Test>::get(2).unwrap().rewards_earned, 100);
            assert_eq!(reward_claimed(3), None);
            assert_eq!(escrow_balance(0), 0);
            assert!(!escrow_exists(0));
        });
    }

    #[test]
    fn several_skipped_shares_and_a_reaped_proposer_all_go_to_rank_zero() {
        new_test_ext().execute_with(|| {
            // ED 10: a 3-winner weighted split of 19 pays 10, 6 and 3. Both
            // the 6 and the 3 are skipped; their 9 is below ED and the proposer
            // is reaped, so rank 0 takes the whole 19.
            ExistentialDeposit::set(10);
            let spec = register_spec();
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(1),
                spec,
                chain_params(3),
                19,
                JobMode::Open,
                RewardResolution::TopNWeighted { n: 3 },
                1,
                1,
                ResultDelivery::OnChainOnly,
            ));
            // Energies on the 3-chain: k = 0 is -2000, k = 2 is 0, k = 1 is 2000.
            for (solver, k) in [(2, 0), (3, 2), (4, 1)] {
                assert_ok!(QuantumComputeMempool::register_solver(
                    RuntimeOrigin::signed(solver),
                    MinerType::Cpu
                ));
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(solver),
                    0,
                    chain_row(3, k)
                ));
            }
            run_to_block(2);
            assert_eq!(
                crate::OrderTopSolvers::<Test>::get(0)
                    .iter()
                    .map(|entry| entry.solver)
                    .collect::<Vec<_>>(),
                vec![2, 3, 4]
            );
            reap_proposer();
            let before: Vec<u128> = [2, 3, 4].map(Balances::free_balance).to_vec();
            let issuance = Balances::total_issuance();
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                0
            ));
            let after: Vec<u128> = [2, 3, 4].map(Balances::free_balance).to_vec();
            assert_eq!(after, vec![before[0] + 19, before[1], before[2]]);
            assert_eq!(Balances::total_issuance(), issuance, "no dust was burned");
            assert_eq!(dust_lost_events(), 0);
            assert_eq!(reward_claimed(2), Some(19));
            assert_eq!(escrow_balance(0), 0);
            assert!(!escrow_exists(0));
        });
    }
}

/// W1-012: one place per distinct answer, enforced through the OrderAnswers
/// index (one entry per solver: its current canonical answer).
mod w1_012 {
    use super::*;
    use crate::OrderAnswers;
    use codec::Encode;
    use frame_support::traits::Currency;
    use sp_runtime::DispatchResult;

    fn solver(who: AccountId) {
        let _ = Balances::make_free_balance_be(&who, 1_000_000);
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(who),
            MinerType::Cpu
        ));
    }

    fn propose(params: crate::IsingParamsOf<Test>, resolution: RewardResolution) -> u64 {
        let id = NextOrderId::<Test>::get();
        let spec = match JobSpecs::<Test>::iter_keys().next() {
            Some(spec) => spec,
            None => register_spec(),
        };
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec,
            params,
            100,
            JobMode::Open,
            resolution,
            2,
            1,
            ResultDelivery::OnChainOnly,
        ));
        id
    }

    fn submit(who: AccountId, id: u64, rows: crate::SolutionsOf<Test>) -> DispatchResult {
        QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(who), id, rows)
            .map(|_| ())
            .map_err(|error| error.error)
    }

    fn row(spins: &[i8]) -> crate::SolutionsOf<Test> {
        bounded(vec![bounded(spins.to_vec())])
    }

    /// The index holds exactly each stored solver's answer, mapped to it.
    fn assert_index_consistent(id: u64) {
        let mut stored = 0;
        for (who, entry) in OrderSolutions::<Test>::iter_prefix(id) {
            assert_eq!(
                OrderAnswers::<Test>::get(id, entry.answer).map(|owner| owner.solver),
                Some(who)
            );
            stored += 1;
        }
        assert_eq!(OrderAnswers::<Test>::iter_prefix(id).count(), stored);
    }

    /// No raw key remains under twox128(pallet) ++ twox128(OrderAnswers) ++
    /// blake2_128_concat(order), with the prefix built from literals.
    fn assert_no_answer_keys(id: u64) {
        use sp_core::hashing::{blake2_128, twox_128};
        let mut prefix = Vec::new();
        prefix.extend(twox_128(b"QuantumComputeMempool"));
        prefix.extend(twox_128(b"OrderAnswers"));
        prefix.extend(blake2_128(&id.encode()));
        prefix.extend(id.encode());
        let next = sp_io::storage::next_key(&prefix);
        assert!(
            !next.is_some_and(|key| key.starts_with(&prefix)),
            "answer index entries remain for order {id}"
        );
    }

    #[test]
    fn answer_index_key_layout_is_pinned() {
        use sp_core::hashing::{blake2_128, twox_128};
        new_test_ext().execute_with(|| {
            let (id, answer) = (7_u64, sp_core::H256::repeat_byte(0xAB));
            let mut expected = Vec::new();
            expected.extend(twox_128(b"QuantumComputeMempool"));
            expected.extend(twox_128(b"OrderAnswers"));
            expected.extend(blake2_128(&id.encode()));
            expected.extend(id.encode());
            expected.extend(blake2_128(&answer.encode()));
            expected.extend(answer.encode());
            assert_eq!(OrderAnswers::<Test>::hashed_key_for(id, answer), expected);
        });
    }

    #[test]
    fn duplicate_answers_are_rejected_across_solvers_ranked_or_not() {
        new_test_ext().execute_with(|| {
            let id = propose(chain_params(4), RewardResolution::SingleBest);
            for who in [2, 3, 4] {
                solver(who);
            }
            // Solver 2 holds a worse answer and is not ranked once solver 3
            // takes the front-runner slot.
            assert_ok!(submit(2, id, chain_row(4, 3)));
            assert_ok!(submit(3, id, chain_row(4, 0)));
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 3);
            for taken in [3, 0] {
                assert_noop!(
                    submit(4, id, chain_row(4, taken)),
                    crate::Error::<Test>::DuplicateAnswer
                );
            }
            // A distinct answer is accepted.
            assert_ok!(submit(4, id, chain_row(4, 5)));
            assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 3);
            assert_index_consistent(id);
        });
    }

    #[test]
    fn canonical_equivalents_are_one_answer() {
        new_test_ext().execute_with(|| {
            // Nodes 0-1-2 chained; node 3 has no field and no coupling.
            let mut params = chain_params(4);
            params.edges = bounded(vec![(0, 1), (1, 2)]);
            params.j_values = bounded(vec![-1_000, -1_000]);
            let id = propose(params, RewardResolution::TopNEqual { n: 3 });
            for who in [2, 3] {
                solver(who);
            }
            assert_ok!(submit(2, id, row(&[1, -1, 1, 1])));
            for equivalent in [
                [-1, 1, -1, -1], // global flip
                [1, -1, 1, -1],  // isolated spin differs
                [-1, 1, -1, 1],  // both
            ] {
                assert_noop!(
                    submit(3, id, row(&equivalent)),
                    crate::Error::<Test>::DuplicateAnswer
                );
            }
            assert_ok!(submit(3, id, row(&[1, 1, -1, 1])));
            assert_index_consistent(id);
        });
    }

    /// The answer a submission is indexed under is the canonical form of its
    /// BEST row, wherever that row sits in the submission. Every other W1-012
    /// test submits one row, where row 0 is the best row; here the best row
    /// is the middle one, and flipped, so keying the first row or the last
    /// row both fail.
    #[test]
    fn the_answer_key_is_the_best_row_wherever_it_is_submitted() {
        new_test_ext().execute_with(|| {
            let id = propose(chain_params(4), RewardResolution::TopNEqual { n: 3 });
            for who in [2, 3, 4] {
                solver(who);
            }
            let worse: &[i8] = &[1, -1, -1, 1]; // two domain walls
            let ground_flipped: &[i8] = &[-1, -1, -1, -1];
            let one_wall: &[i8] = &[1, 1, -1, -1]; // one domain wall
            assert_ok!(submit(
                2,
                id,
                bounded(vec![
                    bounded(worse.to_vec()),
                    bounded(ground_flipped.to_vec()),
                    bounded(one_wall.to_vec())
                ])
            ));
            // Solver 2's answer is the ground state, in canonical form ...
            assert_noop!(
                submit(3, id, row(&[1, 1, 1, 1])),
                crate::Error::<Test>::DuplicateAnswer
            );
            // ... and neither of its other rows holds a place in the index.
            assert_ok!(submit(3, id, row(worse)));
            assert_ok!(submit(4, id, row(one_wall)));
            assert_index_consistent(id);
        });
    }

    #[test]
    fn same_solver_same_answer_meets_the_improvement_rule() {
        new_test_ext().execute_with(|| {
            let id = propose(chain_params(4), RewardResolution::SingleBest);
            solver(2);
            assert_ok!(submit(2, id, chain_row(4, 3)));
            // The flip of its own row is its own answer: not a duplicate.
            assert_noop!(
                submit(2, id, row(&[-1, 1, -1, -1])),
                crate::Error::<Test>::SolutionNotImproved
            );
        });
    }

    #[test]
    fn improvement_frees_the_previous_answer() {
        new_test_ext().execute_with(|| {
            let id = propose(chain_params(4), RewardResolution::TopNEqual { n: 3 });
            for who in [2, 3] {
                solver(who);
            }
            let old = chain_row(4, 3);
            assert_ok!(submit(2, id, old.clone()));
            assert_noop!(
                submit(3, id, old.clone()),
                crate::Error::<Test>::DuplicateAnswer
            );
            assert_ok!(submit(2, id, chain_row(4, 0)));
            assert_eq!(OrderAnswers::<Test>::iter_prefix(id).count(), 1);
            assert_ok!(submit(3, id, old));
            assert_index_consistent(id);
        });
    }

    #[test]
    fn equal_energy_distinct_answer_does_not_displace_the_incumbent() {
        new_test_ext().execute_with(|| {
            // On a 3-chain, k = 2 ([1, 1, -1]) and k = 3 ([1, -1, -1]) each have
            // one domain wall: equal energy, different answers.
            let id = propose(chain_params(3), RewardResolution::SingleBest);
            for who in [2, 3] {
                solver(who);
            }
            assert_ok!(submit(2, id, chain_row(3, 2)));
            assert_ok!(submit(3, id, chain_row(3, 3)));
            assert_eq!(
                OrderSolutions::<Test>::get(id, 2)
                    .unwrap()
                    .best_energy_milli,
                OrderSolutions::<Test>::get(id, 3)
                    .unwrap()
                    .best_energy_milli
            );
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 2);
        });
    }

    #[test]
    fn expiry_and_retention_pruning_clear_the_answer_index() {
        // The answers are COMMITTED before each clear: `clear_prefix`'s limit
        // applies to backend keys only and removes overlay keys regardless, so
        // without the commits a limit of 1 would pass this test.
        let mut ext = new_test_ext();
        let id = ext.execute_with(|| {
            let id = propose(chain_params(4), RewardResolution::TopNEqual { n: 2 });
            for who in [2, 3] {
                solver(who);
            }
            assert_ok!(submit(2, id, chain_row(4, 1)));
            assert_ok!(submit(3, id, chain_row(4, 2)));
            assert_eq!(OrderAnswers::<Test>::iter_prefix(id).count(), 2);
            id
        });
        ext.commit_all().expect("commit");
        ext.execute_with(|| {
            // Expiry finalises the order and prunes its solutions and answers.
            run_to_block(2);
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Expired
            );
            assert_no_answer_keys(id);
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                id
            ));
            assert_no_answer_keys(id);
            // Retention pruning of the closed order also clears the index
            // (defensively: plant stray entries first).
            OrderAnswers::<Test>::insert(
                id,
                sp_core::H256::repeat_byte(1),
                crate::types::AnswerOwner {
                    solver: 2,
                    best_energy_milli: 0,
                    effective_at: 0,
                    seq: 0,
                },
            );
            OrderAnswers::<Test>::insert(
                id,
                sp_core::H256::repeat_byte(2),
                crate::types::AnswerOwner {
                    solver: 3,
                    best_energy_milli: 0,
                    effective_at: 0,
                    seq: 0,
                },
            );
        });
        ext.commit_all().expect("commit");
        ext.execute_with(|| {
            run_to_block(System::block_number() + 21);
            assert!(!JobOrders::<Test>::contains_key(id));
            assert_no_answer_keys(id);
        });
    }

    #[test]
    fn answer_index_matches_solutions_after_a_random_sequence() {
        new_test_ext().execute_with(|| {
            let id = propose(chain_params(5), RewardResolution::TopNWeighted { n: 4 });
            let solvers: Vec<AccountId> = (10..18).collect();
            for &who in &solvers {
                solver(who);
            }
            let mut state = 0x2545_f491_4f6c_dd1d_u64;
            let (mut accepted, mut duplicates) = (0, 0);
            for _ in 0..400 {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let who = solvers[(state % solvers.len() as u64) as usize];
                let k = ((state >> 8) % 16) as u32;
                let flip = (state >> 16) & 1 == 1;
                let mut rows = chain_row(5, k);
                if flip {
                    for spin in rows[0].iter_mut() {
                        *spin = -*spin;
                    }
                }
                match submit(who, id, rows) {
                    Ok(()) => accepted += 1,
                    Err(error) if error == crate::Error::<Test>::DuplicateAnswer.into() => {
                        duplicates += 1
                    }
                    Err(error) => {
                        assert_eq!(error, crate::Error::<Test>::SolutionNotImproved.into())
                    }
                }
                assert_index_consistent(id);
            }
            // The sequence exercises both accept and duplicate paths.
            assert!(accepted >= solvers.len() && duplicates > 0);
            // Each stored answer hashes the canonical form of its best row.
            for (_, entry) in OrderSolutions::<Test>::iter_prefix(id) {
                let params = JobOrders::<Test>::get(id).unwrap().ising_params;
                let topology = quantum_validation::TopologyIndex::new(
                    params.nodes.as_slice(),
                    params.edges.as_slice(),
                )
                .unwrap();
                let best = entry
                    .solutions
                    .iter()
                    .find(|row| {
                        quantum_validation::energy_of_solution_indexed(
                            row.as_slice(),
                            params.h_values.as_slice(),
                            params.j_values.as_slice(),
                            &topology,
                        )
                        .unwrap()
                            == entry.best_energy_milli
                    })
                    .unwrap();
                let canonical = quantum_validation::canonical_answer_row(
                    best.as_slice(),
                    params.h_values.as_slice(),
                    params.j_values.as_slice(),
                    &topology,
                )
                .unwrap();
                assert_eq!(
                    entry.answer,
                    sp_core::H256(sp_core::hashing::blake2_256(&canonical.encode()))
                );
            }
        });
    }
}

/// W3-006 (design §B4 v2) -- the job-order storage deposit.
mod w3_006_deposit {
    use super::*;
    use crate::OrderDeposit;
    use codec::{Encode, MaxEncodedLen};
    use frame_support::traits::Currency;

    fn set_rates() {
        OrderDepositPerItem::set(10);
        OrderDepositPerByte::set(1);
    }

    fn released(order_id: u64) -> Vec<(AccountId, u128)> {
        System::events()
            .into_iter()
            .filter_map(|record| match record.event {
                RuntimeEvent::QuantumComputeMempool(crate::Event::OrderDepositReleased {
                    order_id: id,
                    proposer,
                    amount,
                }) if id == order_id => Some((proposer, amount)),
                _ => None,
            })
            .collect()
    }

    /// The deposit is reserved at `propose_job`, is still held after the order
    /// closes (it covers the record through the retention window), and is
    /// released exactly once when the order is pruned, with the amount in the
    /// event.
    #[test]
    fn reserved_at_propose_and_released_once_at_prune() {
        new_test_ext().execute_with(|| {
            set_rates();
            let reserved = Balances::reserved_balance(1);
            let id = propose_single_best(1, 1, 1);
            let deposit = OrderDeposit::<Test>::get(id).expect("a deposit");
            let order = JobOrders::<Test>::get(id).unwrap();
            assert_eq!(deposit, QuantumComputeMempool::order_deposit(&order));
            assert!(
                deposit > 10 + order.encoded_size() as u128,
                "the state bound is priced"
            );
            assert_eq!(Balances::reserved_balance(1), reserved + deposit);

            run_to_block(2);
            // Closed (refunded) at expiry, W3-006.
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Closed
            );
            assert_eq!(
                Balances::reserved_balance(1),
                reserved + deposit,
                "still held after close"
            );
            assert!(released(id).is_empty());

            run_to_block(22);
            assert!(JobOrders::<Test>::get(id).is_none(), "pruned");
            assert!(OrderDeposit::<Test>::get(id).is_none());
            assert_eq!(Balances::reserved_balance(1), reserved);
            assert_eq!(released(id), vec![(1, deposit)]);
            run_to_block(30);
            assert_eq!(released(id).len(), 1, "released once");
        });
    }

    /// B4a2 (kimi R8): the deposit covers the record as it is after it has
    /// grown -- here, `first_solution_at` set by the first accepted solution
    /// -- plus the counted side state at its bound.
    #[test]
    fn deposit_covers_the_order_after_a_solution_sets_first_solution_at() {
        new_test_ext().execute_with(|| {
            set_rates();
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let id = propose_single_best(1, 1, 1);
            let proposed_size = JobOrders::<Test>::get(id).unwrap().encoded_size();
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                id,
                sample_solution()
            ));
            let order = JobOrders::<Test>::get(id).unwrap();
            assert!(order.first_solution_at.is_some());
            assert!(order.encoded_size() > proposed_size, "the record grew");

            let payee = u64::max_encoded_len()
                + AccountId::max_encoded_len()
                + crate::types::WinnerPayee::max_encoded_len();
            let side = crate::TopSolversOf::<Test>::max_encoded_len()
                .max(crate::FrontRunnerOf::<Test>::max_encoded_len())
                + payee * crate::types::MAX_SOLVERS_PER_OPEN_ORDER as usize
                + u64::max_encoded_len()
                // `NextSubmissionSeq`: a u64 key and a u32 value (W3-001).
                + u64::max_encoded_len()
                + u32::max_encoded_len();
            let needed = 10 + (order.encoded_size() + side) as u128;
            let deposit = OrderDeposit::<Test>::get(id).unwrap();
            assert!(deposit >= needed, "deposit {deposit} < needed {needed}");
            assert_eq!(Balances::reserved_balance(1), deposit);
        });
    }

    /// `OrderResults` is priced only for `CallbackWithPoll`: the same order
    /// with a polled callback costs exactly that state's bound more.
    #[test]
    fn callback_with_poll_prices_the_stored_result() {
        new_test_ext().execute_with(|| {
            set_rates();
            let id = propose_single_best(1, 1, 1);
            let mut order = JobOrders::<Test>::get(id).unwrap();
            let endpoint: BoundedVec<u8, _> = bounded(b"https://example.invalid".to_vec());
            order.delivery = ResultDelivery::Callback {
                endpoint: endpoint.clone(),
            };
            let callback = QuantumComputeMempool::order_deposit(&order);
            order.delivery = ResultDelivery::CallbackWithPoll { endpoint };
            let polled = QuantumComputeMempool::order_deposit(&order);
            assert_eq!(
                polled - callback,
                crate::StoredResultOf::<Test>::max_encoded_len() as u128
            );
        });
    }

    /// A proposer funded with exactly reward + deposit + ED can propose; one
    /// unit less cannot reserve the deposit.
    #[test]
    fn exact_funding_covers_reward_deposit_and_ed() {
        new_test_ext().execute_with(|| {
            set_rates();
            ExistentialDeposit::set(10);
            // The deposit of the same order, measured from a funded proposer;
            // account ids have one encoded size, so it is the same for 77/78.
            let deposit = OrderDeposit::<Test>::get(propose_single_best(1, 1, 1)).unwrap();
            let need = 100 + deposit + 10;
            Balances::make_free_balance_be(&77, need);
            propose_single_best(77, 1, 1);
            assert_eq!(Balances::free_balance(77), 10);
            assert_eq!(Balances::reserved_balance(77), deposit);

            Balances::make_free_balance_be(&78, need - 1);
            let spec_id = JobSpecs::<Test>::iter_keys().next().unwrap();
            assert_noop!(
                QuantumComputeMempool::propose_job(
                    RuntimeOrigin::signed(78),
                    spec_id,
                    sample_params(),
                    100,
                    JobMode::Open,
                    RewardResolution::SingleBest,
                    1,
                    1,
                    ResultDelivery::OnChainOnly,
                ),
                crate::Error::<Test>::DepositNotAvailable
            );
        });
    }
}

/// W3-006 (design §B4 v2 item 2) -- unsolved orders close at expiry.
mod w3_006_auto_close {
    use super::*;
    use crate::weights::WeightInfo as _;

    fn mempool_events() -> Vec<crate::Event<Test>> {
        System::events()
            .into_iter()
            .filter_map(|record| match record.event {
                RuntimeEvent::QuantumComputeMempool(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    /// No call needed: finalisation refunds the escrow, closes the order,
    /// releases the slot, queues pruning, and emits `reclaim_order`'s events;
    /// the retention sweep then deletes it.
    #[test]
    fn unsolved_order_is_refunded_closed_and_pruned_with_no_call() {
        new_test_ext().execute_with(|| {
            let id = propose_single_best(1, 1, 1);
            let after_posting = Balances::free_balance(1);
            let escrow = QuantumComputeMempool::escrow_account(id);
            System::reset_events();
            run_to_block(2);

            let order = JobOrders::<Test>::get(id).unwrap();
            assert_eq!(order.status, OrderStatus::Closed);
            assert_eq!(Balances::free_balance(1), after_posting + 100);
            assert_eq!(escrow_balance(id), 0);
            assert!(!OpenOrders::<Test>::contains_key(id));
            assert!(!crate::ProposerOrders::<Test>::contains_key(1));
            let prune_at = 2 + OrderRetentionBlocks::get();
            assert!(crate::OrdersPrunableAt::<Test>::contains_key(prune_at, id));
            let events = mempool_events();
            let tail = &events[events.len() - 4..];
            assert_eq!(
                tail,
                &[
                    crate::Event::OrderExpired { order_id: id },
                    crate::Event::EscrowReleased {
                        order_id: id,
                        escrow,
                        to_proposer: 100
                    },
                    crate::Event::RewardReclaimed {
                        order_id: id,
                        proposer: 1,
                        amount: 100
                    },
                    crate::Event::OrderClosed {
                        order_id: id,
                        successful: false
                    },
                ]
            );
            assert_noop!(
                QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(1), id),
                crate::Error::<Test>::AlreadyClaimed
            );

            run_to_block(prune_at);
            assert!(JobOrders::<Test>::get(id).is_none(), "pruned with no call");
        });
    }

    /// A solved order is left `Expired` with its reward in escrow, for the
    /// permissionless `claim_reward` (design §B4 v2 item 4).
    #[test]
    fn solved_order_is_not_auto_closed() {
        new_test_ext().execute_with(|| {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(2),
                MinerType::Cpu
            ));
            let id = propose_single_best(1, 1, 1);
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                id,
                sample_solution()
            ));
            System::reset_events();
            run_to_block(2);
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Expired
            );
            assert_eq!(escrow_balance(id), 100);
            assert!(crate::ProposerOrders::<Test>::get(1).contains(&id));
            assert!(!mempool_events()
                .iter()
                .any(|e| matches!(e, crate::Event::RewardReclaimed { .. })));
        });
    }

    /// Transfer first: when the escrow cannot pay the refund (inconsistent
    /// storage), nothing is closed. The order stays `Expired`, holding its slot
    /// and unqueued, and `reclaim_order` remains the manual fallback. The
    /// failure is `defensive!`, which panics in debug builds; the state is
    /// checked in release (the workspace gate runs `--release`).
    #[test]
    #[cfg_attr(
        debug_assertions,
        should_panic(expected = "unsolved order refund failed")
    )]
    fn unsolved_refund_failure_leaves_the_order_expired_for_reclaim() {
        new_test_ext().execute_with(|| {
            let id = propose_single_best(1, 1, 1);
            let escrow = QuantumComputeMempool::escrow_account(id);
            let _ = <Balances as frame_support::traits::Currency<AccountId>>::slash(&escrow, 1);
            System::reset_events();
            run_to_block(2);

            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Expired
            );
            assert_eq!(escrow_balance(id), 99);
            assert!(crate::ProposerOrders::<Test>::get(1).contains(&id));
            assert_eq!(crate::OrdersPrunableAt::<Test>::iter().count(), 0);
            assert!(!mempool_events().iter().any(|e| matches!(
                e,
                crate::Event::EscrowReleased { .. }
                    | crate::Event::RewardReclaimed { .. }
                    | crate::Event::OrderClosed { .. }
            )));
            assert_noop!(
                QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(1), id),
                crate::Error::<Test>::RewardReserveMissing
            );
            assert_ok!(
                <Balances as frame_support::traits::Currency<AccountId>>::transfer(
                    &4,
                    &escrow,
                    1,
                    frame_support::traits::ExistenceRequirement::AllowDeath
                )
            );
            assert_ok!(QuantumComputeMempool::reclaim_order(
                RuntimeOrigin::signed(1),
                id
            ));
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Closed
            );
            assert!(!crate::ProposerOrders::<Test>::contains_key(1));
        });
    }

    /// `on_initialize` charges the auto-close allowance (transfer,
    /// `ProposerOrders` mutate, pruning insert) on every finalised order, on
    /// top of the measured finalisation and solution pruning.
    #[test]
    fn on_initialize_charges_the_auto_close_per_finalised_order() {
        new_test_ext().execute_with(|| {
            let ids = propose_nine_orders_expiring_at_block_2();
            System::set_block_number(2);
            let charged = QuantumComputeMempool::on_initialize(2);
            assert_eq!(status_count(&ids, OrderStatus::Closed), 8);
            let floor = <<Test as crate::Config>::WeightInfo>::finalise_expired_orders(8)
                .saturating_add(crate::weights::solution_pruning_weight::<Test>().saturating_mul(8))
                .saturating_add(<<Test as crate::Config>::WeightInfo>::auto_close_unsolved(
                    8,
                ));
            assert!(charged.all_gte(floor), "charged {charged:?} < {floor:?}");
        });
    }
}

/// W3-006 (design §B4 v2 item 3) -- lazy finalisation via `bucket_of`.
mod w3_006_lazy {
    use super::*;

    fn order(id: u64) -> crate::JobOrderOf<Test> {
        JobOrders::<Test>::get(id).unwrap()
    }

    /// The order's only queue key, wherever it is.
    fn queued_at(id: u64) -> Vec<u64> {
        OrdersExpiringAt::<Test>::iter_keys()
            .filter(|(_, order_id)| *order_id == id)
            .map(|(bucket, _)| bucket)
            .collect()
    }

    fn solver() {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(2),
            MinerType::Cpu
        ));
    }

    fn solve(id: u64) {
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            id,
            sample_solution()
        ));
    }

    /// `bucket_of` names the order's queue key in every scheduling case, and
    /// never depends on the current block.
    #[test]
    fn bucket_of_names_the_queue_key_in_every_case() {
        new_test_ext().execute_with(|| {
            solver();
            // (deadline, block_wait, solved at, expected bucket); proposed at 1.
            let cases = [
                // Unsolved: the hard deadline E = 1 + 5.
                (5, 3, None, 6),
                // Solved at 2, closes_at = 4 < hard 6: E = 4.
                (5, 2, Some(2), 4),
                // Solved at 2 with block_wait 0: E = 2, scheduled at 2 + 1.
                (5, 0, Some(2), 3),
                // Solved at 2, closes_at = min(6, 2 + 9) = hard: unmoved, E = 6.
                (5, 9, Some(2), 6),
            ];
            let mut ids = Vec::new();
            for (deadline, wait, _, _) in cases {
                ids.push(propose_single_best(1, deadline, wait));
            }
            System::set_block_number(2);
            for (&id, (_, _, solved_at, _)) in ids.iter().zip(cases) {
                if solved_at.is_some() {
                    solve(id);
                }
            }
            for later in [2, 50] {
                System::set_block_number(later);
                for (&id, (_, _, _, expected)) in ids.iter().zip(cases) {
                    assert_eq!(queued_at(id), vec![expected], "order {id}");
                    assert_eq!(QuantumComputeMempool::bucket_of(&order(id)), expected);
                }
            }
        });
    }

    /// `reclaim_order` on a due unsolved order the queue has not reached (no
    /// hooks run): finalised inline, auto-closed, refunded, `Ok`, key gone.
    #[test]
    fn lazy_reclaim_returns_ok_after_the_inline_auto_close() {
        new_test_ext().execute_with(|| {
            let id = propose_single_best(1, 1, 1);
            let after_posting = Balances::free_balance(1);
            let bucket = QuantumComputeMempool::bucket_of(&order(id));
            System::set_block_number(5);
            assert!(OrdersExpiringAt::<Test>::contains_key(bucket, id));
            assert_eq!(order(id).status, OrderStatus::Opened);

            assert_ok!(QuantumComputeMempool::reclaim_order(
                RuntimeOrigin::signed(1),
                id
            ));
            assert_eq!(order(id).status, OrderStatus::Closed);
            assert_eq!(Balances::free_balance(1), after_posting + 100);
            assert!(queued_at(id).is_empty());
            assert!(!crate::ProposerOrders::<Test>::contains_key(1));
            assert!(crate::OrdersPrunableAt::<Test>::contains_key(
                5 + OrderRetentionBlocks::get(),
                id
            ));
        });
    }

    /// `claim_reward` on a due solved order the queue has not reached:
    /// finalised inline to `Expired` (solutions pruned), then settled from
    /// there in the same call.
    #[test]
    fn lazy_claim_continues_from_expired() {
        new_test_ext().execute_with(|| {
            solver();
            let id = propose_single_best(1, 3, 1);
            System::set_block_number(2);
            solve(id);
            let bucket = QuantumComputeMempool::bucket_of(&order(id));
            assert_eq!(bucket, 3);
            System::set_block_number(9);
            assert!(OrdersExpiringAt::<Test>::contains_key(bucket, id));
            let before = Balances::free_balance(2);
            System::reset_events();

            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(4),
                id
            ));
            assert_eq!(order(id).status, OrderStatus::Closed);
            assert_eq!(Balances::free_balance(2), before + 100);
            assert!(queued_at(id).is_empty());
            assert!(crate::OrderSolutions::<Test>::iter_key_prefix(id)
                .next()
                .is_none());
            let events: Vec<_> = System::events()
                .into_iter()
                .filter_map(|record| match record.event {
                    RuntimeEvent::QuantumComputeMempool(event) => Some(event),
                    _ => None,
                })
                .collect();
            let expired = events
                .iter()
                .position(|e| matches!(e, QuantumComputeMempoolEvent::OrderExpired { .. }));
            let closed = events
                .iter()
                .position(|e| matches!(e, QuantumComputeMempoolEvent::OrderClosed { .. }));
            assert!(
                expired.unwrap() < closed.unwrap(),
                "finalised, then settled"
            );
        });
    }

    /// Wrong-kind lazy calls fail before finalising anything: the order stays
    /// `Opened` and queued.
    #[test]
    fn lazy_calls_of_the_wrong_kind_change_nothing() {
        new_test_ext().execute_with(|| {
            solver();
            let unsolved = propose_single_best(1, 1, 1);
            let solved = propose_single_best(1, 2, 1);
            System::set_block_number(2);
            solve(solved);
            System::set_block_number(9);
            assert_noop!(
                QuantumComputeMempool::claim_reward(RuntimeOrigin::signed(4), unsolved),
                crate::Error::<Test>::NoSolutionsAccepted
            );
            assert_noop!(
                QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(1), solved),
                crate::Error::<Test>::NoSolutionsAccepted
            );
            assert_noop!(
                QuantumComputeMempool::reclaim_order(RuntimeOrigin::signed(4), unsolved),
                crate::Error::<Test>::NotProposer
            );
            for id in [unsolved, solved] {
                assert_eq!(order(id).status, OrderStatus::Opened);
                assert_eq!(queued_at(id).len(), 1);
            }
        });
    }
}

/// W3-001 (design §B5 v2 item 8) -- the minimum block wait.
mod w3_001_min_block_wait {
    use super::*;

    fn propose_with_wait(block_wait: u64) -> frame_support::dispatch::DispatchResult {
        let spec_id = match JobSpecs::<Test>::iter_keys().next() {
            Some(spec_id) => spec_id,
            None => register_spec(),
        };
        QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            sample_params(),
            100,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            block_wait,
            ResultDelivery::OnChainOnly,
        )
    }

    /// At the runtimes' minimum of 2, a block wait of 1 is refused and 2 is
    /// admitted.
    #[test]
    fn block_wait_below_the_minimum_is_refused() {
        new_test_ext().execute_with(|| {
            MinBlockWait::set(2);
            register_spec();
            for short in [0, 1] {
                assert_noop!(
                    propose_with_wait(short),
                    crate::Error::<Test>::BlockWaitTooShort
                );
            }
            assert_ok!(propose_with_wait(2));
            assert_eq!(JobOrders::<Test>::get(0).unwrap().timing.block_wait, 2);
        });
    }

    #[test]
    #[should_panic(expected = "MinBlockWait must not exceed MaxBlockWait")]
    fn integrity_test_requires_min_block_wait_at_most_max() {
        new_test_ext().execute_with(|| {
            // The mock's MaxBlockWait is 20.
            MinBlockWait::set(21);
            <QuantumComputeMempool as Hooks<u64>>::integrity_test();
        });
    }

    #[test]
    fn integrity_test_accepts_min_block_wait_equal_to_max() {
        new_test_ext().execute_with(|| {
            MinBlockWait::set(20);
            <QuantumComputeMempool as Hooks<u64>>::integrity_test();
        });
    }
}

/// W3-001 B5b (design §B5 v2, N2) -- the ordering key.
mod w3_001_ordering_key {
    use super::*;

    fn propose(resolution: RewardResolution) -> u64 {
        let spec_id = register_spec();
        let order_id = NextOrderId::<Test>::get();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            chain_params(4),
            100,
            JobMode::Open,
            resolution,
            10,
            5,
            ResultDelivery::OnChainOnly,
        ));
        order_id
    }

    fn submit(who: AccountId, order_id: u64, k: u32) {
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(who),
            order_id,
            chain_row(4, k)
        ));
    }

    fn register(who: AccountId) {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(who),
            MinerType::Cpu
        ));
    }

    /// The architect's tie-break (R8 note on the B5 cut): A submits at +1000
    /// ("7"), B at -1000 ("5"), then A improves to -1000 with a different
    /// answer. Whoever first reached an energy ranks ahead: B, under every
    /// resolution. (This is today's order too: an improved entry was already
    /// behind every entry at its new energy; the key makes it explicit.)
    #[test]
    fn an_improved_resubmission_ranks_after_an_earlier_equal_energy() {
        for resolution in [
            RewardResolution::SingleBest,
            RewardResolution::TopNEqual { n: 2 },
            RewardResolution::TopNWeighted { n: 2 },
        ] {
            new_test_ext().execute_with(|| {
                register(2);
                register(3);
                let id = propose(resolution);
                let (a, b) = (2, 3);
                submit(a, id, 1);
                submit(b, id, 4);
                submit(a, id, 6);
                let energy = |who| {
                    OrderSolutions::<Test>::get(id, who)
                        .unwrap()
                        .best_energy_milli
                };
                assert_eq!((energy(a), energy(b)), (-1_000, -1_000));
                match resolution {
                    RewardResolution::SingleBest => {
                        assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, b);
                    }
                    _ => {
                        let order: Vec<_> = crate::OrderTopSolvers::<Test>::get(id)
                            .iter()
                            .map(|entry| entry.solver)
                            .collect();
                        assert_eq!(order, vec![b, a], "{resolution:?}");
                    }
                }
            });
        }
    }

    /// M1 (R8 on B5b): SingleBest compares the whole key. A candidate at the
    /// front runner's energy with an EARLIER `effective_at` but a LATER `seq`
    /// replaces it; a later `effective_at` does not. (Crafted keys: until B5d
    /// a submission's `effective_at` is its arrival block.)
    #[test]
    fn single_best_replaces_on_an_earlier_effective_time_at_equal_energy() {
        new_test_ext().execute_with(|| {
            let id = 7;
            let front = |solver: u128, effective_at: u64, seq: u32| crate::types::FrontRunner {
                solver,
                energy_milli: -1_000,
                effective_at,
                seq,
            };
            OrderFrontRunner::<Test>::insert(id, front(2, 10, 0));
            let answer = sp_core::H256::repeat_byte(9);
            let single = RewardResolution::SingleBest;
            assert_ok!(QuantumComputeMempool::update_ranking(
                id, &single, &3, -1_000, answer, 12, 5
            ));
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap(), front(2, 10, 0));
            assert_ok!(QuantumComputeMempool::update_ranking(
                id, &single, &3, -1_000, answer, 3, 5
            ));
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap(), front(3, 3, 5));
        });
    }

    /// Each accepted submission takes the order's next `seq` and, uncommitted,
    /// its arrival block as `effective_at`; the solution, the answer index and
    /// the ranking all carry them, and the counter is cleared with the
    /// solutions at finalisation.
    #[test]
    fn submissions_carry_effective_at_and_seq_and_the_counter_is_cleared() {
        new_test_ext().execute_with(|| {
            register(2);
            register(3);
            let id = propose(RewardResolution::TopNEqual { n: 2 });
            submit(2, id, 1);
            run_to_block(2);
            submit(3, id, 4);
            submit(2, id, 6);
            assert_eq!(crate::NextSubmissionSeq::<Test>::get(id), 3);
            for (who, k_seq, at) in [(3u128, 1u32, 2u64), (2, 2, 2)] {
                let solution = OrderSolutions::<Test>::get(id, who).unwrap();
                assert_eq!((solution.seq, solution.effective_at), (k_seq, at), "{who}");
                let owner = crate::OrderAnswers::<Test>::get(id, solution.answer).unwrap();
                assert_eq!(
                    (
                        owner.solver,
                        owner.best_energy_milli,
                        owner.effective_at,
                        owner.seq
                    ),
                    (who, solution.best_energy_milli, at, k_seq)
                );
                let ranked = crate::OrderTopSolvers::<Test>::get(id)
                    .into_iter()
                    .find(|entry| entry.solver == who)
                    .unwrap();
                assert_eq!((ranked.effective_at, ranked.seq), (at, k_seq));
            }
            run_to_block(20);
            assert_eq!(
                JobOrders::<Test>::get(id).unwrap().status,
                OrderStatus::Expired
            );
            assert!(!crate::NextSubmissionSeq::<Test>::contains_key(id));
        });
    }
}

/// W3-001 B5b2 -- storage version 6 for B5b's layout change, and the upgrade
/// guard that leaves a chain with orders untouched.
mod w3_001_storage_version {
    use super::*;
    use frame_support::traits::{GetStorageVersion, StorageVersion};

    #[test]
    fn the_storage_version_is_6() {
        assert_eq!(
            QuantumComputeMempool::in_code_storage_version(),
            StorageVersion::new(6)
        );
    }

    /// Its solutions, rankings and answer index would no longer decode, so a
    /// v5 chain with an order is not moved (a warning is logged).
    #[test]
    fn a_v5_chain_with_an_order_stays_at_5() {
        new_test_ext().execute_with(|| {
            propose_single_best(1, 1, 1);
            StorageVersion::new(5).put::<QuantumComputeMempool>();
            <QuantumComputeMempool as Hooks<u64>>::on_runtime_upgrade();
            assert_eq!(
                QuantumComputeMempool::on_chain_storage_version(),
                StorageVersion::new(5)
            );
        });
    }

    /// Under try-runtime the upgrade fails closed on such a chain.
    #[cfg(feature = "try-runtime")]
    #[test]
    fn pre_upgrade_refuses_a_v5_chain_with_orders() {
        new_test_ext().execute_with(|| {
            StorageVersion::new(5).put::<QuantumComputeMempool>();
            assert!(<QuantumComputeMempool as Hooks<u64>>::pre_upgrade().is_ok());
            propose_single_best(1, 1, 1);
            assert!(<QuantumComputeMempool as Hooks<u64>>::pre_upgrade().is_err());
            StorageVersion::new(6).put::<QuantumComputeMempool>();
            assert!(<QuantumComputeMempool as Hooks<u64>>::pre_upgrade().is_ok());
        });
    }

    #[test]
    fn an_order_free_v5_chain_moves_to_6() {
        new_test_ext().execute_with(|| {
            StorageVersion::new(5).put::<QuantumComputeMempool>();
            <QuantumComputeMempool as Hooks<u64>>::on_runtime_upgrade();
            assert_eq!(
                QuantumComputeMempool::on_chain_storage_version(),
                StorageVersion::new(6)
            );
        });
    }
}

/// W3-001 B5c (design §B5 v2 item 3; owner decision of 2026-11-25) -- answer
/// commitments, their deposits, and the forfeit of those still outstanding at
/// finalisation.
mod w3_001_commitments {
    use super::*;
    use crate::{AnswerCommitments, CommitmentCount};
    use frame_support::traits::{Currency, ReservableCurrency};
    use sp_core::H256;

    fn rates() {
        OrderDepositPerItem::set(10);
        OrderDepositPerByte::set(1);
    }

    fn register(who: AccountId) {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(who),
            MinerType::Cpu
        ));
    }

    fn commit(who: AccountId, id: u64, byte: u8) -> frame_support::dispatch::DispatchResult {
        QuantumComputeMempool::commit_answer(
            RuntimeOrigin::signed(who),
            id,
            H256::repeat_byte(byte),
        )
    }

    fn forfeits(id: u64) -> Vec<(AccountId, u128, u128)> {
        System::events()
            .into_iter()
            .filter_map(|record| match record.event {
                RuntimeEvent::QuantumComputeMempool(crate::Event::CommitmentForfeited {
                    order_id,
                    solver,
                    to_proposer,
                    burned,
                }) if order_id == id => Some((solver, to_proposer, burned)),
                _ => None,
            })
            .collect()
    }

    fn assert_no_commitments(id: u64) {
        assert!(AnswerCommitments::<Test>::iter_prefix(id).next().is_none());
        assert!(!CommitmentCount::<Test>::contains_key(id));
    }

    #[test]
    fn a_commitment_is_stored_with_its_deposit_reserved() {
        new_test_ext().execute_with(|| {
            rates();
            register(2);
            let id = propose_single_best(1, 10, 5);
            let deposit = QuantumComputeMempool::commitment_deposit();
            assert!(deposit > 10, "priced on the entry's bounded size");
            let reserved = Balances::reserved_balance(2);
            assert_ok!(commit(2, id, 7));
            let entry = AnswerCommitments::<Test>::get(id, 2).unwrap();
            assert_eq!(
                (entry.commitment, entry.commit_block, entry.deposit),
                (H256::repeat_byte(7), 1, deposit)
            );
            assert_eq!(Balances::reserved_balance(2), reserved + deposit);
            assert_eq!(CommitmentCount::<Test>::get(id), 1);
            System::assert_last_event(
                crate::Event::AnswerCommitted {
                    order_id: id,
                    solver: 2,
                    commitment: H256::repeat_byte(7),
                    commit_block: 1,
                }
                .into(),
            );
        });
    }

    /// A second commitment replaces the first in place: a new hash and block,
    /// the same deposit, no new reserve, the same count.
    #[test]
    fn recommitting_replaces_in_place() {
        new_test_ext().execute_with(|| {
            rates();
            register(2);
            let id = propose_single_best(1, 10, 5);
            assert_ok!(commit(2, id, 7));
            let reserved = Balances::reserved_balance(2);
            let deposit = AnswerCommitments::<Test>::get(id, 2).unwrap().deposit;
            System::set_block_number(3);
            assert_ok!(commit(2, id, 8));
            let entry = AnswerCommitments::<Test>::get(id, 2).unwrap();
            assert_eq!(
                (entry.commitment, entry.commit_block, entry.deposit),
                (H256::repeat_byte(8), 3, deposit)
            );
            assert_eq!(Balances::reserved_balance(2), reserved);
            assert_eq!(CommitmentCount::<Test>::get(id), 1);
        });
    }

    /// Each refusal changes nothing, and the cap is checked before the
    /// reserve: a solver who cannot pay is told the cap is reached.
    #[test]
    fn refused_commitments_change_nothing() {
        new_test_ext().execute_with(|| {
            rates();
            register(2);
            register(3);
            let id = propose_single_best(1, 10, 5);
            assert_noop!(commit(4, id, 1), crate::Error::<Test>::SolverNotRegistered);
            assert_noop!(commit(2, 99, 1), crate::Error::<Test>::OrderNotFound);
            // Solver 3 with less free balance than the deposit: it cannot
            // reserve. (Free cannot go to 0 while it holds a reserve.)
            Balances::make_free_balance_be(&3, 5);
            assert_noop!(
                commit(3, id, 1),
                crate::Error::<Test>::CommitmentDepositNotAvailable
            );
            MaxCommitmentsPerOrder::set(1);
            assert_ok!(commit(2, id, 1));
            assert_noop!(commit(3, id, 2), crate::Error::<Test>::CommitmentCapReached);
            // Not eligible: a bid order that names only solver 2.
            let spec_id = JobSpecs::<Test>::iter_keys().next().unwrap();
            let bid = NextOrderId::<Test>::get();
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
                10,
                5,
                ResultDelivery::OnChainOnly,
            ));
            Balances::make_free_balance_be(&3, 1_000_000);
            assert_noop!(commit(3, bid, 1), crate::Error::<Test>::NotEligibleSolver);
            // Expired: no commitment after the order's deadline.
            System::set_block_number(20);
            assert_noop!(commit(2, bid, 1), crate::Error::<Test>::OrderNotOpen);
        });
    }

    /// Unrevealed commitments are forfeited to the proposer when the order is
    /// finalised, on the queue's path, solved or not; nothing is left.
    #[test]
    fn outstanding_commitments_are_forfeited_to_the_proposer_at_finalisation() {
        for solved in [false, true] {
            new_test_ext().execute_with(|| {
                rates();
                for who in [2, 3, 4] {
                    register(who);
                }
                let id = propose_single_best(1, 2, 2);
                if solved {
                    assert_ok!(QuantumComputeMempool::submit_solution(
                        RuntimeOrigin::signed(2),
                        id,
                        sample_solution()
                    ));
                }
                assert_ok!(commit(3, id, 3));
                assert_ok!(commit(4, id, 4));
                let deposit = QuantumComputeMempool::commitment_deposit();
                let reserved = |who| Balances::reserved_balance(who);
                let (r3, r4, free1) = (reserved(3), reserved(4), Balances::free_balance(1));
                System::reset_events();
                run_to_block(3);
                assert_ne!(
                    JobOrders::<Test>::get(id).unwrap().status,
                    OrderStatus::Opened
                );
                let mut got = forfeits(id);
                got.sort();
                assert_eq!(
                    got,
                    vec![(3, deposit, 0), (4, deposit, 0)],
                    "solved {solved}"
                );
                assert_eq!((reserved(3), reserved(4)), (r3 - deposit, r4 - deposit));
                let refund = if solved { 0 } else { 100 };
                assert_eq!(Balances::free_balance(1), free1 + 2 * deposit + refund);
                assert_no_commitments(id);
            });
        }
    }

    /// The lazy settlement (B4c) finalises inline, and forfeits too.
    #[test]
    fn lazy_settlement_forfeits_too() {
        new_test_ext().execute_with(|| {
            rates();
            register(3);
            let id = propose_single_best(1, 1, 1);
            assert_ok!(commit(3, id, 3));
            let deposit = QuantumComputeMempool::commitment_deposit();
            System::set_block_number(5);
            assert_ok!(QuantumComputeMempool::reclaim_order(
                RuntimeOrigin::signed(1),
                id
            ));
            assert_eq!(forfeits(id), vec![(3, deposit, 0)]);
            assert_no_commitments(id);
        });
    }

    /// The proposer reaped before finalisation: the deposit recreates its
    /// account when it covers the existential deposit, and is burned when it
    /// does not. (A solved order, so no refund recreates the proposer.)
    #[test]
    fn a_reaped_proposer_is_recreated_or_the_deposit_burned() {
        for (ed, per_byte, recreated) in [(1u128, 1u128, true), (50, 0, false)] {
            new_test_ext().execute_with(|| {
                OrderDepositPerItem::set(10);
                OrderDepositPerByte::set(per_byte);
                ExistentialDeposit::set(ed);
                register(2);
                register(3);
                let id = propose_single_best(1, 2, 2);
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    sample_solution()
                ));
                assert_ok!(commit(3, id, 3));
                let deposit = QuantumComputeMempool::commitment_deposit();
                assert_eq!(deposit >= ed, recreated);
                // Reap the proposer: its order deposit, then its free balance.
                let held = Balances::reserved_balance(1);
                let _ = Balances::slash_reserved(&1, held);
                Balances::make_free_balance_be(&1, 0);
                assert!(!System::account_exists(&1));
                let issuance = Balances::total_issuance();
                System::reset_events();
                run_to_block(3);
                if recreated {
                    assert_eq!(forfeits(id), vec![(3, deposit, 0)]);
                    assert_eq!(Balances::free_balance(1), deposit);
                    assert_eq!(Balances::total_issuance(), issuance);
                } else {
                    assert_eq!(forfeits(id), vec![(3, 0, deposit)]);
                    assert!(!System::account_exists(&1));
                    assert_eq!(Balances::total_issuance(), issuance - deposit);
                }
                assert_no_commitments(id);
            });
        }
    }

    /// `on_initialize` charges exactly the forfeits it performed: the same
    /// block with and without three commitments differs by
    /// `forfeit_commitments(3) - forfeit_commitments(0)`.
    #[test]
    fn on_initialize_charges_the_commitments_forfeited() {
        use crate::weights::WeightInfo as _;
        let charged = |commitments: bool| {
            new_test_ext().execute_with(|| {
                rates();
                for who in [2, 3, 4] {
                    register(who);
                }
                let id = propose_single_best(1, 1, 1);
                if commitments {
                    for who in [2, 3, 4] {
                        assert_ok!(commit(who, id, who as u8));
                    }
                }
                System::set_block_number(2);
                QuantumComputeMempool::on_initialize(2)
            })
        };
        let weights = <Test as crate::Config>::WeightInfo::forfeit_commitments;
        assert_eq!(charged(true) - charged(false), weights(3) - weights(0));
    }
}

/// W3-001 B5d (design §B5 v2 items 1, 3, 8; N3) -- revealing a commitment.
mod w3_001_reveal {
    use super::*;
    use crate::{AnswerCommitments, CommitmentCount};
    use sp_core::H256;

    const SALT: [u8; 32] = [7; 32];

    fn register(who: AccountId) {
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(who),
            MinerType::Cpu
        ));
    }

    fn propose(resolution: RewardResolution, block_wait: u64) -> u64 {
        let spec_id = match JobSpecs::<Test>::iter_keys().next() {
            Some(spec_id) => spec_id,
            None => register_spec(),
        };
        let id = NextOrderId::<Test>::get();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            chain_params(4),
            100,
            JobMode::Open,
            resolution,
            20,
            block_wait,
            ResultDelivery::OnChainOnly,
        ));
        id
    }

    fn commit(who: AccountId, id: u64, k: u32) {
        let commitment =
            QuantumComputeMempool::answer_commitment(id, &who, &chain_row(4, k), &None, &SALT);
        assert_ok!(QuantumComputeMempool::commit_answer(
            RuntimeOrigin::signed(who),
            id,
            commitment
        ));
    }

    fn reveal(
        who: AccountId,
        id: u64,
        k: u32,
    ) -> frame_support::dispatch::DispatchResultWithPostInfo {
        QuantumComputeMempool::reveal_solution(
            RuntimeOrigin::signed(who),
            id,
            chain_row(4, k),
            None,
            SALT,
        )
    }

    /// The reveal consumes the commitment -- deleted, count decremented,
    /// deposit returned -- and the solution carries the commitment's block.
    #[test]
    fn a_reveal_consumes_the_commitment_and_carries_its_block() {
        new_test_ext().execute_with(|| {
            OrderDepositPerItem::set(10);
            OrderDepositPerByte::set(1);
            register(2);
            let id = propose(RewardResolution::SingleBest, 5);
            let reserved = Balances::reserved_balance(2);
            commit(2, id, 4);
            assert!(Balances::reserved_balance(2) > reserved);
            run_to_block(3);
            assert_ok!(reveal(2, id, 4));
            assert!(AnswerCommitments::<Test>::get(id, 2).is_none());
            assert_eq!(CommitmentCount::<Test>::get(id), 0);
            assert_eq!(Balances::reserved_balance(2), reserved);
            let solution = OrderSolutions::<Test>::get(id, 2).unwrap();
            assert_eq!((solution.effective_at, solution.seq), (1, 0));
            let owner = crate::OrderAnswers::<Test>::get(id, solution.answer).unwrap();
            assert_eq!((owner.solver, owner.effective_at), (2, 1));
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().effective_at, 1);
        });
    }

    /// No commitment, the same block, and any mismatch -- solutions, payee or
    /// salt -- are refused, changing nothing.
    #[test]
    fn invalid_reveals_are_refused() {
        new_test_ext().execute_with(|| {
            register(2);
            let id = propose(RewardResolution::SingleBest, 5);
            assert_noop!(reveal(2, id, 4), crate::Error::<Test>::CommitmentNotFound);
            commit(2, id, 4);
            assert_noop!(reveal(2, id, 4), crate::Error::<Test>::RevealTooEarly);
            run_to_block(2);
            assert_noop!(reveal(2, id, 6), crate::Error::<Test>::CommitmentMismatch);
            assert_noop!(
                QuantumComputeMempool::reveal_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    chain_row(4, 4),
                    None,
                    [8; 32]
                ),
                crate::Error::<Test>::CommitmentMismatch
            );
            assert_noop!(
                QuantumComputeMempool::reveal_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    chain_row(4, 4),
                    Some(crate::types::WinnerPayee::V1 {
                        chain_id: 20033,
                        address: sp_core::H160::repeat_byte(1),
                    }),
                    SALT
                ),
                crate::Error::<Test>::CommitmentMismatch
            );
            assert_ok!(reveal(2, id, 4));
        });
    }

    /// A stored commitment is not standing eligibility (kimi's reminder): the
    /// reveal re-checks registration exactly as a plain submission does, so a
    /// solver that deregistered after committing cannot reveal, and the
    /// commitment stays.
    #[test]
    fn a_reveal_rechecks_registration() {
        new_test_ext().execute_with(|| {
            register(2);
            let id = propose(RewardResolution::SingleBest, 5);
            commit(2, id, 4);
            assert_ok!(QuantumComputeMempool::deregister_solver(
                RuntimeOrigin::signed(2)
            ));
            run_to_block(2);
            assert_noop!(
                reveal(2, id, 4).map_err(|e| e.error),
                crate::Error::<Test>::SolverNotRegistered
            );
            assert!(AnswerCommitments::<Test>::contains_key(id, 2));
        });
    }

    /// The preimage binds the solver and the order (B5d2). A watcher who
    /// front-runs Y's commit transaction with Y's own hash H, then replays Y's
    /// public preimage, gets `CommitmentMismatch`: the hash recomputed for the
    /// watcher differs from H. Y's own reveal still lands. And H cannot be
    /// replayed on another order.
    #[test]
    fn a_copied_commitment_cannot_be_revealed_by_another_solver() {
        new_test_ext().execute_with(|| {
            register(2);
            register(3);
            let id = propose(RewardResolution::SingleBest, 5);
            let other = propose(RewardResolution::SingleBest, 5);
            // Y (2) commits H; the watcher X (3) copies H, here on both orders.
            commit(2, id, 4);
            let h = crate::AnswerCommitments::<Test>::get(id, 2)
                .unwrap()
                .commitment;
            for order in [id, other] {
                assert_ok!(QuantumComputeMempool::commit_answer(
                    RuntimeOrigin::signed(3),
                    order,
                    h
                ));
            }
            run_to_block(2);
            // X replays Y's preimage: the same solutions, payee and salt.
            for order in [id, other] {
                assert_noop!(
                    reveal(3, order, 4).map_err(|e| e.error),
                    crate::Error::<Test>::CommitmentMismatch
                );
            }
            // Y's preimage does not open Y's own hash on another order either.
            commit(2, other, 6);
            assert_ne!(
                QuantumComputeMempool::answer_commitment(other, &2, &chain_row(4, 4), &None, &SALT),
                h
            );
            assert_ok!(reveal(2, id, 4));
            assert_eq!(OrderSolutions::<Test>::get(id, 2).unwrap().effective_at, 1);
        });
    }

    /// B5d3: a solver that plain-submitted an answer and also committed to it
    /// earlier can reveal it. Admission compares the full rank key, so the
    /// reveal at equal energy and an earlier `effective_at` replaces the
    /// solver's own entry -- not `SolutionNotImproved`, and not
    /// `DuplicateAnswer` against its own index entry -- and the deposit is
    /// refunded, under every resolution.
    #[test]
    fn a_reveal_replaces_the_solvers_own_later_plain_submission() {
        for resolution in [
            RewardResolution::SingleBest,
            RewardResolution::TopNEqual { n: 2 },
            RewardResolution::TopNWeighted { n: 2 },
        ] {
            new_test_ext().execute_with(|| {
                OrderDepositPerItem::set(10);
                register(2);
                let id = propose(resolution, 10);
                let reserved = Balances::reserved_balance(2);
                commit(2, id, 4);
                run_to_block(2);
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    id,
                    chain_row(4, 4)
                ));
                assert_eq!(OrderSolutions::<Test>::get(id, 2).unwrap().effective_at, 2);
                run_to_block(3);
                assert_ok!(reveal(2, id, 4));
                let solution = OrderSolutions::<Test>::get(id, 2).unwrap();
                assert_eq!(
                    (solution.effective_at, solution.seq),
                    (1, 1),
                    "{resolution:?}"
                );
                let owner = crate::OrderAnswers::<Test>::get(id, solution.answer).unwrap();
                assert_eq!((owner.solver, owner.effective_at, owner.seq), (2, 1, 1));
                assert_eq!(crate::OrderAnswers::<Test>::iter_prefix(id).count(), 1);
                match resolution {
                    RewardResolution::SingleBest => {
                        let front = OrderFrontRunner::<Test>::get(id).unwrap();
                        assert_eq!((front.solver, front.effective_at, front.seq), (2, 1, 1));
                    }
                    _ => {
                        let ranked = crate::OrderTopSolvers::<Test>::get(id);
                        assert_eq!(ranked.len(), 1);
                        assert_eq!(
                            (ranked[0].solver, ranked[0].effective_at, ranked[0].seq),
                            (2, 1, 1)
                        );
                    }
                }
                assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 1);
                assert!(AnswerCommitments::<Test>::get(id, 2).is_none());
                assert_eq!(Balances::reserved_balance(2), reserved);
                // A plain resubmission at equal energy is still refused.
                assert_noop!(
                    QuantumComputeMempool::submit_solution(
                        RuntimeOrigin::signed(2),
                        id,
                        chain_row(4, 4)
                    )
                    .map_err(|e| e.error),
                    crate::Error::<Test>::SolutionNotImproved
                );
            });
        }
    }

    /// B5d3: re-committing resets the commitment's block, so garbage
    /// committed early and replaced by a real commitment later does not lend
    /// the real answer the early time: an equal-energy plain answer that
    /// landed in between still ranks first.
    #[test]
    fn a_recommitment_carries_its_own_later_block() {
        new_test_ext().execute_with(|| {
            register(2);
            register(3);
            let id = propose(RewardResolution::TopNEqual { n: 2 }, 10);
            assert_ok!(QuantumComputeMempool::commit_answer(
                RuntimeOrigin::signed(2),
                id,
                H256::repeat_byte(9)
            ));
            run_to_block(2);
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(3),
                id,
                chain_row(4, 6)
            ));
            run_to_block(3);
            commit(2, id, 4);
            assert_eq!(
                AnswerCommitments::<Test>::get(id, 2).unwrap().commit_block,
                3
            );
            run_to_block(4);
            assert_ok!(reveal(2, id, 4));
            assert_eq!(OrderSolutions::<Test>::get(id, 2).unwrap().effective_at, 3);
            let order: Vec<_> = crate::OrderTopSolvers::<Test>::get(id)
                .iter()
                .map(|entry| entry.solver)
                .collect();
            assert_eq!(order, vec![3, 2]);
        });
    }

    /// N3: a reveal whose submission is refused (here a duplicate answer)
    /// rolls back entirely -- the commitment, the count and the deposit stay.
    #[test]
    fn a_failed_reveal_leaves_the_commitment_count_and_deposit() {
        new_test_ext().execute_with(|| {
            OrderDepositPerItem::set(10);
            register(2);
            register(3);
            let id = propose(RewardResolution::SingleBest, 5);
            commit(2, id, 4);
            let entry = AnswerCommitments::<Test>::get(id, 2).unwrap();
            let reserved = Balances::reserved_balance(2);
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(3),
                id,
                chain_row(4, 4)
            ));
            run_to_block(2);
            assert_noop!(
                reveal(2, id, 4).map_err(|e| e.error),
                crate::Error::<Test>::DuplicateAnswer
            );
            assert_eq!(AnswerCommitments::<Test>::get(id, 2), Some(entry));
            assert_eq!(CommitmentCount::<Test>::get(id), 1);
            assert_eq!(Balances::reserved_balance(2), reserved);
        });
    }

    /// The point of committing: a reveal ranks at its commitment's block, so
    /// at equal energy it beats an uncommitted answer that landed first --
    /// under every resolution.
    #[test]
    fn a_revealed_answer_outranks_an_earlier_landed_equal_energy_one() {
        for resolution in [
            RewardResolution::SingleBest,
            RewardResolution::TopNEqual { n: 2 },
            RewardResolution::TopNWeighted { n: 2 },
        ] {
            new_test_ext().execute_with(|| {
                register(2);
                register(3);
                let id = propose(resolution, 10);
                commit(2, id, 4);
                run_to_block(2);
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(3),
                    id,
                    chain_row(4, 6)
                ));
                run_to_block(3);
                assert_ok!(reveal(2, id, 4));
                let energy = |who| {
                    OrderSolutions::<Test>::get(id, who)
                        .unwrap()
                        .best_energy_milli
                };
                assert_eq!(energy(2), energy(3));
                match resolution {
                    RewardResolution::SingleBest => {
                        assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 2)
                    }
                    _ => {
                        let order: Vec<_> = crate::OrderTopSolvers::<Test>::get(id)
                            .iter()
                            .map(|entry| entry.solver)
                            .collect();
                        assert_eq!(order, vec![2, 3], "{resolution:?}");
                    }
                }
            });
        }
    }

    /// E4: why `MinBlockWait` is 2. A first solution at block M starts the
    /// block wait; an honest reveal that slips to M + 1 still lands with a
    /// wait of 2. With a wait of 1 the order is finalised at M + 1, before the
    /// reveal, which forfeits the commitment: the reveal finds none. (Taking a
    /// copied answer's key is eviction, B5e.)
    #[test]
    fn a_reveal_one_block_behind_the_first_solution_needs_a_wait_of_two() {
        for (wait, lands) in [(2u64, true), (1, false)] {
            new_test_ext().execute_with(|| {
                register(2);
                register(3);
                let id = propose(RewardResolution::SingleBest, wait);
                commit(2, id, 4);
                run_to_block(5);
                assert_ok!(QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(3),
                    id,
                    chain_row(4, 6)
                ));
                run_to_block(6);
                if lands {
                    assert_ok!(reveal(2, id, 4));
                    assert_eq!(OrderSolutions::<Test>::get(id, 2).unwrap().effective_at, 1);
                } else {
                    assert_eq!(
                        JobOrders::<Test>::get(id).unwrap().status,
                        OrderStatus::Expired
                    );
                    assert!(AnswerCommitments::<Test>::get(id, 2).is_none(), "forfeited");
                    assert_noop!(reveal(2, id, 4), crate::Error::<Test>::CommitmentNotFound);
                }
            });
        }
    }
}

/// W3-001 B5e (design §B5 v2 items 5-9, N1, N2) -- eviction by an earlier
/// commitment.
mod w3_001_eviction {
    use super::*;
    use crate::{AnswerCommitments, OrderAnswers, OrderPayees, OrderTopSolvers};
    use frame_support::traits::Currency;

    const SALT: [u8; 32] = [7; 32];
    // On `field_params`: GOOD and FLIP are global flips of each other, so one
    // answer, at different energies; BEST and MID are two other answers.
    const GOOD: [i8; 4] = [1, 1, 1, -1]; // -2000
    const FLIP: [i8; 4] = [-1, -1, -1, 1]; // 0
    const BEST: [i8; 4] = [1, 1, 1, 1]; // -5000
    const MID: [i8; 4] = [1, 1, -1, -1]; // -1000

    /// A ferromagnetic 4-chain with a field on every spin, so a global flip
    /// is the same answer at a different energy (E1).
    fn field_params() -> crate::IsingParamsOf<Test> {
        let mut params = chain_params(4);
        params.h_values = bounded(vec![-500; 4]);
        params
    }

    fn row(spins: &[i8]) -> crate::SolutionsOf<Test> {
        bounded(vec![bounded(spins.to_vec())])
    }

    fn register(who: AccountId) {
        if who > 4 {
            Balances::make_free_balance_be(&who, 1_000_000);
        }
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(who),
            MinerType::Cpu
        ));
    }

    fn propose(params: crate::IsingParamsOf<Test>, resolution: RewardResolution, wait: u64) -> u64 {
        let spec_id = match JobSpecs::<Test>::iter_keys().next() {
            Some(spec_id) => spec_id,
            None => register_spec(),
        };
        let id = NextOrderId::<Test>::get();
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(1),
            spec_id,
            params,
            100,
            JobMode::Open,
            resolution,
            20,
            wait,
            ResultDelivery::OnChainOnly,
        ));
        id
    }

    fn commit(who: AccountId, id: u64, solutions: &crate::SolutionsOf<Test>) {
        let commitment =
            QuantumComputeMempool::answer_commitment(id, &who, solutions, &None, &SALT);
        assert_ok!(QuantumComputeMempool::commit_answer(
            RuntimeOrigin::signed(who),
            id,
            commitment
        ));
    }

    fn reveal(
        who: AccountId,
        id: u64,
        solutions: &crate::SolutionsOf<Test>,
    ) -> Result<(), sp_runtime::DispatchError> {
        QuantumComputeMempool::reveal_solution(
            RuntimeOrigin::signed(who),
            id,
            solutions.clone(),
            None,
            SALT,
        )
        .map(|_| ())
        .map_err(|e| e.error)
    }

    fn submit(who: AccountId, id: u64, solutions: &crate::SolutionsOf<Test>) {
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(who),
            id,
            solutions.clone()
        ));
    }

    fn ranking(id: u64) -> Vec<(AccountId, i64)> {
        OrderTopSolvers::<Test>::get(id)
            .iter()
            .map(|entry| (entry.solver, entry.energy_milli))
            .collect()
    }

    fn energy(who: AccountId, id: u64) -> i64 {
        OrderSolutions::<Test>::get(id, who)
            .unwrap()
            .best_energy_milli
    }

    const RESOLUTIONS: [RewardResolution; 3] = [
        RewardResolution::SingleBest,
        RewardResolution::TopNEqual { n: 2 },
        RewardResolution::TopNWeighted { n: 2 },
    ];

    /// E1 (and E5): a copier's worse flip of a committed answer lands first;
    /// the honest reveal takes the answer, the copier's solution, payee and
    /// ranking entry go, and the honest solver keeps its slot at its OWN
    /// energy -- so a third solver at an intermediate energy ranks behind it.
    #[test]
    fn an_earlier_commitment_evicts_a_copied_worse_flip_at_its_own_energy() {
        for resolution in RESOLUTIONS {
            new_test_ext().execute_with(|| {
                for who in [2, 3, 4] {
                    register(who);
                }
                let id = propose(field_params(), resolution, 10);
                commit(2, id, &row(&GOOD));
                run_to_block(2);
                assert_ok!(QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(3),
                    id,
                    row(&FLIP),
                    payee(20033, 3)
                ));
                assert_eq!(energy(3, id), 0);
                assert!(OrderPayees::<Test>::contains_key(id, 3));
                run_to_block(3);
                assert_ok!(reveal(2, id, &row(&GOOD)));
                let answer = OrderSolutions::<Test>::get(id, 2).unwrap().answer;
                System::assert_has_event(
                    crate::Event::SolutionEvicted {
                        order_id: id,
                        evicted: 3,
                        by: 2,
                        answer,
                    }
                    .into(),
                );
                assert!(OrderSolutions::<Test>::get(id, 3).is_none());
                assert!(!OrderPayees::<Test>::contains_key(id, 3));
                let owner = OrderAnswers::<Test>::get(id, answer).unwrap();
                assert_eq!(
                    (owner.solver, owner.best_energy_milli, owner.effective_at),
                    (2, -2000, 1)
                );
                assert_eq!(energy(2, id), -2000);
                assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 1);
                // The copier cannot take it back, flipped or not.
                for spins in [FLIP, GOOD] {
                    assert_noop!(
                        QuantumComputeMempool::submit_solution(
                            RuntimeOrigin::signed(3),
                            id,
                            row(&spins)
                        )
                        .map_err(|e| e.error),
                        crate::Error::<Test>::DuplicateAnswer
                    );
                }
                submit(4, id, &row(&MID));
                match resolution {
                    RewardResolution::SingleBest => {
                        let front = OrderFrontRunner::<Test>::get(id).unwrap();
                        assert_eq!((front.solver, front.energy_milli), (2, -2000));
                    }
                    _ => assert_eq!(ranking(id), vec![(2, -2000), (4, -1000)], "{resolution:?}"),
                }
            });
        }
    }

    /// N1: the ranking is rebuilt after an eviction, so a solver truncated
    /// out of the top-n comes back. n = 2: A = -5000, X = -2000 (a better
    /// flip of H's answer), Y = -1000 (truncated), H = 0 committed first:
    /// after the eviction the list is [A, Y]. Under SingleBest with X the
    /// front runner, the front runner becomes Y, not H.
    #[test]
    fn an_eviction_rebuilds_the_ranking_from_the_answer_index() {
        for resolution in RESOLUTIONS {
            new_test_ext().execute_with(|| {
                for who in [2, 3, 4, 5] {
                    register(who);
                }
                let id = propose(field_params(), resolution, 10);
                let single = resolution == RewardResolution::SingleBest;
                commit(2, id, &row(&FLIP));
                run_to_block(2);
                if !single {
                    submit(3, id, &row(&BEST));
                }
                submit(4, id, &row(&GOOD));
                submit(5, id, &row(&MID));
                if single {
                    assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 4);
                } else {
                    assert_eq!(ranking(id), vec![(3, -5000), (4, -2000)]);
                }
                run_to_block(3);
                assert_ok!(reveal(2, id, &row(&FLIP)));
                assert!(OrderSolutions::<Test>::get(id, 4).is_none());
                if single {
                    let front = OrderFrontRunner::<Test>::get(id).unwrap();
                    assert_eq!((front.solver, front.energy_milli), (5, -1000));
                    System::assert_has_event(
                        crate::Event::FrontRunnerChanged {
                            order_id: id,
                            solver: 5,
                            energy_milli: -1000,
                        }
                        .into(),
                    );
                } else {
                    assert_eq!(ranking(id), vec![(3, -5000), (5, -1000)], "{resolution:?}");
                }
            });
        }
    }

    /// E2: the cap is checked after the eviction decision. A full order (32
    /// solvers, the copy in slot 32) refuses a 33rd plain solver but admits
    /// the reveal that evicts the copy; the count stays 32.
    #[test]
    fn a_full_order_admits_a_reveal_that_evicts() {
        new_test_ext().execute_with(|| {
            register(2);
            register(3);
            register(4);
            let id = propose(chain_params(7), RewardResolution::TopNEqual { n: 2 }, 10);
            commit(2, id, &chain_row(7, 40));
            run_to_block(2);
            for k in 1..=31u32 {
                let who = 100 + AccountId::from(k);
                register(who);
                submit(who, id, &chain_row(7, k));
            }
            submit(3, id, &chain_row(7, 40));
            assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 32);
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(4),
                    id,
                    chain_row(7, 41)
                )
                .map_err(|e| e.error),
                crate::Error::<Test>::OrderSolverCapReached
            );
            run_to_block(3);
            assert_ok!(reveal(2, id, &chain_row(7, 40)));
            assert!(OrderSolutions::<Test>::get(id, 3).is_none());
            assert!(OrderSolutions::<Test>::get(id, 2).is_some());
            assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 32);
            assert_eq!(OrderAnswers::<Test>::iter_prefix(id).count(), 32);
        });
    }

    /// E3: `solution_count += is_new_solver - evicted`. A new evictor leaves
    /// the count unchanged; an existing solver evicting with a second answer
    /// lowers it (its own previous answer is replaced, the copier's removed).
    #[test]
    fn the_solution_count_nets_new_solvers_and_evictions() {
        new_test_ext().execute_with(|| {
            for who in [2, 3] {
                register(who);
            }
            let id = propose(field_params(), RewardResolution::TopNEqual { n: 2 }, 10);
            commit(2, id, &row(&GOOD));
            run_to_block(2);
            submit(2, id, &row(&MID));
            submit(3, id, &row(&FLIP));
            assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 2);
            run_to_block(3);
            assert_ok!(reveal(2, id, &row(&GOOD)));
            assert_eq!(JobOrders::<Test>::get(id).unwrap().solution_count, 1);
            assert_eq!(OrderAnswers::<Test>::iter_prefix(id).count(), 1);
            assert_eq!(OrderSolutions::<Test>::iter_prefix(id).count(), 1);
            assert_eq!(ranking(id), vec![(2, -2000)]);
        });
    }

    /// E4, the copy half: a copy at M -- the first solution, which starts the
    /// block wait -- and the honest reveal at M + 1. With a wait of 2 the
    /// reveal lands and evicts the copy; with a wait of 1 the order was
    /// finalised at M + 1, the commitment forfeited, and the copy keeps the
    /// answer. Why `MinBlockWait` is 2.
    #[test]
    fn a_copy_one_block_ahead_of_the_reveal_is_evicted_only_with_a_wait_of_two() {
        for (wait, lands) in [(2u64, true), (1, false)] {
            new_test_ext().execute_with(|| {
                register(2);
                register(3);
                let id = propose(chain_params(4), RewardResolution::SingleBest, wait);
                commit(2, id, &chain_row(4, 4));
                run_to_block(5);
                submit(3, id, &chain_row(4, 4));
                run_to_block(6);
                if lands {
                    assert_ok!(reveal(2, id, &chain_row(4, 4)));
                    assert!(OrderSolutions::<Test>::get(id, 3).is_none());
                    assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 2);
                } else {
                    assert_eq!(
                        reveal(2, id, &chain_row(4, 4)),
                        Err(crate::Error::<Test>::CommitmentNotFound.into())
                    );
                    assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 3);
                }
            });
        }
    }

    /// The interim risk B5d left open: a watcher front-runs the reveal
    /// transaction, in the same block, with a plain submission of the same
    /// answer. The reveal still lands and evicts it.
    #[test]
    fn a_same_block_front_run_of_a_reveal_is_evicted() {
        new_test_ext().execute_with(|| {
            register(2);
            register(3);
            let id = propose(chain_params(4), RewardResolution::SingleBest, 10);
            commit(2, id, &chain_row(4, 4));
            run_to_block(3);
            submit(3, id, &chain_row(4, 4));
            assert_ok!(reveal(2, id, &chain_row(4, 4)));
            assert!(OrderSolutions::<Test>::get(id, 3).is_none());
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 2);
            assert!(AnswerCommitments::<Test>::get(id, 2).is_none());
        });
    }

    /// Eviction needs a strictly earlier effective time: two solvers that
    /// committed to the same answer in the same block -- the first reveal
    /// keeps it, the second is a duplicate and rolls back.
    #[test]
    fn an_equal_effective_time_does_not_evict() {
        new_test_ext().execute_with(|| {
            register(2);
            register(3);
            let id = propose(chain_params(4), RewardResolution::SingleBest, 10);
            commit(2, id, &chain_row(4, 4));
            commit(3, id, &chain_row(4, 4));
            run_to_block(2);
            assert_ok!(reveal(2, id, &chain_row(4, 4)));
            assert_noop!(
                reveal(3, id, &chain_row(4, 4)),
                crate::Error::<Test>::DuplicateAnswer
            );
            assert!(AnswerCommitments::<Test>::contains_key(id, 3));
            assert_eq!(OrderFrontRunner::<Test>::get(id).unwrap().solver, 2);
        });
    }

    /// N2: the rebuild orders ties by arrival. Two equal-energy solvers in
    /// the same block keep their order across an unrelated eviction -- for
    /// either assignment of the two answers, so the index's storage order
    /// cannot stand in for `seq`.
    #[test]
    fn same_block_equal_energy_solvers_keep_their_order_across_an_eviction() {
        for (first, second) in [(4u32, 6u32), (6, 4)] {
            new_test_ext().execute_with(|| {
                for who in [2, 3, 4, 5] {
                    register(who);
                }
                let id = propose(chain_params(4), RewardResolution::TopNEqual { n: 3 }, 10);
                commit(2, id, &chain_row(4, 1));
                run_to_block(2);
                submit(3, id, &chain_row(4, first));
                submit(4, id, &chain_row(4, second));
                assert_eq!(energy(3, id), energy(4, id));
                submit(5, id, &chain_row(4, 1));
                run_to_block(3);
                assert_ok!(reveal(2, id, &chain_row(4, 1)));
                let order: Vec<_> = ranking(id).into_iter().map(|(who, _)| who).collect();
                let position = |who| order.iter().position(|w| *w == who).unwrap();
                assert!(position(3) < position(4), "{order:?}");
                assert!(!order.contains(&5));
            });
        }
    }
}

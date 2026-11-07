use super::*;
use crate::{xqvm::QuantumVm, Error};
use codec::Encode;
use frame_support::dispatch::GetDispatchInfo;
use xqvm::{Instruction, InstructionBuilder, Program, RegVal, Register, Vm};

fn bytes(f: impl FnOnce(&mut InstructionBuilder)) -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    f(&mut b);
    b.emit_halt();
    b.build().unwrap().encode()
}

fn input(b: &mut InstructionBuilder) {
    b.emit_push(0).emit_input(Register(0));
}
fn output(b: &mut InstructionBuilder) {
    b.emit_push(0).emit_output(Register(0));
}
fn set_first(b: &mut InstructionBuilder, spin: i64) {
    input(b);
    b.emit_push(0)
        .emit_push(spin)
        .emit(Instruction::VecSet { reg: Register(0) });
    output(b);
}
fn store(bytes: &[u8]) -> sp_core::H256 {
    let hash = <Test as frame_system::Config>::Hashing::hash(bytes);
    sp_io::storage::set(hash.as_bytes(), bytes);
    hash
}
fn register_program_spec(hash: sp_core::H256) -> sp_core::H256 {
    let name = bounded(b"transform".to_vec());
    assert_ok!(QuantumComputeMempool::register_job_spec(
        RuntimeOrigin::root(),
        1,
        name.clone(),
        Formulation::Ising,
        None,
        Some(hash),
    ));
    <Test as frame_system::Config>::Hashing::hash_of(&(
        name,
        Formulation::Ising,
        None::<sp_core::H256>,
        Some(hash),
    ))
}
fn setup(spec: sp_core::H256) {
    assert_ok!(QuantumComputeMempool::register_solver(
        RuntimeOrigin::signed(2),
        MinerType::Cpu
    ));
    assert_ok!(QuantumComputeMempool::propose_job(
        RuntimeOrigin::signed(1),
        spec,
        sample_params(),
        MinReward::get(),
        JobMode::Open,
        RewardResolution::SingleBest,
        10,
        1,
        ResultDelivery::OnChainOnly,
    ));
}
fn run_steps(bytes: &[u8], spins: Vec<i8>, limit: u64) -> u64 {
    let mut vm = Vm::new();
    vm.set_calldata(vec![RegVal::VecInt(
        spins.into_iter().map(i64::from).collect(),
    )])
    .set_output_slots(1)
    .set_memory_limit(VmMemory::get())
    .set_step_limit(limit);
    let _ = vm.run(&Program::decode(bytes).unwrap());
    vm.steps()
}
fn base_weight() -> frame_support::weights::Weight {
    <() as crate::weights::WeightInfo>::submit_solution(
        MaxNodes::get(),
        MaxEdges::get(),
        MaxSolutions::get(),
    )
}

#[test]
fn registration_rejects_absent_bad_and_unverified_programs_without_changes() {
    new_test_ext().execute_with(|| {
        use_real_vm();
        let bad = store(&[1, 2, 3]);
        // Well-formed container, but NEG has no stack operand.
        let rejected = store(&Program::new(vec![0x27, 0xFF]).encode());
        for (hash, error) in [
            (
                sp_core::H256::repeat_byte(99),
                Error::<Test>::VmProgramNotFound,
            ),
            (bad, Error::<Test>::VmBadBytecode),
            (rejected, Error::<Test>::VmVerificationFailed),
        ] {
            let error: sp_runtime::DispatchError = error.into();
            for validation in [false, true] {
                let (vp, tp) = if validation {
                    (Some(hash), None)
                } else {
                    (None, Some(hash))
                };
                assert_noop!(
                    QuantumComputeMempool::register_job_spec(
                        RuntimeOrigin::root(),
                        1,
                        bounded(b"bad".to_vec()),
                        Formulation::Ising,
                        vp,
                        tp
                    ),
                    error
                );
            }
        }
    });
}

#[test]
fn real_transform_changes_stored_spins_and_charges_steps() {
    new_test_ext().execute_with(|| {
        use_real_vm();
        let program = bytes(|b| set_first(b, -1));
        let spec = register_program_spec(store(&program));
        setup(spec);
        let expected_steps = run_steps(&program, vec![1, 1], MaxTransformSteps::get());
        assert!(expected_steps > 0);
        let info =
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution())
                .unwrap();
        assert_eq!(
            OrderSolutions::<Test>::get(0, 2).unwrap().solutions[0].as_slice(),
            &[-1, 1]
        );
        assert_eq!(
            info.actual_weight,
            Some(
                base_weight().saturating_add(QuantumComputeMempool::transform_weight(
                    expected_steps,
                    true
                ))
            )
        );
        let declared = crate::Call::<Test>::submit_solution {
            order_id: 0,
            solutions: sample_solution(),
        }
        .get_dispatch_info()
        .call_weight;
        assert!(info.actual_weight.unwrap().all_lte(declared));
        assert!(declared.ref_time() > info.actual_weight.unwrap().ref_time());
    });
}

#[test]
fn transform_failures_are_named_atomic_and_retain_consumed_steps() {
    let cases = [
        (
            bytes(|b| {
                b.emit_push(1).emit_stow(Register(0));
                output(b);
            }),
            Error::<Test>::VmOutputType,
        ),
        // Sample and Model are not integer-spin vectors, even at the right size.
        (
            bytes(|b| {
                b.emit_push(2).emit_ssmx(Register(0));
                output(b);
            }),
            Error::<Test>::VmOutputType,
        ),
        (
            bytes(|b| {
                b.emit_push(2).emit_sqmx(Register(0));
                output(b);
            }),
            Error::<Test>::VmOutputType,
        ),
        (
            bytes(|b| {
                b.emit(Instruction::VecI { reg: Register(0) });
                output(b);
            }),
            Error::<Test>::VmOutputLength,
        ),
        (bytes(|b| set_first(b, 0)), Error::<Test>::VmOutputSpin),
        (
            bytes(|b| {
                b.emit_push(1).emit_push(0).emit_div();
            }),
            Error::<Test>::VmExecutionFailed,
        ),
    ];
    for (program, error) in cases {
        new_test_ext().execute_with(|| {
            use_real_vm();
            let spec = register_program_spec(store(&program));
            setup(spec);
            let steps = run_steps(&program, vec![1, 1], MaxTransformSteps::get());
            assert!(steps > 0);
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    sample_solution()
                )
                .map_err(|error| error.error),
                error
            );
            let failure = QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                sample_solution(),
            )
            .unwrap_err();
            assert_eq!(
                failure.post_info.actual_weight,
                Some(
                    base_weight()
                        .saturating_add(QuantumComputeMempool::transform_weight(steps, true))
                )
            );
            assert!(
                failure.post_info.actual_weight.unwrap().ref_time()
                    >= steps * VmStepWeight::get().ref_time()
            );
        });
    }
}

#[test]
fn aggregate_budget_is_not_reset_between_solutions() {
    new_test_ext().execute_with(|| {
        use_real_vm();
        let program = bytes(|b| {
            input(b);
            output(b);
        });
        let spec = register_program_spec(store(&program));
        setup(spec);
        let one = run_steps(&program, vec![1, 1], 1000);
        MaxTransformSteps::set(one + 1);
        let solutions = bounded(vec![bounded(vec![1, 1]), bounded(vec![-1, 1])]);
        assert_noop!(
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, solutions.clone())
                .map_err(|error| error.error),
            Error::<Test>::VmStepLimitExceeded
        );
        let err = QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, solutions)
            .unwrap_err();
        assert_eq!(
            err.post_info.actual_weight,
            Some(
                base_weight()
                    .saturating_add(QuantumComputeMempool::transform_weight(one + 1, true))
            )
        );
    });
}

#[test]
fn late_output_failure_and_post_transform_scoring_failure_keep_steps() {
    new_test_ext().execute_with(|| {
        use_real_vm();
        let program = bytes(|b| {
            input(b);
            output(b);
        });
        let spec = register_program_spec(store(&program));
        setup(spec);
        let one = run_steps(&program, vec![1, 1], 1000);
        let bad = bounded(vec![bounded(vec![1, 1]), bounded(vec![0, 1])]);
        assert_noop!(
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, bad.clone())
                .map_err(|error| error.error),
            Error::<Test>::VmOutputSpin
        );
        let err =
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, bad).unwrap_err();
        assert_eq!(
            err.post_info.actual_weight,
            Some(
                base_weight()
                    .saturating_add(QuantumComputeMempool::transform_weight(2 * one, true))
            )
        );
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(2),
            0,
            sample_solution()
        ));
        assert_noop!(
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution())
                .map_err(|error| error.error),
            Error::<Test>::SolutionNotImproved
        );
        let err =
            QuantumComputeMempool::submit_solution(RuntimeOrigin::signed(2), 0, sample_solution())
                .unwrap_err();
        assert_eq!(
            err.post_info.actual_weight,
            Some(base_weight().saturating_add(QuantumComputeMempool::transform_weight(one, true)))
        );
    });
}

#[test]
fn missing_or_corrupt_transform_after_registration_rejects_without_state_changes() {
    for corrupt in [false, true] {
        new_test_ext().execute_with(|| {
            use_real_vm();
            let hash = store(&bytes(|b| {
                input(b);
                output(b);
            }));
            let spec = register_program_spec(hash);
            setup(spec);
            if corrupt {
                sp_io::storage::set(hash.as_bytes(), &[1]);
            } else {
                sp_io::storage::clear(hash.as_bytes());
            }
            let error = if corrupt {
                Error::<Test>::VmBadBytecode
            } else {
                Error::<Test>::VmProgramNotFound
            };
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    sample_solution()
                )
                .map_err(|error| error.error),
                error
            );
            let err = QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                sample_solution(),
            )
            .unwrap_err();
            assert_eq!(
                err.post_info.actual_weight,
                Some(
                    base_weight().saturating_add(QuantumComputeMempool::transform_weight(0, true))
                )
            );
        });
    }
}

#[test]
fn default_spec_is_byte_identical_to_identity_vm() {
    for spins in [vec![1, 1], vec![1, -1], vec![0, 1], vec![1]] {
        let run = |real: bool| {
            new_test_ext_with_default_ising_spec(true).execute_with(|| {
                if real {
                    use_real_vm();
                }
                setup(QuantumComputeMempool::default_ising_spec_id());
                let result = QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    bounded(vec![bounded(spins.clone())]),
                )
                .map(|_| ())
                .map_err(|error| error.error);
                (
                    result,
                    OrderSolutions::<Test>::get(0, 2),
                    JobOrders::<Test>::get(0),
                    System::events(),
                )
                    .encode()
            })
        };
        assert_eq!(run(false), run(true));
    }
}

#[test]
fn validation_program_is_verified_but_never_executed_at_reward_validation() {
    new_test_ext().execute_with(|| {
        let program = bytes(|b| {
            b.emit_push(1).emit_push(0).emit_div();
        });
        let hash = store(&program);
        type Adapter = crate::xqvm::XqvmAdapter<Test>;
        assert_ok!(Adapter::validate_programs(Some(&hash), None));
        assert_ok!(Adapter::validate_result(&hash, Some(&hash), None, &[]));
        let outcome = Adapter::transform_solutions(&hash, Some(&hash), None, &2, vec![vec![1, 1]]);
        assert_eq!(outcome.steps_used, 0);
        assert_eq!(outcome.result.unwrap(), vec![vec![1, 1]]);
    });
}

#[test]
fn memory_limit_and_zero_step_budget_reject_without_changes() {
    for zero_steps in [false, true] {
        new_test_ext().execute_with(|| {
            use_real_vm();
            let program = bytes(|b| {
                input(b);
                output(b);
            });
            let spec = register_program_spec(store(&program));
            setup(spec);
            let error = if zero_steps {
                MaxTransformSteps::set(0);
                Error::<Test>::VmStepLimitExceeded
            } else {
                VmMemory::set(1);
                Error::<Test>::VmMemoryLimitExceeded
            };
            assert_noop!(
                QuantumComputeMempool::submit_solution(
                    RuntimeOrigin::signed(2),
                    0,
                    sample_solution()
                )
                .map_err(|error| error.error),
                error
            );
            let failure = QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(2),
                0,
                sample_solution(),
            )
            .unwrap_err();
            let steps = if zero_steps {
                0
            } else {
                run_steps(&program, vec![1, 1], MaxTransformSteps::get())
            };
            assert_eq!(
                failure.post_info.actual_weight,
                Some(
                    base_weight()
                        .saturating_add(QuantumComputeMempool::transform_weight(steps, true))
                )
            );
        });
    }
}

#[test]
fn payee_submission_uses_the_same_meter_on_success_and_fault() {
    for fail in [false, true] {
        new_test_ext().execute_with(|| {
            use_real_vm();
            let program = if fail {
                bytes(|b| {
                    b.emit_push(1).emit_push(0).emit_div();
                })
            } else {
                bytes(|b| set_first(b, -1))
            };
            let spec = register_program_spec(store(&program));
            setup(spec);
            let steps = run_steps(&program, vec![1, 1], 1000);
            let expected = <() as crate::weights::WeightInfo>::submit_solution_with_payee(
                MaxNodes::get(),
                MaxEdges::get(),
                MaxSolutions::get(),
            )
            .saturating_add(QuantumComputeMempool::transform_weight(steps, true));
            if fail {
                assert_noop!(
                    QuantumComputeMempool::submit_solution_with_payee(
                        RuntimeOrigin::signed(2),
                        0,
                        sample_solution(),
                        payee(20033, 1)
                    )
                    .map_err(|error| error.error),
                    Error::<Test>::VmExecutionFailed
                );
                let err = QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(2),
                    0,
                    sample_solution(),
                    payee(20033, 1),
                )
                .unwrap_err();
                assert_eq!(err.post_info.actual_weight, Some(expected));
            } else {
                let info = QuantumComputeMempool::submit_solution_with_payee(
                    RuntimeOrigin::signed(2),
                    0,
                    sample_solution(),
                    payee(20033, 1),
                )
                .unwrap();
                assert_eq!(info.actual_weight, Some(expected));
                assert_eq!(
                    OrderSolutions::<Test>::get(0, 2).unwrap().solutions[0].as_slice(),
                    &[-1, 1]
                );
            }
        });
    }
}

//! Tests exercise the production runtime adapter and its actual program source.
use super::*;
use frame_support::{assert_ok, dispatch::GetDispatchInfo, traits::Currency};
use pallet_quantum_compute_mempool::{
    self as mempool,
    types::{
        Formulation, IsingParams, JobMode, MinerType, ResultDelivery, RewardResolution,
        WinnerSummary,
    },
    xqvm::{QuantumVm, TransformOutcome},
};
use sp_runtime::{traits::Hash as _, BuildStorage, DispatchResult};
use xqvm::{Instruction, InstructionBuilder, Register};

// Test-only mutation target. It is never an exported production adapter.
pub struct NoOpVm;
impl QuantumVm<AccountId, Balance, Hash> for NoOpVm {
    fn validate_programs(_: Option<&Hash>, _: Option<&Hash>) -> DispatchResult {
        Ok(())
    }
    fn transform_solutions(
        _: &Hash,
        _: Option<&Hash>,
        _: Option<&Hash>,
        _: &AccountId,
        solutions: Vec<Vec<i8>>,
    ) -> TransformOutcome {
        TransformOutcome {
            result: Ok(solutions),
            steps_used: 0,
        }
    }
    fn validate_result(
        _: &Hash,
        _: Option<&Hash>,
        _: Option<&Hash>,
        _: &[WinnerSummary<AccountId, Balance>],
    ) -> DispatchResult {
        Ok(())
    }
}

#[test]
fn real_runtime_wiring_changes_persisted_spins() {
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    ext.execute_with(|| {
        System::set_block_number(1);
        let proposer = AccountId::new([1; 32]);
        let solver = AccountId::new([2; 32]);
        <Balances as Currency<AccountId>>::make_free_balance_be(&proposer, 100 * UNIT);
        let mut b = InstructionBuilder::new();
        b.emit_push(0)
            .emit_input(Register(0))
            .emit_push(0)
            .emit_push(-1)
            .emit(Instruction::VecSet { reg: Register(0) })
            .emit_push(0)
            .emit_output(Register(0))
            .emit_halt();
        let bytes = b.build().unwrap().encode();
        let hash = <Runtime as frame_system::Config>::Hashing::hash(&bytes);
        // Exercise the real storage owner and the runtime ProgramLookup bridge.
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(proposer.clone()),
            bytes.try_into().unwrap()
        ));
        let name: frame_support::BoundedVec<u8, frame_support::traits::ConstU32<128>> =
            b"real-transform".to_vec().try_into().unwrap();
        let spec = <Runtime as frame_system::Config>::Hashing::hash_of(&(
            name.clone(),
            Formulation::Ising,
            None::<Hash>,
            Some(hash),
        ));
        assert_ok!(QuantumComputeMempool::register_job_spec(
            RuntimeOrigin::root(),
            proposer.clone(),
            name,
            Formulation::Ising,
            None,
            Some(hash)
        ));
        assert_ok!(QuantumComputeMempool::register_solver(
            RuntimeOrigin::signed(solver.clone()),
            MinerType::Cpu
        ));
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(proposer),
            spec,
            IsingParams {
                nodes: vec![0, 1].try_into().unwrap(),
                edges: vec![(0, 1)].try_into().unwrap(),
                h_values: vec![0, 0].try_into().unwrap(),
                j_values: vec![-1000].try_into().unwrap(),
                max_energy_milli: None,
                min_diversity_milli: None,
                min_solutions: None,
            },
            2 * UNIT,
            JobMode::Open,
            RewardResolution::SingleBest,
            10,
            1,
            ResultDelivery::OnChainOnly
        ));
        assert_ok!(QuantumComputeMempool::submit_solution(
            RuntimeOrigin::signed(solver.clone()),
            0,
            vec![vec![1, 1].try_into().unwrap()].try_into().unwrap()
        ));
        let accepted = mempool::OrderSolutions::<Runtime>::get(0, &solver).unwrap();
        assert_eq!(
            accepted.solutions[0].as_slice(),
            &[-1, 1],
            "production runtime must execute the transform, not identity"
        );
        assert_eq!(accepted.best_energy_milli, 1000);
    });
}

#[test]
fn vm_limits_are_shared_and_submission_precharge_fits_the_block() {
    assert!(
        <Runtime as mempool::Config>::MaxTransformStepsPerSubmission::get()
            <= <Runtime as pallet_xqvm::Config>::MaxStepLimit::get()
    );
    assert_eq!(
        <Runtime as mempool::Config>::MaxVmMemory::get(),
        <Runtime as pallet_xqvm::Config>::MaxVmMemory::get()
    );
    assert_eq!(
        <Runtime as mempool::Config>::VmWeightPerStep::get(),
        <Runtime as pallet_xqvm::Config>::WeightPerStep::get()
    );
    // One aggregate cap for the whole batch, not one full cap per solution.
    let reservation = QuantumComputeMempool::transform_weight(
        <Runtime as mempool::Config>::MaxTransformStepsPerSubmission::get(),
        true,
    );
    let call = mempool::Call::<Runtime>::submit_solution {
        order_id: 0,
        solutions: Default::default(),
    };
    let declared = call.get_dispatch_info().call_weight;
    assert!(reservation.all_lte(declared));
    let limits = <Runtime as frame_system::Config>::BlockWeights::get();
    let normal = limits.get(frame_support::dispatch::DispatchClass::Normal);
    assert!(declared.all_lte(normal.max_extrinsic.unwrap()));
    assert!(declared.all_lte(normal.max_total.unwrap()));
}

#[test]
fn no_program_adapter_matches_test_only_noop() {
    type RealVm = <Runtime as mempool::Config>::VM;
    let input = vec![vec![1, -1]];
    let solver = AccountId::new([2; 32]);
    let real = RealVm::transform_solutions(&Hash::zero(), None, None, &solver, input.clone());
    let noop = NoOpVm::transform_solutions(&Hash::zero(), None, None, &solver, input);
    assert_eq!(real.steps_used, noop.steps_used);
    assert_eq!(real.result, noop.result);
}

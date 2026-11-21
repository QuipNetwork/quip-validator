//! Consensus execution of registered spin transforms.
//!
//! None/None specs are inert. Referenced transform programs execute on chain;
//! changes to their semantics require a runtime change, not an operational update.
//! Validation programs are checked at registration but never executed on the
//! summary-only reward path. Extending that payload and metering it is a future
//! owner decision.

use alloc::vec::Vec;

use sp_runtime::{DispatchError, DispatchResult};

use crate::types::WinnerSummary;

/// Mempool-owned integration contract; runtimes bridge it to program storage.
/// The mempool does not depend on the pallet that owns the program bytes.
pub trait QuantumVm<AccountId, Balance, Hash> {
    /// Validate referenced validation/transform programs when a job spec is registered.
    fn validate_programs(
        validation_program: Option<&Hash>,
        transform_program: Option<&Hash>,
    ) -> DispatchResult;

    /// Transform submitted solutions before the pallet scores them.
    fn transform_solutions(
        spec_id: &Hash,
        validation_program: Option<&Hash>,
        transform_program: Option<&Hash>,
        solver: &AccountId,
        solutions: Vec<Vec<i8>>,
    ) -> TransformOutcome;

    /// The payload contains solver, energy and amount, not spins. This slice
    /// executes no validation program here: it is verified at registration only.
    /// Spin validation belongs to `transform_solutions`. Executing validation
    /// programs requires an owner-approved payload extension and metering.
    fn validate_result(
        spec_id: &Hash,
        validation_program: Option<&Hash>,
        transform_program: Option<&Hash>,
        winners: &[WinnerSummary<AccountId, Balance>],
    ) -> DispatchResult;
}

/// Source of immutable, content-addressed program bytes. Implementations must
/// bound byte size and verifier control-flow complexity before admitting bytes
/// to storage, and supply weights covering their lookup/decode/verification bounds.
/// Runtime wiring reads pallet-xqvm storage; the mempool owns no program storage.
pub trait ProgramLookup<Hash> {
    fn program(hash: &Hash) -> Option<Vec<u8>>;
}

/// Execution accounting survives both VM failures and output validation errors.
pub struct TransformOutcome {
    pub result: Result<Vec<Vec<i8>>, DispatchError>,
    pub steps_used: u64,
}

/// Deterministic xqvm 0.5.0 adapter with one budget shared by all submitted spins.
pub struct XqvmAdapter<T>(core::marker::PhantomData<T>);

impl<T: crate::Config> QuantumVm<T::AccountId, crate::BalanceOf<T>, T::Hash> for XqvmAdapter<T> {
    fn validate_programs(
        validation_program: Option<&T::Hash>,
        transform_program: Option<&T::Hash>,
    ) -> DispatchResult {
        for hash in [validation_program, transform_program]
            .into_iter()
            .flatten()
        {
            let bytes =
                T::ProgramSource::program(hash).ok_or(crate::Error::<T>::VmProgramNotFound)?;
            let program =
                ::xqvm::Program::decode(&bytes).map_err(|_| crate::Error::<T>::VmBadBytecode)?;
            ::xqvm::verifier::verify(&program)
                .map_err(|_| crate::Error::<T>::VmVerificationFailed)?;
        }
        Ok(())
    }

    fn transform_solutions(
        _spec_id: &T::Hash,
        _validation_program: Option<&T::Hash>,
        transform_program: Option<&T::Hash>,
        _solver: &T::AccountId,
        solutions: Vec<Vec<i8>>,
    ) -> TransformOutcome {
        let mut steps_used = 0;
        let result = Self::transform(transform_program, solutions, &mut steps_used);
        TransformOutcome { result, steps_used }
    }

    fn validate_result(
        _spec_id: &T::Hash,
        _validation_program: Option<&T::Hash>,
        _transform_program: Option<&T::Hash>,
        _winners: &[WinnerSummary<T::AccountId, crate::BalanceOf<T>>],
    ) -> DispatchResult {
        // Summary-only: do not execute unmetered code that cannot inspect spins.
        Ok(())
    }
}

/// The transform calldata ABI: exactly one `VecInt` carrying a solution's
/// spins. It can carry no model, sample or model vector. That is load-bearing
/// for the runtime's VM memory bound: `LVAL` clones a model without charging
/// memory, and it is unreachable on chain only because no on-chain input can
/// populate a `VecXqmx` (see the runtime's `MaxVmMemory`). A change that lets
/// this ABI carry a model must fail the runtime's calldata tripwire test.
pub fn transform_calldata(solution: Vec<i8>) -> Vec<::xqvm::RegVal> {
    alloc::vec![::xqvm::RegVal::VecInt(
        solution.into_iter().map(i64::from).collect()
    )]
}

impl<T: crate::Config> XqvmAdapter<T> {
    fn transform(
        hash: Option<&T::Hash>,
        solutions: Vec<Vec<i8>>,
        steps_used: &mut u64,
    ) -> Result<Vec<Vec<i8>>, DispatchError> {
        use ::xqvm::{Program, RegVal, Vm};
        use frame_support::{ensure, traits::Get};
        let Some(hash) = hash else {
            return Ok(solutions);
        };
        // One immutable program per spec. All lookup/decode errors precede
        // execution; later solution failures preserve earlier consumed steps.
        let bytes = T::ProgramSource::program(hash).ok_or(crate::Error::<T>::VmProgramNotFound)?;
        let program = Program::decode(&bytes).map_err(|_| crate::Error::<T>::VmBadBytecode)?;
        let mut transformed = Vec::with_capacity(solutions.len());
        for solution in solutions {
            let remaining = T::MaxTransformStepsPerSubmission::get().saturating_sub(*steps_used);
            ensure!(remaining > 0, crate::Error::<T>::VmStepLimitExceeded);
            let expected_len = solution.len();
            // This narrow ABI transports integer spins in one VecInt. The
            // pallet-xqvm execute API accepts scalar integers; introducing
            // arbitrary model/sample output semantics here would be a liability.
            // Only one VM lives at a time: drop it before the next solution, so
            // peak VM residency is a single `MaxVmMemory` cap, never
            // `MaxSolutions` simultaneous caps. Two terms are retained across
            // the loop rather than per run: the one decoded program, and the
            // `transformed` accumulator below, which grows one output per
            // solution and is bounded by `MaxSolutions` × `MaxNodes` bytes
            // (~0.1 MiB). Both are small beside a VM cap.
            let mut vm = Vm::new();
            vm.set_memory_limit(T::MaxVmMemory::get())
                .set_step_limit(remaining)
                .set_output_slots(1)
                .set_calldata(transform_calldata(solution));
            let result = vm.run(&program);
            *steps_used = steps_used.saturating_add(vm.steps());
            result.map_err(|error| match error {
                ::xqvm::Error::StepLimitExceeded { .. } => crate::Error::<T>::VmStepLimitExceeded,
                ::xqvm::Error::MemoryLimitExceeded { .. } => {
                    crate::Error::<T>::VmMemoryLimitExceeded
                }
                _ => crate::Error::<T>::VmExecutionFailed,
            })?;
            let [RegVal::VecInt(spins)] = vm.outputs() else {
                return Err(crate::Error::<T>::VmOutputType.into());
            };
            ensure!(
                spins.len() == expected_len,
                crate::Error::<T>::VmOutputLength
            );
            ensure!(
                spins.iter().all(|spin| matches!(spin, -1 | 1)),
                crate::Error::<T>::VmOutputSpin
            );
            transformed.push(
                spins
                    .iter()
                    .map(|spin| if *spin == -1 { -1 } else { 1 })
                    .collect(),
            );
        }
        Ok(transformed)
    }
}

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(test)]
mod mock;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

pub mod weights;
pub use weights::*;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::vec::Vec;
    use frame_support::dispatch::PostDispatchInfo;
    use frame_support::pallet_prelude::*;
    use frame_support::traits::fungible::{Mutate, MutateHold};
    use frame_support::traits::tokens::Precision;
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::{Hash as _, Saturating};

    use xqvm::{Program, RegVal, Vm};

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// The balance type of the configured currency.
    pub type BalanceOf<T> = <<T as Config>::Currency as frame_support::traits::fungible::Inspect<
        <T as frame_system::Config>::AccountId,
    >>::Balance;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// The currency the storage deposit is taken in.
        type Currency: Mutate<Self::AccountId>
            + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;

        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;

        /// Flat part of a program's storage deposit.
        ///
        /// Covers the two map entries a stored program occupies, in the same
        /// spirit as `pallet_revive`'s per-item deposit.
        #[pallet::constant]
        type DepositBase: Get<BalanceOf<Self>>;

        /// Per-byte part of a program's storage deposit.
        ///
        /// This is what makes storage cost something: the extrinsic's weight
        /// prices decode and verification as execution time, and says nothing
        /// about every full node keeping the bytes forever.
        #[pallet::constant]
        type DepositPerByte: Get<BalanceOf<Self>>;

        /// Maximum size of a stored XQVM program in bytes.
        #[pallet::constant]
        type MaxProgramSize: Get<u32>;

        /// Maximum number of calldata entries (i64 values).
        #[pallet::constant]
        type MaxCallDataLen: Get<u32>;

        /// Maximum number of output slots.
        #[pallet::constant]
        type MaxOutputSlots: Get<u32>;

        /// Maximum bytes an execution may allocate.
        ///
        /// Bounds the VM's allocation budget, which is a distinct resource
        /// from the step budget: steps price the *work* an opcode does, this
        /// bounds what it may ask the runtime to hold while doing it.
        #[pallet::constant]
        type MaxVmMemory: Get<u64>;

        /// Maximum step limit per execution.
        #[pallet::constant]
        type MaxStepLimit: Get<u64>;

        /// Weight charged per XQVM execution step (ref_time component).
        #[pallet::constant]
        type WeightPerStep: Get<Weight>;

        type WeightInfo: WeightInfo;
    }

    // ── Storage ──────────────────────────────────────────────────────────

    /// Stored XQVM programs, keyed by Blake2-256 hash of the encoded bytes.
    #[pallet::storage]
    pub type Programs<T: Config> =
        StorageMap<_, Identity, T::Hash, BoundedVec<u8, T::MaxProgramSize>>;

    /// Who stored each program. Confers the right to remove it and receive
    /// the deposit back.
    #[pallet::storage]
    pub type ProgramOwner<T: Config> = StorageMap<_, Identity, T::Hash, T::AccountId>;

    /// The deposit held against each stored program.
    ///
    /// Recorded rather than recomputed from the byte length, so that a later
    /// change to `DepositBase` or `DepositPerByte` cannot release more or
    /// less than was actually taken.
    #[pallet::storage]
    pub type ProgramDeposit<T: Config> = StorageMap<_, Identity, T::Hash, BalanceOf<T>>;

    /// Reasons this pallet holds funds.
    #[pallet::composite_enum]
    pub enum HoldReason {
        /// Deposit backing a program stored on chain.
        #[codec(index = 0)]
        StoredProgram,
    }

    // ── Events ───────────────────────────────────────────────────────────

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A program was stored on-chain.
        ProgramStored {
            program_hash: T::Hash,
            owner: T::AccountId,
            size: u32,
            deposit: BalanceOf<T>,
        },

        /// A program was removed by its owner and the deposit released.
        ProgramRemoved {
            program_hash: T::Hash,
            owner: T::AccountId,
            deposit: BalanceOf<T>,
        },

        /// A program was evicted by root. The deposit is returned: eviction
        /// clears state, it does not punish, and content addressing means
        /// anyone can store the same bytes again.
        ProgramEvicted {
            program_hash: T::Hash,
            owner: T::AccountId,
            deposit: BalanceOf<T>,
        },

        /// A program executed successfully.
        ProgramExecuted {
            caller: T::AccountId,
            program_hash: T::Hash,
            steps_used: u64,
            outputs: BoundedVec<i64, T::MaxOutputSlots>,
        },
    }

    // ── Errors ───────────────────────────────────────────────────────────

    #[pallet::error]
    pub enum Error<T> {
        /// The bytecode is not a well-formed XQBC container: bad magic,
        /// unsupported format version, length mismatch, or CRC-32 mismatch.
        ///
        /// This covers the container only. Faults in the instruction stream
        /// itself get their own variants below, from the verifier.
        InvalidBytecode,
        /// Verifier: an instruction is truncated or uses an unknown opcode.
        VerifierBadInstruction,
        /// Verifier: a jump references a target that does not exist.
        VerifierUndefinedJumpTarget,
        /// Verifier: loop opens and closes do not balance, or a loop-context
        /// read occurs with no active loop.
        VerifierLoopImbalance,
        /// Verifier: a register is read before it is written, on at least one
        /// path through the program.
        VerifierReadUnsetRegister,
        /// Verifier: a register is read at a type it was not written as.
        VerifierRegisterTypeMismatch,
        /// Verifier: the program can underflow or overflow the value stack, or
        /// reaches a join point at inconsistent stack depths.
        VerifierStackFault,
        /// A program with this hash already exists.
        ProgramAlreadyExists,
        /// No program found for the given hash.
        ProgramNotFound,
        /// The caller is not the account that stored this program.
        NotProgramOwner,
        /// Requested step limit exceeds MaxStepLimit.
        StepLimitTooHigh,
        /// A step limit of zero was requested.
        ///
        /// Rejected rather than forwarded: a zero-step execution can never
        /// succeed, and under xqvm 0.3.x the `0` sentinel even meant
        /// "unlimited". The guard keeps the bound independent of the
        /// library's convention.
        ZeroStepLimit,
        /// Output slot count exceeds MaxOutputSlots.
        TooManyOutputSlots,
        /// XQVM: stack underflow.
        VmStackUnderflow,
        /// XQVM: stack overflow.
        VmStackOverflow,
        /// XQVM: division by zero.
        VmDivisionByZero,
        /// XQVM: step limit exceeded.
        VmStepLimitExceeded,
        /// XQVM: the program asked to allocate more than `MaxVmMemory`.
        ///
        /// "Asked for too much memory" is a budgeting answer the caller can
        /// act on by splitting the work up, not a broken program.
        VmMemoryLimitExceeded,
        /// XQVM: bad opcode or truncated instruction.
        VmBadBytecode,
        /// XQVM: register type mismatch.
        VmRegisterType,
        /// XQVM: a register was read before anything was written to it.
        ///
        /// The verifier rejects this statically at `store_program`, so
        /// reaching it at run time means a path it could not prove.
        VmUnsetRegister,
        /// XQVM: an arithmetic operation left the `i64` range.
        ///
        /// Since xqvm 0.4.0 overflow raises instead of wrapping, so this is
        /// a fault a program can hit rather than a silently wrong result.
        VmArithmeticOverflow,
        /// XQVM: a vector or sample index was outside its length.
        VmIndexOutOfBounds,
        /// XQVM: a loop opcode ran with no active loop, or a loop was left
        /// unclosed at the end of the stream.
        VmLoopStructure,
        /// XQVM: loop nesting exceeded the interpreter's depth limit.
        VmLoopStackOverflow,
        /// XQVM: a jump named a target or label that does not resolve.
        VmBadJump,
        /// XQVM: `INPUT` addressed a calldata slot the call did not supply.
        VmCallDataIndex,
        /// XQVM: `OUTPUT` addressed a slot beyond the requested count.
        VmOutputIndex,
        /// XQVM: two operands disagreed on size, or a vector's length did
        /// not match what the opcode required.
        VmSizeMismatch,
        /// XQVM: a shift amount was negative or at least 64.
        VmInvalidShift,
        /// XQVM: grid dimensions were negative, or exceeded the register's
        /// declared size.
        VmInvalidGridDimensions,
        /// XQVM: a discrete sample was allocated with `k < 2`.
        VmInvalidDiscreteK,
        /// XQVM: an allocation size was negative or otherwise not an
        /// allocation.
        VmInvalidAllocation,
        /// XQVM: the tracing interpreter failed.
        ///
        /// Unreachable from this pallet: `TraceFailed` is only raised by
        /// `Vm::run_trace`, and `execute` calls `Vm::run`. Mapped explicitly
        /// rather than absorbed, so that the match below stays exhaustive.
        VmTraceFailed,
    }

    /// Map a static-verification failure onto a dispatch error.
    ///
    /// Exhaustive on purpose: a new `VerifierError` variant upstream should
    /// stop this compiling rather than be absorbed by a catch-all, the same
    /// discipline `map_vm_error` applies to runtime faults.
    fn map_verifier_error<T: Config>(e: &xqvm::VerifierError) -> Error<T> {
        use xqvm::VerifierError as V;
        match e {
            V::TruncatedInstruction { .. } | V::BadOpcode { .. } => {
                Error::<T>::VerifierBadInstruction
            }
            V::UndefinedJumpTarget { .. } => Error::<T>::VerifierUndefinedJumpTarget,
            V::NoActiveLoop { .. } | V::UnmatchedLoop { .. } => Error::<T>::VerifierLoopImbalance,
            V::ReadUnsetRegister { .. } => Error::<T>::VerifierReadUnsetRegister,
            V::RegisterTypeMismatch { .. } => Error::<T>::VerifierRegisterTypeMismatch,
            V::StackUnderflow { .. }
            | V::StackOverflowRisk { .. }
            | V::LoopStackImbalance { .. }
            | V::StackDepthMismatch { .. } => Error::<T>::VerifierStackFault,
        }
    }

    /// Map a run-time fault onto a dispatch error.
    ///
    /// Exhaustive, and deliberately without a catch-all. The wildcard this
    /// replaced absorbed every variant the pallet had not thought about,
    /// which is how nine opcodes' worth of new faults arrived across the
    /// QUI-997 bump without a compile error: on chain they all read as
    /// "something went wrong in the VM", which is not enough to debug a
    /// submitted program from the outside. Now a new upstream variant stops
    /// the build until someone decides what it means here.
    ///
    /// Faults are grouped only where the grouping is the answer a caller
    /// would act on -- an unclosed loop and a loop-context read with no
    /// active loop are both "the loop structure is wrong".
    fn map_vm_error<T: Config>(e: &xqvm::Error) -> Error<T> {
        use xqvm::Error as E;
        match e {
            E::StackUnderflow { .. } => Error::<T>::VmStackUnderflow,
            E::StackOverflow { .. } => Error::<T>::VmStackOverflow,
            E::DivisionByZero { .. } => Error::<T>::VmDivisionByZero,
            E::StepLimitExceeded { .. } => Error::<T>::VmStepLimitExceeded,
            E::BadOpcode { .. } | E::TruncatedInstruction { .. } => Error::<T>::VmBadBytecode,
            // `IncompatibleType` is the same fault as `RegisterType` raised
            // from a site that does not track which register caused it, so
            // the caller-visible answer is identical.
            E::RegisterType { .. } | E::IncompatibleType(_) => Error::<T>::VmRegisterType,
            E::MemoryLimitExceeded { .. } => Error::<T>::VmMemoryLimitExceeded,
            E::UnsetRegister { .. } => Error::<T>::VmUnsetRegister,
            E::ArithmeticOverflow { .. } => Error::<T>::VmArithmeticOverflow,
            E::IndexOutOfBounds { .. } => Error::<T>::VmIndexOutOfBounds,
            E::NoActiveLoop { .. } | E::UnmatchedLoop { .. } => Error::<T>::VmLoopStructure,
            E::LoopStackOverflow { .. } => Error::<T>::VmLoopStackOverflow,
            E::BadJumpTarget { .. } | E::InvalidLabel { .. } => Error::<T>::VmBadJump,
            E::CallDataIndex { .. } => Error::<T>::VmCallDataIndex,
            E::OutputIndex { .. } => Error::<T>::VmOutputIndex,
            E::SizeMismatch { .. } | E::VecLengthMismatch { .. } => Error::<T>::VmSizeMismatch,
            E::InvalidShift { .. } => Error::<T>::VmInvalidShift,
            E::InvalidGridDimensions { .. } => Error::<T>::VmInvalidGridDimensions,
            E::InvalidDiscreteK { .. } => Error::<T>::VmInvalidDiscreteK,
            E::InvalidAllocation { .. } => Error::<T>::VmInvalidAllocation,
            E::TraceFailed { .. } => Error::<T>::VmTraceFailed,
        }
    }

    // ── Extrinsics ───────────────────────────────────────────────────────

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Store an XQVM program on-chain.
        ///
        /// The bytecode is decoded, which checks the XQBC container, and then
        /// statically verified, which checks the instruction stream. Both must
        /// pass before anything is written, so a program that is accepted here
        /// cannot fail `execute` for a reason the verifier covers.
        ///
        /// Verifying at store time rather than at execute time puts the cost
        /// on the account that introduced the program, once, instead of on
        /// every account that runs it. It is also what lets `execute` skip
        /// re-verification later (QUI-1057).
        ///
        /// The program is stored keyed by its Blake2-256 hash for
        /// deduplication.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::store_program(bytecode.len() as u32))]
        pub fn store_program(
            origin: OriginFor<T>,
            bytecode: BoundedVec<u8, T::MaxProgramSize>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Container first, then the instruction stream.
            let program = Program::decode(&bytecode).map_err(|_| Error::<T>::InvalidBytecode)?;
            xqvm::verifier::verify(&program).map_err(|e| map_verifier_error::<T>(&e))?;

            let hash = T::Hashing::hash(&bytecode);
            ensure!(
                !Programs::<T>::contains_key(&hash),
                Error::<T>::ProgramAlreadyExists
            );

            let size = bytecode.len() as u32;

            // Taken before the write, so a caller who cannot afford the
            // deposit does not get the storage.
            let deposit = Self::deposit_for(size);
            T::Currency::hold(&HoldReason::StoredProgram.into(), &who, deposit)?;

            Programs::<T>::insert(&hash, bytecode);
            ProgramOwner::<T>::insert(&hash, &who);
            ProgramDeposit::<T>::insert(&hash, deposit);

            Self::deposit_event(Event::ProgramStored {
                program_hash: hash,
                owner: who,
                size,
                deposit,
            });
            Ok(())
        }

        /// Remove a program stored by the caller and release its deposit.
        ///
        /// Removal is not destructive in any lasting sense: programs are
        /// addressed by the hash of their bytecode, so anyone can store the
        /// same bytes again by paying a fresh deposit. That is what makes it
        /// safe to let the owner delete unilaterally without worrying about
        /// callers mid-way through using it.
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::remove_program(T::MaxProgramSize::get()))]
        pub fn remove_program(
            origin: OriginFor<T>,
            program_hash: T::Hash,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;

            let owner = ProgramOwner::<T>::get(&program_hash).ok_or(Error::<T>::ProgramNotFound)?;
            ensure!(owner == who, Error::<T>::NotProgramOwner);

            let (size, deposit) = Self::do_remove(&program_hash, &owner)?;

            Self::deposit_event(Event::ProgramRemoved {
                program_hash,
                owner,
                deposit,
            });

            // Pre-charged at MaxProgramSize because the length is not known
            // until storage is read, and refunded to what was actually there,
            // the same shape `execute` uses for its size component.
            Ok(Some(T::WeightInfo::remove_program(size)).into())
        }

        /// Evict a stored program by root, returning the deposit to whoever
        /// stored it.
        ///
        /// A state-management tool rather than a punitive one: the deposit is
        /// returned, and the same bytes can be stored again by anyone.
        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::evict_program(T::MaxProgramSize::get()))]
        pub fn evict_program(
            origin: OriginFor<T>,
            program_hash: T::Hash,
        ) -> DispatchResultWithPostInfo {
            ensure_root(origin)?;

            let owner = ProgramOwner::<T>::get(&program_hash).ok_or(Error::<T>::ProgramNotFound)?;

            let (size, deposit) = Self::do_remove(&program_hash, &owner)?;

            Self::deposit_event(Event::ProgramEvicted {
                program_hash,
                owner,
                deposit,
            });

            Ok(Some(T::WeightInfo::evict_program(size)).into())
        }

        /// Execute a stored XQVM program.
        ///
        /// Both dimensions of the cost are caller-influenced and neither is
        /// known from the call arguments alone, so both are pre-charged at
        /// their worst case and refunded via `PostDispatchInfo`:
        ///
        /// * **Program size.** The call carries only a hash, so the length is
        ///   unknown until storage is read. Decoding runs a full verifier scan
        ///   over every instruction, which is linear in the byte length, so
        ///   `MaxProgramSize` is pre-charged and the actual length refunded.
        /// * **Steps.** Pre-charged on the caller's `step_limit`, refunded to
        ///   the steps actually executed.
        ///
        /// The error path deliberately does not refund: over-charging a failed
        /// execution is the conservative direction.
        #[pallet::call_index(1)]
        #[pallet::weight(
            T::WeightInfo::execute(T::MaxProgramSize::get())
                .saturating_add(
                    T::WeightPerStep::get().saturating_mul(*step_limit)
                )
        )]
        pub fn execute(
            origin: OriginFor<T>,
            program_hash: T::Hash,
            calldata: BoundedVec<i64, T::MaxCallDataLen>,
            output_slots: u32,
            step_limit: u64,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;

            // Defence in depth. `xqvm 0.3.1` mapped a step limit of `0` to
            // `u64::MAX`, so an unguarded pass-through ran unbounded while
            // pre-charging only the base weight. xqvm 0.4.0 removed the
            // sentinel (QUI-1053); this check stays regardless, because a
            // consensus-critical bound must not depend on a library's
            // sentinel convention.
            ensure!(step_limit > 0, Error::<T>::ZeroStepLimit);
            ensure!(
                step_limit <= T::MaxStepLimit::get(),
                Error::<T>::StepLimitTooHigh
            );
            ensure!(
                output_slots <= T::MaxOutputSlots::get(),
                Error::<T>::TooManyOutputSlots
            );

            let bytecode = Programs::<T>::get(&program_hash).ok_or(Error::<T>::ProgramNotFound)?;
            let program_len = bytecode.len() as u32;
            let program = Program::decode(&bytecode).map_err(|_| Error::<T>::VmBadBytecode)?;

            // `Vm::new` installs a 1 GiB budget sized for an off-chain
            // host, so this is not an optimisation -- it is the bound. The
            // budget has to be set explicitly because the step limit does not
            // imply it: steps price the work an allocation performs, not the
            // residency it leaves behind, and the two diverge by orders of
            // magnitude. Left at the default, a program could ask the runtime
            // for more than its heap holds, and a failed allocation inside
            // Wasm traps the whole execution rather than returning a fault
            // this pallet could report.
            let mut vm = Vm::new();
            vm.set_memory_limit(T::MaxVmMemory::get())
                .set_step_limit(step_limit)
                .set_output_slots(output_slots as usize)
                .set_calldata(calldata.iter().map(|&v| RegVal::Int(v)).collect());

            match vm.run(&program) {
                Ok(()) => {
                    let steps_used = vm.steps();
                    let outputs: BoundedVec<i64, T::MaxOutputSlots> = vm
                        .outputs()
                        .iter()
                        .map(|r| match r {
                            RegVal::Int(v) => *v,
                            _ => 0i64,
                        })
                        .collect::<Vec<_>>()
                        .try_into()
                        .expect("output_slots <= MaxOutputSlots checked above");

                    Self::deposit_event(Event::ProgramExecuted {
                        caller: who,
                        program_hash,
                        steps_used,
                        outputs,
                    });

                    let actual_weight = T::WeightInfo::execute(program_len)
                        .saturating_add(T::WeightPerStep::get().saturating_mul(steps_used));
                    Ok(PostDispatchInfo {
                        actual_weight: Some(actual_weight),
                        pays_fee: Pays::Yes,
                    })
                }
                Err(e) => Err(map_vm_error::<T>(&e).into()),
            }
        }
    }

    impl<T: Config> Pallet<T> {
        /// The deposit a program of `size` bytes costs to keep on chain.
        pub fn deposit_for(size: u32) -> BalanceOf<T> {
            T::DepositBase::get()
                .saturating_add(T::DepositPerByte::get().saturating_mul(size.into()))
        }

        /// Clear a program's three storage entries and release its deposit.
        ///
        /// Returns the program's byte length and the deposit released, both
        /// of which the callers use for the weight refund and the event.
        fn do_remove(
            program_hash: &T::Hash,
            owner: &T::AccountId,
        ) -> Result<(u32, BalanceOf<T>), DispatchError> {
            let bytecode = Programs::<T>::take(program_hash).ok_or(Error::<T>::ProgramNotFound)?;
            let size = bytecode.len() as u32;
            let deposit = ProgramDeposit::<T>::take(program_hash).unwrap_or_default();

            // BestEffort rather than Exact: the entry is being removed either
            // way, and a hold that has already been reduced elsewhere must not
            // strand the storage. There is no path that reduces it today, so
            // this is defence against a future one, not a known case.
            T::Currency::release(
                &HoldReason::StoredProgram.into(),
                owner,
                deposit,
                Precision::BestEffort,
            )?;

            ProgramOwner::<T>::remove(program_hash);

            Ok((size, deposit))
        }
    }
}

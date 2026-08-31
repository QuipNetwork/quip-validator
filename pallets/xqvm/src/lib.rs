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
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::Hash as _;

    use xqvm::{Program, RegVal, Vm};

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Maximum size of a stored XQVM program in bytes.
        #[pallet::constant]
        type MaxProgramSize: Get<u32>;

        /// Maximum number of calldata entries (i64 values).
        #[pallet::constant]
        type MaxCallDataLen: Get<u32>;

        /// Maximum number of output slots.
        #[pallet::constant]
        type MaxOutputSlots: Get<u32>;

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

    /// Who stored each program (for future deposit/removal support).
    #[pallet::storage]
    pub type ProgramOwner<T: Config> = StorageMap<_, Identity, T::Hash, T::AccountId>;

    // ── Events ───────────────────────────────────────────────────────────

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A program was stored on-chain.
        ProgramStored {
            program_hash: T::Hash,
            owner: T::AccountId,
            size: u32,
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
        /// The bytecode failed to decode as a valid XQVM program.
        InvalidBytecode,
        /// A program with this hash already exists.
        ProgramAlreadyExists,
        /// No program found for the given hash.
        ProgramNotFound,
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
        /// XQVM: bad opcode or truncated instruction.
        VmBadBytecode,
        /// XQVM: register type mismatch.
        VmRegisterType,
        /// XQVM: other runtime fault.
        VmRuntimeError,
    }

    fn map_vm_error<T: Config>(e: &xqvm::Error) -> Error<T> {
        use xqvm::Error as E;
        match e {
            E::StackUnderflow { .. } => Error::<T>::VmStackUnderflow,
            E::StackOverflow { .. } => Error::<T>::VmStackOverflow,
            E::DivisionByZero { .. } => Error::<T>::VmDivisionByZero,
            E::StepLimitExceeded { .. } => Error::<T>::VmStepLimitExceeded,
            E::BadOpcode { .. } | E::TruncatedInstruction { .. } => Error::<T>::VmBadBytecode,
            E::RegisterType { .. } => Error::<T>::VmRegisterType,
            _ => Error::<T>::VmRuntimeError,
        }
    }

    // ── Extrinsics ───────────────────────────────────────────────────────

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Store an XQVM program on-chain.
        ///
        /// The bytecode is validated by decoding it. The program is stored
        /// keyed by its Blake2-256 hash for deduplication.
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::store_program(bytecode.len() as u32))]
        pub fn store_program(
            origin: OriginFor<T>,
            bytecode: BoundedVec<u8, T::MaxProgramSize>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            // Validate bytecode
            Program::decode(&bytecode).map_err(|_| Error::<T>::InvalidBytecode)?;

            let hash = T::Hashing::hash(&bytecode);
            ensure!(
                !Programs::<T>::contains_key(&hash),
                Error::<T>::ProgramAlreadyExists
            );

            let size = bytecode.len() as u32;
            Programs::<T>::insert(&hash, bytecode);
            ProgramOwner::<T>::insert(&hash, &who);

            Self::deposit_event(Event::ProgramStored {
                program_hash: hash,
                owner: who,
                size,
            });
            Ok(())
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

            let mut vm = Vm::new();
            vm.set_step_limit(step_limit)
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
}

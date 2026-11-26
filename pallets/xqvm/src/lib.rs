#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

/// The structural gate `store_program` runs (QUI-2516): loop nesting must
/// balance and no jump may cross a loop-body boundary (W1-011). With both, the
/// run-time loop stack equals the lexical nesting at every pc, so
/// `MaxLoopDepth` bounds the loop frames a run can hold.
///
/// Public so the runtime's VM-memory census and the benchmarks check exactly
/// what admission runs, not xqvm's full checker.
pub fn structural_gate(program: &xqvm::Program) -> Result<(), xqvm::CheckerError> {
    use xqvm::checker::{Checker, JumpSpanPhase, LoopNestingPhase};
    Checker::new()
        .with_phase(LoopNestingPhase)
        .with_phase(JumpSpanPhase)
        .run(program)
}

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
    use frame_support::traits::fungible::MutateHold;
    use frame_support::traits::tokens::Precision;
    use frame_support::traits::StorageVersion;
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::{Hash as _, Saturating};

    use xqvm::{Program, RegVal, Vm};

    /// The storage layout this code expects: `Programs`, `ProgramOwner` and
    /// `ProgramDeposit`, all keyed by program hash.
    ///
    /// Declared now, while the layout is fresh and no live chain holds any
    /// program, so the first in-place upgrade has a recorded version to
    /// migrate from rather than an implicit zero.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
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
        ///
        /// The pallet itself only ever holds and releases. Benchmarks also
        /// need to conjure a balance to hold against, so they ask for
        /// `Mutate` on top; production currencies are not made to promise
        /// arbitrary mutation for the benchmarks' sake.
        #[cfg(not(feature = "runtime-benchmarks"))]
        type Currency: MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;
        #[cfg(feature = "runtime-benchmarks")]
        type Currency: frame_support::traits::fungible::Mutate<Self::AccountId>
            + MutateHold<Self::AccountId, Reason = Self::RuntimeHoldReason>;

        /// The overarching hold reason.
        type RuntimeHoldReason: From<HoldReason>;

        /// Flat part of a program's storage deposit.
        ///
        /// Covers the fixed footprint of a stored program -- its entries in
        /// `Programs`, `ProgramOwner` and `ProgramDeposit`, plus the hold
        /// recorded against the storer -- in the same spirit as
        /// `pallet_revive`'s per-item deposit.
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

        /// Maximum number of basic blocks a stored program may have, counted
        /// the way xqvm's CFG builder counts them: see [`ControlFlowShape`].
        ///
        /// Bounds what `store_program` allocates before it can report a
        /// fault, which `MaxProgramSize` does not: a block can be a single
        /// byte. Decoding records every `TARGET`, and the structural gate's
        /// jump-span pass records every loop opener, `NEXT` and jump; each of
        /// those starts a block. A failed allocation inside Wasm traps the
        /// whole execution rather than returning a fault this pallet could
        /// report, which is the same reason `MaxVmMemory` exists for the VM's
        /// own allocations.
        #[pallet::constant]
        type MaxProgramBlocks: Get<u32>;

        /// Maximum static loop nesting depth of a stored program.
        ///
        /// Also the bound on the loop frames a run can hold. The structural
        /// gate rejects unbalanced nesting and every jump across a loop-body
        /// boundary (W1-011), so the run-time loop stack equals the lexical
        /// nesting at every pc. The runtime's W1-002 VM memory bound depends
        /// on it: raising it needs the census re-run.
        #[pallet::constant]
        type MaxLoopDepth: Get<u32>;

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
        /// itself get their own variants below, from the structural gate.
        InvalidBytecode,
        /// Structural gate: an instruction is truncated or uses an unknown
        /// opcode.
        VerifierBadInstruction,
        /// Structural gate: a jump references a target that does not exist.
        VerifierUndefinedJumpTarget,
        /// Structural gate: loop opens and closes do not balance, or a
        /// loop-context read occurs with no active loop.
        VerifierLoopImbalance,
        /// Retired (QUI-2516): only the full checker raised this, and
        /// `store_program` no longer runs it. The VM reports the fault at
        /// `execute` as `VmUnsetRegister`. Kept so no error index moves.
        VerifierReadUnsetRegister,
        /// Retired (QUI-2516): only the full checker raised this, and
        /// `store_program` no longer runs it. The VM reports the fault at
        /// `execute` as `VmRegisterType`. Kept so no error index moves.
        VerifierRegisterTypeMismatch,
        /// Retired (QUI-2516): only the full checker raised this, and
        /// `store_program` no longer runs it. The VM reports the fault at
        /// `execute` as `VmStackUnderflow` or `VmStackOverflow`. Kept so no
        /// error index moves.
        VerifierStackFault,
        /// The program has more basic blocks than `MaxProgramBlocks`.
        ///
        /// Checked before the structural gate's passes, whose memory it
        /// bounds.
        TooManyBlocks,
        /// The program nests loops deeper than `MaxLoopDepth`.
        ///
        /// Checked before the structural gate's passes. The depth also bounds
        /// the loop frames a run can hold: see `MaxLoopDepth`.
        LoopNestingTooDeep,
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
        /// `store_program` does not check this (QUI-2516); the VM raises it
        /// when a run reaches the read.
        VmUnsetRegister,
        /// XQVM: an arithmetic operation left the `i64` range.
        ///
        /// Since xqvm 0.4.0 overflow raises instead of wrapping, so this is
        /// a fault a program can hit rather than a silently wrong result.
        VmArithmeticOverflow,
        /// XQVM: an index was outside what it addressed -- a vector or
        /// sample element, a model's coefficient, or a grid cell.
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
        /// XQVM: grid dimensions were zero or negative, or described more
        /// cells than the register declares.
        VmInvalidGridDimensions,
        /// XQVM: an integer-domain model or sample was allocated with
        /// `k < 2`.
        VmInvalidIntegerK,
        /// XQVM: an allocation size was negative or otherwise not an
        /// allocation.
        VmInvalidAllocation,
        /// XQVM: the tracing interpreter failed.
        ///
        /// Unreachable from this pallet: `TraceFailed` is only raised by
        /// `Vm::run_trace`, and `execute` calls `Vm::run`. Mapped explicitly
        /// rather than absorbed, so that the match below stays exhaustive.
        VmTraceFailed,
        /// XQVM: `SETLINE` or `ADDLINE` wrote a value outside the domain
        /// the sample was allocated with.
        ///
        /// Appended rather than placed beside the other sample faults so
        /// that the indices of the existing variants do not move.
        VmSampleOutOfDomain,
        /// Retired (QUI-2516): only the full checker's analyses raised this,
        /// and `store_program` no longer runs them. Kept so no error index
        /// moves.
        VerifierAnalysisBudgetExceeded,
        /// The stored bytes are not the program's canonical encoding (W1-003a):
        /// `Program::decode` recomputes the XQBC header's slot bytes, so bytes
        /// that differ only in the header decode to the same program yet hash
        /// differently. Rejected so `hash(bytecode)` is a stable identity.
        /// Appended to preserve every existing pallet error index.
        NonCanonicalProgram,
        /// A jump's target lies in a different loop body from the jump
        /// (W1-011, xqvm 0.6.0): a back-edge that re-enters a loop opener
        /// without its `NEXT`, or a jump into or out of a loop body.
        /// Appended to preserve every existing pallet error index.
        VerifierJumpAcrossLoopBoundary,
        /// XQVM: a constraint opcode (`ONEHOTR`, `ONEHOTC`, `EXCLUDE`,
        /// `IMPLIES`, `EQUALITY`, `ATLEAST`, `ATLEASTW`, `REDUCE`) was applied
        /// to a model whose declared domain is not binary (xqvm 0.6.0).
        /// Appended to preserve every existing pallet error index.
        VmNonBinaryModel,
    }

    /// Map a structural-gate failure onto a dispatch error.
    ///
    /// Exhaustive on purpose, with no catch-all: a new `CheckerError`
    /// variant upstream should stop this compiling rather than be absorbed,
    /// the same discipline `map_vm_error` applies to run-time faults.
    ///
    /// `LoopNestingPhase` and `JumpSpanPhase` emit only the first six
    /// variants below. The other seven come from the CFG phases the gate no
    /// longer runs; each keeps the dispatch error it had before, so a client
    /// matching on an error index never sees a reused slot.
    fn map_structural_error<T: Config>(e: &xqvm::CheckerError) -> Error<T> {
        use xqvm::CheckerError as C;
        match e {
            // Emitted by the two phases.
            C::TruncatedInstruction { .. } | C::BadOpcode { .. } => {
                Error::<T>::VerifierBadInstruction
            }
            C::NoActiveLoop { .. } | C::UnmatchedLoop { .. } => Error::<T>::VerifierLoopImbalance,
            C::UndefinedJumpTarget { .. } => Error::<T>::VerifierUndefinedJumpTarget,
            C::JumpAcrossLoopBoundary { .. } => Error::<T>::VerifierJumpAcrossLoopBoundary,
            // Only the CFG phases emit these; mapped to their former slots.
            C::AnalysisBudgetExceeded => Error::<T>::VerifierAnalysisBudgetExceeded,
            C::ReadUnsetRegister { .. } => Error::<T>::VerifierReadUnsetRegister,
            C::RegisterTypeMismatch { .. } => Error::<T>::VerifierRegisterTypeMismatch,
            C::StackUnderflow { .. }
            | C::StackOverflowRisk { .. }
            | C::LoopStackImbalance { .. }
            | C::StackDepthMismatch { .. } => Error::<T>::VerifierStackFault,
        }
    }

    #[cfg(test)]
    #[test]
    fn analysis_budget_failure_has_a_distinct_dispatch_error() {
        assert!(matches!(
            map_structural_error::<crate::mock::Test>(&xqvm::CheckerError::AnalysisBudgetExceeded),
            Error::<crate::mock::Test>::VerifierAnalysisBudgetExceeded
        ));
    }

    #[cfg(test)]
    #[test]
    fn first_witness_stack_diagnostics_preserve_the_dispatch_error() {
        use xqvm::CheckerError as V;

        // The upstream FIFO policy can change which stack fault is reported.
        // Offsets and depths remain upstream details; the dispatch ABI groups
        // all four variants under the existing VerifierStackFault index.
        for error in [
            V::StackUnderflow { offset: 0 },
            V::StackOverflowRisk {
                offset: 1,
                depth: 8193,
            },
            V::LoopStackImbalance {
                opener: 2,
                entry: 1,
                exit: 0,
            },
            V::StackDepthMismatch {
                target_offset: 3,
                depth_a: 1,
                depth_b: 0,
            },
        ] {
            assert!(matches!(
                map_structural_error::<crate::mock::Test>(&error),
                Error::<crate::mock::Test>::VerifierStackFault
            ));
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
            E::NonBinaryModel { .. } => Error::<T>::VmNonBinaryModel,
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
            E::InvalidIntegerK { .. } => Error::<T>::VmInvalidIntegerK,
            E::SampleOutOfDomain { .. } => Error::<T>::VmSampleOutOfDomain,
            E::InvalidAllocation { .. } => Error::<T>::VmInvalidAllocation,
            E::TraceFailed { .. } => Error::<T>::VmTraceFailed,
        }
    }

    /// The static control-flow shape of an instruction stream.
    ///
    /// `blocks` counts basic-block leaders exactly as xqvm's CFG builder
    /// does: the entry, every `TARGET`, and the position after every jump,
    /// `RANGE`, `ITER` and `NEXT` -- a set, so a `TARGET` right after a jump
    /// is one block, not two. Counting only `TARGET`s, as this pallet first
    /// did, left a single `TARGET` followed by two thousand `JUMP`s at "one
    /// block" while the CFG had two thousand.
    ///
    /// `loop_depth` is the deepest static nesting of `RANGE`/`ITER` openers
    /// before their `NEXT`. It only counts: an unmatched `NEXT` does not go
    /// negative and an unclosed opener is not reported. Balance is
    /// `LoopNestingPhase`'s job at `store_program`. For every program that
    /// phase accepts, this depth equals its deepest nesting
    /// (`control_flow_shape_depth_agrees_with_loop_nesting`).
    ///
    /// One linear pass with xqvm's instruction decoder, so it costs what
    /// decoding costs and is covered by the per-byte weight term.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ControlFlowShape {
        /// Basic blocks in xqvm's CFG.
        pub blocks: u32,
        /// Deepest static loop nesting.
        pub loop_depth: u32,
    }

    impl ControlFlowShape {
        /// Measure `code`, the raw instruction stream of a decoded program.
        pub fn of(code: &[u8]) -> Self {
            use xqvm::{Instruction, InstructionStream};

            // The entry is always a leader. `leader_pending` records that the
            // previous instruction already made the current position a
            // leader, so a `TARGET` here must not be counted again.
            let mut blocks: u32 = 1;
            let mut leader_pending = true;
            let mut depth: u32 = 0;
            let mut loop_depth: u32 = 0;

            let mut stream = InstructionStream::new(code);
            while let Some(item) = stream.next_instruction() {
                // A structural fault ends the walk; the structural gate
                // reports it. Everything counted so far still bounds the work
                // the gate does before it gets there.
                let Ok((_, _, instr)) = item else { break };
                match instr {
                    Instruction::Target {} => {
                        if !leader_pending {
                            blocks = blocks.saturating_add(1);
                        }
                        leader_pending = false;
                    }
                    Instruction::Jump1 { .. }
                    | Instruction::Jump2 { .. }
                    | Instruction::JumpI1 { .. }
                    | Instruction::JumpI2 { .. } => {
                        blocks = blocks.saturating_add(1);
                        leader_pending = true;
                    }
                    Instruction::Range {} | Instruction::Iter { .. } => {
                        blocks = blocks.saturating_add(1);
                        leader_pending = true;
                        depth = depth.saturating_add(1);
                        loop_depth = loop_depth.max(depth);
                    }
                    Instruction::Next {} => {
                        blocks = blocks.saturating_add(1);
                        leader_pending = true;
                        depth = depth.saturating_sub(1);
                    }
                    _ => leader_pending = false,
                }
            }

            Self { blocks, loop_depth }
        }
    }

    // ── Extrinsics ───────────────────────────────────────────────────────

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Store an XQVM program on-chain.
        ///
        /// The bytecode is decoded, which checks the XQBC container, and must
        /// be the program's canonical encoding. The structural gate then
        /// bounds its shape (`MaxProgramBlocks`, `MaxLoopDepth`) and runs two
        /// lexical passes: every instruction must decode
        /// (`VerifierBadInstruction`), every jump must name an existing
        /// `TARGET` (`VerifierUndefinedJumpTarget`), loop nesting must balance,
        /// and no jump may cross a loop-body boundary (W1-011). Together they
        /// bound the loop frames a run can hold by `MaxLoopDepth` (QUI-2516).
        ///
        /// The gate does not prove register, type or stack properties. A
        /// stored program can still fault at `execute` for those reasons: the
        /// VM detects them at run time and the caller pays the pre-charged
        /// weight. `remove_program` returns the deposit of a program nobody
        /// can use.
        ///
        /// Decoding and both passes are linear in byte length up to a log
        /// factor: the jump-span pass binary-searches once per jump.
        ///
        /// The weight takes the byte length and the block count: the call
        /// pre-charges at `MaxProgramBlocks` and reports the weight at the
        /// verified block count. On the reference machine the block count
        /// has no measured cost beyond the per-byte one (the generated weight
        /// drops it), so the charge is linear in byte length and nothing is
        /// refunded; the refund path stays for a regeneration that measures
        /// a block cost.
        ///
        /// The program is stored keyed by its Blake2-256 hash for
        /// deduplication.
        #[pallet::call_index(0)]
        #[pallet::weight(
            T::WeightInfo::store_program(bytecode.len() as u32, T::MaxProgramBlocks::get())
        )]
        pub fn store_program(
            origin: OriginFor<T>,
            bytecode: BoundedVec<u8, T::MaxProgramSize>,
        ) -> DispatchResultWithPostInfo {
            let who = ensure_signed(origin)?;

            // Container first, then the shape bounds, then the structural
            // check. The shape is one linear walk over the bytes -- already
            // priced by the per-byte term -- and it bounds what the two
            // structural passes below allocate before they report any fault.
            let program = Program::decode(&bytecode).map_err(|_| Error::<T>::InvalidBytecode)?;
            // W1-003a: the stored bytes must be the program's own canonical
            // encoding. `decode` validates the CRC and length but ignores the
            // header's slot bytes (it recomputes them), so a non-canonical
            // header decodes cleanly yet re-encodes -- and hashes -- to
            // something else. Reject it here so the stored byte string is the
            // unique canonical form behind each program hash. Cheap: `encode`
            // is one allocation already bounded by the per-byte weight term.
            ensure!(
                program.encode().as_slice() == bytecode.as_slice(),
                Error::<T>::NonCanonicalProgram
            );
            let shape = ControlFlowShape::of(program.code());
            ensure!(
                shape.blocks <= T::MaxProgramBlocks::get(),
                Error::<T>::TooManyBlocks
            );
            ensure!(
                shape.loop_depth <= T::MaxLoopDepth::get(),
                Error::<T>::LoopNestingTooDeep
            );
            let blocks = shape.blocks;
            // QUI-2516: the structural gate, not the full checker. Loop
            // nesting must balance (`RANGE`/`ITER` against `NEXT`, `LIDX` and
            // `LVAL` only inside a loop) and no jump may cross a loop-body
            // boundary (W1-011). Together they make the run-time loop stack
            // equal the lexical nesting at every pc, so `MaxLoopDepth` bounds
            // the loop frames a run can hold. Both passes are lexical and
            // linear. Register, stack and type properties are left to the VM,
            // which faults on them at `execute` with the caller paying.
            crate::structural_gate(&program).map_err(|e| map_structural_error::<T>(&e))?;

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
            Ok(Some(T::WeightInfo::store_program(size, blocks)).into())
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
        ///   unknown until storage is read. Decoding runs xqvm's instruction
        ///   scan over every instruction, which is linear in the byte length, so
        ///   `MaxProgramSize` is pre-charged and the actual length refunded.
        /// * **Steps.** Pre-charged on the caller's `step_limit`, refunded to
        ///   the steps actually executed.
        ///
        /// A third dimension is pre-charged but deliberately **not** refunded:
        ///
        /// * **Output slots (W1-008 part 2).** The caller declares
        ///   `output_slots`, which sizes the VM's output vector up front. The
        ///   declared count is known from the call arguments, so it is charged
        ///   exactly — pre-charged and kept in the actual weight — rather than
        ///   charged at `MaxOutputSlots` and refunded. Charging the declared
        ///   count on both sides means a caller cannot declare a large output
        ///   area, pay the base weight, and have the output cost refunded away.
        ///
        /// The error path deliberately does not refund: over-charging a failed
        /// execution is the conservative direction.
        #[pallet::call_index(1)]
        #[pallet::weight(
            T::WeightInfo::execute(T::MaxProgramSize::get(), *output_slots)
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

                    // `output_slots` is charged at the declared count, the same
                    // value pre-charged above, so the output-slot term is kept
                    // (not refunded). Only program size and steps are refunded.
                    let actual_weight = T::WeightInfo::execute(program_len, output_slots)
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
        /// Returns the program's byte length and the amount actually
        /// released, which the callers use for the weight refund and the
        /// event.
        fn do_remove(
            program_hash: &T::Hash,
            owner: &T::AccountId,
        ) -> Result<(u32, BalanceOf<T>), DispatchError> {
            let bytecode = Programs::<T>::take(program_hash).ok_or(Error::<T>::ProgramNotFound)?;
            let size = bytecode.len() as u32;
            let recorded = ProgramDeposit::<T>::take(program_hash).unwrap_or_default();

            // BestEffort rather than Exact: the entry is being removed either
            // way, and a hold that has already been reduced elsewhere must not
            // strand the storage. There is no path that reduces it today, so
            // this is defence against a future one, not a known case -- and
            // the event reports what came back, not what was recorded, so it
            // stays truthful if that future arrives.
            let released = T::Currency::release(
                &HoldReason::StoredProgram.into(),
                owner,
                recorded,
                Precision::BestEffort,
            )?;

            ProgramOwner::<T>::remove(program_hash);

            Ok((size, released))
        }
    }
}

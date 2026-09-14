//! Benchmarking setup for pallet-xqvm.

use super::*;

#[allow(unused)]
use crate::Pallet as Xqvm;
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::BoundedVec;
use frame_system::RawOrigin;
use sp_runtime::traits::Hash as _;
use xqvm::{InstructionBuilder, Program};

/// Byte length of the XQBC wire-format header emitted by `Program::encode`.
const XQBC_HEADER_LEN: usize = 15;

/// The `HALT` opcode byte.
const HALT_OPCODE: u8 = 0xFF;

/// The `TARGET` opcode byte. Restated because the sled below is assembled
/// by hand: `InstructionBuilder` refuses to emit a label nothing jumps to,
/// and that refusal is exactly what makes this shape unreachable through
/// the builder while remaining perfectly acceptable to the verifier.
const TARGET_OPCODE: u8 = 0x00;

/// Build a valid XQVM program of exactly `target_len` bytes that is the worst
/// case for `store_program`.
///
/// `store_program` decodes and then statically verifies. Those two have
/// *different* worst-case shapes, and the expensive one wins:
///
/// * `Program::decode` walks the instruction stream once, so its cost is
///   driven by instruction count: a few nanoseconds per byte for any
///   one-byte opcode.
/// * `verifier::verify` additionally builds a control-flow graph and runs
///   its worklist analyses over it, so its cost is driven by basic-block
///   count, at roughly a microsecond per block natively. Every `TARGET`
///   opens a new block, and `TARGET` is one byte, so a sled of nothing but
///   `TARGET`s is one block per byte -- the densest CFG the wire format can
///   express.
///
/// Measured natively at `MaxProgramSize`, decode plus verify costs about
/// 1,270 ns/byte for the sled against 160 ns/byte for the densest shape the
/// builder can produce (`TARGET; PUSH 1; JUMPI` blocks, five bytes each),
/// and 25 ns/byte for a `NOP` sled. Getting this wrong is how
/// `store_program` came to be underpriced tenfold before QUI-1054, and then
/// eightfold again after it: each fix benchmarked the worst shape its
/// author could *build*, not the worst shape the verifier *accepts*.
///
/// The verifier accepts the sled because an unreferenced `TARGET` is not a
/// fault -- it is a no-op at run time and a block boundary at verify time.
/// The builder rejects it as an unused label, which is why the bytes are
/// assembled directly and wrapped with `Program::new`.
fn build_verifier_worst_case_program(target_len: u32) -> Vec<u8> {
    let len = target_len as usize;
    assert!(
        len > XQBC_HEADER_LEN,
        "minimum encoded program is header + HALT"
    );

    let mut code = alloc::vec![TARGET_OPCODE; len - XQBC_HEADER_LEN - 1];
    code.push(HALT_OPCODE);
    let bytes = Program::new(code).encode();

    assert_eq!(bytes.len(), len, "sled must land on the requested length");
    let decoded = Program::decode(&bytes).expect("sled is a well-formed container");
    xqvm::verifier::verify(&decoded).expect("an unreferenced TARGET is not a verifier fault");
    bytes
}

/// Build a valid XQVM program of exactly `target_len` bytes that halts on its
/// first step.
///
/// `HALT` comes first and the remaining bytes are NOPs, so decoding walks the
/// whole instruction stream while execution stops immediately. That separates
/// the per-byte decode cost from the per-step execution cost.
fn build_halt_first_program(target_len: u32) -> Vec<u8> {
    let len = target_len as usize;
    assert!(
        len > XQBC_HEADER_LEN,
        "minimum encoded program is header + HALT"
    );

    let mut b = InstructionBuilder::new();
    b.emit_halt();
    for _ in 0..(len - XQBC_HEADER_LEN - 1) {
        b.emit_nop();
    }
    let bytes = b.build().expect("HALT NOP* is a valid program").encode();

    assert_eq!(bytes.len(), len);
    assert!(
        Program::decode(&bytes).is_ok(),
        "program must round-trip through decode"
    );
    bytes
}

/// Build a fixed-length program that executes `iterations` loop steps.
///
/// `PUSH 0; PUSH iterations; RANGE; NEXT; HALT`, then padded with unreachable
/// NOPs to a constant encoded length so that varying `iterations` changes the
/// step count without changing the decode cost. `PUSH` is variable-width, so
/// the padding is what holds the length constant.
fn build_counted_loop_program(iterations: u32) -> Vec<u8> {
    /// Fixed encoded length for every counted-loop program. Comfortably above
    /// the longest `PUSH8` encoding of the loop bound.
    const LOOP_PROGRAM_LEN: usize = 64;

    let mut b = InstructionBuilder::new();
    b.emit_push(0);
    b.emit_push(i64::from(iterations));
    b.emit_range();
    b.emit_next();
    b.emit_halt();
    let core = b.build().expect("counted loop is a valid program").encode();

    assert!(
        core.len() <= LOOP_PROGRAM_LEN,
        "counted loop must fit the fixed length"
    );

    // Pad after HALT: scanned by decode, never executed.
    let mut b = InstructionBuilder::new();
    b.emit_push(0);
    b.emit_push(i64::from(iterations));
    b.emit_range();
    b.emit_next();
    b.emit_halt();
    for _ in 0..(LOOP_PROGRAM_LEN - core.len()) {
        b.emit_nop();
    }
    let bytes = b.build().expect("padded counted loop is valid").encode();

    assert_eq!(bytes.len(), LOOP_PROGRAM_LEN);
    assert!(
        Program::decode(&bytes).is_ok(),
        "program must round-trip through decode"
    );
    bytes
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn store_program(s: Linear<16, { 65_536 }>) {
        let caller: T::AccountId = whitelisted_caller();
        let bytecode = build_verifier_worst_case_program(s);
        let bounded: BoundedVec<u8, T::MaxProgramSize> =
            bytecode.try_into().expect("s <= MaxProgramSize");

        #[extrinsic_call]
        store_program(RawOrigin::Signed(caller), bounded);
    }

    /// Cost of `execute` as a function of stored program length, with
    /// execution itself held to a single step.
    ///
    /// The program is `HALT` followed by `s - XQBC_HEADER_LEN - 1` NOPs, so
    /// the VM stops at step 1 while `Program::decode` still walks all `s`
    /// bytes. Decoding rebuilds the jump table through `verifier::scan`,
    /// which is linear in instruction count; NOPs are one byte each, so this
    /// is the worst case per byte.
    #[benchmark]
    fn execute(s: Linear<16, { 65_536 }>) {
        let caller: T::AccountId = whitelisted_caller();

        let bytecode = build_halt_first_program(s);
        let hash = T::Hashing::hash(&bytecode);
        let bounded: BoundedVec<u8, T::MaxProgramSize> =
            bytecode.try_into().expect("s <= MaxProgramSize");
        crate::Programs::<T>::insert(&hash, bounded);

        let calldata: BoundedVec<i64, T::MaxCallDataLen> = BoundedVec::default();

        #[extrinsic_call]
        execute(RawOrigin::Signed(caller), hash, calldata, 0u32, 1u64);
    }

    /// Cost of `execute` as a function of executed steps, with program length
    /// held constant.
    ///
    /// The program is a `RANGE` loop with an empty body, so each iteration is
    /// a single `NEXT`, and the encoding is padded to a fixed length so the
    /// per-byte decode cost measured by `execute(s)` does not leak into this
    /// slope.
    ///
    /// `NEXT` is the most expensive of the cheaply-constructible opcodes, so
    /// it is the right floor to calibrate against: it charges one step like
    /// `NOP`, the unit a step is defined as, but costs more to dispatch.
    ///
    /// Since xqvm 0.4.0 the floor is also a ceiling for the opcodes it does
    /// not reach. Operand-scaling opcodes no longer run unbounded work for
    /// one step (QUI-1056); they charge additional steps in proportion to
    /// the work, each unit calibrated upstream so that it is not cheaper
    /// than the operation it stands for. Pricing every step at this slope is
    /// therefore conservative for them too.
    #[benchmark]
    fn execute_step(t: Linear<0, { 50_000 }>) {
        let caller: T::AccountId = whitelisted_caller();

        let bytecode = build_counted_loop_program(t);
        let hash = T::Hashing::hash(&bytecode);
        let bounded: BoundedVec<u8, T::MaxProgramSize> = bytecode
            .try_into()
            .expect("loop program fits MaxProgramSize");
        crate::Programs::<T>::insert(&hash, bounded);

        let calldata: BoundedVec<i64, T::MaxCallDataLen> = BoundedVec::default();

        // PUSH, PUSH, RANGE, t x NEXT, HALT -- plus headroom, so the step
        // limit is never the binding constraint.
        let step_limit = u64::from(t).saturating_add(16);

        #[extrinsic_call]
        execute(RawOrigin::Signed(caller), hash, calldata, 0u32, step_limit);
    }

    impl_benchmark_test_suite!(Xqvm, crate::mock::new_test_ext(), crate::mock::Test);
}

//! Benchmarking setup for pallet-xqvm.

use super::*;

#[allow(unused)]
use crate::Pallet as Xqvm;
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::traits::Get as _;
use frame_support::BoundedVec;
use frame_system::RawOrigin;
use sp_runtime::traits::Hash as _;
use xqvm::{InstructionBuilder, Program};

/// Byte length of the XQBC wire-format header emitted by `Program::encode`.
const XQBC_HEADER_LEN: usize = 15;

/// The `HALT` opcode byte.
const HALT_OPCODE: u8 = 0xFF;

/// Opcode bytes for the hand-assembled shapes below. `InstructionBuilder`
/// refuses a label nothing jumps to, and that refusal is exactly what makes
/// the densest shapes unreachable through the builder while remaining
/// perfectly acceptable to the verifier.
const TARGET_OPCODE: u8 = 0x00;
const NEXT_OPCODE: u8 = 0x07;
const RANGE_OPCODE: u8 = 0x08;
const PUSH1_OPCODE: u8 = 0x11;
const NOP_OPCODE: u8 = 0xF0;

/// Upper bounds of the block component and the nesting the shape uses.
/// Constants because `Linear` needs one; the benchmark asserts they do not
/// exceed the configured `MaxProgramBlocks`/`MaxLoopDepth`, which is where
/// the bounds actually live.
const MAX_BENCH_BLOCKS: u32 = 2_048;
const MAX_BENCH_LOOP_DEPTH: u32 = 32;

/// Bytes of one loop level in the shape: `PUSH1 0; PUSH1 1; RANGE` opening
/// it and `NEXT` closing it.
const LOOP_LEVEL_BYTES: usize = 6;

/// Build a valid XQVM program of exactly `target_len` bytes whose verifier
/// CFG has exactly `blocks` basic blocks, at the deepest loop nesting the
/// block budget allows (up to `MAX_BENCH_LOOP_DEPTH`).
///
/// `store_program` decodes and then statically verifies, and the two cost
/// different things:
///
/// * `Program::decode` walks the instruction stream once, so its cost is
///   driven by byte count: a few nanoseconds per one-byte opcode, whatever
///   the opcode is.
/// * `verifier::verify` builds a control-flow graph and runs its analyses
///   over it, so its cost -- and, more to the point, its memory -- is
///   driven by basic-block count: roughly a microsecond and 3 KiB per block
///   natively. Its loop check then re-walks every loop region's blocks once
///   per enclosing loop, so on top of that it is linear in block count
///   times nesting depth, at ~40 ns per block visit.
///
/// The shape is therefore `n` nested loops around a run of `TARGET`s, with
/// `n` as deep as `MAX_BENCH_LOOP_DEPTH` and the block budget allow, then
/// `NOP` padding to the requested length. Every block sits inside every
/// loop, which is the most work the verifier can be made to do for the
/// block count -- and since the per-block price is measured at maximum
/// nesting, it is conservative for every shallower program.
///
/// Measured natively at 2,048 blocks: a flat `TARGET` sled verifies in
/// ~2 ms, the same blocks inside 32 nested loops in ~4.7 ms, inside 64 in
/// ~7.8 ms. Unbounded, the same opcodes are a different story: 2,048 bare
/// nested `RANGE`/`NEXT` pairs take 255 ms and 16,384 take 20 s, and a
/// pure `TARGET` sled at `MaxProgramSize` exhausted the runtime allocator
/// on the reference machine. Those are why blocks and nesting are bounded
/// and the block count is a separately priced, refunded component.
///
/// The verifier accepts the shape because an unreferenced `TARGET` is not
/// a fault -- it is a no-op at run time and a block boundary at verify
/// time. The builder rejects it as an unused label, which is why the bytes
/// are assembled directly and wrapped with `Program::new`.
fn build_blocks_program(target_len: u32, blocks: u32) -> Vec<u8> {
    let len = target_len as usize;
    assert!(
        len > XQBC_HEADER_LEN,
        "minimum encoded program is header + HALT"
    );
    assert!(blocks >= 1, "every program has at least its entry block");
    let body = len - XQBC_HEADER_LEN - 1;
    let blocks = blocks as usize;

    // The entry block is free: a bare `HALT` is one block in zero body
    // bytes. Past that, each loop level costs two leaders (after RANGE,
    // after NEXT) and `LOOP_LEVEL_BYTES`; a TARGET costs one leader and one
    // byte, except the first, which shares the leader the last RANGE (or
    // the entry) already made. So `blocks = 2n + t`, `bytes = 6n + t`, and
    // the nesting is the deepest that both the block count and the byte
    // budget allow.
    let (nesting, targets) = if blocks == 1 {
        (0, 0)
    } else {
        assert!(
            blocks <= body,
            "{blocks} blocks do not fit in {body} body bytes"
        );
        let nesting = (MAX_BENCH_LOOP_DEPTH as usize)
            .min((blocks - 1) / 2)
            .min((body - blocks) / (LOOP_LEVEL_BYTES - 2));
        (nesting, blocks - 2 * nesting)
    };

    let mut code = Vec::with_capacity(body + 1);
    for _ in 0..nesting {
        code.extend_from_slice(&[PUSH1_OPCODE, 0, PUSH1_OPCODE, 1, RANGE_OPCODE]);
    }
    code.extend(core::iter::repeat_n(TARGET_OPCODE, targets));
    code.extend(core::iter::repeat_n(NEXT_OPCODE, nesting));
    code.resize(body, NOP_OPCODE);
    code.push(HALT_OPCODE);
    let bytes = Program::new(code).encode();

    assert_eq!(
        bytes.len(),
        len,
        "program must land on the requested length"
    );
    let decoded = Program::decode(&bytes).expect("well-formed container");
    let shape = ControlFlowShape::of(decoded.code());
    assert_eq!(
        shape.blocks as usize, blocks,
        "shape must have the requested blocks"
    );
    assert_eq!(
        shape.loop_depth as usize, nesting,
        "shape must nest as computed"
    );
    xqvm::verifier::verify(&decoded).expect("nested loops around unreferenced TARGETs verify");
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

    /// Cost of `store_program` in its two dimensions: `s` bytes to decode
    /// and `b` basic blocks to verify, the blocks nested as deeply as the
    /// bounds allow.
    ///
    /// FRAME varies one component with the others at their maximum, so the
    /// `s` series is 2,048 blocks at full nesting plus `NOP` padding (the
    /// per-byte decode slope) and the `b` series is 64 KiB with a growing
    /// block count (the per-block verify slope, nesting included). Below
    /// ~2,200 bytes the maximum block count does not fit and is clamped to
    /// the body length; that bends the first two of fifty `s` points and
    /// nothing else.
    #[benchmark]
    fn store_program(s: Linear<16, { 65_536 }>, b: Linear<1, MAX_BENCH_BLOCKS>) {
        assert!(
            MAX_BENCH_BLOCKS <= T::MaxProgramBlocks::get(),
            "benchmark block range must stay within MaxProgramBlocks"
        );
        assert!(
            MAX_BENCH_LOOP_DEPTH <= T::MaxLoopDepth::get(),
            "benchmark nesting must stay within MaxLoopDepth"
        );
        let caller: T::AccountId = whitelisted_caller();
        let blocks = b.min(s - XQBC_HEADER_LEN as u32 - 1).max(1);
        let bytecode = build_blocks_program(s, blocks);
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

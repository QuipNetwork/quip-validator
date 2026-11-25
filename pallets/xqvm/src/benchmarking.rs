//! Benchmarking setup for pallet-xqvm.

use super::*;

#[allow(unused)]
use crate::Pallet as Xqvm;
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::traits::fungible::{Inspect, Mutate};
use frame_support::traits::Get as _;
use frame_support::BoundedVec;
use frame_system::RawOrigin;
use sp_runtime::traits::Hash as _;
use sp_runtime::traits::Saturating as _;
use xqvm::{InstructionBuilder, Program};

/// Byte length of the XQBC wire-format header emitted by `Program::encode`.
const XQBC_HEADER_LEN: usize = 15;

/// The `HALT` opcode byte.
const HALT_OPCODE: u8 = 0xFF;

/// Opcode bytes for the hand-assembled shape below, which is assembled
/// directly so its block count and byte length are exact.
const TARGET_OPCODE: u8 = 0x00;
/// `JUMP` with a two-byte big-endian label: the index of the `TARGET` it
/// names, counting from 0 in program order.
const JUMP2_OPCODE: u8 = 0x03;
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

/// Upper bound of the `execute` output-slot component (W1-008 part 2). A
/// constant because `Linear` needs one; the benchmark asserts it does not
/// exceed the configured `MaxOutputSlots`, which is where the bound lives.
const MAX_BENCH_OUTPUT_SLOTS: u32 = 256;

/// Bytes of one loop level in the shape: `PUSH1 0; PUSH1 1; RANGE` opening
/// it and `NEXT` closing it.
const LOOP_LEVEL_BYTES: usize = 6;

/// Body bytes `build_blocks_program` needs for `blocks` blocks: `n` loop
/// levels at `LOOP_LEVEL_BYTES` each, then `t = blocks - 2n` `TARGET`s
/// chained by `t - 1` three-byte `JUMP2`s (`6n + 4t - 3`), with `n` as deep as
/// `MAX_BENCH_LOOP_DEPTH` and the block count allow.
fn blocks_shape(blocks: usize) -> (usize, usize, usize) {
    if blocks == 1 {
        return (0, 0, 0);
    }
    let nesting = (MAX_BENCH_LOOP_DEPTH as usize).min((blocks - 1) / 2);
    let targets = blocks - 2 * nesting;
    (
        nesting,
        targets,
        LOOP_LEVEL_BYTES * nesting + 4 * targets - 3,
    )
}

/// Build a program of exactly `target_len` bytes with exactly `blocks` basic
/// blocks (as `ControlFlowShape` counts them), the shape the structural gate
/// does the most work for at that block count.
///
/// What `store_program` spends, since QUI-2516 replaced xqvm's full checker
/// with the structural gate (`crate::structural_gate`):
///
/// * `Program::decode`, the canonical re-encode and `ControlFlowShape` each
///   walk the bytes once: a cost per byte, whatever the opcodes are;
/// * `LoopNestingPhase` walks the instructions once with a stack of open
///   loops: per instruction;
/// * `JumpSpanPhase` checks every jump against the loop body it sits in, with
///   a binary search over the loop and target offsets: per jump, times the
///   logarithm of their count. That is the one term that is not linear in
///   bytes, and jumps are bounded by the block count (each needs a `TARGET`,
///   and each `TARGET` is a block leader).
///
/// The full checker's control-flow graph -- about 3 KiB and a microsecond per
/// block natively, and its re-walk of every loop region once per enclosing
/// loop -- is gone from `store_program`. mythos measured the gate natively
/// and saw no block-count effect over the byte cost (`audit-evidence/
/// 2026-11-25-mythos-checker-removal/`).
///
/// The shape: `n` nested `RANGE` loops, as deep as `MAX_BENCH_LOOP_DEPTH` and
/// the block count allow, around a chain `TARGET 0; JUMP2 1; TARGET 1;
/// JUMP2 2; ...; TARGET t-1` -- every block but the loop leaders carries a
/// jump, each a forward jump inside the innermost loop body (so the gate
/// admits it) that `JumpSpanPhase` must search for -- then `NOP` padding and
/// `HALT`. Each loop level is two leaders (after `RANGE`, after `NEXT`); a
/// `JUMP2` is followed by its own target, so it adds no leader; the first
/// `TARGET` shares the leader the last `RANGE` made. So `blocks = 2n + t`.
/// Under the full checker the shape was a run of unreferenced `TARGET`s, the
/// most CFG work per block; jumps are now what costs per block.
fn build_blocks_program(target_len: u32, blocks: u32) -> Vec<u8> {
    let len = target_len as usize;
    assert!(
        len > XQBC_HEADER_LEN,
        "minimum encoded program is header + HALT"
    );
    assert!(blocks >= 1, "every program has at least its entry block");
    let body = len - XQBC_HEADER_LEN - 1;
    let blocks = blocks as usize;
    let (nesting, targets, needed) = blocks_shape(blocks);
    assert!(
        needed <= body,
        "{blocks} blocks need {needed} body bytes; {body} available"
    );

    let mut code = Vec::with_capacity(body + 1);
    for _ in 0..nesting {
        code.extend_from_slice(&[PUSH1_OPCODE, 0, PUSH1_OPCODE, 1, RANGE_OPCODE]);
    }
    for label in 0..targets {
        code.push(TARGET_OPCODE);
        if label + 1 < targets {
            let next = u16::try_from(label + 1).expect("blocks <= MaxProgramBlocks fit a u16");
            code.push(JUMP2_OPCODE);
            code.extend_from_slice(&next.to_be_bytes());
        }
    }
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
    crate::structural_gate(&decoded)
        .expect("forward jumps inside the innermost loop body pass the gate");
    bytes
}

/// The most blocks `build_blocks_program` fits in a `target_len`-byte
/// program, capped at `blocks`.
fn fitting_blocks(target_len: u32, blocks: u32) -> u32 {
    let body = (target_len as usize).saturating_sub(XQBC_HEADER_LEN + 1);
    let mut b = blocks.max(1);
    while b > 1 && blocks_shape(b as usize).2 > body {
        b -= 1;
    }
    b
}

/// Build a valid XQVM program whose encoded byte length equals
/// `target_len`.
///
/// Layout: XQBC header + (target_len - XQBC_HEADER_LEN - 1) NOPs + HALT.
/// NOP and HALT are one byte each, so the encoded length is exact.
/// Minimum `target_len` is `XQBC_HEADER_LEN + 1` (header + HALT).
fn build_padded_program(target_len: u32) -> Vec<u8> {
    let len = target_len as usize;
    assert!(
        len > XQBC_HEADER_LEN,
        "minimum encoded program is header + HALT"
    );

    let mut b = InstructionBuilder::new();
    for _ in 0..(len - XQBC_HEADER_LEN - 1) {
        b.emit_nop();
    }
    b.emit_halt();
    let bytes = b.build().expect("NOP* HALT is a valid program").encode();

    assert_eq!(bytes.len(), len);
    assert!(
        Program::decode(&bytes).is_ok(),
        "program must round-trip through decode"
    );
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

    /// Fund `who` well past any deposit a benchmarked program can cost, so
    /// the hold in `store_program` is never what fails.
    fn fund<T: Config>(who: &T::AccountId) {
        let existential = <T::Currency as Inspect<T::AccountId>>::minimum_balance();
        let deposit = Pallet::<T>::deposit_for(T::MaxProgramSize::get());
        let target = existential
            .saturating_add(deposit)
            .saturating_mul(1_000u32.into());
        let _ = T::Currency::set_balance(who, target);
    }

    /// Cost of `store_program` in its two dimensions: `s` bytes to decode,
    /// re-encode and walk through the structural gate, and `b` basic blocks,
    /// almost every one carrying a jump the gate's jump-span phase searches
    /// for (`build_blocks_program`).
    ///
    /// FRAME varies one component with the others at their maximum, so the
    /// `s` series is 2,048 blocks at full nesting plus `NOP` padding (the
    /// per-byte slope) and the `b` series is 64 KiB with a growing block and
    /// jump count (the per-block slope). Below 8,141 B the maximum block
    /// count does not fit and is clamped to what does (`fitting_blocks`);
    /// that bends the first few of fifty `s` points and nothing else.
    ///
    /// Whether `b` stays a priced, refunded component: it is kept, because
    /// jumps -- the gate's one term that is not linear in bytes -- are bounded
    /// by it. mythos saw no block-count effect natively; if the reference
    /// machine also fits a negligible per-block slope, `b` can be dropped in a
    /// later change, which would also drop the `MaxProgramBlocks` pre-charge
    /// and its refund.
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
        fund::<T>(&caller);
        let bytecode = build_blocks_program(s, fitting_blocks(s, b));
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
    /// bytes. Decoding rebuilds the jump table through `checker::scan`,
    /// which is linear in instruction count; NOPs are one byte each, so this
    /// is the worst case per byte.
    #[benchmark]
    fn execute(s: Linear<16, { 65_536 }>, o: Linear<0, { MAX_BENCH_OUTPUT_SLOTS }>) {
        assert!(
            MAX_BENCH_OUTPUT_SLOTS <= T::MaxOutputSlots::get(),
            "benchmark output-slot range must stay within MaxOutputSlots"
        );
        let caller: T::AccountId = whitelisted_caller();

        let bytecode = build_halt_first_program(s);
        let hash = T::Hashing::hash(&bytecode);
        let bounded: BoundedVec<u8, T::MaxProgramSize> =
            bytecode.try_into().expect("s <= MaxProgramSize");
        crate::Programs::<T>::insert(&hash, bounded);

        let calldata: BoundedVec<i64, T::MaxCallDataLen> = BoundedVec::default();

        // `o` declared output slots are pre-charged and not refunded, so the
        // benchmark declares them on the call. The HALT-first program writes
        // none; the cost measured is sizing and returning the output vector.
        #[extrinsic_call]
        execute(RawOrigin::Signed(caller), hash, calldata, o, 1u64);
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

    /// Cost of `remove_program` as a function of stored program length.
    ///
    /// The program is stored through the extrinsic rather than inserted
    /// directly, so the deposit exists and the release path is measured
    /// rather than skipped. A `NOP` sled is the right shape here: removal
    /// reads the bytes to learn their length and neither decodes nor
    /// verifies them, so cost is driven by length alone.
    #[benchmark]
    fn remove_program(s: Linear<16, { 65_536 }>) {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller);

        let bytecode = build_padded_program(s);
        let hash = T::Hashing::hash(&bytecode);
        let bounded: BoundedVec<u8, T::MaxProgramSize> =
            bytecode.try_into().expect("s <= MaxProgramSize");

        Xqvm::<T>::store_program(RawOrigin::Signed(caller.clone()).into(), bounded)
            .expect("program stores");

        #[extrinsic_call]
        remove_program(RawOrigin::Signed(caller), hash);
    }

    /// Cost of `evict_program`, the root-origin twin of `remove_program`.
    ///
    /// Benchmarked separately rather than assumed equal: it skips the owner
    /// check and takes a different origin, and a weight that is asserted
    /// rather than measured is the mistake QUI-1054 was about.
    #[benchmark]
    fn evict_program(s: Linear<16, { 65_536 }>) {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller);

        let bytecode = build_padded_program(s);
        let hash = T::Hashing::hash(&bytecode);
        let bounded: BoundedVec<u8, T::MaxProgramSize> =
            bytecode.try_into().expect("s <= MaxProgramSize");

        Xqvm::<T>::store_program(RawOrigin::Signed(caller).into(), bounded)
            .expect("program stores");

        #[extrinsic_call]
        evict_program(RawOrigin::Root, hash);
    }

    impl_benchmark_test_suite!(Xqvm, crate::mock::new_test_ext(), crate::mock::Test);
}

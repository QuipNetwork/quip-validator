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

/// Build a valid XQVM program of exactly `target_len` bytes that is the worst
/// case for `store_program`.
///
/// `store_program` decodes and then statically verifies. Those two have
/// *different* worst-case shapes, and the expensive one wins:
///
/// * `Program::decode` walks the instruction stream once, so its cost is
///   driven by instruction count. A `NOP` sled maximises that -- one byte per
///   instruction -- at roughly 2.4 ns/byte natively.
/// * `verifier::verify` additionally builds a control-flow graph and runs
///   three worklist analyses over it, so its cost is driven by basic-block
///   and edge count. Measured natively, a `NOP` sled verifies at ~21 ns/byte
///   because it is a single basic block; a program densely packed with blocks
///   verifies at ~175 ns/byte. Eight times worse, and it dominates decode.
///
/// So the program is `N` blocks of `TARGET; PUSH 1; JUMPI .next`, cyclic, then
/// `HALT`, then `NOP` padding to land on an exact length. At `MaxProgramSize`
/// that is about 10,960 blocks.
///
/// Getting this backwards -- benchmarking the `NOP` sled while the extrinsic
/// charges for every shape -- is how `store_program` came to be underpriced
/// tenfold before QUI-1054.
///
/// Block widths are deterministic because `InstructionBuilder::build` narrows
/// `JUMPI2` to `JUMPI1` when the sequential target id fits in a `u8`: five
/// bytes per block for the first 256 blocks, six thereafter.
fn build_verifier_worst_case_program(target_len: u32) -> Vec<u8> {
    /// Bytes per block while the target id fits in a `u8`.
    const NARROW_BLOCK: usize = 5;
    /// Bytes per block once the target id needs a `u16`.
    const WIDE_BLOCK: usize = 6;
    /// Number of blocks addressable by a narrow jump.
    const NARROW_BLOCKS: usize = 256;

    let len = target_len as usize;
    assert!(
        len > XQBC_HEADER_LEN,
        "minimum encoded program is header + HALT"
    );

    // Budget after the header and the trailing HALT.
    let available = len - XQBC_HEADER_LEN - 1;
    let narrow_budget = NARROW_BLOCKS * NARROW_BLOCK;

    let (blocks, pad) = if available <= narrow_budget {
        (available / NARROW_BLOCK, available % NARROW_BLOCK)
    } else {
        let wide = available - narrow_budget;
        (NARROW_BLOCKS + wide / WIDE_BLOCK, wide % WIDE_BLOCK)
    };

    // Below one full block there is nothing to make dense; fall back to the
    // straight-line shape so the low end of the component range still builds.
    if blocks == 0 {
        return build_padded_program(target_len);
    }

    let mut b = InstructionBuilder::new();
    let labels: Vec<_> = (0..blocks).map(|_| b.label()).collect();
    for i in 0..blocks {
        b.place(labels[i]).expect("label placed once");
        b.emit_push(1);
        // Cyclic, so every label is used and every block has two predecessors.
        b.emit_jump_if(labels[(i + 1) % blocks]);
    }
    b.emit_halt();
    for _ in 0..pad {
        b.emit_nop();
    }
    let bytes = b.build().expect("dense CFG is a valid program").encode();

    debug_assert_eq!(bytes.len(), len);
    debug_assert!(Program::decode(&bytes).is_ok());
    bytes
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

    debug_assert_eq!(bytes.len(), len);
    // Sanity: must round-trip through Program::decode.
    debug_assert!(Program::decode(&bytes).is_ok());
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

    debug_assert_eq!(bytes.len(), len);
    debug_assert!(Program::decode(&bytes).is_ok());
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

    debug_assert_eq!(bytes.len(), LOOP_PROGRAM_LEN);
    debug_assert!(Program::decode(&bytes).is_ok());
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

    #[benchmark]
    fn store_program(s: Linear<16, { 65_536 }>) {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller);
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

    /// Cost of `remove_program` as a function of stored program length.
    ///
    /// The program is stored through the extrinsic rather than inserted
    /// directly, so the deposit exists and the release path is measured
    /// rather than skipped. A `NOP` sled is the right shape here: removal
    /// reads and decodes the bytes but runs no verifier, so cost is driven
    /// by length, not by block structure.
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

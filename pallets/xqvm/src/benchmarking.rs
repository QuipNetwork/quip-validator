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

    #[benchmark]
    fn store_program(s: Linear<16, { 65_536 }>) {
        let caller: T::AccountId = whitelisted_caller();
        let bytecode = build_padded_program(s);
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
    /// `NEXT` is the most expensive of the cheaply-constructible opcodes
    /// (measured 6.87 ns/step native, against 3.89 for `NOP`), so it is the
    /// right floor to calibrate against. It is only a floor: opcodes whose
    /// cost scales with their operands -- `ENERGY` over a model above all --
    /// are one step each and unbounded. See QUI-1056.
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

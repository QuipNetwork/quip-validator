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

    #[benchmark]
    fn execute_base() {
        let caller: T::AccountId = whitelisted_caller();

        // Store a minimal HALT program.
        let bytecode = build_padded_program(16);
        let hash = T::Hashing::hash(&bytecode);
        let bounded: BoundedVec<u8, T::MaxProgramSize> =
            bytecode.try_into().expect("16 <= MaxProgramSize");
        crate::Programs::<T>::insert(&hash, bounded);

        let calldata: BoundedVec<i64, T::MaxCallDataLen> = BoundedVec::default();

        #[extrinsic_call]
        execute(RawOrigin::Signed(caller), hash, calldata, 0u32, 1u64);
    }

    impl_benchmark_test_suite!(Xqvm, crate::mock::new_test_ext(), crate::mock::Test);
}

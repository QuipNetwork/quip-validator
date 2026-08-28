use crate::{mock::*, Error, Event, ProgramOwner, Programs};
use frame_support::{assert_noop, assert_ok, BoundedVec};
use sp_runtime::{traits::Hash, DispatchError};
use xqvm::{InstructionBuilder, Register};

/// Encode a program built with `InstructionBuilder` into raw bytes.
fn build_program(f: impl FnOnce(&mut InstructionBuilder)) -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    f(&mut b);
    b.build().unwrap().encode()
}

fn bounded(bytes: Vec<u8>) -> BoundedVec<u8, MaxProgramSize> {
    bytes.try_into().expect("test program fits MaxProgramSize")
}

fn program_hash(bytes: &[u8]) -> <Test as frame_system::Config>::Hash {
    <<Test as frame_system::Config>::Hashing as Hash>::hash(bytes)
}

// ── store_program ────────────────────────────────────────────────────────

#[test]
fn store_program_works() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);
        let len = bytecode.len() as u32;

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert!(Programs::<Test>::contains_key(&hash));
        assert_eq!(ProgramOwner::<Test>::get(&hash), Some(1));
        System::assert_last_event(
            Event::ProgramStored {
                program_hash: hash,
                owner: 1,
                size: len,
            }
            .into(),
        );
    });
}

#[test]
fn store_duplicate_fails() {
    new_test_ext().execute_with(|| {
        let bytecode = build_program(|b| {
            b.emit_halt();
        });

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode.clone()),
        ));
        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(2), bounded(bytecode)),
            Error::<Test>::ProgramAlreadyExists
        );
    });
}

#[test]
fn store_invalid_bytecode_fails() {
    new_test_ext().execute_with(|| {
        let garbage: Vec<u8> = vec![0xFF, 0xFE, 0xFD];
        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(garbage)),
            Error::<Test>::InvalidBytecode
        );
    });
}

// ── execute ──────────────────────────────────────────────────────────────

#[test]
fn execute_addition() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // Program: PUSH 3, PUSH 4, ADD, STOW r0, PUSH 0, OUTPUT r0, HALT
        // This stores 7 into r0, then writes r0 to output slot 0.
        let bytecode = build_program(|b| {
            b.emit_push(3)
                .emit_push(4)
                .emit_add()
                .emit_stow(Register(0))
                .emit_push(0)
                .emit_output(Register(0))
                .emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_ok!(Xqvm::execute(
            RuntimeOrigin::signed(1),
            hash,
            BoundedVec::default(),
            1, // 1 output slot
            1_000,
        ));

        System::assert_last_event(
            Event::ProgramExecuted {
                caller: 1,
                program_hash: hash,
                steps_used: 7,
                outputs: vec![7i64].try_into().unwrap(),
            }
            .into(),
        );
    });
}

#[test]
fn execute_with_calldata() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // Program: INPUT r0 (from calldata[0]), LOAD r0, PUSH 10, MUL,
        //          STOW r1, PUSH 0, OUTPUT r1, HALT
        let bytecode = build_program(|b| {
            b.emit_push(0)
                .emit_input(Register(0))
                .emit_load(Register(0))
                .emit_push(10)
                .emit_mul()
                .emit_stow(Register(1))
                .emit_push(0)
                .emit_output(Register(1))
                .emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        let calldata: BoundedVec<i64, MaxCallDataLen> = vec![5i64].try_into().unwrap();

        assert_ok!(Xqvm::execute(
            RuntimeOrigin::signed(1),
            hash,
            calldata,
            1,
            1_000,
        ));

        // 5 * 10 = 50
        System::assert_last_event(
            Event::ProgramExecuted {
                caller: 1,
                program_hash: hash,
                steps_used: 9,
                outputs: vec![50i64].try_into().unwrap(),
            }
            .into(),
        );
    });
}

#[test]
fn execute_step_limit_exceeded() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // Infinite loop: label -> PUSH 1 -> JUMPI label
        let bytecode = build_program(|b| {
            let top = b.label();
            b.place(top).unwrap().emit_push(1).emit_jump_if(top);
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                0,
                10, // very low step limit
            ),
            Error::<Test>::VmStepLimitExceeded
        );
    });
}

#[test]
fn execute_division_by_zero() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let bytecode = build_program(|b| {
            b.emit_push(1).emit_push(0).emit_div().emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                0,
                1_000,
            ),
            Error::<Test>::VmDivisionByZero
        );
    });
}

#[test]
fn execute_program_not_found() {
    new_test_ext().execute_with(|| {
        let fake_hash = program_hash(b"nonexistent");

        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                fake_hash,
                BoundedVec::default(),
                0,
                1_000,
            ),
            Error::<Test>::ProgramNotFound
        );
    });
}

#[test]
fn execute_step_limit_too_high() {
    new_test_ext().execute_with(|| {
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        // MaxStepLimit in mock is 100_000
        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                0,
                100_001,
            ),
            Error::<Test>::StepLimitTooHigh
        );
    });
}

#[test]
fn execute_too_many_output_slots() {
    new_test_ext().execute_with(|| {
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        // MaxOutputSlots in mock is 32
        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                33,
                1_000,
            ),
            Error::<Test>::TooManyOutputSlots
        );
    });
}

// ── static verification at store time ────────────────────────────────────

#[test]
fn store_rejects_stack_underflow() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // ADD with nothing on the stack. The XQBC container is perfectly well
        // formed -- correct magic, version, length and CRC -- so this was
        // stored happily before the verifier ran, and only faulted when
        // somebody executed it.
        let bytecode = build_program(|b| {
            b.emit_add().emit_halt();
        });

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierStackFault
        );
    });
}

#[test]
fn store_rejects_read_of_unset_register() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let bytecode = build_program(|b| {
            b.emit_load(Register(3)).emit_halt();
        });

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierReadUnsetRegister
        );
    });
}

#[test]
fn store_accepts_a_verifiable_program() {
    // The counterpart to the rejection tests: verification must not have
    // become so strict that ordinary programs stop being storable.
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let bytecode = build_program(|b| {
            b.emit_push(3).emit_push(4).emit_add().emit_halt();
        });

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));
    });
}

#[test]
fn store_still_rejects_a_corrupt_container() {
    // Container faults keep their own error, distinct from stream faults.
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let mut bytecode = build_program(|b| {
            b.emit_halt();
        });
        // Corrupt the CRC-32 in the header (bytes 11..15).
        bytecode[11] ^= 0xFF;

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::InvalidBytecode
        );
    });
}

// ── weight accounting ────────────────────────────────────────────────────

#[test]
fn execute_rejects_zero_step_limit() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // A program that never halts on its own. Under xqvm 0.3.x,
        // `set_step_limit(0)` meant u64::MAX, so without the guard in
        // `execute` this call ran forever -- precisely the on-chain failure
        // mode being guarded against. xqvm 0.4.0 dropped that sentinel, but
        // the guard (and this test) stay: zero-step executions are rejected
        // up front rather than charged and run.
        let bytecode = build_program(|b| {
            let top = b.label();
            b.place(top).unwrap().emit_nop().emit_jump(top);
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_noop!(
            Xqvm::execute(RuntimeOrigin::signed(1), hash, BoundedVec::default(), 0, 0),
            Error::<Test>::ZeroStepLimit
        );
    });
}

#[test]
fn execute_refunds_unused_size_and_steps() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let len = bytecode.len() as u32;
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        let step_limit = 10_000u64;
        let post = Xqvm::execute(
            RuntimeOrigin::signed(1),
            hash,
            BoundedVec::default(),
            0,
            step_limit,
        )
        .expect("HALT executes");

        let actual = post.actual_weight.expect("execute reports actual weight");

        // Refunded to what was actually done: this program's real length and
        // the one step it took.
        let expected = <() as crate::WeightInfo>::execute(len)
            .saturating_add(TestWeightPerStep::get().saturating_mul(1));
        assert_eq!(actual, expected);

        // And that is strictly less than what was pre-charged, on both axes.
        let pre_charged = <() as crate::WeightInfo>::execute(MaxProgramSize::get())
            .saturating_add(TestWeightPerStep::get().saturating_mul(step_limit));
        assert!(
            actual.ref_time() < pre_charged.ref_time(),
            "actual {} should be below pre-charged {}",
            actual.ref_time(),
            pre_charged.ref_time(),
        );
    });
}

#[test]
fn execute_weight_grows_with_program_length() {
    // The whole point of the size component: a longer program costs more to
    // decode, because decoding rebuilds the jump table by walking every
    // instruction.
    let short = <() as crate::WeightInfo>::execute(16);
    let long = <() as crate::WeightInfo>::execute(65_536);
    assert!(
        long.ref_time() > short.ref_time(),
        "execute({}) should exceed execute(16)",
        65_536,
    );
}

// ── allocation budget (QUI-1012) ─────────────────────────────────────────

/// Build `PUSH size; BSMX r0; HALT` -- a program whose only work is to
/// allocate a binary sample of `size` variables.
///
/// `BSMX` charges the allocation budget `size * 8` bytes (one `i64` per
/// variable) before it allocates, so this is the smallest program that can
/// be pushed against the budget from either side.
fn build_allocating_program(size: i64) -> Vec<u8> {
    build_program(|b| {
        b.emit_push(size).emit_bsmx(Register(0)).emit_halt();
    })
}

#[test]
fn execute_rejects_an_allocation_above_the_budget() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // 256 variables is 2048 bytes against the mock's 1024-byte budget.
        let bytecode = build_allocating_program(256);
        let hash = program_hash(&bytecode);
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        // The step limit is deliberately generous: the budget must be what
        // stops this, not the step bound.
        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                0,
                10_000,
            ),
            Error::<Test>::VmMemoryLimitExceeded
        );
    });
}

#[test]
fn execute_allows_an_allocation_within_the_budget() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // 64 variables is 512 bytes, inside the mock's 1024-byte budget.
        let bytecode = build_allocating_program(64);
        let hash = program_hash(&bytecode);
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_ok!(Xqvm::execute(
            RuntimeOrigin::signed(1),
            hash,
            BoundedVec::default(),
            0,
            10_000,
        ));
    });
}

#[test]
fn the_budget_binds_independently_of_the_step_limit() {
    // Regression guard for the shape of the hole QUI-1012 closed: before the
    // budget was set, allocation was bounded only incidentally, by how many
    // steps the allocation happened to cost. A program that allocates far
    // more than the budget while costing far fewer steps than the limit must
    // still be refused -- that gap is exactly what a 1 GiB default left open.
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        let bytecode = build_allocating_program(1_000);
        let hash = program_hash(&bytecode);
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        // 1_000 variables costs ~1_000 steps, comfortably inside MaxStepLimit,
        // but 8_000 bytes is eight times the budget.
        assert!(1_000 < MaxStepLimit::get());
        assert!(1_000 * 8 > MaxVmMemory::get());

        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                0,
                MaxStepLimit::get(),
            ),
            Error::<Test>::VmMemoryLimitExceeded
        );
    });
}

// ── VM fault mapping (QUI-1014) ──────────────────────────────────────────

/// Store `bytecode`, execute it, and return the dispatch error it failed
/// with. Panics if the program stores badly or unexpectedly succeeds.
fn execute_expecting_failure(bytecode: Vec<u8>, calldata: Vec<i64>, slots: u32) -> DispatchError {
    let hash = program_hash(&bytecode);
    assert_ok!(Xqvm::store_program(
        RuntimeOrigin::signed(1),
        bounded(bytecode),
    ));

    let calldata: BoundedVec<i64, MaxCallDataLen> =
        calldata.try_into().expect("calldata fits MaxCallDataLen");

    Xqvm::execute(RuntimeOrigin::signed(1), hash, calldata, slots, 10_000)
        .expect_err("program was expected to fault")
        .error
}

#[test]
fn arithmetic_overflow_maps_to_its_own_error() {
    // The headline behaviour change in xqvm 0.4.0: i64::MAX + 1 raises where
    // it used to wrap. Under the old wildcard this was indistinguishable
    // from any other runtime fault.
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_push(i64::MAX).emit_push(1).emit_add().emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmArithmeticOverflow.into()
        );
    });
}

#[test]
fn calldata_index_out_of_range_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // INPUT pops a calldata slot index; nothing was supplied.
        let bytecode = build_program(|b| {
            b.emit_push(0).emit_input(Register(0)).emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmCallDataIndex.into()
        );
    });
}

#[test]
fn output_index_out_of_range_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // OUTPUT addresses slot 0 while the call requested none.
        let bytecode = build_program(|b| {
            b.emit_push(7)
                .emit_stow(Register(0))
                .emit_push(0)
                .emit_output(Register(0))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmOutputIndex.into()
        );
    });
}

#[test]
fn oversized_shift_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // SHL pops b then a; a shift of 64 discards every significant bit.
        let bytecode = build_program(|b| {
            b.emit_push(1).emit_push(64).emit_shl().emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmInvalidShift.into()
        );
    });
}

#[test]
fn discrete_sample_with_small_k_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // XSMX pops k then size; the discrete domain requires k >= 2.
        let bytecode = build_program(|b| {
            b.emit_push(4)
                .emit_push(1)
                .emit_xsmx(Register(0))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmInvalidDiscreteK.into()
        );
    });
}

#[test]
fn negative_allocation_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // A negative size is not an allocation at all.
        let bytecode = build_program(|b| {
            b.emit_push(-1).emit_bsmx(Register(0)).emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmInvalidAllocation.into()
        );
    });
}

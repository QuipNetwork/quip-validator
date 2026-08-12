use crate::{mock::*, Error, Event, ProgramOwner, Programs};
use frame_support::{assert_noop, assert_ok, BoundedVec};
use sp_runtime::traits::Hash;
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

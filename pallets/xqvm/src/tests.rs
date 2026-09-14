use crate::{mock::*, Error, Event, ProgramOwner, Programs};
use frame_support::{assert_noop, assert_ok, BoundedVec};
use sp_runtime::traits::Hash;
use xqvm::{InstructionBuilder, Program, Register};

/// Encode a program built with `InstructionBuilder` into raw bytes.
fn build_program(f: impl FnOnce(&mut InstructionBuilder)) -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    f(&mut b);
    b.build().unwrap().encode()
}

/// Wrap a hand-assembled instruction stream in a well-formed XQBC
/// container. For faults the builder refuses to emit but the verifier
/// must still catch.
fn raw_program(code: Vec<u8>) -> Vec<u8> {
    Program::new(code).encode()
}

/// Opcode bytes for the hand-assembled streams above.
const TARGET: u8 = 0x00;
const JUMP1: u8 = 0x01;
const HALT: u8 = 0xFF;

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
fn store_rejects_an_unknown_opcode() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // 0x0D is the reserved gap in the opcode table. `Program::decode`
        // tolerates it -- the container is well formed and the scan that
        // rebuilds the jump table only records the fault -- so this reaches
        // the verifier rather than `InvalidBytecode`.
        let bytecode = raw_program(vec![0x0D, HALT]);

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierBadInstruction
        );
    });
}

#[test]
fn store_rejects_a_jump_to_a_missing_target() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // JUMP1 to label 3 in a stream with no TARGET at all. The builder
        // refuses to assemble this, which is the point: the wire format can
        // still carry it, so the verifier has to be the one to say no.
        let bytecode = raw_program(vec![JUMP1, 0x03, HALT]);

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierUndefinedJumpTarget
        );
    });
}

#[test]
fn store_rejects_an_unclosed_loop() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // RANGE opens a loop that no NEXT ever closes.
        let bytecode = build_program(|b| {
            b.emit_push(0).emit_push(4).emit_range().emit_halt();
        });

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierLoopImbalance
        );
    });
}

#[test]
fn store_rejects_a_register_read_at_the_wrong_type() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // STOW writes r0 as an int; LOAD of a model register would be fine,
        // but ENERGY wants a model in r0 and reads it as one.
        let bytecode = build_program(|b| {
            b.emit_push(1)
                .emit_stow(Register(0))
                .emit_push(2)
                .emit_bsmx(Register(1))
                .emit_energy(Register(0), Register(1))
                .emit_halt();
        });

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierRegisterTypeMismatch
        );
    });
}

// ── basic-block bound ────────────────────────────────────────────────────

/// A program of exactly `blocks` basic blocks: that many `TARGET`s, then
/// `HALT`. The builder refuses unreferenced labels, so this is assembled
/// by hand; the verifier accepts it.
fn target_sled(blocks: u32) -> Vec<u8> {
    let mut code = vec![TARGET; blocks as usize];
    code.push(HALT);
    raw_program(code)
}

#[test]
fn store_accepts_a_program_at_the_block_bound() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = target_sled(MaxProgramBlocks::get());
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));
        assert!(Programs::<Test>::contains_key(&hash));
    });
}

#[test]
fn store_rejects_a_program_over_the_block_bound() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // One TARGET past the bound. Every one of them is a valid, empty
        // basic block, so nothing else about the program is wrong: only the
        // count is, and it is what bounds the verifier's memory.
        let bytecode = target_sled(MaxProgramBlocks::get() + 1);
        let hash = program_hash(&bytecode);

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::TooManyBlocks
        );
        assert!(!Programs::<Test>::contains_key(&hash));
    });
}

#[test]
fn store_refunds_unused_blocks() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // Straight-line program: no TARGET, so zero basic blocks to verify
        // against a pre-charge at MaxProgramBlocks.
        let bytecode = build_program(|b| {
            b.emit_push(1).emit_push(2).emit_add().emit_halt();
        });
        let len = bytecode.len() as u32;

        let post = Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode))
            .expect("program stores");
        let actual = post.actual_weight.expect("store reports actual weight");

        assert_eq!(actual, <() as crate::WeightInfo>::store_program(len, 0));

        let pre_charged = <() as crate::WeightInfo>::store_program(len, MaxProgramBlocks::get());
        assert!(
            actual.ref_time() < pre_charged.ref_time(),
            "actual {} should be below pre-charged {}",
            actual.ref_time(),
            pre_charged.ref_time(),
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

use crate::{mock::*, ControlFlowShape, Error, Event, ProgramOwner, Programs};
use frame_support::{assert_noop, assert_ok, BoundedVec};
use sp_runtime::{traits::Hash, DispatchError};
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
const NEXT: u8 = 0x07;
const RANGE: u8 = 0x08;
const PUSH1: u8 = 0x11;
const NOP: u8 = 0xF0;
const HALT: u8 = 0xFF;

/// Byte length of the XQBC header `Program::encode` emits.
const XQBC_HEADER_LEN: usize = 15;

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
                deposit: Xqvm::deposit_for(len),
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

// W1-003a: the stored bytes must be the program's own canonical encoding.
// `Program::decode` validates the CRC and length but ignores the header's
// slot bytes (5 = input_slots, 6 = output_slots) -- it recomputes them -- so a
// program whose header slot bytes are tampered still decodes and verifies, and
// on the base (no canonical check) would be stored under a different hash.
#[test]
fn store_rejects_non_canonical_header_slot_bytes() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let canonical = build_program(|b| {
            b.emit_halt();
        });
        assert_eq!(canonical.len(), XQBC_HEADER_LEN + 1);
        // Control: the canonical form stores.
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(canonical.clone())
        ));

        for index in [5usize, 6] {
            let mut tampered = canonical.clone();
            tampered[index] ^= 0xFF;
            assert_ne!(tampered, canonical);
            // Still decodes to the same program: it is the canonical check,
            // not decode, that rejects it (fail-on-base + mutation control --
            // remove the check and this store succeeds).
            assert!(Program::decode(&tampered).is_ok());
            assert_noop!(
                Xqvm::store_program(RuntimeOrigin::signed(2), bounded(tampered)),
                Error::<Test>::NonCanonicalProgram
            );
        }
    });
}

// A program whose header slot bytes are the canonical (recomputed) values --
// here non-zero, from a real INPUT/OUTPUT -- is accepted: the check admits the
// canonical form, it does not merely reject non-zero slot bytes.
#[test]
fn store_accepts_canonical_non_zero_slot_bytes() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let canonical = build_program(|b| {
            b.emit_push(0)
                .emit_input(Register(0))
                .emit_push(0)
                .emit_output(Register(0))
                .emit_halt();
        });
        // The canonical header carries the recomputed non-zero slot counts.
        assert_eq!(canonical[5], 1, "input_slots");
        assert_eq!(canonical[6], 1, "output_slots");
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(canonical)
        ));
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

        // One past the mock's MaxOutputSlots.
        assert_noop!(
            Xqvm::execute(
                RuntimeOrigin::signed(1),
                hash,
                BoundedVec::default(),
                MaxOutputSlots::get() + 1,
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
fn store_rejects_a_jump_across_a_loop_boundary() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // W1-011: a back-edge from inside a RANGE body to a TARGET before the
        // opener -- the shape that re-entered a loop without its NEXT and
        // piled up a loop frame per pass. Stack-neutral, so only the
        // jump-span phase of xqvm 0.6.0 rejects it.
        //   TARGET; PUSH1 0; PUSH1 1; RANGE; PUSH1 1; JUMPI1 0; NEXT; HALT
        let bytecode = raw_program(vec![
            0x00, 0x11, 0x00, 0x11, 0x01, 0x08, 0x11, 0x01, 0x02, 0x00, 0x07, HALT,
        ]);

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::VerifierJumpAcrossLoopBoundary
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
fn store_rejects_blocks_made_by_jumps_not_targets() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // One TARGET and MaxProgramBlocks jumps to it. The jump table has
        // a single entry; the verifier's CFG has a block after every jump.
        // Counting TARGETs alone let this through at "one block".
        let mut code = vec![TARGET];
        for _ in 0..MaxProgramBlocks::get() {
            code.extend([JUMP1, 0x00]);
        }
        code.push(HALT);
        let bytecode = raw_program(code);
        assert_eq!(
            ControlFlowShape::of(&bytecode[XQBC_HEADER_LEN..]).blocks,
            MaxProgramBlocks::get() + 1
        );

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::TooManyBlocks
        );
    });
}

/// `depth` nested `RANGE` loops around a `TARGET`, each opened with the
/// two pushes it needs and closed with `NEXT`.
fn nested_loops(depth: u32) -> Vec<u8> {
    let mut code = Vec::new();
    for _ in 0..depth {
        code.extend([PUSH1, 0, PUSH1, 1, RANGE]);
    }
    code.push(TARGET);
    code.extend(vec![NEXT; depth as usize]);
    code.push(HALT);
    raw_program(code)
}

#[test]
fn store_accepts_loops_nested_to_the_depth_bound() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = nested_loops(MaxLoopDepth::get());
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));
    });
}

#[test]
fn store_rejects_loops_nested_past_the_depth_bound() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // Well under the block bound: the admission policy limits nesting
        // independently. The pinned verifier no longer walks each loop body,
        // but this dependency update does not relax the runtime's depth cap.
        let bytecode = nested_loops(MaxLoopDepth::get() + 1);
        let hash = program_hash(&bytecode);

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::LoopNestingTooDeep
        );
        assert!(!Programs::<Test>::contains_key(&hash));
    });
}

#[test]
fn bare_loop_sled_is_refused_before_verification() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // RANGE with nothing to pop, repeated, then the matching NEXTs: the
        // verifier would reject it for stack underflow, but only after
        // walking every region -- 255 ms natively at this size. The shape
        // bounds refuse it first: 4,097 blocks, 2,048 deep.
        let mut code = vec![RANGE; 2_048];
        code.extend(vec![NEXT; 2_048]);
        code.push(HALT);
        let bytecode = raw_program(code);

        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
            Error::<Test>::TooManyBlocks
        );
    });
}

#[test]
fn control_flow_shape_counts_leaders_like_the_verifier() {
    // Entry only.
    assert_eq!(
        ControlFlowShape::of(&[HALT]),
        ControlFlowShape {
            blocks: 1,
            loop_depth: 0
        }
    );
    // A TARGET at the entry shares the entry's leader.
    assert_eq!(ControlFlowShape::of(&[TARGET, HALT]).blocks, 1);
    // A TARGET right after a jump is the same leader the jump made.
    assert_eq!(
        ControlFlowShape::of(&[TARGET, JUMP1, 0, TARGET, HALT]).blocks,
        2
    );
    // A jump into padding still opens a block after it.
    assert_eq!(
        ControlFlowShape::of(&[TARGET, JUMP1, 0, NOP, TARGET, HALT]).blocks,
        3
    );
    // Loops: a leader after the opener and after NEXT; depth is the
    // deepest static nesting, and an unmatched NEXT does not go negative.
    assert_eq!(
        ControlFlowShape::of(&[PUSH1, 0, PUSH1, 1, RANGE, NEXT, HALT]),
        ControlFlowShape {
            blocks: 3,
            loop_depth: 1
        }
    );
    assert_eq!(
        ControlFlowShape::of(&[RANGE, RANGE, NEXT, NEXT, HALT]).loop_depth,
        2
    );
    assert_eq!(ControlFlowShape::of(&[NEXT, NEXT, HALT]).loop_depth, 0);
}

#[test]
fn store_refunds_unused_blocks() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // Straight-line program: one basic block -- the entry -- to verify
        // against a pre-charge at MaxProgramBlocks.
        let bytecode = build_program(|b| {
            b.emit_push(1).emit_push(2).emit_add().emit_halt();
        });
        let len = bytecode.len() as u32;

        let post = Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode))
            .expect("program stores");
        let actual = post.actual_weight.expect("store reports actual weight");

        assert_eq!(actual, <() as crate::WeightInfo>::store_program(len, 1));

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
        let expected = <() as crate::WeightInfo>::execute(len, 0)
            .saturating_add(TestWeightPerStep::get().saturating_mul(1));
        assert_eq!(actual, expected);

        // And that is strictly less than what was pre-charged, on both axes.
        let pre_charged = <() as crate::WeightInfo>::execute(MaxProgramSize::get(), 0)
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
    let short = <() as crate::WeightInfo>::execute(16, 0);
    let long = <() as crate::WeightInfo>::execute(65_536, 0);
    assert!(
        long.ref_time() > short.ref_time(),
        "execute({}) should exceed execute(16)",
        65_536,
    );
}

/// W1-008 part 2 plumbing: the declared `output_slots` must reach both the
/// pre-charged weight and the actual weight, and the output-slot term must be
/// charged identically on both sides (pre-charged, not refunded).
///
/// The mock `WeightInfo` (`TestXqvmWeights`) carries a non-zero output-slot
/// slope, so a correctly wired dispatch shows `output_slots` in both places.
/// This is also the mutation control: dropping `output_slots` from the
/// pre-charge collapses the `get_dispatch_info` delta; dropping it from the
/// actual weight collapses the `actual_weight` delta; charging it at
/// `MaxOutputSlots` on one side and the declared count on the other makes the
/// two deltas disagree. Each defeats a distinct assertion below.
#[test]
fn execute_charges_declared_output_slots_in_precharge_and_actual() {
    use frame_support::dispatch::GetDispatchInfo;

    new_test_ext().execute_with(|| {
        System::set_block_number(1);

        // A program that halts immediately: no work scales with output_slots,
        // so any weight difference between the two runs is the output-slot term
        // alone.
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        let n = MaxOutputSlots::get();
        assert!(n > 0, "test needs a non-zero MaxOutputSlots to vary");
        let step_limit = 10_000u64;

        let make_call = |slots: u32| crate::Call::<Test>::execute {
            program_hash: hash,
            calldata: BoundedVec::default(),
            output_slots: slots,
            step_limit,
        };

        // Pre-charge: declared weight must rise with declared output_slots.
        let pre0 = make_call(0).get_dispatch_info().call_weight;
        let pre_n = make_call(n).get_dispatch_info().call_weight;
        assert!(
            pre_n.ref_time() > pre0.ref_time(),
            "pre-charged weight must include the declared output_slots",
        );

        // Actual: run both; the executed work is identical (immediate HALT),
        // so the actual-weight difference is the kept output-slot term.
        let act0 = Xqvm::execute(
            RuntimeOrigin::signed(1),
            hash,
            BoundedVec::default(),
            0,
            step_limit,
        )
        .expect("HALT executes")
        .actual_weight
        .expect("execute reports actual weight");
        let act_n = Xqvm::execute(
            RuntimeOrigin::signed(1),
            hash,
            BoundedVec::default(),
            n,
            step_limit,
        )
        .expect("HALT executes")
        .actual_weight
        .expect("execute reports actual weight");
        assert!(
            act_n.ref_time() > act0.ref_time(),
            "actual weight must include the declared output_slots and not refund it",
        );

        // Not refunded: the output-slot term is the same pre-charged and actual.
        let pre_delta = pre_n.ref_time() - pre0.ref_time();
        let act_delta = act_n.ref_time() - act0.ref_time();
        assert_eq!(
            pre_delta, act_delta,
            "output-slot term must be charged identically pre-charge and actual (not refunded)",
        );
    });
}

/// Design test (ii), W1-008 part 2: the production `execute` weight must charge
/// more for a declared output slot than for none. While `weights.rs` still
/// carries the `BENCHMARK-PLACEHOLDER` sentinel the `o` coefficient is a
/// deliberate zero, so this skips with a notice. Once the reference machine
/// regenerates `weights.rs`, the sentinel is gone and this assertion becomes
/// binding automatically — no `#[ignore]` to remove, no edit needed. If the
/// regenerated coefficient comes back at noise level (zero), this fails, which
/// is the architect's cue to take it to the owner rather than ship a zero term.
#[test]
fn execute_weight_charges_output_slots_once_regenerated() {
    if include_str!("weights.rs").contains("BENCHMARK-PLACEHOLDER") {
        eprintln!(
            "skipping: the execute output-slot weight is an unmeasured \
             BENCHMARK-PLACEHOLDER (zero); run the benchmark-weights job to make \
             this assertion binding"
        );
        return;
    }
    let s = 1_024u32;
    let none = <crate::SubstrateWeight<Test> as crate::WeightInfo>::execute(s, 0);
    let one = <crate::SubstrateWeight<Test> as crate::WeightInfo>::execute(s, 1);
    assert!(
        one.ref_time() > none.ref_time(),
        "regenerated execute weight must rise with declared output slots: \
         execute({s}, 1) ref_time {} !> execute({s}, 0) ref_time {}",
        one.ref_time(),
        none.ref_time(),
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
fn integer_sample_with_small_k_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // XSMX pops k then size; the integer domain requires k >= 2.
        let bytecode = build_program(|b| {
            b.emit_push(4)
                .emit_push(1)
                .emit_xsmx(Register(0))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmInvalidIntegerK.into()
        );
    });
}

#[test]
fn sample_write_outside_its_domain_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // A binary sample holds {0, 1}; SETLINE pops value then index, and
        // writing 2 to variable 0 leaves the domain.
        let bytecode = build_program(|b| {
            b.emit_push(4)
                .emit_bsmx(Register(0))
                .emit_push(0)
                .emit_push(2)
                .emit_set_line(Register(0))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmSampleOutOfDomain.into()
        );
    });
}

#[test]
fn coefficient_index_past_the_model_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // A four-variable model has indices 0..4; SETLINE pops value then
        // index, and index 9 addresses a variable that was never declared.
        let bytecode = build_program(|b| {
            b.emit_push(4)
                .emit_bqmx(Register(0))
                .emit_push(9)
                .emit_push(1)
                .emit_set_line(Register(0))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmIndexOutOfBounds.into()
        );
    });
}

#[test]
fn sample_of_the_wrong_size_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // ENERGY of a two-element sample against a four-variable model. Both
        // registers hold the type the verifier expects, so this is a fault
        // only the VM can see.
        let bytecode = build_program(|b| {
            b.emit_push(4)
                .emit_bqmx(Register(0))
                .emit_push(2)
                .emit_bsmx(Register(1))
                .emit_energy(Register(0), Register(1))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmSizeMismatch.into()
        );
    });
}

#[test]
fn grid_larger_than_the_model_maps_to_its_own_error() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // RESIZE pops cols then rows. A 3 x 3 grid describes nine cells over
        // a model that declared four variables.
        let bytecode = build_program(|b| {
            b.emit_push(4)
                .emit_bqmx(Register(0))
                .emit_push(3)
                .emit_push(3)
                .emit_resize(Register(0))
                .emit_halt();
        });

        assert_eq!(
            execute_expecting_failure(bytecode, vec![], 0),
            Error::<Test>::VmInvalidGridDimensions.into()
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

// ── storage deposit and removal (QUI-1058) ───────────────────────────────

use crate::ProgramDeposit;
use frame_support::traits::fungible::{InspectHold, MutateHold};
use frame_support::traits::tokens::Precision;

/// The amount currently held against `who` for stored programs.
fn held(who: u64) -> u64 {
    <Balances as InspectHold<u64>>::balance_on_hold(&crate::HoldReason::StoredProgram.into(), &who)
}

#[test]
fn store_holds_a_deposit_and_remove_releases_it() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);
        let len = bytecode.len() as u32;
        let expected = Xqvm::deposit_for(len);

        assert_eq!(held(1), 0);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_eq!(held(1), expected);
        assert_eq!(ProgramDeposit::<Test>::get(&hash), Some(expected));

        assert_ok!(Xqvm::remove_program(RuntimeOrigin::signed(1), hash));

        assert_eq!(held(1), 0, "the deposit must come back on removal");
        assert!(!Programs::<Test>::contains_key(&hash));
        assert!(!ProgramOwner::<Test>::contains_key(&hash));
        assert!(!ProgramDeposit::<Test>::contains_key(&hash));

        System::assert_last_event(
            Event::ProgramRemoved {
                program_hash: hash,
                owner: 1,
                deposit: expected,
            }
            .into(),
        );
    });
}

#[test]
fn removal_reports_what_was_actually_released() {
    // The release is BestEffort so a hold that something else has already
    // reduced cannot strand the storage. The event must then report what
    // came back, not the amount that was recorded at store time.
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);
        let recorded = Xqvm::deposit_for(bytecode.len() as u32);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        // Something outside the pallet lets part of the hold go. Nothing in
        // the pallet does this today; the test stands in for the path that
        // might.
        let reduced_by = recorded / 4;
        assert_ok!(<Balances as MutateHold<u64>>::release(
            &crate::HoldReason::StoredProgram.into(),
            &1,
            reduced_by,
            Precision::Exact,
        ));
        assert_eq!(held(1), recorded - reduced_by);

        assert_ok!(Xqvm::remove_program(RuntimeOrigin::signed(1), hash));

        assert_eq!(held(1), 0, "whatever was still held must come back");
        assert!(!Programs::<Test>::contains_key(&hash));
        System::assert_last_event(
            Event::ProgramRemoved {
                program_hash: hash,
                owner: 1,
                deposit: recorded - reduced_by,
            }
            .into(),
        );
    });
}

#[test]
fn deposit_scales_with_program_length() {
    // The point of the per-byte term: a bigger program locks up more.
    new_test_ext().execute_with(|| {
        let small = build_program(|b| {
            b.emit_halt();
        });
        let large = build_program(|b| {
            for _ in 0..500 {
                b.emit_nop();
            }
            b.emit_halt();
        });

        assert!(
            Xqvm::deposit_for(large.len() as u32) > Xqvm::deposit_for(small.len() as u32),
            "a longer program must cost a larger deposit"
        );
    });
}

#[test]
fn store_fails_when_the_deposit_is_unaffordable() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        // Account 3 was never funded in genesis.
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert!(Xqvm::store_program(RuntimeOrigin::signed(3), bounded(bytecode)).is_err());

        // And nothing was written on the way out.
        assert!(!Programs::<Test>::contains_key(&hash));
        assert!(!ProgramOwner::<Test>::contains_key(&hash));
        assert!(!ProgramDeposit::<Test>::contains_key(&hash));
    });
}

#[test]
fn only_the_owner_can_remove() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        assert_noop!(
            Xqvm::remove_program(RuntimeOrigin::signed(2), hash),
            Error::<Test>::NotProgramOwner
        );

        // The deposit stays with the owner, untouched.
        assert_eq!(held(1), Xqvm::deposit_for(16));
        assert_eq!(held(2), 0);
        assert!(Programs::<Test>::contains_key(&hash));
    });
}

#[test]
fn removing_an_unknown_program_fails() {
    new_test_ext().execute_with(|| {
        let hash = program_hash(b"nothing stored under this");

        assert_noop!(
            Xqvm::remove_program(RuntimeOrigin::signed(1), hash),
            Error::<Test>::ProgramNotFound
        );
    });
}

#[test]
fn root_can_evict_and_the_depositor_is_made_whole() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);
        let expected = Xqvm::deposit_for(bytecode.len() as u32);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));
        assert_eq!(held(1), expected);

        assert_ok!(Xqvm::evict_program(RuntimeOrigin::root(), hash));

        assert_eq!(
            held(1),
            0,
            "eviction returns the deposit, it does not slash"
        );
        assert!(!Programs::<Test>::contains_key(&hash));

        System::assert_last_event(
            Event::ProgramEvicted {
                program_hash: hash,
                owner: 1,
                deposit: expected,
            }
            .into(),
        );
    });
}

#[test]
fn eviction_requires_root() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode),
        ));

        // Not even the owner may take the root path.
        assert!(Xqvm::evict_program(RuntimeOrigin::signed(1), hash).is_err());
        assert!(Programs::<Test>::contains_key(&hash));
    });
}

#[test]
fn a_removed_program_can_be_stored_again() {
    // Content addressing is what makes unilateral removal safe: deleting is
    // never permanent, it just returns the bytes to nobody-has-paid-for-this.
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let bytecode = build_program(|b| {
            b.emit_halt();
        });
        let hash = program_hash(&bytecode);

        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(1),
            bounded(bytecode.clone()),
        ));
        assert_ok!(Xqvm::remove_program(RuntimeOrigin::signed(1), hash));

        // A different account may now claim it, paying its own deposit.
        assert_ok!(Xqvm::store_program(
            RuntimeOrigin::signed(2),
            bounded(bytecode),
        ));

        assert_eq!(ProgramOwner::<Test>::get(&hash), Some(2));
        assert_eq!(held(2), Xqvm::deposit_for(16));
        assert_eq!(held(1), 0);
    });
}

// Audit acceptance goes through the signed dispatchable: rejecting after a
// direct verifier call would not prove that an untrusted transaction returns.
#[test]
fn audit_signed_store_rejects_all_register_cycle_without_hanging() {
    use std::{sync::mpsc, time::Duration};

    for registers in [255_u16, 256] {
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            new_test_ext().execute_with(|| {
                let bytecode = build_program(|b| {
                    b.emit(xqvm::Instruction::Nop {});
                    let cycle = b.label();
                    b.place(cycle).unwrap();
                    b.emit(xqvm::Instruction::Target {});
                    for slot in 0..registers {
                        b.emit(xqvm::Instruction::Bsmx {
                            reg: Register(slot as u8),
                        });
                    }
                    b.emit_jump(cycle).emit_halt();
                });
                let program = Program::decode(&bytecode).unwrap();
                let shape = ControlFlowShape::of(program.code());
                assert!(shape.blocks <= MaxProgramBlocks::get());
                assert!(shape.loop_depth <= MaxLoopDepth::get());
                let hash = program_hash(&bytecode);
                assert_noop!(
                    Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
                    Error::<Test>::VerifierStackFault,
                );
                assert!(!Programs::<Test>::contains_key(hash));
                assert!(!ProgramOwner::<Test>::contains_key(hash));
                assert!(!crate::ProgramDeposit::<Test>::contains_key(hash));
                assert_eq!(Balances::free_balance(1), INITIAL_BALANCE);
            });
            sender.send(()).unwrap();
        });
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("signed all-register-cycle rejection must terminate");
        worker.join().unwrap();
    }
}

#[test]
fn audit_signed_store_distinguishes_dead_reads_and_rejects_hidden_peak() {
    use xqvm::{bytecode::codec, Instruction};

    new_test_ext().execute_with(|| {
        let code = |live: bool| {
            let mut instructions = Vec::new();
            if !live {
                instructions.push(Instruction::Halt {});
            }
            instructions.extend([
                Instruction::Target {},
                Instruction::Drop { reg: Register(0) },
                Instruction::Jump2 { label: 1 },
                Instruction::Target {},
                Instruction::Load { reg: Register(0) },
                Instruction::Halt {},
            ]);
            raw_program(instructions.iter().flat_map(codec::encode).collect())
        };
        let dead = code(false);
        let hash = program_hash(&dead);
        assert_ok!(Xqvm::store_program(RuntimeOrigin::signed(1), bounded(dead)));
        assert_eq!(ProgramOwner::<Test>::get(hash), Some(1));
        assert_noop!(
            Xqvm::store_program(RuntimeOrigin::signed(1), bounded(code(true))),
            Error::<Test>::VerifierReadUnsetRegister,
        );
        for peak in [8192, 8193] {
            let bytecode = build_program(|b| {
                for _ in 0..peak {
                    b.emit_push(1);
                }
                b.emit_sclr().emit_halt();
            });
            if peak == 8192 {
                assert_ok!(Xqvm::store_program(
                    RuntimeOrigin::signed(1),
                    bounded(bytecode)
                ));
            } else {
                assert_noop!(
                    Xqvm::store_program(RuntimeOrigin::signed(1), bounded(bytecode)),
                    Error::<Test>::VerifierStackFault,
                );
            }
        }
    });
}

// W1-003a repin gate. A checked-in corpus of canonical program bytes with
// their pinned identity hashes. The corpus is synthetic, not historical: no
// programs exist on chain to draw from (`store_program` is filtered on runtime
// 121, and the testnet `Programs` map is empty), so the entries are chosen to
// exercise the encoder's header computation rather than sampled from use. The bytes are stored literally, not rebuilt
// from the builder, so a future xqvm repin that changes the canonical encoding
// -- the header's recomputed slot bytes, the CRC, or the layout -- breaks
// `decode(x).encode() == x`, and any change to the hashed bytes breaks the
// pinned hash. The gate therefore cannot be satisfied vacuously by
// regenerating fixtures; regenerate it deliberately (the `print_canonical_corpus`
// helper lives in this file's git history) only after reviewing the diff.
#[test]
fn canonical_corpus_round_trips_and_hashes_are_pinned() {
    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }
    // The output-slot entry is 256 `PUSH1 0; OUTPUT r0` pairs (`11000f00`)
    // behind its literal header: spelled as a repeat, but still literal bytes.
    let many_outputs = format!(
        "585142430100ff00000405e4a51ab911010b00{}ff",
        "11000f00".repeat(256)
    );
    // (canonical XQBC bytes, BlakeTwo256 hash, header input slots, header
    // output slots) -- halt, input/output, arithmetic, a model builder, a range
    // loop, and three entries for scan's lexical slot counting: INPUT/OUTPUT in
    // dead code after HALT (still counted), INPUT inside a RANGE...NEXT body
    // (counted once), and 256 OUTPUTs (the output count saturates at 255).
    let corpus: Vec<(&str, &str, u8, u8)> = vec![
        (
            "5851424301000000000001ff000000ff",
            "6868acc3b12d6fefd2cbe190b3a80f83b4501b3d83777a6089f4ae0d9f8d67cc",
            0,
            0,
        ),
        (
            "585142430101010000000976532f4311000e0011000f00ff",
            "04a2b90df7ef476042a1507c79dc70390debed539e57ccd7e7888c7ce3a6e993",
            1,
            1,
        ),
        (
            "5851424301000000000008e725b11111061107220b00ff",
            "d59b0eb6206a76cf4b603c8cf053e1d6b2d3ac877acb196374f48ee905f7db50",
            0,
            0,
        ),
        (
            "585142430100000000000b9304092d11044000110011016100ff",
            "91fe729a703e1d93cc5fc492773e00a92c1543fac076b8994372da370d223c21",
            0,
            0,
        ),
        (
            "5851424301000000000008482a67471100110308f007ff",
            "db0ca5528a09843a19b6c32ece331d577a066570995a482562060d7b3f0198c7",
            0,
            0,
        ),
        // HALT; PUSH1 0; INPUT r0; PUSH1 0; OUTPUT r0; HALT -- dead slots count.
        (
            "585142430101010000000a9297848fff11000e0011000f00ff",
            "a399e3d621b2e1180d0427e50f7e81b8e6cbe60daed0cd254725d24f9c5414d5",
            1,
            1,
        ),
        // PUSH1 0; PUSH1 1; RANGE; PUSH1 0; INPUT r0; NEXT; HALT.
        (
            "585142430101000000000b0a176d6b110011010811000e0007ff",
            "afe198a7fdd91d7648a80b9b97c82c851e689b07727aff7b320829b9cec57069",
            1,
            0,
        ),
        // PUSH1 1; STOW r0; 256 x (PUSH1 0; OUTPUT r0); HALT.
        (
            many_outputs.as_str(),
            "1b50324f0704d06d657a4ec8e3499ebf393c5a3a82d1c875bf94e960ee2baa23",
            0,
            255,
        ),
    ];
    for (bytes_hex, hash_hex, input_slots, output_slots) in corpus {
        let bytes = unhex(bytes_hex);
        // Canonical round-trip: decode then re-encode must reproduce the bytes.
        let reencoded = Program::decode(&bytes)
            .expect("corpus program decodes")
            .encode();
        assert_eq!(reencoded, bytes, "non-canonical corpus entry {bytes_hex}");
        // The header's slot bytes are what scan's lexical count computed.
        let program = Program::decode(&bytes).expect("corpus program decodes");
        assert_eq!(
            (program.input_slots(), program.output_slots()),
            (input_slots, output_slots),
            "header slot counts moved for corpus entry {bytes_hex}"
        );
        // Pinned identity hash: a silent hash move fails here.
        assert_eq!(
            program_hash(&bytes).as_ref(),
            unhex(hash_hex).as_slice(),
            "hash moved for corpus entry {bytes_hex}"
        );
        // The pallet admits the canonical form.
        new_test_ext().execute_with(|| {
            System::set_block_number(1);
            assert_ok!(Xqvm::store_program(
                RuntimeOrigin::signed(1),
                bounded(bytes.clone())
            ));
        });
    }
}

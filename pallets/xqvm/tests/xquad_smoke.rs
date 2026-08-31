//! Smoke test for the `xquad` (`xqvm`) integration.
//!
//! This suite is the canary for whether the pinned `xqvm` release can still
//! be integrated: it exercises exactly the `xqvm` API surface and semantics
//! that `pallet-xqvm` depends on, and nothing else. If a toolchain bump
//! breaks any assumption here, this test fails (or stops compiling) before
//! the drift reaches the pallet.
//!
//! Linking this target also recompiles `pallet_xqvm` itself, whose
//! deliberately exhaustive matches over `xqvm::VerifierError` and
//! `xqvm::Error` turn any new upstream error variant into a compile error.
//!
//! Run with: `cargo test -p pallet-xqvm --test xquad_smoke`

// Pull in the pallet so its exhaustive error mappings are part of the canary.
use pallet_xqvm as _;

use xqvm::{Error, InstructionBuilder, Program, RegVal, Register, Vm};

/// Build a program with `InstructionBuilder` and return its encoded bytes.
fn encode_program(f: impl FnOnce(&mut InstructionBuilder)) -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    f(&mut b);
    b.build().expect("smoke program must assemble").encode()
}

/// The container round-trip the pallet's `store_program` depends on:
/// `encode` output decodes, and a corrupted container is rejected.
#[test]
fn container_roundtrip_and_corruption_detection() {
    let bytes = encode_program(|b| {
        b.emit_push(1).emit_halt();
    });

    assert!(
        Program::decode(&bytes).is_ok(),
        "encoded program must decode"
    );

    // Flip one byte in the payload; the CRC-32 container check must catch it.
    let mut corrupt = bytes.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0xFF;
    assert!(
        Program::decode(&corrupt).is_err(),
        "corrupted container must be rejected at decode time"
    );
}

/// The static verification gate of `store_program`: a well-formed program
/// passes, a program reading an unset register is rejected.
#[test]
fn verifier_accepts_good_and_rejects_bad_programs() {
    let good = encode_program(|b| {
        b.emit_push(7).emit_stow(Register(0)).emit_halt();
    });
    let good = Program::decode(&good).expect("container is valid");
    assert!(
        xqvm::verifier::verify(&good).is_ok(),
        "verifier must accept a well-formed program"
    );

    let bad = encode_program(|b| {
        b.emit_load(Register(3)).emit_halt(); // r3 is never written
    });
    let bad = Program::decode(&bad).expect("container is valid");
    assert!(
        xqvm::verifier::verify(&bad).is_err(),
        "verifier must reject a read of an unset register"
    );
}

/// The execution contract of `execute`: calldata in via `INPUT`, arithmetic,
/// outputs out via `OUTPUT` as `RegVal::Int`, and a meaningful step count.
#[test]
fn execute_calldata_to_output_roundtrip() {
    let bytes = encode_program(|b| {
        // out[0] = calldata[0] * 10
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
    let program = Program::decode(&bytes).expect("container is valid");

    let mut vm = Vm::new();
    vm.set_step_limit(1_000)
        .set_output_slots(1)
        .set_calldata(vec![RegVal::Int(5)]);
    vm.run(&program).expect("program must run to completion");

    assert_eq!(vm.outputs(), &[RegVal::Int(50)]);
    assert!(vm.steps() > 0, "steps() must report executed steps");
    assert!(
        vm.steps() <= 1_000,
        "steps() must stay within the configured limit"
    );
}

/// The consensus-critical bound: a step limit stops a non-halting program
/// with `Error::StepLimitExceeded` (which the pallet maps to
/// `VmStepLimitExceeded`).
#[test]
fn step_limit_stops_runaway_programs() {
    let bytes = encode_program(|b| {
        let top = b.label();
        b.place(top).unwrap().emit_nop().emit_jump(top);
    });
    let program = Program::decode(&bytes).expect("container is valid");

    let mut vm = Vm::new();
    vm.set_step_limit(10);
    assert!(
        matches!(vm.run(&program), Err(Error::StepLimitExceeded { .. })),
        "a bounded run of a non-halting program must fail with StepLimitExceeded"
    );
}

/// Since xqvm 0.4.0, `set_step_limit(0)` means "zero steps", not "unlimited".
/// The pallet rejects a zero limit itself either way, but this pins the
/// upstream semantics: if the old 0-as-unlimited sentinel ever came back, this
/// program would halt normally and the assertion would fail (fast, no hang).
#[test]
fn zero_step_limit_means_zero_steps() {
    let bytes = encode_program(|b| {
        b.emit_halt();
    });
    let program = Program::decode(&bytes).expect("container is valid");

    let mut vm = Vm::new();
    vm.set_step_limit(0);
    assert!(
        matches!(vm.run(&program), Err(Error::StepLimitExceeded { .. })),
        "a zero step limit must execute zero steps, not unlimited ones"
    );
}

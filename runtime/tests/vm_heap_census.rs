//! W1-002 / V-A step 0: measurement harness (WP1-DESIGNS.md rev 3, V-A §1).
//!
//! Not the fix. Runs the compiled Wasm runtime through
//! `sc_executor::WasmExecutor` with the node's default heap, the way the
//! author (one runtime call per extrinsic) and the importer (one
//! `Core_execute_block`) do, and reports `address_space_used` from the
//! executor's own freeing-bump allocator.
//!
//! Programs are written into `pallet_xqvm::Programs` at genesis, for speed
//! (on main, e40cfba filters `Xqvm::store_program`). `execute` reads only
//! that map.
//!
//! A candidate cap L' is emulated by inputs whose total charge stays <= L': the
//! compiled cap is now `MaxVmMemory` = 1 MiB (W1-002), so a run that charges
//! <= L' <= 1 MiB never reaches the compiled check and allocates exactly what
//! it would under a cap of L'. Emulated L' values therefore stay at or below 1 MiB
//! with headroom for the k ~ 2 clone overhead; a larger L' would just hit the
//! real cap with `VmMemoryLimitExceeded` and is not a valid per-run measurement.
//!
//! Run in release (the native harness -- the executor's Wasm compile, block
//! building and hybrid signing -- is very slow in debug), with `<package>`
//! this copy's runtime: `quip-protocol-runtime` for runtime/,
//! `quip-testnet-runtime` for runtime-testnet/ (one line for both copies, so
//! the parity gate sees no difference):
//! `cargo test --release -p <package> --test vm_heap_census \
//!       -- --ignored --nocapture --test-threads 1`

use codec::{Decode, Encode};
use frame_support::{dispatch::DispatchClass, traits::Get, BoundedVec};
use quip_protocol_runtime::{
    native_tx_extension, Address, Block, Header, Nonce, Runtime, RuntimeCall, RuntimeGenesisConfig,
    Signature, SignedPayload, TxExtension, UncheckedExtrinsic, ETH_TRANSACT_PAYLOAD_TOO_LARGE,
    MAX_ETH_TRANSACT_PAYLOAD, REVIVE_BLOB_BYTES, REVIVE_CALLDATA_BYTES, SLOT_DURATION, UNIT,
    WASM_BINARY,
};
use quip_transaction_crypto::{account_id_from_public, HybridPair, HybridTxSignature};
use sc_executor::WasmExecutor;
use sc_executor_common::runtime_blob::RuntimeBlob;
use sp_consensus_babe::{
    digests::{PreDigest, SecondaryPlainPreDigest},
    Slot, BABE_ENGINE_ID,
};
use sp_core::{
    traits::{CallContext, CodeExecutor, RuntimeCode, WrappedRuntimeCode},
    Pair as _, H256,
};
use sp_io::{SubstrateHostFunctions, TestExternalities};
use sp_runtime::{
    generic,
    traits::{BlakeTwo256, Hash as _, Header as _},
    ApplyExtrinsicResult, BuildStorage, Digest, DigestItem, Storage,
};
use xqvm::{Instruction as I, InstructionBuilder, Register as R};

type Executor = WasmExecutor<SubstrateHostFunctions>;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;
/// The shipped per-run VM cap (`MaxVmMemory`, owner's second decision of
/// 2026-11-22: 1 MiB, a power of two).
const SHIPPED_CAP: u64 = 1_048_576;

fn mib(bytes: u32) -> f64 {
    f64::from(bytes) / MIB as f64
}

fn max_vm_memory() -> u64 {
    <quip_protocol_runtime::configs::MaxVmMemory as Get<u64>>::get()
}

fn max_steps() -> u64 {
    <quip_protocol_runtime::configs::MaxStepLimit as Get<u64>>::get()
}

// ---------------------------------------------------------------------------
// Programs. Each isolates one allocation family from the census.
// ---------------------------------------------------------------------------

fn encode(b: InstructionBuilder) -> Vec<u8> {
    b.build().expect("program builds").encode()
}

/// Auditor shape: `INPUT r0`, then 255 x (`LOAD r0`; `BSMX r_i`).
fn prog_auditor() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    for i in 1..=255u8 {
        b.emit(I::Load { reg: R(0) }).emit(I::Bsmx { reg: R(i) });
    }
    b.emit(I::Halt {});
    encode(b)
}

/// calldata `[n, count, energy]`: allocate up to `count` samples of `n`
/// elements; if `energy != 0`, `ENERGY` against a size-1 model, which faults
/// VmSizeMismatch for n > 1 (n = 1 matches and succeeds). Before xqvm 0.6.0
/// ENERGY first cloned r1's sample buffer (uncharged); W1-013 removed the
/// clone, and the fault is unchanged.
fn prog_samples() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    let done = b.label();
    let end = b.label();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit_push(1).emit(I::Input { reg: R(255) });
    b.emit_push(2).emit(I::Input { reg: R(253) });
    b.emit(I::Load { reg: R(0) }).emit(I::Bsmx { reg: R(1) });
    for i in 2..=252u8 {
        b.emit(I::Load { reg: R(255) })
            .emit_push(i64::from(i))
            .emit(I::Lt {})
            .emit_jump_if(done);
        b.emit(I::Load { reg: R(0) }).emit(I::Bsmx { reg: R(i) });
    }
    b.place(done).expect("label");
    b.emit(I::Load { reg: R(253) })
        .emit(I::Not {})
        .emit_jump_if(end);
    b.emit_push(1)
        .emit(I::Bqmx { reg: R(254) })
        .emit_energy(R(254), R(1))
        .emit(I::Pop {});
    b.place(end).expect("label");
    b.emit(I::Halt {});
    encode(b)
}

/// calldata `[n, equality]`: push `n` zeros into a `vec<int>`; if
/// `equality != 0`, `EQUALITY m, v, v` clones `v` twice before the quadratic
/// charge faults.
fn prog_vec() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    let end = b.label();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit_push(1).emit(I::Input { reg: R(2) });
    b.emit(I::VecI { reg: R(1) });
    b.emit_push(0).emit(I::Load { reg: R(0) }).emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(1) });
    b.emit(I::Next {});
    b.emit(I::Load { reg: R(2) })
        .emit(I::Not {})
        .emit_jump_if(end);
    b.emit_push(1).emit(I::Bqmx { reg: R(3) });
    b.emit_push(0).emit_push(1).emit(I::Equality {
        model: R(3),
        indices: R(1),
        coeffs: R(1),
    });
    b.place(end).expect("label");
    b.emit(I::Halt {});
    encode(b)
}

/// calldata `[n]`: a size-`n` model with `n` linear coefficients.
fn prog_linear() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit(I::Load { reg: R(0) }).emit(I::Bqmx { reg: R(1) });
    b.emit_push(0).emit(I::Load { reg: R(0) }).emit(I::Range {});
    b.emit(I::Lidx { reg: R(2) })
        .emit(I::Load { reg: R(2) })
        .emit_push(1)
        .emit(I::SetLine { reg: R(1) });
    b.emit(I::Next {});
    b.emit(I::Halt {});
    encode(b)
}

/// calldata `[n]`: the same model as [`prog_linear`], written in the order
/// that packs the coefficient `BTreeMap` worst: keys `0..5` first, then `n-1`
/// down to `5`. Every later key lands at edge 5 of the leftmost leaf, so each
/// split leaves a 5-entry leaf (the B-tree's minimum) that is never written
/// again: about 5.15 entries per 256-B node against 6.0 in ascending order.
/// The same charge, more chunks (claude_reviewer's repin R8 probe).
fn prog_linear_anchored() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit(I::Load { reg: R(0) }).emit(I::Bqmx { reg: R(1) });
    b.emit_push(0).emit_push(5).emit(I::Range {});
    b.emit(I::Lidx { reg: R(2) })
        .emit(I::Load { reg: R(2) })
        .emit_push(1)
        .emit(I::SetLine { reg: R(1) });
    b.emit(I::Next {});
    b.emit_push(0)
        .emit(I::Load { reg: R(0) })
        .emit_push(5)
        .emit(I::Sub {})
        .emit(I::Range {});
    b.emit(I::Lidx { reg: R(2) })
        .emit(I::Load { reg: R(0) })
        .emit(I::Dec {})
        .emit(I::Load { reg: R(2) })
        .emit(I::Sub {})
        .emit_push(1)
        .emit(I::SetLine { reg: R(1) });
    b.emit(I::Next {});
    b.emit(I::Halt {});
    encode(b)
}

/// calldata `[n]`: a size-`n` model with `n-1` quadratic coefficients `(0, j)`.
fn prog_quad() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit(I::Load { reg: R(0) }).emit(I::Bqmx { reg: R(1) });
    b.emit_push(1)
        .emit(I::Load { reg: R(0) })
        .emit(I::Dec {})
        .emit(I::Range {});
    b.emit(I::Lidx { reg: R(2) })
        .emit_push(0)
        .emit(I::Load { reg: R(2) })
        .emit_push(1)
        .emit(I::SetQuad { reg: R(1) });
    b.emit(I::Next {});
    b.emit(I::Halt {});
    encode(b)
}

/// The VM's value-stack limit (xqvm `Vm::STACK_LIMIT`, vm.rs:895). The stack
/// is a `Vec<i64>` bounded by this count, not by the VM memory meter.
const VM_STACK_LIMIT: usize = 8192;

/// Worst-case fixed per-run overhead (W1-002 R8 batch item 2): every
/// L-independent allocation the memory meter does not charge, each at its
/// static limit, in one program that `store_program`'s checks admit
/// (asserted in `i_worst_case_fixed_run`). calldata: 256 words.
/// - all 256 registers written (`INPUT` from 256 calldata words);
/// - `MaxLoopDepth` (32) nested one-iteration `RANGE` loops: since xqvm 0.6.0
///   (W1-011) the most frames a storable program can hold. A program stored
///   before 0.6.0 can hold more, because `execute` does not re-verify; the
///   runtime's pins assume no such program exists (the precondition in
///   `MaxVmMemory`'s docs);
/// - inside them the value stack pushed to `VM_STACK_LIMIT` (8,192), then
///   cleared, so the stack `Vec`'s whole doubling history is allocated;
/// - all 256 output slots written (`execute` declares 256);
/// - `NOP`-padded to exactly `MaxProgramSize`, so the stored bytes and the
///   decoded program are maximal.
fn prog_fixed_worst() -> Vec<u8> {
    fixed_worst_program(0, Pile::None, false)
}

/// Holder (i), R9 §A (QUI-2516): `prog_fixed_worst_jumps` with every holder
/// class at its maximum at once -- all 256 registers, all 256 OUTPUT clones,
/// `MaxLoopDepth` open loops and the value stack at `VM_STACK_LIMIT` -- and
/// then one more `PUSH`, which faults `StackOverflow`. Under the full checker
/// this was refused (`StackOverflowRisk`); the structural gate stores it. A
/// fault must not raise the run's high-water mark: census i compares it with
/// the completed run.
fn prog_fault_at_peak() -> Vec<u8> {
    let max_blocks = <quip_protocol_runtime::configs::MaxProgramBlocks as Get<u32>>::get() as usize;
    let max_depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get() as usize;
    fixed_worst_program(max_blocks - 1 - 2 * max_depth, Pile::None, true)
}

/// xqvm's loop-stack limit (`Vm::LOOP_LIMIT`). Up to xqvm 0.5.0 a back-edge
/// that re-entered a `RANGE`/`ITER` without its `NEXT` pushed another
/// uncharged `LoopFrame` each pass, so only this count bounded the loop stack.
/// xqvm 0.6.0's jump-span phase (W1-011) rejects such a jump at
/// `store_program`, so a storable program holds at most `MaxLoopDepth` frames;
/// the pile-up builders below remain only to check that they are rejected.
/// `execute` does not re-verify, so a program stored before 0.6.0 still piles
/// up to this limit (933,504 B with `ITER` slice copies); the pins hold only
/// on a chain with no such program (`MaxVmMemory`'s precondition).
const VM_LOOP_LIMIT: usize = 8192;

/// A loop-frame pile-up appended after the stack fill (claude's R8 finding).
/// Unstorable since xqvm 0.6.0 (W1-011): kept to assert the rejection.
#[derive(Clone, Copy, PartialEq)]
enum Pile {
    None,
    /// `RANGE` frames, re-entered by a counted back-edge to `VM_LOOP_LIMIT`.
    Range,
    /// `ITER` frames over a one-element vec: the same frame plus a charged
    /// one-element slice copy each.
    Iter,
}

/// Basic blocks a pile-up uses: its `TARGET`, the opener, the back-edge
/// `JUMPI` and the `NEXT`.
const PILE_BLOCKS: usize = 4;

/// `prog_fixed_worst` plus a chain of `TARGET; JUMP next` pairs that uses every
/// basic block `MaxProgramBlocks` leaves over, so decode's `checker::scan`
/// builds its largest `targets` (the kept jump table) and `jump_refs`
/// alongside everything else. A `JUMP` followed by a `TARGET` is ONE leader
/// in both the pallet's `ControlFlowShape` and the verifier's dataflow, so an
/// admitted program can carry about 2,048 targets AND 2,047 jumps together.
fn prog_fixed_worst_jumps() -> Vec<u8> {
    let max_blocks = <quip_protocol_runtime::configs::MaxProgramBlocks as Get<u32>>::get() as usize;
    let max_depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get() as usize;
    // The entry block, plus one block per RANGE and per NEXT of the loops,
    // plus one per jump of the chain.
    fixed_worst_program(max_blocks - 1 - 2 * max_depth, Pile::None, false)
}

/// `prog_fixed_worst_jumps`, plus a loop-frame pile-up to `VM_LOOP_LIMIT`
/// after the stack fill (the chain gives up the blocks the pile-up needs).
fn prog_fixed_worst_pile(pile: Pile) -> Vec<u8> {
    let max_blocks = <quip_protocol_runtime::configs::MaxProgramBlocks as Get<u32>>::get() as usize;
    let max_depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get() as usize;
    fixed_worst_program(max_blocks - 1 - 2 * max_depth - PILE_BLOCKS, pile, false)
}

fn fixed_worst_program(jumps: usize, pile: Pile, fault_at_peak: bool) -> Vec<u8> {
    let max_size = <quip_protocol_runtime::configs::MaxProgramSize as Get<u32>>::get() as usize;
    let max_depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get();
    let build = |pad: usize| {
        let mut b = InstructionBuilder::new();
        if jumps > 0 {
            let labels: Vec<_> = (0..=jumps).map(|_| b.label()).collect();
            for i in 0..jumps {
                b.place(labels[i]).expect("label");
                b.emit_jump(labels[i + 1]);
            }
            b.place(labels[jumps]).expect("label");
        }
        for i in 0..=255u8 {
            b.emit_push(i64::from(i)).emit(I::Input { reg: R(i) });
        }
        if fault_at_peak {
            // The OUTPUT clones first, so every holder is live at the fault.
            for i in 0..=255u8 {
                b.emit_push(i64::from(i)).emit(I::Output { reg: R(i) });
            }
        }
        for _ in 0..max_depth {
            b.emit_push(0).emit_push(1).emit(I::Range {});
        }
        for _ in 0..VM_STACK_LIMIT {
            b.emit_push(1);
        }
        if fault_at_peak {
            // The 8,193rd value: StackOverflow, with everything still held.
            b.emit_push(1);
        }
        b.emit(I::Sclr {});
        for _ in 0..max_depth {
            b.emit(I::Next {});
        }
        if pile != Pile::None {
            // r255 counts the passes; r254 is a one-element vec for ITER. The
            // stack's buffer from the fill above is still allocated, so the
            // frames pile up on top of it.
            b.emit_push(VM_LOOP_LIMIT as i64)
                .emit(I::Stow { reg: R(255) });
            if pile == Pile::Iter {
                b.emit(I::VecI { reg: R(254) });
                b.emit_push(0).emit(I::VecPush { reg: R(254) });
            }
            let again = b.label();
            b.place(again).expect("label");
            b.emit_push(0).emit_push(1);
            if pile == Pile::Iter {
                b.emit(I::Iter { reg: R(254) });
            } else {
                b.emit(I::Range {});
            }
            // Back above the opener while passes remain: one frame per pass.
            b.emit(I::Load { reg: R(255) })
                .emit(I::Dec {})
                .emit(I::Stow { reg: R(255) });
            b.emit(I::Load { reg: R(255) }).emit_jump_if(again);
            b.emit(I::Next {});
            // r254 and r255 back to plain integers for the OUTPUTs.
            b.emit_push(254).emit(I::Input { reg: R(254) });
            b.emit_push(255).emit(I::Input { reg: R(255) });
        }
        if !fault_at_peak {
            for i in 0..=255u8 {
                b.emit_push(i64::from(i)).emit(I::Output { reg: R(i) });
            }
        }
        for _ in 0..pad {
            b.emit(I::Nop {});
        }
        b.emit(I::Halt {});
        encode(b)
    };
    let unpadded = build(0).len();
    assert!(
        unpadded <= max_size,
        "worst-case program core must fit MaxProgramSize"
    );
    let bytes = build(max_size - unpadded);
    assert_eq!(
        bytes.len(),
        max_size,
        "NOP padding lands exactly on MaxProgramSize"
    );
    bytes
}

/// The `ITER` pile-up of W1-002 R8 item 10 (unstorable since xqvm 0.6.0,
/// W1-011; kept to assert the rejection). calldata `[e, frames]`: an `ITER`
/// pile-up of `frames` frames, each copying an `e`-element slice. A copy is charged at 8 B per element but lands in a
/// power-of-two allocator chunk, so `e = chunk/16 + 1` elements (just over half
/// the chunk) occupy the whole chunk -- about twice the charge -- and up to
/// `VM_LOOP_LIMIT` frames can fill one allocator order that 252 sample buffers
/// cannot reach.
fn prog_iter_slices() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(3) }); // r3 = e
    b.emit_push(1).emit(I::Input { reg: R(2) }); // r2 = frames (counter)
    b.emit(I::VecI { reg: R(1) });
    b.emit_push(0).emit(I::Load { reg: R(3) }).emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(1) });
    b.emit(I::Next {});
    let again = b.label();
    b.place(again).expect("label");
    b.emit_push(0)
        .emit(I::Load { reg: R(3) })
        .emit(I::Iter { reg: R(1) });
    b.emit(I::Load { reg: R(2) })
        .emit(I::Dec {})
        .emit(I::Stow { reg: R(2) });
    // Back above the opener only while passes remain (0 < counter): once it
    // reaches 0 the last frame's NEXT walks its slice and exits.
    b.emit_push(0)
        .emit(I::Load { reg: R(2) })
        .emit(I::Lt {})
        .emit_jump_if(again);
    b.emit(I::Next {});
    b.emit(I::Halt {});
    encode(b)
}

// ---------------------------------------------------------------------------
// Holder classes (W1-002 R8 item 19, from claude's probe on a34b96c). Besides
// loop frames, a run can keep small buffers alive in its 252 free registers
// and in up to 256 OUTPUT clones; in the orders where LOOP_LIMIT caps the
// frames, those holders fill an order further.
// ---------------------------------------------------------------------------

/// calldata `[e]`: r3 = e.
fn holders_head(b: &mut InstructionBuilder) {
    b.emit_push(0).emit(I::Input { reg: R(3) });
}

/// calldata `[e]`: r4..r255 each an e-element sample and 256 OUTPUT clones of
/// r4 -- every holder in the same allocator order. (Since xqvm 0.6.0, W1-011,
/// no loop-frame pile-up can follow: at most `MaxLoopDepth` frames.)
fn prog_holders_samples() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    holders_head(&mut b);
    for i in 4..=255u8 {
        b.emit(I::Load { reg: R(3) }).emit(I::Bsmx { reg: R(i) });
    }
    for s in 0..256u16 {
        b.emit_push(i64::from(s)).emit(I::Output { reg: R(4) });
    }
    b.emit(I::Halt {});
    encode(b)
}

/// No calldata. r4..r255 each a size-2 model with one linear and one
/// quadratic coefficient -- two BTreeMap root leaves (144 B and 184 B on
/// wasm32), so two 256-B-order chunks for 96 B of charge -- and 256 OUTPUT
/// clones of them. (It must not read calldata: it runs with none, and an
/// INPUT would fault VmCallDataIndex before any holder is built.)
fn prog_holders_models() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    for i in 4..=255u8 {
        b.emit_push(2).emit(I::Bqmx { reg: R(i) });
        b.emit_push(0).emit_push(1).emit(I::SetLine { reg: R(i) });
        b.emit_push(0)
            .emit_push(1)
            .emit_push(1)
            .emit(I::SetQuad { reg: R(i) });
    }
    for s in 0..256u16 {
        let r = 4 + u8::try_from(s % 252).expect("fits");
        b.emit_push(i64::from(s)).emit(I::Output { reg: R(r) });
    }
    b.emit(I::Halt {});
    encode(b)
}

/// Holder (ii), R9 §A (QUI-2516): the value stack filled to
/// `VM_STACK_LIMIT` by a 3-byte loop body (`PUSH1 1; NEXT`) instead of
/// 8,192 `PUSH` instructions. The full checker refused a loop that grows the
/// stack (`LoopStackImbalance`); the structural gate stores it, so a program
/// of a few dozen bytes now reaches the stack's whole doubling history.
fn prog_stack_loop() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0)
        .emit_push(VM_STACK_LIMIT as i64)
        .emit(I::Range {});
    b.emit_push(1);
    b.emit(I::Next {});
    b.emit(I::Halt {});
    encode(b)
}

/// Holder (iii), R9 §A (QUI-2516): a type-confused `EQUALITY` or `ATLEASTW`.
/// calldata `[e]`: r1 an `e`-element vec, used as both indices and
/// coefficients; r3 an integer where the model belongs. Both opcodes clone
/// the two vecs before anything else, so each run makes two uncharged
/// `e`-element copies and then faults: `RegisterType` at the model register,
/// or, for larger `e`, `MemoryLimitExceeded` at the expansion charge that
/// precedes it (`typeconf_run` derives which, natively). The full checker
/// refused it (`RegisterTypeMismatch`); the structural gate stores it.
fn prog_typeconf(atleastw: bool) -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit(I::VecI { reg: R(1) });
    b.emit_push(0).emit(I::Load { reg: R(0) }).emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(1) });
    b.emit(I::Next {});
    b.emit_push(7).emit(I::Stow { reg: R(3) });
    // EQUALITY pops penalty then target; ATLEASTW penalty then k, and k must
    // be positive to get past its first check.
    b.emit_push(i64::from(atleastw)).emit_push(1);
    if atleastw {
        b.emit(I::AtLeastW {
            model: R(3),
            indices: R(1),
            coeffs: R(1),
        });
    } else {
        b.emit(I::Equality {
            model: R(3),
            indices: R(1),
            coeffs: R(1),
        });
    }
    b.emit(I::Halt {});
    encode(b)
}

/// xqvm 0.6.0's charge rates the holder bounds use. They are private in
/// `vm.rs` (`VARIABLE_BYTES` at :351, `VEC_ELEMENT_BYTES` at :358), so they
/// are restated here and pinned natively by `holder_charge_rates_are_xqvm_s`.
/// An integer register, a sample element and a model variable each charge
/// `VARIABLE_BYTES`; an element appended to a `vec<int>` charges
/// `VEC_ELEMENT_BYTES`, twice its buffer size.
const VARIABLE_BYTES: u64 = 8;
const VEC_ELEMENT_BYTES: u64 = 16;

/// How a native run ended, for comparing two runs of the same program.
#[derive(Debug)]
struct NativeRun {
    result: Result<(), xqvm::Error>,
    used: u64,
    steps: u64,
}

impl NativeRun {
    /// The same run: it ended the same way at the same pc, after the same
    /// steps, holding the same charge. A `MemoryLimitExceeded` is compared
    /// without its `limit`, the one field that names the cap.
    fn same_as(&self, other: &Self) -> bool {
        fn key(r: &Result<(), xqvm::Error>) -> String {
            match r {
                Err(xqvm::Error::MemoryLimitExceeded {
                    pos,
                    requested,
                    used,
                    ..
                }) => format!("MemoryLimitExceeded {pos} {requested} {used}"),
                other => format!("{other:?}"),
            }
        }
        key(&self.result) == key(&other.result)
            && self.used == other.used
            && self.steps == other.steps
    }
}

/// Run `bytes` natively at memory cap `cap`, with an `execute` call's step
/// limit, output slots and calldata.
fn native_run(
    bytes: &[u8],
    calldata: &[i64],
    step_limit: u64,
    outputs: u32,
    cap: u64,
) -> NativeRun {
    let program = xqvm::Program::decode(bytes).expect("decodes");
    let mut vm = xqvm::Vm::new();
    vm.set_step_limit(step_limit);
    vm.set_memory_limit(cap);
    vm.set_output_slots(outputs as usize);
    vm.set_calldata(calldata.iter().map(|&v| xqvm::RegVal::Int(v)).collect());
    let result = vm.run(&program);
    NativeRun {
        result,
        used: vm.memory_used(),
        steps: vm.steps(),
    }
}

/// A step limit for a run that took `steps` natively: the same run on chain,
/// with a quarter's margin, never above `MaxStepLimit`.
fn step_budget(steps: u64) -> u64 {
    (steps * 5 / 4 + 100).min(max_steps())
}

/// The code offset of the first `EQUALITY`/`ATLEASTW` in `bytes`: the
/// instruction that clones its two vec operands before any charge.
fn clone_op_pos(bytes: &[u8]) -> usize {
    let program = xqvm::Program::decode(bytes).expect("decodes");
    xqvm::InstructionStream::new(program.code())
        .map(|item| item.expect("decodes"))
        .find(|(_, _, instr)| matches!(instr, I::Equality { .. } | I::AtLeastW { .. }))
        .map(|(pos, _, _)| pos)
        .expect("the program has a clone opcode")
}

/// A clone holder's fault, NON-MASKING (claude's R8 on A6c/A6d): only a fault
/// that follows the two vec clones counts -- `RegisterType` on `model`, which
/// only the clone opcode reads as a model, or `MemoryLimitExceeded` at the
/// clone opcode's own pc (its growth or expansion charge). An earlier fault,
/// such as `MemoryLimitExceeded` at the `VECPUSH` that builds the vec, never
/// made the clones and panics here instead of passing as the designed code.
fn post_clone_code(r: &Result<(), xqvm::Error>, model: u8, op_pos: usize, what: &str) -> u8 {
    match r {
        Err(xqvm::Error::RegisterType { reg, .. }) if *reg == model => REGISTER_TYPE_FAULT,
        Err(xqvm::Error::MemoryLimitExceeded { pos, .. }) if *pos == op_pos => VEC_EQUALITY_FAULT,
        other => panic!("{what}: {other:?} is not a fault after the clones at pc {op_pos}"),
    }
}

/// `bytes` with its code cut at `pos` and a `HALT` there: the run up to an
/// instruction, without it. (Loops left open are fine: `HALT` ends the run.)
fn truncate_at(bytes: &[u8], pos: usize) -> Vec<u8> {
    let program = xqvm::Program::decode(bytes).expect("decodes");
    let mut code = program.code()[..pos].to_vec();
    code.push(0xFF); // HALT
    xqvm::Program::new(code).encode()
}

/// Whether `r` is a fault after the clones at `op_pos` (see
/// [`post_clone_code`]), without panicking.
fn is_post_clone(r: &Result<(), xqvm::Error>, model: u8, op_pos: usize) -> bool {
    matches!(r, Err(xqvm::Error::RegisterType { reg, .. }) if *reg == model)
        || matches!(r, Err(xqvm::Error::MemoryLimitExceeded { pos, .. }) if *pos == op_pos)
}

/// The rates above, measured: an `INPUT` of an integer charges
/// `VARIABLE_BYTES`, each `VECPUSH` `VEC_ELEMENT_BYTES`, and a `BSMX` of `n`
/// elements `n * VARIABLE_BYTES`. If xqvm changes a rate, every holder bound
/// built on it is wrong, and this fails first.
#[test]
fn holder_charge_rates_are_xqvm_s() {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit(I::Halt {});
    let input = native_run(&encode(b), &[5], max_steps(), 0, max_vm_memory());
    assert_eq!((input.result.is_ok(), input.used), (true, VARIABLE_BYTES));
    let vec = native_run(&prog_vec(), &[5, 0], max_steps(), 0, max_vm_memory());
    assert_eq!(
        (vec.result.is_ok(), vec.used),
        (true, 2 * VARIABLE_BYTES + 5 * VEC_ELEMENT_BYTES)
    );
    let mut b = InstructionBuilder::new();
    b.emit_push(5).emit(I::Bsmx { reg: R(1) });
    b.emit(I::Halt {});
    let sample = native_run(&encode(b), &[], max_steps(), 0, max_vm_memory());
    assert_eq!(
        (sample.result.is_ok(), sample.used),
        (true, 5 * VARIABLE_BYTES)
    );
}

/// A6 follow-up, shape 1: `ATLEASTW`'s success path with slack variables.
/// calldata `[e]`: r1 is `e` zeros (indices into a size-1 binary model) and r2
/// is `e` ones (weights), with k = 1, so the excess `e - 1` adds
/// bit_length(e - 1) slacks. Besides the two up-front vec clones, the success
/// path collects the indices (`Vec<usize>`) and clones the weights again
/// (`all_coeffs`), then pushes the slacks onto both, and each first push on an
/// exact-capacity vec reallocates it at double size. All of that is
/// uncharged; the quadratic expansion charge bounds `e` to a few hundred.
fn prog_atleastw_slack() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit(I::VecI { reg: R(1) });
    b.emit(I::VecI { reg: R(2) });
    b.emit_push(0).emit(I::Load { reg: R(0) }).emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(1) });
    b.emit_push(1).emit(I::VecPush { reg: R(2) });
    b.emit(I::Next {});
    b.emit_push(1).emit(I::Bqmx { reg: R(3) });
    // k = 1, then penalty = 1
    b.emit_push(1).emit_push(1);
    b.emit(I::AtLeastW {
        model: R(3),
        indices: R(1),
        coeffs: R(2),
    });
    b.emit(I::Halt {});
    encode(b)
}

/// A6 follow-up, shape 2: holder iii inside a sample-holder run. calldata
/// `[e]`: r4..r254 each an `e`-element sample and 256 OUTPUT clones of r4,
/// as in `holders_samples`, then r255 an `e`-element vec and the
/// type-confused `EQUALITY`/`ATLEASTW` over it, so its two clones are made
/// while every holder's chunk is live.
fn prog_holders_typeconf(atleastw: bool) -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    holders_head(&mut b);
    for i in 4..=254u8 {
        b.emit(I::Load { reg: R(3) }).emit(I::Bsmx { reg: R(i) });
    }
    for s in 0..256u16 {
        b.emit_push(i64::from(s)).emit(I::Output { reg: R(4) });
    }
    b.emit(I::VecI { reg: R(255) });
    b.emit_push(0).emit(I::Load { reg: R(3) }).emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(255) });
    b.emit(I::Next {});
    b.emit_push(7).emit(I::Stow { reg: R(2) });
    b.emit_push(i64::from(atleastw)).emit_push(1);
    if atleastw {
        b.emit(I::AtLeastW {
            model: R(2),
            indices: R(255),
            coeffs: R(255),
        });
    } else {
        b.emit(I::Equality {
            model: R(2),
            indices: R(255),
            coeffs: R(255),
        });
    }
    b.emit(I::Halt {});
    encode(b)
}

/// Mixed-family holder `mix_clone` (claude's R8 on A6c/A6d, shape 1 of its
/// (d)). calldata `[n, k, e]`: r4..r(3+k) each an `n`-element sample, then r1
/// an `e`-element vec and the type-confused `EQUALITY r3(int), r1, r1`, so one
/// run holds the samples, the vec and its two clones in the same order -- one
/// block more per order than the single-family runs.
fn prog_mix_clone() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    let done = b.label();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit_push(1).emit(I::Input { reg: R(2) });
    b.emit_push(2).emit(I::Input { reg: R(255) });
    for k in 1..=251u8 {
        b.emit(I::Load { reg: R(2) })
            .emit_push(i64::from(k))
            .emit(I::Lt {})
            .emit_jump_if(done);
        b.emit(I::Load { reg: R(0) })
            .emit(I::Bsmx { reg: R(3 + k) });
    }
    b.place(done).expect("label");
    b.emit(I::VecI { reg: R(1) });
    b.emit_push(0)
        .emit(I::Load { reg: R(255) })
        .emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(1) });
    b.emit(I::Next {});
    b.emit_push(7).emit(I::Stow { reg: R(3) });
    b.emit_push(0).emit_push(1);
    b.emit(I::Equality {
        model: R(3),
        indices: R(1),
        coeffs: R(1),
    });
    b.emit(I::Halt {});
    encode(b)
}

/// Mixed-family holder `models_mix` (claude's R8 on A6c/A6d, shape 2 of its
/// (d)). calldata `[n]`: `holders_models`' 252 size-2 models and 256 OUTPUT
/// clones (1,016 BTreeMap nodes in the 256-B order), then r1 a size-`n` model
/// with `n` linear coefficients written in `linear_anchored`'s order, spending
/// the rest of the cap in the same run. Succeeds.
fn prog_models_mix() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    for i in 4..=255u8 {
        b.emit_push(2).emit(I::Bqmx { reg: R(i) });
        b.emit_push(0).emit_push(1).emit(I::SetLine { reg: R(i) });
        b.emit_push(0)
            .emit_push(1)
            .emit_push(1)
            .emit(I::SetQuad { reg: R(i) });
    }
    for s in 0..256u16 {
        let r = 4 + u8::try_from(s % 252).expect("fits");
        b.emit_push(i64::from(s)).emit(I::Output { reg: R(r) });
    }
    b.emit(I::Load { reg: R(0) }).emit(I::Bqmx { reg: R(1) });
    b.emit_push(0).emit_push(5).emit(I::Range {});
    b.emit(I::Lidx { reg: R(2) })
        .emit(I::Load { reg: R(2) })
        .emit_push(1)
        .emit(I::SetLine { reg: R(1) });
    b.emit(I::Next {});
    b.emit_push(0)
        .emit(I::Load { reg: R(0) })
        .emit_push(5)
        .emit(I::Sub {})
        .emit(I::Range {});
    b.emit(I::Lidx { reg: R(2) })
        .emit(I::Load { reg: R(0) })
        .emit(I::Dec {})
        .emit(I::Load { reg: R(2) })
        .emit(I::Sub {})
        .emit_push(1)
        .emit(I::SetLine { reg: R(1) });
    b.emit(I::Next {});
    b.emit(I::Halt {});
    encode(b)
}

/// Mixed-family holder `holders_iter` (claude's follow-up R8 on A6c/A6d).
/// calldata `[e]`: r1 an `e`-element vec; r0 and r4..r255 `e`-element samples
/// (253); 256 OUTPUT clones of r4; then `MaxLoopDepth` nested `ITER r1` over
/// [0, e), each copying `e` elements into its frame; then, innermost, the
/// type-confused `EQUALITY r2(int), r1, r1` and its two clones. Every holder
/// slot a run has -- registers, OUTPUT slots, loop frames -- in one order.
fn prog_holders_iter() -> Vec<u8> {
    let depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get();
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(3) });
    b.emit(I::VecI { reg: R(1) });
    b.emit_push(0).emit(I::Load { reg: R(3) }).emit(I::Range {});
    b.emit_push(0).emit(I::VecPush { reg: R(1) });
    b.emit(I::Next {});
    b.emit(I::Load { reg: R(3) }).emit(I::Bsmx { reg: R(0) });
    for i in 4..=255u8 {
        b.emit(I::Load { reg: R(3) }).emit(I::Bsmx { reg: R(i) });
    }
    for s in 0..256u16 {
        b.emit_push(i64::from(s)).emit(I::Output { reg: R(4) });
    }
    b.emit_push(7).emit(I::Stow { reg: R(2) });
    for _ in 0..depth {
        b.emit_push(0)
            .emit(I::Load { reg: R(3) })
            .emit(I::Iter { reg: R(1) });
    }
    b.emit_push(0).emit_push(1);
    b.emit(I::Equality {
        model: R(2),
        indices: R(1),
        coeffs: R(1),
    });
    for _ in 0..depth {
        b.emit(I::Next {});
    }
    b.emit(I::Halt {});
    encode(b)
}

/// Charges a holder program makes: 252 registers and 256 OUTPUT clones of
/// `per` bytes each, plus a margin for the INPUT register.
fn holders_charge(per: u64) -> u64 {
    (252 + 256) * per + 64
}

/// The holder runs at emulated cap `l`, each with its designed outcome. Every
/// run is bounded by its real charge (the rates above) and built through
/// [`Chain::legal_call`], which checks natively that a chain whose real cap is
/// `l` makes the same run; the emulating chain's 1 MiB cap would otherwise
/// admit runs no chain at `l` can make (claude's R8 on A6c/A6d: the 20.88·L'
/// that holder iii measured at 256 KiB was such an artifact).
/// - `holders_samples` in every order its 508 holders fit, with
///   e = chunk/16 + 1, 256 output slots; succeeds;
/// - `holders_models` in the 256-B order, 256 output slots; succeeds;
/// - `stack_loop` once (holder ii); succeeds;
/// - `typeconf_equality` and `typeconf_atleastw` (holder iii) in every order
///   an e = chunk/16 + 1 vec fits at 16 B an element, faulting after their
///   clones ([`post_clone_code`]);
/// - `holders_typeconf_equality` and `holders_typeconf_atleastw` in every
///   order the sample holders and the vec fit: holder iii's clones made while
///   every holder is live (A6 shape 2); faulting after the clones;
/// - `atleastw_slack` in every order until the run stops succeeding at a real
///   cap of `l`, so it reaches the success path's uncharged temporaries
///   (A6 shape 1, kimi's R8 on A6c/A6d); succeeds;
/// - `mix_clone` in every order c <= l/2: samples, a vec and its two clones in
///   one run (claude's R8 on A6c/A6d); faulting after the clones;
/// - `models_mix` once: the model holders and an anchored model spending the
///   rest of `l` in one run (claude's R8 on A6c/A6d); succeeds;
/// - `holders_iter` in every order its holders fit: every holder slot a run
///   has (253 samples, 256 OUTPUT clones, `MaxLoopDepth` ITER frames) plus a
///   vec and its two clones in one order (claude's follow-up R8); faulting
///   after the clones.
///
/// Clone holders go through [`Chain::legal_clone_call`] instead, which
/// compares the run up to the clone opcode across caps.
fn holder_sweep(chain: &Chain, l: u64, ms: u64) -> Vec<(String, RuntimeCall, Option<u8>)> {
    let mut out = Vec::new();
    let ok = |out: &mut Vec<(String, RuntimeCall, Option<u8>)>,
              name: String,
              (call, r): (RuntimeCall, Result<(), xqvm::Error>)| {
        assert!(r.is_ok(), "{name}: designed to succeed, got {r:?}");
        out.push((name, call, None));
    };
    // Every order the 508 holders fit: e = chunk/16 + 1 elements, just over
    // half the chunk, charged 8 B each (a sample and its OUTPUT clone).
    let mut chunk = 8u64;
    loop {
        let e = chunk / 16 + 1;
        if holders_charge(VARIABLE_BYTES * e) > l {
            break;
        }
        let steps = ((600 * e + 20_000).saturating_mul(5) / 4).min(ms);
        ok(
            &mut out,
            format!("holders_samples e={e}"),
            chain.legal_call("holders_samples", &[e as i64], steps, 256, l),
        );
        chunk <<= 1;
    }
    if holders_charge(96) <= l {
        ok(
            &mut out,
            "holders_models".into(),
            chain.legal_call("holders_models", &[], 20_000, 256, l),
        );
    }
    ok(
        &mut out,
        "stack_loop".into(),
        chain.legal_call("stack_loop", &[], (8 * VM_STACK_LIMIT as u64).min(ms), 0, l),
    );
    // Holder iii, each order a vec of e = chunk/16 + 1 elements lands in: the
    // INPUT and STOW integers plus the vec at 16 B an element must fit; its
    // two clones are uncharged.
    let mut chunk = 16u64;
    loop {
        let e = chunk / 16 + 1;
        if VEC_ELEMENT_BYTES * e + 2 * VARIABLE_BYTES > l {
            break;
        }
        for (name, atleastw) in [("typeconf_equality", false), ("typeconf_atleastw", true)] {
            let probe = native_run(
                &prog_typeconf(atleastw),
                &[e as i64],
                ms,
                0,
                max_vm_memory(),
            );
            let steps = step_budget(probe.steps).min(ms);
            let (call, code) = chain.legal_clone_call(name, &[e as i64], steps, 0, l, 3);
            out.push((format!("{name} e={e}"), call, Some(code)));
        }
        chunk <<= 1;
    }
    // A6 shape 2: holder iii while the sample holders are live, in every order
    // they and the vec fit: INPUT and STOW integers, 251 samples and 256
    // OUTPUT clones of e elements, and the vec.
    let mut chunk = 8u64;
    loop {
        let e = chunk / 16 + 1;
        if (251 + 256) * VARIABLE_BYTES * e + VEC_ELEMENT_BYTES * e + 2 * VARIABLE_BYTES > l {
            break;
        }
        for (name, atleastw) in [
            ("holders_typeconf_equality", false),
            ("holders_typeconf_atleastw", true),
        ] {
            let bytes = prog_holders_typeconf(atleastw);
            let probe = native_run(&bytes, &[e as i64], ms, 256, max_vm_memory());
            let steps = step_budget(probe.steps).min(ms);
            let (call, code) = chain.legal_clone_call(name, &[e as i64], steps, 256, l, 2);
            out.push((format!("{name} e={e}"), call, Some(code)));
        }
        chunk <<= 1;
    }
    // A6 shape 1: ATLEASTW's success path, in every order its vecs reach
    // while the run still succeeds at a real cap of `l` (the quadratic
    // expansion charge ends it at a few hundred elements).
    let mut slack_runs = 0usize;
    let mut chunk = 16u64;
    loop {
        let e = chunk / 16 + 1;
        let probe = native_run(&prog_atleastw_slack(), &[e as i64], ms, 0, l);
        if probe.result.is_err() {
            println!(
                "atleastw_slack at l={l}: e={e} no longer succeeds ({:?}); sweep ends",
                probe.result
            );
            break;
        }
        // The run took the slack path (claude's R8 on A6c..A6f): weights of
        // one and k = 1 leave an excess of e - 1, so bit_length(e - 1) slack
        // variables, and `memory_used` must be exactly the INPUT integer, the
        // two vecs, the size-1 model's variable, the slack variables and the
        // expansion charge over N = e + slacks terms (24·N² + 8·N).
        let slacks = u64::from(u64::BITS - (e - 1).leading_zeros());
        let n_terms = e + slacks;
        let expected_used = VARIABLE_BYTES
            + 2 * VEC_ELEMENT_BYTES * e
            + VARIABLE_BYTES
            + VARIABLE_BYTES * slacks
            + 24 * n_terms * n_terms
            + 8 * n_terms;
        assert!(
            slacks >= 1 && probe.used == expected_used,
            "atleastw_slack e={e}: {slacks} slacks, memory_used {} (expected {expected_used})",
            probe.used
        );
        let steps = step_budget(probe.steps).min(ms);
        let (call, r) = chain.legal_call("atleastw_slack", &[e as i64], steps, 0, l);
        assert!(
            r.is_ok(),
            "atleastw_slack e={e}: designed to succeed, got {r:?}"
        );
        out.push((format!("atleastw_slack e={e}"), call, None));
        slack_runs += 1;
        chunk <<= 1;
    }
    assert!(
        slack_runs >= 4,
        "atleastw_slack at l={l}: only {slack_runs} orders reach the success path (want >= 4)"
    );
    // mix_clone, each order c <= l/2: k samples of n = c/16 + 1 elements and a
    // vec of e = c/16 + 1, after the three INPUT and one STOW integers.
    let mut chunk = 32u64;
    loop {
        let n = chunk / 16 + 1;
        let e = chunk / 16 + 1;
        let fixed = 4 * VARIABLE_BYTES + VEC_ELEMENT_BYTES * e;
        if fixed > l {
            break;
        }
        let k = ((l - fixed) / (VARIABLE_BYTES * n)).min(251);
        let calldata = [n as i64, k as i64, e as i64];
        let probe = native_run(&prog_mix_clone(), &calldata, ms, 0, max_vm_memory());
        let steps = step_budget(probe.steps).min(ms);
        let (call, code) = chain.legal_clone_call("mix_clone", &calldata, steps, 0, l, 3);
        out.push((
            format!("mix_clone c={chunk} n={n} k={k} e={e}"),
            call,
            Some(code),
        ));
        chunk <<= 1;
    }
    // holders_iter, each order: the INPUT and STOW integers, the vec at 16 B
    // an element, 253 samples, 256 OUTPUT clones and `MaxLoopDepth` ITER
    // frames of e elements at 8 B each.
    let depth = u64::from(<quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get());
    let mut chunk = 16u64;
    loop {
        let e = chunk / 16 + 1;
        let charge =
            2 * VARIABLE_BYTES + VEC_ELEMENT_BYTES * e + (253 + 256 + depth) * VARIABLE_BYTES * e;
        if charge > l {
            break;
        }
        let probe = native_run(&prog_holders_iter(), &[e as i64], ms, 256, max_vm_memory());
        let steps = step_budget(probe.steps).min(ms);
        let (call, code) = chain.legal_clone_call("holders_iter", &[e as i64], steps, 256, l, 2);
        out.push((format!("holders_iter c={chunk} e={e}"), call, Some(code)));
        chunk <<= 1;
    }
    // models_mix once: the 508 model holders (96 B each), the INPUT integer
    // and an anchored model of n variables at 40 B each (a variable and a
    // linear entry), within `l`.
    let holders = 508 * 96 + VARIABLE_BYTES;
    if holders + 256 < l {
        let n = (l - holders - 256) / 40;
        let probe = native_run(&prog_models_mix(), &[n as i64], ms, 256, l);
        let steps = step_budget(probe.steps).min(ms);
        let (call, r) = chain.legal_call("models_mix", &[n as i64], steps, 256, l);
        assert!(
            r.is_ok(),
            "models_mix n={n}: designed to succeed, got {r:?}"
        );
        out.push((format!("models_mix n={n}"), call, None));
    } else {
        panic!("models_mix: its 508 holders ({holders} B) do not fit under l = {l} B");
    }
    out
}

fn prog_halt() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    b.emit(I::Halt {});
    encode(b)
}

fn outcome(r: &ApplyExtrinsicResult) -> String {
    match r {
        Ok(Ok(())) => "ok".into(),
        Ok(Err(e)) => format!("{e:?}").chars().take(80).collect(),
        Err(e) => format!("INVALID {e:?}"),
    }
}

/// The xqvm pallet's index in the runtime, and the designed post-allocation
/// faults (pallet error indices; `designed_fault_codes_are_the_pallet_errors`
/// pins them to the enum). A `samples` run with energy=1 and n > 1 computes
/// ENERGY of a size-1 model against an n-variable sample and faults on the
/// size mismatch; a `vec` run faults in EQUALITY over the built vectors.
const XQVM_PALLET_INDEX: u8 = 8;
const SAMPLES_ENERGY_FAULT: u8 = 30;
/// (`VmMemoryLimitExceeded`; also holder iii's fault at larger vecs.)
const VEC_EQUALITY_FAULT: u8 = 19;
/// Holder (iii)'s designed fault: the model register holds an integer.
const REGISTER_TYPE_FAULT: u8 = 21;
/// Holder (i)'s designed fault: the 8,193rd value.
const STACK_OVERFLOW_FAULT: u8 = 16;

/// `r` is the designed outcome: success (`None`) or exactly the xqvm module
/// error `Some(code)`. Any other result -- an early fault that never built
/// what the run is there to measure -- panics, naming `what`.
fn assert_designed_outcome(r: &ApplyExtrinsicResult, expect: Option<u8>, what: &str) {
    match expect {
        None => assert!(
            matches!(r, Ok(Ok(()))),
            "{what}: expected success, got {}",
            outcome(r)
        ),
        Some(code) => assert!(
            matches!(
                r,
                Ok(Err(sp_runtime::DispatchError::Module(m)))
                    if m.index == XQVM_PALLET_INDEX && m.error[0] == code
            ),
            "{what}: expected xqvm module error {code}, got {}",
            outcome(r)
        ),
    }
}

/// The designed outcome of an `order_sweep` run, by its label: `samples/o0`
/// (n = 1, the size-1 model matches) succeeds; every other `samples/o*` faults
/// on the size mismatch; `vec+equality` faults in EQUALITY by design; the rest
/// succeed.
fn sweep_designed_outcome(name: &str) -> Option<u8> {
    match name {
        "samples/o0" => None,
        n if n.starts_with("samples/") => Some(SAMPLES_ENERGY_FAULT),
        "vec+equality" => Some(VEC_EQUALITY_FAULT),
        _ => None,
    }
}

#[test]
fn designed_fault_codes_are_the_pallet_errors() {
    use frame_support::traits::PalletInfoAccess;
    assert_eq!(
        pallet_xqvm::Pallet::<Runtime>::index(),
        usize::from(XQVM_PALLET_INDEX)
    );
    assert_eq!(
        pallet_xqvm::Error::<Runtime>::VmSizeMismatch.encode(),
        [SAMPLES_ENERGY_FAULT]
    );
    assert_eq!(
        pallet_xqvm::Error::<Runtime>::VmMemoryLimitExceeded.encode(),
        [VEC_EQUALITY_FAULT]
    );
    assert_eq!(
        pallet_xqvm::Error::<Runtime>::VmRegisterType.encode(),
        [REGISTER_TYPE_FAULT]
    );
    assert_eq!(
        pallet_xqvm::Error::<Runtime>::VmStackOverflow.encode(),
        [STACK_OVERFLOW_FAULT]
    );
}

// ---------------------------------------------------------------------------
// Chain harness
// ---------------------------------------------------------------------------

/// `sc-basic-authorship`'s `MAX_SKIPPED_TRANSACTIONS`.
const MAX_SKIPPED_TRANSACTIONS: usize = 8;

struct Chain {
    executor: Executor,
    genesis: Storage,
    genesis_hash: H256,
    attacker: HybridPair,
    programs: Vec<(&'static str, H256, Vec<u8>)>,
}

impl Chain {
    fn new() -> Self {
        let attacker = HybridPair::from_seed_slice(&[0xA7; 32]).expect("valid seed");
        let attacker_id = account_id_from_public(&attacker.public());
        let mut genesis: Storage = RuntimeGenesisConfig {
            balances: pallet_balances::GenesisConfig {
                balances: vec![(attacker_id, 100_000 * UNIT)],
                ..Default::default()
            },
            ..Default::default()
        }
        .build_storage()
        .expect("genesis builds");
        let mut programs = Vec::new();
        for (name, bytes) in [
            ("auditor", prog_auditor()),
            ("samples", prog_samples()),
            ("vec", prog_vec()),
            ("linear", prog_linear()),
            ("linear_anchored", prog_linear_anchored()),
            ("quad", prog_quad()),
            ("halt", prog_halt()),
            ("fixed_worst", prog_fixed_worst()),
            ("fixed_worst_jumps", prog_fixed_worst_jumps()),
            ("holders_samples", prog_holders_samples()),
            ("holders_models", prog_holders_models()),
            ("fault_at_peak", prog_fault_at_peak()),
            ("stack_loop", prog_stack_loop()),
            ("typeconf_equality", prog_typeconf(false)),
            ("typeconf_atleastw", prog_typeconf(true)),
            ("atleastw_slack", prog_atleastw_slack()),
            ("holders_typeconf_equality", prog_holders_typeconf(false)),
            ("holders_typeconf_atleastw", prog_holders_typeconf(true)),
            ("mix_clone", prog_mix_clone()),
            ("models_mix", prog_models_mix()),
            ("holders_iter", prog_holders_iter()),
        ] {
            let program = xqvm::Program::decode(&bytes).expect("decodes");
            assert_eq!(program.encode(), bytes, "{name}: canonical");
            let verdict = pallet_xqvm::structural_gate(&program);
            println!(
                "program {name}: {} bytes, structural gate {}",
                bytes.len(),
                if verdict.is_ok() {
                    "accepts"
                } else {
                    "REJECTS"
                }
            );
            let hash = BlakeTwo256::hash(&bytes);
            let key = pallet_xqvm::Programs::<Runtime>::hashed_key_for(hash);
            genesis.top.insert(key, bytes.encode());
            programs.push((name, hash, bytes));
        }
        let genesis_hash = TestExternalities::new(genesis.clone())
            .execute_with(|| frame_system::BlockHash::<Runtime>::get(0));
        Self {
            executor: Executor::builder().build(),
            genesis,
            genesis_hash,
            attacker,
            programs,
        }
    }

    fn program(&self, name: &str) -> H256 {
        self.programs
            .iter()
            .find(|(n, _, _)| *n == name)
            .expect("known")
            .1
    }

    /// The `execute` call for a run sized for an emulated cap `l`, after
    /// checking natively that a chain whose REAL cap is `l` makes the same run
    /// (claude's R8 on A6c/A6d). The emulating chain's cap is the compiled
    /// `MaxVmMemory`, so a run that fits only under it -- one that would fault
    /// earlier at a real cap of `l` -- would put into the block what no chain
    /// at `l` can, and Σ at `l` would count it. Both native runs use the call's
    /// step limit, output slots and calldata; they must be the same run
    /// ([`NativeRun::same_as`]) holding at most `l`. Returns the call and the
    /// native result, for the caller's designed-outcome check.
    fn legal_call(
        &self,
        name: &str,
        calldata: &[i64],
        step_limit: u64,
        output_slots: u32,
        l: u64,
    ) -> (RuntimeCall, Result<(), xqvm::Error>) {
        let bytes = &self
            .programs
            .iter()
            .find(|(n, _, _)| *n == name)
            .expect("known")
            .2;
        let chain_cap = native_run(bytes, calldata, step_limit, output_slots, max_vm_memory());
        let real_cap = native_run(bytes, calldata, step_limit, output_slots, l);
        assert!(
            chain_cap.used <= l && chain_cap.same_as(&real_cap),
            "{name} {calldata:?} is not a run a chain at a real cap of {l} B makes: \
             at the chain cap {chain_cap:?}, at {l} B {real_cap:?}"
        );
        (
            self.execute_with_outputs(name, calldata, step_limit, output_slots),
            chain_cap.result,
        )
    }

    /// [`Chain::legal_call`] for a clone holder: a run whose last instruction
    /// is the type-confused `EQUALITY`/`ATLEASTW` on `model`, made to fault
    /// after its two clones. The whole run cannot be compared across caps --
    /// the opcode's growth and expansion charges come after the clones and
    /// allocate nothing, but one that fails at a real cap of `l` can pass at
    /// the emulating 1 MiB cap and fault on the step charge or the model type
    /// instead -- so the check is split at the opcode:
    /// - the run up to it ([`truncate_at`]) is the same run at both caps,
    ///   holding at most `l`, so the heap the clones are made into is one a
    ///   chain at `l` reaches;
    /// - the whole run faults after the clones at both caps
    ///   ([`post_clone_code`], non-masking).
    ///
    /// Returns the call and its designed fault code at the chain cap.
    fn legal_clone_call(
        &self,
        name: &str,
        calldata: &[i64],
        step_limit: u64,
        output_slots: u32,
        l: u64,
        model: u8,
    ) -> (RuntimeCall, u8) {
        let bytes = &self
            .programs
            .iter()
            .find(|(n, _, _)| *n == name)
            .expect("known")
            .2;
        let op = clone_op_pos(bytes);
        let prefix = truncate_at(bytes, op);
        let pre_chain = native_run(&prefix, calldata, step_limit, output_slots, max_vm_memory());
        let pre_real = native_run(&prefix, calldata, step_limit, output_slots, l);
        assert!(
            pre_chain.result.is_ok() && pre_chain.used <= l && pre_chain.same_as(&pre_real),
            "{name} {calldata:?}: the run up to the clones at pc {op} is not one a chain at a \
             real cap of {l} B makes: at the chain cap {pre_chain:?}, at {l} B {pre_real:?}"
        );
        let real = native_run(bytes, calldata, step_limit, output_slots, l);
        assert!(
            is_post_clone(&real.result, model, op),
            "{name} {calldata:?} at a real cap of {l} B: {:?} is not a fault after the clones \
             at pc {op}",
            real.result
        );
        let full = native_run(bytes, calldata, step_limit, output_slots, max_vm_memory());
        let code = post_clone_code(&full.result, model, op, &format!("{name} {calldata:?}"));
        (
            self.execute_with_outputs(name, calldata, step_limit, output_slots),
            code,
        )
    }

    /// `call` with its program swapped for `halt`, keeping its calldata,
    /// output slots and step limit, so the extrinsic encodes to the same
    /// length. Calls other than `execute` are kept as they are.
    fn as_halt(&self, call: &RuntimeCall) -> RuntimeCall {
        match call {
            RuntimeCall::Xqvm(pallet_xqvm::Call::execute {
                calldata,
                output_slots,
                step_limit,
                ..
            }) => pallet_xqvm::Call::<Runtime>::execute {
                program_hash: self.program("halt"),
                calldata: calldata.clone(),
                output_slots: *output_slots,
                step_limit: *step_limit,
            }
            .into(),
            other => other.clone(),
        }
    }

    /// The same-count control for a block of `calls`: the same extrinsics,
    /// each `execute` turned into a `halt` run ([`Chain::as_halt`]). Returns
    /// its importer high-water net of the empty block (`empty_import`): the
    /// block-level share -- decoding, per-extrinsic overhead and what the block
    /// allocates when it finalizes, including the allocator doubling the 85th
    /// extrinsic crosses at 256 KiB -- that a Σ measured against an empty
    /// block also contains.
    fn control_import(&self, calls: &[RuntimeCall], empty_import: u32) -> u32 {
        let control: Vec<RuntimeCall> = calls.iter().map(|c| self.as_halt(c)).collect();
        let supplied = control.len();
        let (block, applied) = self.author_fast(control);
        assert_eq!(
            applied.len(),
            supplied,
            "the builder includes every control run"
        );
        let (imported, heap) = self.import(&block);
        assert!(imported.is_ok(), "the control block imports");
        heap.saturating_sub(empty_import)
    }

    fn state(&self) -> TestExternalities {
        TestExternalities::new(self.genesis.clone())
    }

    fn call(&self, state: &mut TestExternalities, method: &str, data: &[u8]) -> Vec<u8> {
        let wasm = WASM_BINARY.expect("wasm");
        let fetcher = WrappedRuntimeCode(wasm.into());
        let code = RuntimeCode {
            code_fetcher: &fetcher,
            heap_pages: None,
            hash: sp_core::blake2_256(wasm).to_vec(),
        };
        let (result, _) =
            self.executor
                .call(&mut state.ext(), &code, method, data, CallContext::Onchain);
        result.unwrap_or_else(|e| panic!("{method}: {e}"))
    }

    /// One fresh-heap runtime call; returns (result, address_space_used).
    fn measured(
        &self,
        state: &mut TestExternalities,
        method: &str,
        data: &[u8],
    ) -> (Result<Vec<u8>, String>, u32) {
        let wasm = WASM_BINARY.expect("wasm");
        let blob = RuntimeBlob::uncompress_if_needed(wasm).expect("blob");
        let (result, stats) = self.executor.uncached_call_with_allocation_stats(
            blob,
            &mut state.ext(),
            false,
            method,
            data,
        );
        (
            result.map_err(|e| e.to_string()),
            stats.map(|s| s.address_space_used).unwrap_or_default(),
        )
    }

    fn sign(&self, call: RuntimeCall, nonce: Nonce) -> UncheckedExtrinsic {
        let mut state = self.state();
        self.sign_in(&mut state, call, nonce)
    }

    /// Sign inside an existing externalities (the implicit reads the genesis
    /// block hash from storage). Bulk callers reuse one state to avoid
    /// rebuilding genesis per extrinsic.
    fn sign_in(
        &self,
        state: &mut TestExternalities,
        call: RuntimeCall,
        nonce: Nonce,
    ) -> UncheckedExtrinsic {
        let who = account_id_from_public(&self.attacker.public());
        state.execute_with(|| {
            let ext = native_tx_extension(generic::Era::Immortal, nonce, 0);
            let payload = SignedPayload::new(call.clone(), ext.clone()).expect("payload");
            let sig = payload.using_encoded(|b| HybridTxSignature::sign(&self.attacker, b));
            generic::UncheckedExtrinsic::new_signed(call, Address::Id(who), sig, ext).into()
        })
    }

    fn header(&self, slot: u64) -> Header {
        let pre = PreDigest::SecondaryPlain(SecondaryPlainPreDigest {
            authority_index: 0,
            slot: Slot::from(slot),
        });
        Header::new(
            1,
            H256::zero(),
            H256::zero(),
            self.genesis_hash,
            Digest {
                logs: vec![DigestItem::PreRuntime(BABE_ENGINE_ID, pre.encode())],
            },
        )
    }

    fn timestamp(slot: u64) -> UncheckedExtrinsic {
        generic::UncheckedExtrinsic::<Address, RuntimeCall, Signature, TxExtension>::new_bare(
            pallet_timestamp::Call::<Runtime>::set {
                now: SLOT_DURATION * slot,
            }
            .into(),
        )
        .into()
    }

    /// Apply one extrinsic the way `sc-block-builder` does
    /// (`client/block-builder/src/lib.rs`, `push`): inside a storage
    /// transaction that is rolled back when the result is
    /// `Err(TransactionValidityError)` and committed otherwise. Without the
    /// rollback, `frame-executive` has already run `note_extrinsic` for the
    /// rejected extrinsic (`frame/executive/src/lib.rs`, before `apply`), so its
    /// `ExtrinsicData` would leak into the authored state root and the block
    /// could not import. Returns the apply result and, when `measure` is set,
    /// the fresh-heap high-water of the apply.
    fn apply_in_transaction(
        &self,
        state: &mut TestExternalities,
        xt: &UncheckedExtrinsic,
        measure: bool,
    ) -> (ApplyExtrinsicResult, u32) {
        state.execute_with(sp_io::storage::start_transaction);
        let (out, heap) = if measure {
            self.measured(state, "BlockBuilder_apply_extrinsic", &xt.encode())
        } else {
            (
                Ok(self.call(state, "BlockBuilder_apply_extrinsic", &xt.encode())),
                0,
            )
        };
        let out = out.unwrap_or_else(|e| panic!("author traps on apply: {e}"));
        let result = ApplyExtrinsicResult::decode(&mut &out[..]).expect("decodes");
        state.execute_with(|| {
            if result.is_err() {
                sp_io::storage::rollback_transaction();
            } else {
                sp_io::storage::commit_transaction();
            }
        });
        (result, heap)
    }

    /// Build block 1 the way a real block builder does, then `finalize_block`
    /// so the header carries the true state/extrinsics roots.
    ///
    /// Each apply runs in a rolled-back-on-reject storage transaction (see
    /// [`Self::apply_in_transaction`]). An extrinsic whose apply returns
    /// `Err(TransactionValidityError)` (e.g. `ExhaustsResources`) is dropped and
    /// the author moves on, as `sc-basic-authorship` does: it allows
    /// `MAX_SKIPPED_TRANSACTIONS` (8) rejections in total -- the counter is not
    /// reset by a later inclusion -- and ends the block at the next one (its
    /// soft-deadline extension is not modelled; this author is deterministic).
    /// Nonces are assigned at apply time from the count of included signed
    /// extrinsics, as a pool hands the builder the next ready transaction, so a
    /// dropped call does not turn every later call into a future nonce. A
    /// dispatch error `Ok(Err(_))` is a valid, paid extrinsic and is kept.
    ///
    /// Skip model, versus the real author: this harness counts EVERY outer
    /// `Err` as a skip and stops at the eighth. `sc-basic-authorship` (at
    /// sdk 8fcc7e2) counts two kinds of rejection towards its limit of 8 --
    /// a block-size-limit hit (basic_authorship.rs:426-452) and
    /// `ExhaustsResources` (:462-481) -- and drops any other invalid
    /// transaction without counting it (:484); past the limit it keeps trying
    /// until its soft deadline. A case that puts smaller calls after larger
    /// ones (as `g` does with its fillers) can therefore end up with a less
    /// full block than a real author would build, when early rejections are
    /// of an uncounted kind or the soft deadline would have allowed more tries.
    ///
    /// Returns the block, the included apply results in inclusion order, and
    /// the number of dropped calls (skipped plus never tried after the limit).
    fn author_with(
        &self,
        calls: Vec<RuntimeCall>,
        measure: bool,
    ) -> (Block, Vec<(ApplyExtrinsicResult, u32)>, usize) {
        let supplied = calls.len();
        let mut sign_state = self.state();
        let mut state = self.state();
        self.call(
            &mut state,
            "Core_initialize_block",
            &self.header(1).encode(),
        );
        let mut xts = vec![Self::timestamp(1)];
        let (ts, _) = self.apply_in_transaction(&mut state, &xts[0], false);
        assert_eq!(ts, Ok(Ok(())), "timestamp");
        let mut applied = Vec::new();
        let mut skipped = 0usize;
        for call in calls {
            let xt = self.sign_in(&mut sign_state, call, applied.len() as Nonce);
            match self.apply_in_transaction(&mut state, &xt, measure) {
                (Err(_), _) => {
                    if skipped == MAX_SKIPPED_TRANSACTIONS {
                        break;
                    }
                    skipped += 1;
                }
                (ok, heap) => {
                    applied.push((ok, heap));
                    xts.push(xt);
                }
            }
        }
        drop(sign_state);
        // Finalize on a fresh instance so nothing a pooled instance retained
        // from the applies can influence the authored header.
        let (out, _) = self.measured(&mut state, "BlockBuilder_finalize_block", &[]);
        let header = Header::decode(&mut &out.expect("finalize")[..]).expect("header");
        let dropped = supplied - applied.len();
        (
            Block {
                header,
                extrinsics: xts,
            },
            applied,
            dropped,
        )
    }

    /// Author with a per-apply fresh-heap measurement for every included
    /// extrinsic. Returns the block and (apply result, apply heap) per included
    /// extrinsic; the dropped count is `calls.len() - applied.len()`.
    fn author(&self, calls: Vec<RuntimeCall>) -> (Block, Vec<(ApplyExtrinsicResult, u32)>) {
        let (block, applied, _) = self.author_with(calls, true);
        (block, applied)
    }

    /// Like `author`, but applies through the cached executor path (no
    /// per-apply reinstantiation or heap measurement). Used by the combined
    /// test, which authors hundreds of extrinsics and only needs the import
    /// high-water.
    fn author_fast(&self, calls: Vec<RuntimeCall>) -> (Block, Vec<ApplyExtrinsicResult>) {
        let (block, applied, _) = self.author_with(calls, false);
        (block, applied.into_iter().map(|(r, _)| r).collect())
    }

    fn import(&self, block: &Block) -> (Result<Vec<u8>, String>, u32) {
        self.measured(&mut self.state(), "Core_execute_block", &block.encode())
    }

    fn execute(&self, program: &str, calldata: &[i64], step_limit: u64) -> RuntimeCall {
        self.execute_with_outputs(program, calldata, step_limit, 0)
    }

    fn execute_with_outputs(
        &self,
        program: &str,
        calldata: &[i64],
        step_limit: u64,
        output_slots: u32,
    ) -> RuntimeCall {
        pallet_xqvm::Call::<Runtime>::execute {
            program_hash: self.program(program),
            calldata: calldata.to_vec().try_into().expect("calldata bound"),
            output_slots,
            step_limit,
        }
        .into()
    }
}

/// Harness regression (W1-002 R8 F1): a builder-rejected extrinsic must leave
/// no trace in the authored state. The rejected call is a remark one byte over
/// the Normal-class block length, so `CheckWeight` rejects it with
/// `ExhaustsResources` -- after `frame-executive` has already run
/// `note_extrinsic` for it, writing `ExtrinsicData` at the current extrinsic
/// index. A rejection does not advance that index, so a later included
/// extrinsic overwrites the entry; a rejection that is the LAST apply before
/// `finalize_block` is not overwritten, and only the per-apply rollback keeps
/// it out of the state root. Both positions are checked.
#[test]
fn author_rolls_back_a_rejected_extrinsic_and_the_block_imports() {
    let chain = Chain::new();
    let max_len = chain.state().execute_with(|| {
        *<Runtime as frame_system::Config>::BlockLength::get()
            .max
            .get(DispatchClass::Normal)
    }) as usize;
    let remark = |p: usize| -> RuntimeCall {
        frame_system::Call::<Runtime>::remark {
            remark: vec![0u8; p],
        }
        .into()
    };
    for (position, calls, included) in [
        (
            "middle",
            vec![remark(16), remark(max_len + 1), remark(16)],
            2,
        ),
        ("last", vec![remark(16), remark(max_len + 1)], 1),
    ] {
        let (block, applied, dropped) = chain.author_with(calls, false);
        assert_eq!(
            dropped, 1,
            "[{position}] the oversized remark must be dropped"
        );
        assert_eq!(applied.len(), included, "[{position}]");
        assert!(applied.iter().all(|(r, _)| matches!(r, Ok(Ok(())))));
        assert_eq!(block.extrinsics.len(), 1 + included, "[{position}]");
        let (imported, _) = chain.import(&block);
        assert!(
            imported.is_ok(),
            "[{position}] a block authored past a rejected extrinsic must import: {imported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. PoC at base: direct, faulting, and batch-wrapped.
// ---------------------------------------------------------------------------

#[test]
fn a_poc_direct_faulting_and_batch_wrappers() {
    let chain = Chain::new();
    let ms = max_steps();
    println!("MaxStepLimit = {ms}, MaxVmMemory = {} B", max_vm_memory());
    let sizes = [8_193i64, 16_385, 32_769, 65_537, 1_048_577];
    let step_limit = 2_100_000u64.min(ms);
    let direct: Vec<RuntimeCall> = sizes
        .iter()
        .map(|&s| chain.execute("auditor", &[s], step_limit))
        .collect();

    let (empty, _) = chain.author(vec![]);
    let (_, empty_heap) = chain.import(&empty);
    let (halt_block, halt_applied) = chain.author(vec![chain.execute("halt", &[], 10)]);
    let (_, halt_import) = chain.import(&halt_block);
    println!(
        "baseline: empty-block import {:.2} MiB; HALT execute author {:.2} MiB, import {:.2} MiB",
        mib(empty_heap),
        mib(halt_applied[0].1),
        mib(halt_import)
    );

    let report = |label: &str, calls: Vec<RuntimeCall>| {
        let (block, applied) = chain.author(calls);
        for (r, heap) in &applied {
            println!(
                "  [{label}] author xt {:6.2} MiB  {}",
                mib(*heap),
                outcome(r)
            );
        }
        let (imported, heap) = chain.import(&block);
        println!(
            "  [{label}] IMPORT {:6.2} MiB  {}",
            mib(heap),
            match &imported {
                Ok(_) => "imported".to_string(),
                Err(e) => e
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(120)
                    .collect(),
            }
        );
        imported.is_ok()
    };

    println!("== direct x5 (calls 2-5 fault VmMemoryLimitExceeded)");
    let ok5 = report("direct5", direct.clone());
    println!("== direct x4");
    let ok4 = report("direct4", direct[..4].to_vec());

    let wrap = |kind: &str, call: RuntimeCall| -> RuntimeCall {
        let calls = vec![call];
        match kind {
            "batch" => pallet_utility::Call::<Runtime>::batch { calls }.into(),
            "batch_all" => pallet_utility::Call::<Runtime>::batch_all { calls }.into(),
            _ => pallet_utility::Call::<Runtime>::force_batch { calls }.into(),
        }
    };
    let mut wrapped = Vec::new();
    for kind in ["batch", "batch_all", "force_batch"] {
        println!("== 5 calls each wrapped alone in Utility::{kind}");
        let calls = direct.iter().cloned().map(|c| wrap(kind, c)).collect();
        wrapped.push((kind, report(kind, calls)));
    }
    println!("SUMMARY poc: direct5_imports={ok5} direct4_imports={ok4} wrapped={wrapped:?}");
    // W1-002 regression + mutation control. At the shipped per-run cap
    // (MaxVmMemory = 1 MiB) the whole-block VM growth stays under the 128 MiB
    // heap, so the auditor's attack block -- which trapped every importer at the
    // old 16 MiB cap -- now imports, directly and under every batch wrapper.
    // Reverting MaxVmMemory to 16 MiB fails this test: the block traps on import.
    assert!(
        max_vm_memory() <= SHIPPED_CAP,
        "this regression assumes the shipped cap; MaxVmMemory is {} B",
        max_vm_memory()
    );
    assert!(
        ok5,
        "W1-002 fix: the 5-call attack block must import at the shipped cap"
    );
    assert!(ok4, "four-call block must import");
    for (kind, ok) in &wrapped {
        assert!(
            ok,
            "W1-002 fix: {kind}-wrapped attack block must import at the shipped cap"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Per-run footprint by allocation family, at emulated and compiled caps.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn b_per_run_footprint_by_family() {
    let chain = Chain::new();
    let ms = max_steps();
    // The designed post-allocation faults: SAMPLES_ENERGY_FAULT and
    // VEC_EQUALITY_FAULT (module level, pinned to the pallet's enum).
    let (halt_block, halt_applied) = chain.author(vec![chain.execute("halt", &[], 10)]);
    let base_author = halt_applied[0].1;
    let (_, base_import) = chain.import(&halt_block);
    println!(
        "HALT baseline: author apply {:.3} MiB, import {:.3} MiB",
        mib(base_author),
        mib(base_import)
    );
    // importer_net is the binding figure: the whole run executes in the
    // block's single heap. O-A2 is sized against it (adjustment 3).
    println!("family,L_KiB,params,charged_B,author_MiB,author_net_MiB,import_MiB,import_net_MiB,import_ratio_to_L,outcome");
    // Emulated caps stay at or below the compiled 1 MiB cap, with headroom for the
    // k ~ 2 clone overhead (matching `c_block_order_sweep_at_emulated_caps`).
    // The old 16 MiB row is dropped: it exceeds the current cap, so it would
    // fault at `VmMemoryLimitExceeded` before reaching its intended allocation
    // and the measured footprint would be meaningless.
    for l in [MIB / 4, MIB / 2, MIB] {
        // Fifth tuple element: the designed outcome (None = success; Some(code)
        // = the exact post-allocation xqvm fault). energy=0 samples, `linear`
        // and `quad` complete; energy=1 samples and `vec` fault after allocating.
        let mut cases: Vec<(String, &str, Vec<i64>, u64, Option<u8>)> = Vec::new();
        // The full-L cases leave room for the program's own `INPUT` register
        // charges (8 B each), so a case at the compiled cap (L' = 1 MiB) still
        // fits it. The allocation stays in the same allocator order.
        const RUN_OVERHEAD: u64 = 64;
        let fit = l - RUN_OVERHEAD;
        for &(n, energy) in &[
            (fit / 8, 0i64),
            (fit / 8, 1),
            (l / 16 + 1, 1),
            (l / 32 + 1, 1),
        ] {
            let count = (l / (8 * n)).clamp(1, 252);
            cases.push((
                format!("n={n};count={count};energy={energy}"),
                "samples",
                vec![n as i64, count as i64, energy],
                8 * n * count,
                if energy == 0 {
                    None
                } else {
                    Some(SAMPLES_ENERGY_FAULT)
                },
            ));
        }
        for &n in &[l / 32 + 1, fit / 16] {
            cases.push((
                format!("n={n};equality=1"),
                "vec",
                vec![n as i64, 1],
                16 * n,
                Some(VEC_EQUALITY_FAULT),
            ));
        }
        cases.push((
            format!("n={}", l / 40),
            "linear",
            vec![(l / 40) as i64],
            40 * (l / 40),
            None,
        ));
        cases.push((
            format!("n={}", l / 56),
            "quad",
            vec![(l / 56) as i64],
            56 * (l / 56),
            None,
        ));
        for (params, family, calldata, charged, expect) in cases {
            let (block, applied) = chain.author(vec![chain.execute(family, &calldata, ms)]);
            let (r, author_heap) = &applied[0];
            let (_, import_heap) = chain.import(&block);
            let import_net = import_heap.saturating_sub(base_import);

            // Encode each case's designed outcome and assert exactly that: a
            // success, or the specific post-allocation xqvm fault. (The old
            // blanket `Ok(Ok(()))` wrongly treated the designed faults as
            // failures.)
            match expect {
                None => assert!(
                    matches!(r, Ok(Ok(()))),
                    "{family} ({params}) at L={} KiB: expected success, got {}",
                    l / KIB,
                    outcome(r),
                ),
                Some(code) => assert!(
                    matches!(
                        r,
                        Ok(Err(sp_runtime::DispatchError::Module(m)))
                            if m.index == XQVM_PALLET_INDEX && m.error[0] == code
                    ),
                    "{family} ({params}) at L={} KiB: expected xqvm module error {code}, got {}",
                    l / KIB,
                    outcome(r),
                ),
            }
            // Separately prove the target allocation really happened: the
            // importer footprint is a large fraction of the charged bytes, which
            // an early compiled-cap `VmMemoryLimitExceeded` (footprint ~0) could
            // not reach. Measured ratios run 0.78..2.02, so charged/2 is a safe
            // floor that still excludes a rejected allocation.
            assert!(
                u64::from(import_net) >= charged / 2,
                "{family} ({params}) at L={} KiB: footprint {import_net} B < charged/2 \
                 ({} B) -- allocation did not reach its target (early VmMemoryLimitExceeded?)",
                l / KIB,
                charged / 2,
            );

            // `vec` faults with VmMemoryLimitExceeded (19) by design, but an early
            // cap fault inside the VECPUSH loop would report the same code, and
            // the charged/2 footprint floor is met before EQUALITY is reached.
            // Pin the fault to EQUALITY itself: rerun the program natively at
            // the same memory and step limits and compare the fault offset.
            if family == "vec" {
                let program = xqvm::Program::decode(&prog_vec()).expect("decodes");
                let equality = xqvm::InstructionStream::new(program.code())
                    .filter_map(Result::ok)
                    .find(|(_, _, instr)| matches!(instr, I::Equality { .. }))
                    .map(|(pos, _, _)| pos)
                    .expect("prog_vec contains EQUALITY");
                let mut vm = xqvm::Vm::new();
                vm.set_memory_limit(max_vm_memory())
                    .set_step_limit(ms)
                    .set_calldata(calldata.iter().map(|&v| xqvm::RegVal::Int(v)).collect());
                let fault = vm.run(&program).expect_err("vec run faults");
                assert!(
                    matches!(fault, xqvm::Error::MemoryLimitExceeded { pos, .. } if pos == equality),
                    "{family} ({params}) at L={} KiB: expected the memory fault at EQUALITY \
                     (offset {equality}), got {fault:?}",
                    l / KIB,
                );
            }

            println!(
                "{family},{},{params},{charged},{:.3},{:.3},{:.3},{:.3},{:.2},{}",
                l / KIB,
                mib(*author_heap),
                mib(author_heap.saturating_sub(base_author)),
                mib(import_heap),
                mib(import_net),
                f64::from(import_net) / l as f64,
                outcome(r)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Block-level order sweep at emulated caps, then import.
// ---------------------------------------------------------------------------

/// Mixed-family order sweep (correction 2). One run per allocator order filled
/// with samples (chunk `8 << o`, + ENERGY clone), PLUS one full-L run for each
/// family that fills small orders many sample buffers cannot reach:
///   - `linear`:  BTreeMap<usize,i64> leaf + internal node chunks,
///   - `quad`:    BTreeMap<(usize,usize),i64> node chunks,
///   - `vec`:     the vec<int> buffer order + the EQUALITY double-clone.
/// Each run charges <= `l`, so under a real cap L'=l none reaches the compiled
/// 1 MiB cap.
///
/// Every order a charge of <= `l` can reach is swept, which is NOT `chunk <= l`:
/// the top reachable order is next_pow2(l). At a non-power-of-two `l` (the
/// 1.5 MiB tried in dc3aa33) the charged sample itself -- chunk/16 + 1
/// elements, just over half of the next_pow2(l) chunk, still <= `l` -- lands in
/// the order above `l`'s, and its uncharged ENERGY clone doubles it there
/// (kimi's R8 on dc3aa33; Σ 29.5225·L at 1.5 MiB before the holder classes).
/// The shipped cap is a power of two for that reason. The loop runs while that
/// smallest occupant fits; at a power-of-two `l` it stops at `l`, as before.
fn order_sweep(chain: &Chain, l: u64, ms: u64) -> Vec<(String, RuntimeCall, u64)> {
    let mut out = Vec::new();
    let mut chunk = 8u64;
    let mut order = 0u32;
    loop {
        let n = if chunk == 8 { 1 } else { chunk / 16 + 1 };
        if 8 * n > l {
            break;
        }
        let count = (l / (8 * n)).clamp(1, 252);
        let charged = 8 * n * count;
        assert!(charged <= l);
        let steps_needed = ((n + 8) * (count + 2) + 4 * 256 + 1_000).min(ms);
        let (call, _) =
            chain.legal_call("samples", &[n as i64, count as i64, 1], steps_needed, 0, l);
        out.push((format!("samples/o{order}"), call, charged));
        chunk <<= 1;
        order += 1;
    }
    for (name, per) in [("linear", 40), ("linear_anchored", 40), ("quad", 56)] {
        let (call, _) = chain.legal_call(name, &[(l / per) as i64], ms, 0, l);
        out.push((name.into(), call, per * (l / per)));
    }
    // Non-masking: its designed `MemoryLimitExceeded` must come from the
    // EQUALITY after the clones, not from the VECPUSH that builds the vec.
    let (call, r) = chain.legal_call("vec", &[(l / 32 + 1) as i64, 1], ms, 0, l);
    post_clone_code(&r, 3, clone_op_pos(&prog_vec()), "vec+equality");
    out.push(("vec+equality".into(), call, 16 * (l / 32 + 1)));
    out
}

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn c_block_order_sweep_at_emulated_caps() {
    let chain = Chain::new();
    let ms = max_steps();
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    println!("empty-block import {:.3} MiB", mib(empty_import));
    // Mixed-family sweep is capped at 1 MiB: the linear/quad families build
    // ~l/40 and ~l/56 interpreted BTreeMap inserts each, so 2-4 MiB runs take
    // minutes in debug Wasm for no new information -- the ratio is linear in L'
    // (confirmed below) and the samples-only sweep already covered 2-4 MiB.
    println!("L_KiB,runs,block_bytes,author_max_MiB,import_MiB,import_minus_empty_MiB,ratio_to_L,n_faults");
    for l in [256 * KIB, 512 * KIB, MIB] {
        let sweep = order_sweep(&chain, l, ms);
        let calls: Vec<RuntimeCall> = sweep.iter().map(|(_, c, _)| c.clone()).collect();
        let (block, applied) = chain.author(calls);
        assert_eq!(applied.len(), sweep.len(), "every sweep run is included");
        for ((name, _, _), (r, _)) in sweep.iter().zip(&applied) {
            assert_designed_outcome(
                r,
                sweep_designed_outcome(name),
                &format!("census c {name} at L'={} KiB", l / KIB),
            );
        }
        let author_max = applied.iter().map(|(_, h)| *h).max().unwrap_or_default();
        let n_faults = applied
            .iter()
            .filter(|(r, _)| !matches!(r, Ok(Ok(()))))
            .count();
        let (imported, heap) = chain.import(&block);
        let net = heap.saturating_sub(empty_import);
        println!(
            "{},{},{},{:.3},{:.3},{:.3},{:.2},{}",
            l / KIB,
            sweep.len(),
            block.encode().len(),
            mib(author_max),
            mib(heap),
            mib(net),
            f64::from(net) / l as f64,
            n_faults
        );
        assert!(
            imported.is_ok(),
            "mixed sweep at L'={} KiB must import",
            l / KIB
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Block residency: valid blocks, imported through finalize (A6h).
//
// Until A6h this section measured hand-built or nonce-repeating blocks that
// the importer rejects at apply or at `final_checks`. Those never finalize,
// and missed what `finalize` allocates (the `ExtrinsicData` re-read and the
// extrinsics-root encoding), several MiB at a full block. Census d now
// measures blocks the builder admitted, imported through finalize, and keeps
// a copy of each trapped at the last apply (no finalize) only as a control.
// ---------------------------------------------------------------------------

/// The body length the importer actually holds per extrinsic: `OpaqueExtrinsic`
/// stores the extrinsic body WITHOUT the outer SCALE compact length prefix
/// (sp-runtime lib.rs:990). Round `xt` through that type to get the exact
/// resident length, so padding targets the body, not the prefixed encoding.
fn opaque_body_len(xt: &UncheckedExtrinsic) -> usize {
    let enc = xt.encode();
    sp_runtime::OpaqueExtrinsic::try_from_encoded_extrinsic(&enc)
        .expect("round-trips to opaque")
        .inner()
        .len()
}

/// The largest `payload` for which `make(payload)` alone is admitted by the
/// builder, by bisection over single-extrinsic blocks; `lo` must be admitted
/// and `hi` refused, so `payload + 1` is refused (QUI-2548: the per-class
/// `BlockLength` counts the header and digest too, so this is the measured
/// limit rather than one derived from the constant).
fn largest_admitted_alone(
    chain: &Chain,
    make: &dyn Fn(usize) -> RuntimeCall,
    mut lo: usize,
    mut hi: usize,
) -> usize {
    let admitted = |p: usize| chain.author_fast(vec![make(p)]).0.extrinsics.len() == 2;
    assert!(admitted(lo), "bisection: {lo} is admitted");
    assert!(!admitted(hi), "bisection: {hi} is refused");
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if admitted(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// One item of a block built by [`author_items`]: a call the attacker signs
/// at its live nonce, or a ready extrinsic (an unsigned `eth_transact`).
enum Item {
    Signed(RuntimeCall),
    Raw(UncheckedExtrinsic),
}

/// Author a block mixing signed calls and ready extrinsics, as the builder
/// does: an extrinsic that fails to apply is skipped (claude_reviewer's
/// `author_items`, A10 R8). Returns the block and each item's apply result.
fn author_items(chain: &Chain, items: Vec<Item>) -> (Block, Vec<ApplyExtrinsicResult>) {
    let who = account_id_from_public(&chain.attacker.public());
    let mut sign_state = chain.state();
    let mut state = chain.state();
    chain.call(
        &mut state,
        "Core_initialize_block",
        &chain.header(1).encode(),
    );
    let mut xts = vec![Chain::timestamp(1)];
    let (ts, _) = chain.apply_in_transaction(&mut state, &xts[0], false);
    assert_eq!(ts, Ok(Ok(())), "timestamp");
    let mut results = Vec::new();
    let mut skipped = 0usize;
    for item in items {
        let xt = match item {
            Item::Signed(call) => {
                let nonce =
                    state.execute_with(|| frame_system::Pallet::<Runtime>::account_nonce(&who));
                chain.sign_in(&mut sign_state, call, nonce)
            }
            Item::Raw(xt) => xt,
        };
        let (r, _) = chain.apply_in_transaction(&mut state, &xt, false);
        let failed = r.is_err();
        results.push(r);
        if failed {
            skipped += 1;
            if skipped > MAX_SKIPPED_TRANSACTIONS {
                break;
            }
        } else {
            xts.push(xt);
        }
    }
    drop(sign_state);
    let (out, _) = chain.measured(&mut state, "BlockBuilder_finalize_block", &[]);
    let header = Header::decode(&mut &out.expect("finalize")[..]).expect("header");
    (
        Block {
            header,
            extrinsics: xts,
        },
        results,
    )
}

/// An unsigned `eth_transact` carrying `payload`.
fn eth_bare(payload: Vec<u8>) -> UncheckedExtrinsic {
    generic::UncheckedExtrinsic::<Address, RuntimeCall, Signature, TxExtension>::new_bare(
        pallet_revive::Call::<Runtime>::eth_transact { payload }.into(),
    )
    .into()
}

/// The signed RLP payload of a legacy transaction from `eth` on `chain`.
fn eth_payload(
    chain: &Chain,
    eth: &pallet_revive::evm::Account,
    to: Option<sp_core::H160>,
    input: Vec<u8>,
    gas: sp_core::U256,
) -> Vec<u8> {
    use pallet_revive::evm::{Bytes, TransactionLegacyUnsigned, TransactionUnsigned};
    let (chain_id, base_fee) = chain.state().execute_with(|| {
        (
            <<Runtime as pallet_revive::Config>::ChainId as Get<u64>>::get(),
            pallet_revive::Pallet::<Runtime>::evm_base_fee(),
        )
    });
    let tx = TransactionLegacyUnsigned {
        chain_id: Some(sp_core::U256::from(chain_id)),
        gas,
        gas_price: base_fee,
        input: Bytes(input),
        nonce: sp_core::U256::zero(),
        to,
        value: sp_core::U256::zero(),
        ..Default::default()
    };
    eth.sign_transaction(TransactionUnsigned::TransactionLegacyUnsigned(tx))
        .signed_payload()
}

/// The signed RLP payload of an EIP-1559 transaction from `eth` on `chain`,
/// carrying `access_list` (part of the signed RLP).
fn eth_payload_1559(
    chain: &Chain,
    eth: &pallet_revive::evm::Account,
    to: Option<sp_core::H160>,
    input: Vec<u8>,
    access_list: pallet_revive::evm::AccessList,
    gas: sp_core::U256,
) -> Vec<u8> {
    use pallet_revive::evm::{Bytes, Transaction1559Unsigned, TransactionUnsigned};
    let (chain_id, base_fee) = chain.state().execute_with(|| {
        (
            <<Runtime as pallet_revive::Config>::ChainId as Get<u64>>::get(),
            pallet_revive::Pallet::<Runtime>::evm_base_fee(),
        )
    });
    let tx = Transaction1559Unsigned {
        chain_id: sp_core::U256::from(chain_id),
        gas,
        gas_price: base_fee,
        max_fee_per_gas: base_fee,
        max_priority_fee_per_gas: sp_core::U256::zero(),
        input: Bytes(input),
        nonce: sp_core::U256::zero(),
        to,
        value: sp_core::U256::zero(),
        access_list,
        ..Default::default()
    };
    eth.sign_transaction(TransactionUnsigned::Transaction1559Unsigned(tx))
        .signed_payload()
}

/// `addresses` entries of `keys` storage keys each.
fn eth_access_list(addresses: u8, keys: u8) -> pallet_revive::evm::AccessList {
    (0..addresses)
        .map(|a| pallet_revive::evm::AccessListEntry {
            address: sp_core::H160::repeat_byte(a.wrapping_add(1)),
            storage_keys: (0..keys)
                .map(|k| H256::repeat_byte(k.wrapping_add(1)))
                .collect(),
        })
        .collect()
}

/// A call to an account with no code whose payload is exactly `len` bytes.
fn eth_call_payload_of_len(
    chain: &Chain,
    eth: &pallet_revive::evm::Account,
    len: usize,
    gas: sp_core::U256,
) -> Vec<u8> {
    let to = Some(sp_core::H160::repeat_byte(0x11));
    let mut input = len - 512;
    for _ in 0..8 {
        let p = eth_payload(chain, eth, to, vec![0x5a; input], gas);
        if p.len() == len {
            return p;
        }
        input = input + len - p.len();
    }
    panic!("no input gives a {len}-B payload");
}

/// The input of the largest instantiate revive accepts at dispatch: a PVM blob
/// header naming exactly `REVIVE_BLOB_BYTES`, then `REVIVE_CALLDATA_BYTES` of
/// constructor data.
fn largest_pvm_instantiate_input() -> Vec<u8> {
    let blob = REVIVE_BLOB_BYTES as usize;
    let mut input = vec![0u8; blob + REVIVE_CALLDATA_BYTES as usize];
    input[..4].copy_from_slice(b"PVM\0");
    input[5..13].copy_from_slice(&(blob as u64).to_le_bytes());
    input
}

/// A10b (QUI-2548 follow-up): an `eth_transact` over
/// `MAX_ETH_TRANSACT_PAYLOAD` is invalid in every block. The builder's apply
/// refuses it with `Custom(ETH_TRANSACT_PAYLOAD_TOO_LARGE)` -- the same
/// `Checkable::check` an importer's `apply_extrinsic` runs -- so it is never
/// included, and a block carrying it anyway does not import, while the same
/// block without it does. (The executor reports a Wasm trap without the
/// runtime's panic message, so the import failure itself is not attributed
/// by its text.) The payload is not even RLP: the cap is checked before any
/// decode.
#[test]
fn a_block_with_an_oversize_eth_transact_fails_import() {
    use sp_runtime::transaction_validity::InvalidTransaction;
    let chain = Chain::new();
    let cap = MAX_ETH_TRANSACT_PAYLOAD as usize;
    let oversize = eth_bare(vec![0xffu8; cap + 1]);
    let (block, results) = author_items(&chain, vec![Item::Raw(oversize.clone())]);
    assert_eq!(
        block.extrinsics.len(),
        1,
        "the builder includes only the timestamp"
    );
    assert_eq!(
        results[0],
        Err(InvalidTransaction::Custom(ETH_TRANSACT_PAYLOAD_TOO_LARGE).into())
    );
    let (without, _) = chain.import(&block);
    assert!(without.is_ok(), "the block without it imports: {without:?}");
    let mut carrying = block.clone();
    carrying.extrinsics.push(oversize);
    let (imported, _) = chain.import(&carrying);
    assert!(imported.is_err(), "a block carrying it must not import");
}

fn set_code_call(p: usize) -> RuntimeCall {
    frame_system::Call::<Runtime>::set_code { code: vec![0u8; p] }.into()
}

/// Block residency, measured THROUGH FINALIZE (A6h; claude_reviewer's R8 on
/// A6c..A6f): what a VALID block -- signed with increasing nonces, admitted by
/// the builder, imported without a trap -- makes the importer hold. The term
/// is `VM_BLOCK_RESIDENCY`, and this test asserts the worst shape plus 1 MiB
/// of margin fits under it.
///
/// Why valid blocks: until A6h this test built blocks that repeat a nonce and
/// trap at apply, so they never finalize. `frame_system::finalize` re-reads
/// every `ExtrinsicData` entry and `extrinsics_data_root` encodes them as one
/// `Vec<Vec<u8>>` through a buffer that grows by doubling, so a block that
/// finalizes holds several MiB more. **A trapping block must never be this
/// term's worst case;** each shape below keeps a trapped copy (its last two
/// extrinsics swapped, so the last one's nonce fails at apply) -- trapped at
/// the last apply (no finalize) -- only as a control, and prints the
/// difference as the finalize share.
///
/// Shapes, all bounded by the builder (length and weight):
/// - fillers whose opaque body sits at each rounding boundary, from the
///   minimal remark (the smallest body, the most extrinsics) to bodies just
///   over each power of two (the worst decode rounding);
/// - one large remark from just over 2 MiB up to the largest the builder
///   admits (Normal, 3.5 MiB `BlockLength`), plus fillers at the worst
///   rounding (the largest decoded call is live while the rest of the block
///   is resident);
/// - the Operational class (QUI-2548): one signed `set_code` -- included even
///   though it fails at dispatch (`BadOrigin`) -- swept across the same 2 MiB
///   cliffs up to the largest the builder admits (3.75 MiB `BlockLength`),
///   plus Normal fillers while they still fit.
///
/// A block with VM runs is census k's combined block; its high-water is the
/// real-block check of the whole budget.
#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn d_encoded_block_residency_and_largest_extrinsic() {
    use core::mem::size_of;
    let chain = Chain::new();
    println!("size_of::<RuntimeCall>() = {} B", size_of::<RuntimeCall>());
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    // Oversupply: more than any class admits (Mandatory, inherents only, is the
    // largest at 5 MiB; Normal 3.5 MiB, Operational 3.75 MiB).
    let max_len = 5usize * 1024 * 1024;
    let remark = |p: usize| -> RuntimeCall {
        frame_system::Call::<Runtime>::remark {
            remark: vec![0u8; p],
        }
        .into()
    };
    let body0 = opaque_body_len(&chain.sign(remark(0), 0));
    let payload_for = |body: usize| body.saturating_sub(body0);
    println!("empty-remark opaque body {body0} B");

    // One shape: author it through the builder, check the head (`head` calls
    // that must all be included, in order) made it, import it valid, then the
    // trapped control. Returns the valid import's net high-water.
    let mut worst = (0u32, String::new());
    // `head_fails_at_dispatch`: the first call is a signed `set_code`,
    // included but failing at dispatch (`BadOrigin`), which is the point.
    let mut measure = |label: String,
                       calls: Vec<RuntimeCall>,
                       head: usize,
                       head_fails_at_dispatch: bool| {
        let supplied = calls.len();
        let (block, applied) = chain.author_fast(calls.clone());
        let included: Vec<RuntimeCall> = block.extrinsics[1..]
            .iter()
            .map(|xt| xt.0.function.clone())
            .collect();
        assert!(
            included.len() >= head && included[..head] == calls[..head],
            "{label}: the leading {head} call(s) must be included"
        );
        assert!(
            applied.iter().enumerate().all(|(i, r)| match r {
                Ok(Ok(())) => true,
                Ok(Err(_)) => head_fails_at_dispatch && i == 0,
                Err(_) => false,
            }),
            "{label}: every included extrinsic applies"
        );
        let (ok, heap) = chain.import(&block);
        assert!(ok.is_ok(), "{label}: the valid block imports: {ok:?}");
        let net = heap.saturating_sub(empty_import);
        // The trapped control needs a filler to swap; a block holding only the
        // big extrinsic has none.
        let control = if block.extrinsics.len() >= 3 {
            let mut trapped = block.clone();
            let n = trapped.extrinsics.len();
            trapped.extrinsics.swap(n - 2, n - 1);
            let (tok, theap) = chain.import(&trapped);
            assert!(tok.is_err(), "{label}: the control traps at apply");
            format!(
                "trapped at the last apply (no finalize) {:.3} MiB | finalize share {:.3} MiB",
                mib(theap.saturating_sub(empty_import)),
                mib(heap.saturating_sub(theap)),
            )
        } else {
            "no filler to trap (control n/a)".to_string()
        };
        println!(
            "(d) {label}: supplied {supplied}, included {}, encoded {:.3} MiB | VALID net {:.3} MiB \
             | {control}",
            applied.len(),
            block.encode().len() as f64 / MIB as f64,
            mib(net),
        );
        if net > worst.0 {
            worst = (net, label);
        }
    };

    // Fillers at each rounding boundary: the minimal remark, then bodies of
    // exactly a power of two and just over it.
    let mut bodies = vec![body0, 2048, 2049];
    let mut p2 = 4096usize;
    while p2 <= MIB as usize {
        bodies.push(p2 + 1);
        p2 <<= 2;
    }
    for body in bodies {
        let wire = chain.sign(remark(payload_for(body)), 1).encode().len();
        let count = max_len / wire + 16;
        measure(
            format!("fillers body {body} B"),
            (0..count).map(|_| remark(payload_for(body))).collect(),
            1,
            false,
        );
    }
    // One large remark plus fillers at the worst rounding. The builder admits
    // a remark up to the Normal class's share of the block length; the
    // largest admitted is measured, and a larger one must be refused.
    let filler = payload_for(2049);
    let filler_wire = chain.sign(remark(filler), 1).encode().len();
    let mut largest_admitted = 0usize;
    let largest_remark = largest_admitted_alone(
        &chain,
        &remark,
        MIB as usize,
        quip_protocol_runtime::configs::NORMAL_BLOCK_LENGTH as usize,
    );
    println!(
        "(d) largest admitted Normal remark: payload {largest_remark} B, body {} B \
         (Normal BlockLength {} B)",
        body0 + largest_remark,
        quip_protocol_runtime::configs::NORMAL_BLOCK_LENGTH
    );
    // Remark payloads across the two cliffs at the 2 MiB order (the opaque
    // body is the payload plus `body0`): just below 2 MiB; just over it, where
    // the first payload copies reach the 4 MiB order; 2 MiB + 768 B, where
    // the rest do (the measured worst, a plateau to about 2 MiB + 1.2 KiB);
    // then larger remarks, with fewer fillers, up to the largest admitted.
    for big_body in [
        body0 + 2 * MIB as usize - 1,
        body0 + 2 * MIB as usize + 1,
        body0 + 2 * MIB as usize + 512,
        body0 + 2 * MIB as usize + 768,
        body0 + 2 * MIB as usize + 1024,
        2 * MIB as usize + 64 * KIB as usize + 1,
        3 * MIB as usize,
        3 * MIB as usize + 448 * KIB as usize,
        body0 + largest_remark,
    ] {
        let big = remark(payload_for(big_body));
        let (probe, _) = chain.author_fast(vec![big.clone()]);
        if probe.extrinsics.len() < 2 {
            println!("(d) a {big_body} B remark is not admitted by the builder");
            continue;
        }
        largest_admitted = largest_admitted.max(big_body);
        let mut calls = vec![big];
        calls.extend((0..max_len / filler_wire + 16).map(|_| remark(filler)));
        measure(
            format!("remark {big_body} B + 2049-B fillers"),
            calls,
            1,
            false,
        );
    }
    assert!(
        largest_admitted >= 2 * MIB as usize,
        "a >2 MiB remark must be measured"
    );
    assert_eq!(
        largest_admitted,
        body0 + largest_remark,
        "the largest admitted Normal remark is measured"
    );

    // Operational (QUI-2548): a signed `set_code`, included though it fails at
    // dispatch, across the same cliffs and up to the largest admitted; Normal
    // fillers follow while the block is under the Normal limit.
    let sc_body0 = opaque_body_len(&chain.sign(set_code_call(0), 0));
    let largest_sc = largest_admitted_alone(
        &chain,
        &set_code_call,
        MIB as usize,
        quip_protocol_runtime::configs::OPERATIONAL_BLOCK_LENGTH as usize,
    );
    println!(
        "(d) largest admitted signed set_code: payload {largest_sc} B, body {} B \
         (Operational BlockLength {} B)",
        sc_body0 + largest_sc,
        quip_protocol_runtime::configs::OPERATIONAL_BLOCK_LENGTH
    );
    let mut sc_measured = 0usize;
    for sc_body in [
        sc_body0 + 2 * MIB as usize - 1,
        sc_body0 + 2 * MIB as usize + 1,
        sc_body0 + 2 * MIB as usize + 512,
        sc_body0 + 2 * MIB as usize + 768,
        sc_body0 + 2 * MIB as usize + 1024,
        2 * MIB as usize + 64 * KIB as usize + 1,
        3 * MIB as usize,
        3 * MIB as usize + 448 * KIB as usize,
        3 * MIB as usize + 640 * KIB as usize,
        sc_body0 + largest_sc,
    ] {
        let p = sc_body - sc_body0;
        assert!(
            p <= largest_sc,
            "{sc_body} B is within the Operational limit"
        );
        let mut calls = vec![set_code_call(p)];
        calls.extend((0..max_len / filler_wire + 16).map(|_| remark(filler)));
        measure(
            format!("signed set_code {sc_body} B (Operational) + 2049-B fillers"),
            calls,
            1,
            true,
        );
        sc_measured = sc_measured.max(sc_body);
    }
    assert_eq!(
        sc_measured,
        sc_body0 + largest_sc,
        "the largest admitted signed set_code is measured"
    );

    // A10b: the `eth_transact` family, up to `MAX_ETH_TRANSACT_PAYLOAD`, valid
    // through finalize, each with the same fillers as a remark of the same
    // opaque body (measured above through `measure`, the control). Calls go
    // to an account with no code (the input is not read, so it dispatches).
    // The largest instantiates -- legacy, and EIP-1559 with an access list
    // filling the rest of the cap (A10c) -- are reverted under A9 (refused by
    // origin; `with_ethereum_context` returns `Ok` with a revert), not deployed,
    // and included. The remark family must stay the worst.
    let eth = pallet_revive::evm::Account::default();
    let fund: RuntimeCall = pallet_balances::Call::<Runtime>::transfer_keep_alive {
        dest: Address::Id(eth.substrate_account()),
        value: 10_000 * UNIT,
    }
    .into();
    let cap = MAX_ETH_TRANSACT_PAYLOAD as usize;
    let admitted = |payload: Vec<u8>| {
        let (_, results) = author_items(
            &chain,
            vec![Item::Signed(fund.clone()), Item::Raw(eth_bare(payload))],
        );
        matches!(results.get(1), Some(Ok(_)))
    };
    let gas = [
        100_000_000u64,
        1_000_000_000,
        10_000_000_000,
        100_000_000_000,
    ]
    .into_iter()
    .map(sp_core::U256::from)
    .find(|&gas| admitted(eth_call_payload_of_len(&chain, &eth, cap, gas)))
    .expect("some gas limit admits a payload at the cap");
    let mut shapes: Vec<(String, Vec<u8>)> =
        [256 * KIB as usize, 512 * KIB as usize, MIB as usize, cap]
            .into_iter()
            .map(|len| {
                (
                    format!("call payload {len} B"),
                    eth_call_payload_of_len(&chain, &eth, len, gas),
                )
            })
            .collect();
    let instantiate = eth_payload(&chain, &eth, None, largest_pvm_instantiate_input(), gas);
    assert!(
        instantiate.len() <= cap,
        "the largest instantiate fits the cap"
    );
    shapes.push((
        format!(
            "largest PVM instantiate (legacy; reverted under A9, not deployed), payload {} B",
            instantiate.len()
        ),
        instantiate,
    ));
    // The largest instantiate with the largest access list that still fits.
    let typed = (1..=255u8)
        .rev()
        .map(|keys| {
            eth_payload_1559(
                &chain,
                &eth,
                None,
                largest_pvm_instantiate_input(),
                eth_access_list(40, keys),
                gas,
            )
        })
        .find(|payload| payload.len() <= cap)
        .expect("some access list fits");
    shapes.push((
        format!(
            "largest PVM instantiate (EIP-1559, access list to the cap; reverted under A9, not \
             deployed), payload {} B",
            typed.len()
        ),
        typed,
    ));
    let mut eth_worst = (0u32, String::new());
    for (label, payload) in shapes {
        let xt = eth_bare(payload);
        let body = opaque_body_len(&xt);
        let mut items = vec![Item::Signed(fund.clone()), Item::Raw(xt.clone())];
        items.extend((0..max_len / filler_wire + 16).map(|_| Item::Signed(remark(filler))));
        let (block, results) = author_items(&chain, items);
        assert!(
            matches!(results.get(1), Some(Ok(_)))
                && block.extrinsics.get(2).map(Encode::encode) == Some(xt.encode()),
            "(d) eth_transact {label}: included"
        );
        let (ok, heap) = chain.import(&block);
        assert!(
            ok.is_ok(),
            "(d) eth_transact {label}: the valid block imports: {ok:?}"
        );
        let net = heap.saturating_sub(empty_import);
        println!(
            "(d) eth_transact {label}: body {body} B, dispatch {:?}, included {}, encoded {:.3} MiB \
             | VALID net {:.3} MiB",
            results[1],
            block.extrinsics.len() - 1,
            block.encode().len() as f64 / MIB as f64,
            mib(net),
        );
        if net > eth_worst.0 {
            eth_worst = (net, label.clone());
        }
        let mut control = vec![remark(body.saturating_sub(body0))];
        control.extend((0..max_len / filler_wire + 16).map(|_| remark(filler)));
        measure(
            format!("remark {body} B (control for eth_transact {label}) + 2049-B fillers"),
            control,
            1,
            false,
        );
    }

    let (worst_net, worst_label) = worst;
    println!(
        "(d) eth_transact worst {:.3} MiB ({}); remark family worst {:.3} MiB",
        mib(eth_worst.0),
        eth_worst.1,
        mib(worst_net)
    );
    assert!(
        eth_worst.0 < worst_net,
        "an eth_transact shape ({}) holds {:.3} MiB, not below the remark family's {:.3} MiB",
        eth_worst.1,
        mib(eth_worst.0),
        mib(worst_net)
    );
    assert!(
        mib(eth_worst.0) <= 62.0,
        "an eth_transact shape holds {:.3} MiB, above 62 MiB -- report before committing",
        mib(eth_worst.0)
    );
    let pinned = quip_protocol_runtime::configs::VM_BLOCK_RESIDENCY;
    println!(
        "RESIDENCY (valid blocks, through finalize): worst {:.3} MiB ({worst_label}); \
         largest admitted remark {largest_admitted} B, signed set_code {sc_measured} B; \
         pinned VM_BLOCK_RESIDENCY {:.1} MiB",
        mib(worst_net),
        pinned as f64 / MIB as f64,
    );
    assert!(
        u64::from(worst_net) + MIB <= pinned,
        "the worst valid block, {:.3} MiB ({worst_label}), plus 1 MiB of margin exceeds \
         VM_BLOCK_RESIDENCY ({:.1} MiB) -- re-pin it",
        mib(worst_net),
        pinned as f64 / MIB as f64,
    );
}

// ---------------------------------------------------------------------------
// Gap 1: largest LEGITIMATE run, not the adversarial families. Two runs are
// executed and measured: a synthetic model-building run (the
// seeded `linear` program; it is NOT from the checked-in conformance corpus,
// which holds no runnable program that builds a model worth measuring) and
// the maximal mempool transform.
// ---------------------------------------------------------------------------

/// Locate the checked-in xquad conformance fixture through `cargo metadata`, so
/// the path follows the pinned `xqvm` dependency rather than a machine-local
/// absolute path.
fn xquad_conformance_fixture() -> std::path::PathBuf {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--quiet"])
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata emits JSON");
    let manifest = meta["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .find(|p| p["name"] == "xqvm")
        .and_then(|p| p["manifest_path"].as_str())
        .expect("xqvm package present in cargo metadata");
    std::path::Path::new(manifest)
        .parent()
        .expect("xqvm crate dir")
        .join("tests/fixtures/work-guard-spine.bin")
}

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn e_largest_legitimate_run() {
    let chain = Chain::new();
    let ms = max_steps();

    // --- Synthetic model-building run ---
    // This is a generated workload, not a conformance vector. The checked-in
    // `work-guard-spine.bin` (xqvm 0.6.0's name for the former
    // `kimi-spine-control.bin`, same sha256 3ffb4e71...) is a raw control-flow
    // STRESS stream (no XQBC header; wrapped, both the full checker and the
    // structural gate refuse it with JumpAcrossLoopBoundary), not a runnable
    // program -- decoding it as XQBC panics on BadMagic. So the
    // synthetic run is the seeded `linear` program, which builds an
    // n-variable model and sets n linear coefficients: it verifies, is
    // canonical, and runs to completion on chain.
    let synthetic = prog_linear();
    assert!(
        pallet_xqvm::structural_gate(&xqvm::Program::decode(&synthetic).expect("linear decodes"))
            .is_ok(),
        "the synthetic program must pass the structural gate"
    );
    let n: i64 = 4096;
    let (_, applied) = chain.author(vec![chain.execute("linear", &[n], ms)]);
    assert!(
        matches!(applied[0].0, Ok(Ok(()))),
        "the synthetic program must run successfully on chain: {}",
        outcome(&applied[0].0)
    );
    println!(
        "synthetic model-building run (linear, n={n}): {} program bytes, \
         author heap {:.3} MiB, ok",
        synthetic.len(),
        mib(applied[0].1),
    );

    // Fixture guard: the checked-in work-guard-spine.bin must be present (fail
    // loud if missing) and is the control-flow stress vector it claims to be --
    // raw code the structural gate refuses when wrapped, never a runnable program.
    let fixture = xquad_conformance_fixture();
    let fixture_bytes = std::fs::read(&fixture)
        .unwrap_or_else(|e| panic!("conformance fixture {} unreadable: {e}", fixture.display()));
    assert!(
        !fixture_bytes.is_empty(),
        "conformance fixture must be non-empty"
    );
    assert_eq!(
        pallet_xqvm::structural_gate(&xqvm::Program::new(fixture_bytes))
            .err()
            .map(|e| e.variant_name()),
        Some("JumpAcrossLoopBoundary"),
        "work-guard-spine.bin is a control-flow stress stream: wrapping it must be \
         refused by the structural gate, not run as a program"
    );

    // --- Largest legitimate mempool transform, through the REAL transform path ---
    // The pallet-xqvm `execute` ABI caps calldata at MaxCallDataLen (256) scalar
    // i64, so it cannot carry a 5000-spin solution: the old test fed the spin
    // vector through `execute` and silently truncated it to empty via
    // `unwrap_or_default`, measuring nothing. The mempool transports spins as one
    // `VecInt` and runs them through `XqvmAdapter::transform_solutions` -- the
    // pallet's own engine, one `Vm` (capped at MaxTransformVmMemory) per
    // solution, accumulating the `transformed: Vec<Vec<i8>>`. Drive that path at
    // the maximum load: QuantumMaxSolutions (20) solutions of QuantumMaxNodes
    // (5000) spins through a stored identity transform program, and assert
    // success and output shape.
    use pallet_quantum_compute_mempool::xqvm::QuantumVm as _;
    type MempoolVm = <Runtime as pallet_quantum_compute_mempool::Config>::VM;
    let max_solutions =
        <Runtime as pallet_quantum_compute_mempool::Config>::MaxSolutions::get() as usize;
    let max_nodes = <Runtime as pallet_quantum_compute_mempool::Config>::MaxNodes::get() as usize;

    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) }); // r0 = calldata[0] (the VecInt)
    b.emit_push(0).emit(I::Output { reg: R(0) }); // echo it to output slot 0
    b.emit(I::Halt {});
    let ident = encode(b);
    let ident_hash = BlakeTwo256::hash(&ident);

    let mut ext = TestExternalities::new(chain.genesis.clone());
    ext.execute_with(|| {
        let bounded: BoundedVec<u8, <Runtime as pallet_xqvm::Config>::MaxProgramSize> = ident
            .clone()
            .try_into()
            .expect("identity program fits MaxProgramSize");
        pallet_xqvm::Programs::<Runtime>::insert(ident_hash, bounded);

        let solver = account_id_from_public(&chain.attacker.public());
        let solutions: Vec<Vec<i8>> = (0..max_solutions)
            .map(|_| {
                (0..max_nodes)
                    .map(|i| if i % 2 == 0 { 1i8 } else { -1 })
                    .collect()
            })
            .collect();
        let transform = MempoolVm::transform_solutions(
            &BlakeTwo256::hash(b"ising-spec"),
            None,
            Some(&ident_hash),
            &solver,
            solutions,
        );
        let transformed = transform
            .result
            .expect("real identity transform must succeed at max load");
        assert_eq!(
            transformed.len(),
            max_solutions,
            "one transformed output per solution"
        );
        for (i, out) in transformed.iter().enumerate() {
            assert_eq!(out.len(), max_nodes, "solution {i} preserves its spin count");
            assert!(
                out.iter().all(|s| *s == 1 || *s == -1),
                "solution {i} spins stay ±1"
            );
        }
        // The `transformed` accumulator holds every solution's output at once.
        // The figure printed here is its PAYLOAD (QuantumMaxSolutions x
        // QuantumMaxNodes bytes), computed, not measured. Its heap footprint is
        // 164,520 B by the allocator's chunk rule (pinned in the budget test),
        // and the importer high-water of a real `submit_solution` block is
        // measured by `h_submit_solution_with_transform_imports`.
        println!(
            "real mempool transform: {max_solutions} x {max_nodes} spins ok, {} VM steps, retained payload {} KiB (computed)",
            transform.steps_used,
            (max_solutions * max_nodes) / 1024
        );
    });
}

// ---------------------------------------------------------------------------
// Gap 2: measured non-VM baseline. Maximal submit_proof -- the weight closure
// eagerly decodes the full TopologyMeta (W1-007), and apply decodes the
// maximal packed proof, before failing on the unregistered miner. The importer
// heap high-water is the decode baseline.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The fixed per-run term (W1-002 R8 batch item 2). The budget's `fixed_run`
// must be the measured WORST case of every L-independent allocation the VM
// memory meter does not charge -- not the HALT baseline.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn i_worst_case_fixed_run() {
    use quip_protocol_runtime::configs::{
        vm_transform_retained_heap, MaxLoopDepth, MaxOutputSlots, MaxProgramBlocks, MaxProgramSize,
        VM_FIXED_RUN_ALLOWANCE, VM_FIXED_RUN_MEASURED,
    };
    let chain = Chain::new();
    let ms = max_steps();
    let calldata: Vec<i64> = (0..256).collect();

    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let (halt_block, halt_applied) = chain.author(vec![chain.execute("halt", &[], 10)]);
    let (_, halt_import) = chain.import(&halt_block);
    println!(
        "fixed_run: HALT baseline author {:.3} MiB, import net {:.3} MiB",
        mib(halt_applied[0].1),
        mib(halt_import.saturating_sub(empty_import)),
    );

    let mut worst = 0u32;
    for (name, bytes) in [
        ("fixed_worst", prog_fixed_worst()),
        ("fixed_worst_jumps", prog_fixed_worst_jumps()),
    ] {
        // Each program passes every check `store_program` applies. (They are
        // seeded at genesis for speed; on main, e40cfba filters `store_program`.)
        let program = xqvm::Program::decode(&bytes).expect("worst-case program decodes");
        assert_eq!(
            bytes.len(),
            MaxProgramSize::get() as usize,
            "{name}: maximal size"
        );
        assert_eq!(program.encode(), bytes, "{name}: canonical");
        assert!(
            pallet_xqvm::structural_gate(&program).is_ok(),
            "{name}: the structural gate admits it"
        );
        let shape = pallet_xqvm::ControlFlowShape::of(program.code());
        assert!(
            shape.blocks <= MaxProgramBlocks::get(),
            "{name}: within MaxProgramBlocks"
        );
        assert_eq!(
            shape.loop_depth,
            MaxLoopDepth::get(),
            "{name}: maximal loop nesting"
        );
        // 256 OUTPUT instructions write slots 0..=255; the header's lexical
        // count saturates at 255 by design (xqvm program.rs, "clamped to 255").
        assert_eq!(
            program.output_slots(),
            u8::MAX,
            "{name}: 256 OUTPUT instructions"
        );
        let (mut targets, mut jumps) = (0u32, 0u32);
        let mut stream = xqvm::InstructionStream::new(program.code());
        while let Some(Ok((_, _, instr))) = stream.next_instruction() {
            match instr {
                I::Target {} => targets += 1,
                I::Jump1 { .. } | I::Jump2 { .. } | I::JumpI1 { .. } | I::JumpI2 { .. } => {
                    jumps += 1
                }
                _ => {}
            }
        }
        if name != "fixed_worst" {
            assert_eq!(
                shape.blocks,
                MaxProgramBlocks::get(),
                "the jump chain uses every block MaxProgramBlocks allows"
            );
        }

        // The same run natively, to confirm the extremes are reached and not
        // faulted short: it completes with every output slot written.
        let mut vm = xqvm::Vm::new();
        vm.set_step_limit(ms);
        vm.set_memory_limit(max_vm_memory());
        vm.set_calldata(calldata.iter().copied().map(xqvm::RegVal::Int).collect());
        vm.set_output_slots(MaxOutputSlots::get() as usize);
        vm.run(&program)
            .unwrap_or_else(|e| panic!("{name}: runs to HALT natively: {e:?}"));

        let (block, applied) = chain.author(vec![chain.execute_with_outputs(
            name,
            &calldata,
            ms,
            MaxOutputSlots::get(),
        )]);
        assert!(
            matches!(applied[0].0, Ok(Ok(()))),
            "{name}: must succeed on chain: {}",
            outcome(&applied[0].0)
        );
        let (imported, import) = chain.import(&block);
        assert!(imported.is_ok(), "{name}: block must import: {imported:?}");
        let net = import.saturating_sub(empty_import);
        worst = worst.max(net);
        println!(
            "fixed_run {name}: {} B program, {} blocks ({targets} targets, {jumps} jumps), \
             stack {VM_STACK_LIMIT}, {} loops, 256 regs, {} outputs: author {:.3} MiB, \
             import {:.3} MiB, import net {:.3} MiB ({net} B)",
            bytes.len(),
            shape.blocks,
            shape.loop_depth,
            MaxOutputSlots::get(),
            mib(applied[0].1),
            mib(import),
            mib(net),
        );
    }
    // Holders (i) and (ii), R9 §A (QUI-2516): programs the full checker refused
    // and the structural gate stores. (i) holds every class at its maximum at
    // once and then faults StackOverflow; (ii) fills the stack from a 3-byte
    // loop body. Both count toward the fixed-run worst case, and (i) is
    // printed against the completed `fixed_worst_jumps` run: a fault must not
    // raise the high-water mark.
    let completed = worst;
    for (name, bytes, calldata, outputs, expect) in [
        (
            "fault_at_peak",
            prog_fault_at_peak(),
            calldata.clone(),
            MaxOutputSlots::get(),
            Some(STACK_OVERFLOW_FAULT),
        ),
        ("stack_loop", prog_stack_loop(), Vec::new(), 0, None),
    ] {
        let program = xqvm::Program::decode(&bytes).expect("decodes");
        assert_eq!(program.encode(), bytes, "{name}: canonical");
        assert!(
            pallet_xqvm::structural_gate(&program).is_ok(),
            "{name}: the structural gate admits it"
        );
        let (block, applied) = chain.author(vec![
            chain.execute_with_outputs(name, &calldata, ms, outputs)
        ]);
        assert_designed_outcome(&applied[0].0, expect, &format!("census i {name}"));
        let (imported, import) = chain.import(&block);
        assert!(imported.is_ok(), "{name}: block must import: {imported:?}");
        let net = import.saturating_sub(empty_import);
        worst = worst.max(net);
        println!(
            "fixed_run {name}: {} B program, outcome {}: author {:.3} MiB, import net \
             {:.3} MiB ({net} B; the completed fixed runs' worst is {completed} B)",
            bytes.len(),
            outcome(&applied[0].0),
            mib(applied[0].1),
            mib(net),
        );
    }
    // The loop-frame pile-ups that set the worst case under xqvm 0.5.0 are no
    // longer storable: 0.6.0's jump-span phase rejects their back-edge
    // (W1-011), so a program stored under 0.6.0 holds at most MaxLoopDepth
    // frames. One stored before it still piles up: `execute` does not
    // re-verify (pallet-xqvm's `execute_does_not_re_verify_a_stored_program`,
    // which pins `execute` only), nor does the mempool transform. The pins
    // assume a chain without one (`MaxVmMemory`'s precondition).
    for (name, bytes) in [
        ("fixed_worst_pile_range", prog_fixed_worst_pile(Pile::Range)),
        ("fixed_worst_pile_iter", prog_fixed_worst_pile(Pile::Iter)),
        ("iter_slices", prog_iter_slices()),
    ] {
        let program = xqvm::Program::decode(&bytes).expect("decodes");
        assert_eq!(
            pallet_xqvm::structural_gate(&program)
                .err()
                .map(|e| e.variant_name()),
            Some("JumpAcrossLoopBoundary"),
            "{name}: the pile-up must be rejected by the jump-span phase"
        );
    }
    println!(
        "fixed_run worst case (max of the above): {:.3} MiB ({worst} B); pinned \
         VM_FIXED_RUN_MEASURED {VM_FIXED_RUN_MEASURED} B",
        mib(worst)
    );
    // One-sided: a decrease passes; an increase fails until the pin is raised
    // from a measured run (and re-budgeted if it no longer fits).
    assert!(
        u64::from(worst) <= VM_FIXED_RUN_MEASURED,
        "the fixed per-run cost grew: measured {worst} B > pinned VM_FIXED_RUN_MEASURED \
         {VM_FIXED_RUN_MEASURED} B"
    );
    assert!(
        VM_FIXED_RUN_MEASURED.saturating_add(vm_transform_retained_heap())
            <= VM_FIXED_RUN_ALLOWANCE,
        "pinned fixed run {VM_FIXED_RUN_MEASURED} B + retained transform {} B exceeds the \
         {VM_FIXED_RUN_ALLOWANCE} B allowance",
        vm_transform_retained_heap()
    );
}

/// Σ_orders evidence (W1-002 R8 items 10, 15, 19 and the xqvm 0.6.0 repin):
/// census g's combined block, with the register, OUTPUT and stack holder runs
/// and the `EQUALITY`/`ATLEASTW` vec-clone runs (holder iii, QUI-2546) added to
/// the mixed sweep, both swept to every reachable order
/// (next_pow2(L)). (Under xqvm 0.5.0 it also carried one `ITER` loop-frame
/// pile-up per order; 0.6.0 makes those unstorable.) At the shipped cap (1 MiB,
/// a power of two) and, for comparison, at an emulated 768 KiB: a
/// non-power-of-two cap of the same shape as the 1.5 MiB that was dropped
/// (3·2^18 against 3·2^19; a cap above the compiled 1 MiB cannot be emulated),
/// so the next-order case stays exercised. Asserts the shipped cap's Σ ratio
/// against `VM_SIGMA_ORDERS_PER_L` -- the comparison cap is expected to exceed
/// it, which is why the budget test requires power-of-two caps -- and that
/// every block imports and leaves at least a quarter of the heap free, after
/// printing every number.
///
/// **This test defines the pin**: Σ vs the empty block at the shipped cap must
/// not exceed `VM_SIGMA_ORDERS_PER_L` (STOP). It also prints the block's
/// same-count control ([`Chain::control_import`], the block-level share) and Σ
/// net of it (the VM's share, a lower bound), and STOPs if that net exceeds
/// the pin at either cap.
#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn k_combined_block_with_holders_at_cap() {
    let chain = Chain::new();
    let ms = max_steps();
    assert_eq!(max_vm_memory(), SHIPPED_CAP, "pins the shipped 1 MiB cap");
    let k = quip_protocol_runtime::configs::VM_SIGMA_ORDERS_PER_L as f64;
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let remark = |p: usize| -> RuntimeCall {
        frame_system::Call::<Runtime>::remark {
            remark: vec![0u8; p],
        }
        .into()
    };
    let body0 = opaque_body_len(&chain.sign(remark(0), 0));
    // The big remark on census d's worst plateau: a payload of 2 MiB + 1 KiB,
    // past both cliffs where its copies reach the 4 MiB allocator order.
    let big = remark(2 * MIB as usize + 1024);
    let fillers: Vec<RuntimeCall> = (0..1200)
        .map(|_| remark(2049usize.saturating_sub(body0)))
        .collect();
    // The shipped cap, and 768 KiB (not a power of two, emulated under the
    // compiled cap) for comparison only. Σ is asserted after both are
    // measured, so one run reports every number.
    // QUI-2548: the largest signed `set_code` admitted alone (Operational,
    // 3.75 MiB `BlockLength`), the start of the VM-then-set_code search below.
    let largest_sc_alone = largest_admitted_alone(
        &chain,
        &set_code_call,
        MIB as usize,
        quip_protocol_runtime::configs::OPERATIONAL_BLOCK_LENGTH as usize,
    );
    let mut sigma = Vec::new();
    let mut low_free = Vec::new();
    for l in [SHIPPED_CAP, 3 * MIB / 4] {
        let sweep = order_sweep(&chain, l, ms);
        // Each run's label and designed outcome, asserted once authored: a run
        // that faults early builds nothing, and Σ would silently lose it.
        let mut expected: Vec<(String, Option<u8>)> = sweep
            .iter()
            .map(|(name, _, _)| (name.clone(), sweep_designed_outcome(name)))
            .collect();
        let mut vm_calls: Vec<RuntimeCall> = sweep.into_iter().map(|(_, c, _)| c).collect();
        let n_sweep = vm_calls.len();
        // Holder classes (item 19): registers and OUTPUT clones in every order
        // they fit and the 256-B model order; and the value stack plus the
        // loop stack at their limits (census i's legal worst program, which
        // charges far below any cap here). No ITER pile-ups: since xqvm 0.6.0
        // (W1-011) a stored program cannot build one.
        let holders = holder_sweep(&chain, l, ms);
        let n_holders = holders.len();
        for (name, call, expect) in holders {
            vm_calls.push(call);
            expected.push((name, expect));
        }
        let stack_calldata: Vec<i64> = (0..256).collect();
        // Its exact step count, natively, with a margin: a max-step limit
        // would crowd the other runs out of the block by weight.
        let stack_steps = {
            let program = xqvm::Program::decode(&prog_fixed_worst_jumps()).expect("decodes");
            let mut vm = xqvm::Vm::new();
            vm.set_step_limit(ms);
            vm.set_memory_limit(max_vm_memory());
            vm.set_calldata(
                stack_calldata
                    .iter()
                    .copied()
                    .map(xqvm::RegVal::Int)
                    .collect(),
            );
            vm.set_output_slots(256);
            vm.run(&program)
                .expect("the stack-holder run completes natively");
            (vm.steps() * 5 / 4).min(ms)
        };
        vm_calls.push(chain.execute_with_outputs(
            "fixed_worst_jumps",
            &stack_calldata,
            stack_steps,
            256,
        ));
        expected.push(("stack run (fixed_worst_jumps)".into(), None));
        let control = chain.control_import(&vm_calls, empty_import);
        let (vm_block, vm_applied) = chain.author_fast(vm_calls.clone());
        assert_eq!(vm_applied.len(), vm_calls.len(), "every VM run is included");
        for (r, (name, expect)) in vm_applied.iter().zip(&expected) {
            assert_designed_outcome(r, *expect, &format!("census k {name} at L={} KiB", l / KIB));
        }
        let (vm_ok, vm_heap) = chain.import(&vm_block);
        assert!(
            vm_ok.is_ok(),
            "sweep + iter at L={} KiB must import",
            l / KIB
        );
        let vm_net = vm_heap.saturating_sub(empty_import);
        let ratio = f64::from(vm_net) / l as f64;
        let ctl = f64::from(control) / l as f64;
        let net_ctl = f64::from(vm_net.saturating_sub(control)) / l as f64;
        println!(
            "Σ+holders@L={}KiB: {n_sweep} sweep + {n_holders} holder + 1 stack runs, importer net {:.3} MiB = {ratio:.4}·L vs empty; same-count control {ctl:.4}·L; net vs control {net_ctl:.4}·L (pinned {k}), import ok",
            l / KIB,
            mib(vm_net),
        );
        assert!(
            net_ctl <= k,
            "Σ at L={} KiB net of a same-count control (a lower bound) is {net_ctl:.4}·L, over the \
             pinned {k} -- STOP and report",
            l / KIB
        );
        sigma.push((l, ratio));
        for (order, calls) in [
            ("remark-first", {
                let mut v = vec![big.clone()];
                v.extend(vm_calls.clone());
                v.extend(fillers.clone());
                v
            }),
            ("vm-first", {
                let mut v = vm_calls.clone();
                v.push(big.clone());
                v.extend(fillers.clone());
                v
            }),
        ] {
            let supplied = calls.len();
            let (block, applied) = chain.author_fast(calls.clone());
            // Full inclusion of what is measured (kimi's R8 on A6c..A6f, with
            // claude_reviewer's correction). The fillers are oversupplied on
            // purpose, so `applied.len() == supplied` is false by design. What
            // must hold: the big remark and every VM run -- the first
            // `1 + vm_calls.len()` calls in both orders -- are in the block,
            // in order (the builder includes in supplied order and only skips
            // rejects), and the fillers reach the block limit. Otherwise a
            // weight change could shrink the measured block and still pass.
            let head = 1 + vm_calls.len();
            let included: Vec<RuntimeCall> = block.extrinsics[1..]
                .iter()
                .map(|xt| xt.0.function.clone())
                .collect();
            assert!(
                included.len() > head && included[..head] == calls[..head],
                "[{order}] L={} KiB: the big remark and every VM run are included",
                l / KIB
            );
            assert!(
                applied.len() < supplied,
                "[{order}] L={} KiB: the fillers reach the block limit",
                l / KIB
            );
            // Each VM run's designed outcome in this block, as in the VM-only
            // block. Extrinsic 0 is the timestamp inherent; `applied[i]` is
            // extrinsic i + 1.
            let vm_results = match order {
                "remark-first" => &applied[1..head],
                _ => &applied[..head - 1],
            };
            for (r, (name, expect)) in vm_results.iter().zip(&expected) {
                assert_designed_outcome(
                    r,
                    *expect,
                    &format!("census k [{order}] {name} at L={} KiB", l / KIB),
                );
            }
            let (imported, heap) = chain.import(&block);
            println!(
                "COMBINED+HOLDERS@L={}KiB [{order}]: supplied {supplied}, included {}, encoded {:.2} MiB; \
                 importer high-water {:.3} MiB (net {:.3}), {:.1} % of 128 MiB free; import {}",
                l / KIB,
                applied.len(),
                block.encode().len() as f64 / MIB as f64,
                mib(heap),
                mib(heap.saturating_sub(empty_import)),
                (128.0 - mib(heap)) / 128.0 * 100.0,
                if imported.is_ok() { "IMPORTED" } else { "TRAP" }
            );
            assert!(
                imported.is_ok(),
                "[{order}] L={} KiB block must import",
                l / KIB
            );
            if mib(heap) > 128.0 * 0.75 {
                low_free.push((l, order, mib(heap)));
            }
        }
        // claude_reviewer's `z_vm_then_setcode` at the new caps (QUI-2548):
        // every VM run, then the largest signed `set_code` that still fits the
        // Operational limit (included though it fails at dispatch). At the old
        // 5 MiB length a 4,196,352-B `set_code` here (the 8 MiB allocator order)
        // made the block unimportable. The payload is found from the VM
        // block's encoded length and checked both ways: it is included and
        // one byte more is not.
        let vm_len: usize = vm_block.extrinsics[1..]
            .iter()
            .map(|xt| xt.encode().len())
            .sum();
        let with_sc = |p: usize| {
            let mut v = vm_calls.clone();
            v.push(set_code_call(p));
            chain.author_fast(v)
        };
        let sc_included = |block: &Block| {
            block.extrinsics.len() == vm_calls.len() + 2
                && matches!(
                    block.extrinsics.last().map(|xt| &xt.0.function),
                    Some(RuntimeCall::System(frame_system::Call::set_code { .. }))
                )
        };
        let mut p = largest_sc_alone.saturating_sub(vm_len);
        let (block, applied) = loop {
            let (block, applied) = with_sc(p);
            if sc_included(&block) {
                break (block, applied);
            }
            p -= 1;
        };
        assert!(
            !sc_included(&with_sc(p + 1).0),
            "L={} KiB: a set_code one byte larger is refused",
            l / KIB
        );
        for (r, (name, expect)) in applied[..vm_calls.len()].iter().zip(&expected) {
            assert_designed_outcome(
                r,
                *expect,
                &format!("census k [vm-then-set_code] {name} at L={} KiB", l / KIB),
            );
        }
        let (imported, heap) = chain.import(&block);
        println!(
            "COMBINED+HOLDERS@L={}KiB [vm-then-set_code]: {} VM runs + a signed set_code of {p} B \
             (largest alone {largest_sc_alone} B, VM extrinsics {vm_len} B), dispatch {:?}, \
             encoded {:.2} MiB; importer high-water {:.3} MiB (net {:.3}), {:.1} % of 128 MiB \
             free; import {}",
            l / KIB,
            vm_calls.len(),
            applied.last(),
            block.encode().len() as f64 / MIB as f64,
            mib(heap),
            mib(heap.saturating_sub(empty_import)),
            (128.0 - mib(heap)) / 128.0 * 100.0,
            if imported.is_ok() { "IMPORTED" } else { "TRAP" }
        );
        assert!(
            imported.is_ok(),
            "[vm-then-set_code] L={} KiB block must import: {imported:?}",
            l / KIB
        );
        if mib(heap) > 128.0 * 0.75 {
            low_free.push((l, "vm-then-set_code", mib(heap)));
        }
    }
    let ratio_of = |cap: u64| {
        sigma
            .iter()
            .find(|(l, _)| *l == cap)
            .map(|(_, ratio)| *ratio)
            .expect("measured")
    };
    let (shipped, comparison) = (ratio_of(SHIPPED_CAP), ratio_of(3 * MIB / 4));
    assert!(
        shipped <= k,
        "Σ at the shipped L={} KiB is {shipped:.4}·L, over the pinned {k} -- STOP and report",
        SHIPPED_CAP / KIB
    );
    // The non-power-of-two cap reaches the next allocator order, so it must
    // measure higher; if it does not, the sweeps stopped covering that order.
    assert!(
        comparison > shipped,
        "768 KiB measured {comparison:.4}·L, not above the shipped {shipped:.4}·L: \
         the next-order coverage of the sweeps has rotted"
    );
    assert!(
        low_free.is_empty(),
        "blocks leave under 25% of the 128 MiB heap free: {low_free:?}"
    );
}

/// The benchmark's verify-worst `store_program` shape (pallet-xqvm
/// `build_blocks_program`): `nesting` RANGE levels around unreferenced
/// TARGETs, NOP-padded to `len` encoded bytes, `blocks` basic blocks.
fn prog_verify_worst(len: usize, blocks: usize, nesting: usize) -> Vec<u8> {
    const XQBC_HEADER_LEN: usize = 15;
    let body = len - XQBC_HEADER_LEN - 1;
    let targets = blocks - 2 * nesting;
    let mut code = Vec::with_capacity(body + 1);
    for _ in 0..nesting {
        code.extend_from_slice(&[0x11, 0, 0x11, 1, 0x08]); // PUSH1 0, PUSH1 1, RANGE
    }
    code.extend(core::iter::repeat_n(0x00u8, targets)); // TARGET
    code.extend(core::iter::repeat_n(0x07u8, nesting)); // NEXT
    code.resize(body, 0xF0); // NOP
    code.push(0xFF); // HALT
    let bytes = xqvm::Program::new(code).encode();
    assert_eq!(bytes.len(), len);
    bytes
}

/// Non-VM term check (W1-002 R8 item 15): `store_program` decodes the program
/// and runs the structural gate over it (block and depth count, loop nesting,
/// jump span). Under xqvm's full checker this peaked at 3.76 MiB, because the
/// checker's CFG grows with basic blocks; the gate measures 1.36 MiB. The
/// budget's non-VM term is sized on `submit_proof` (3.27 MiB, census f).
/// Measures author and importer heap of one `store_program` at
/// `MaxProgramSize` and `MaxProgramBlocks`, for the benchmark's nested shape
/// and census i's programs. The gate runs before the duplicate check, so a
/// program already seeded at genesis still spends the full check.
#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn l_store_program_verify_peak() {
    use quip_protocol_runtime::configs::{MaxLoopDepth, MaxProgramBlocks, MaxProgramSize};
    let chain = Chain::new();
    let len = <MaxProgramSize as Get<u32>>::get() as usize;
    let blocks = <MaxProgramBlocks as Get<u32>>::get() as usize;
    let depth = <MaxLoopDepth as Get<u32>>::get() as usize;
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    for (name, bytes) in [
        (
            "bench nested",
            prog_verify_worst(len, blocks, depth.min((blocks - 1) / 2)),
        ),
        ("bench flat", prog_verify_worst(len, blocks, 0)),
        ("fixed_worst_jumps", prog_fixed_worst_jumps()),
    ] {
        let call: RuntimeCall = pallet_xqvm::Call::<Runtime>::store_program {
            bytecode: bytes.clone().try_into().expect("fits MaxProgramSize"),
        }
        .into();
        let (block, applied) = chain.author(vec![call]);
        let (imported, import) = chain.import(&block);
        println!(
            "store_program [{name}]: {} B, outcome {}, author apply {:.3} MiB, import net {:.3} MiB, import {}",
            bytes.len(),
            outcome(&applied[0].0),
            mib(applied[0].1),
            mib(import.saturating_sub(empty_import)),
            if imported.is_ok() { "ok" } else { "TRAP" }
        );
        assert!(imported.is_ok(), "[{name}] block must import");
        let net = u64::from(import.saturating_sub(empty_import));
        assert!(
            net <= quip_protocol_runtime::configs::VM_NON_VM_BASELINE,
            "[{name}] store_program import net {net} B exceeds the pinned non-VM term \
             VM_NON_VM_BASELINE ({} B)",
            quip_protocol_runtime::configs::VM_NON_VM_BASELINE
        );
    }
}

/// Σ_orders at emulated caps (W1-002 R8 items 10, 19 and the xqvm 0.6.0
/// repin), a SCALE CHECK: census c's mixed order sweep plus every holder run
/// ([`holder_sweep`]), at L' = 256 KiB, 512 KiB and 1 MiB. Census k at the
/// shipped cap is what defines the pin.
///
/// Three figures per cap (claude's and the architect's R8 on A6c/A6d):
/// - Σ vs the empty block. At a small L' this includes allocations that do
///   not scale with L' -- the fixed run and block-level overhead -- divided
///   by L'. Its step of about 0.56 MiB (about 2·L') at 256 KiB is a
///   block-level allocation made when the block finalizes (at 256 KiB the 85th
///   extrinsic crosses an allocator doubling; the same block trapped at apply
///   does not show it), not VM growth.
///   So over the pin it is REPORTED for investigation, not a STOP.
/// - The same-count control ([`Chain::control_import`]): the block-level share.
/// - Σ net of that control: the VM's share, a LOWER bound (the VM runs reuse
///   chunks the control's runs freed). Over the pin it is conclusive: STOP.
///
/// Two kinds of STOP, and both are needed: the legality checks
/// ([`Chain::legal_call`], [`Chain::legal_clone_call`]) are the STOP for a
/// run's SHAPE -- a run no chain at a real cap of L' makes panics there,
/// before it is measured -- and the Σ gates above are the STOP for SIZE. The
/// Σ gates alone do not stop an illegal run: one that adds about 2·L' only
/// reaches the vs-empty figure, which at most reports; at 512 KiB it passed
/// outright.
///
/// Up to xqvm 0.5.0 this test measured `ITER` loop-frame pile-ups (28.65·L'
/// at 256 KiB); 0.6.0's jump-span phase (W1-011) rejects them at
/// `store_program`, which census i asserts.
#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn j_sigma_at_emulated_caps() {
    let chain = Chain::new();
    let ms = max_steps();
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let k = quip_protocol_runtime::configs::VM_SIGMA_ORDERS_PER_L as f64;
    let mut worst = 0f64;
    let mut worst_net = 0f64;
    for lp in [256 * KIB, 512 * KIB, MIB] {
        let sweep = order_sweep(&chain, lp, ms);
        // Each run's label and designed outcome, as in census k: a run that
        // faults early builds nothing, and Σ would silently lose it. This is
        // the only coverage at L' = 256 and 512 KiB.
        let mut expected: Vec<(String, Option<u8>)> = sweep
            .iter()
            .map(|(name, _, _)| (name.clone(), sweep_designed_outcome(name)))
            .collect();
        let mut calls: Vec<RuntimeCall> = sweep.into_iter().map(|(_, c, _)| c).collect();
        let holders = holder_sweep(&chain, lp, ms);
        let n_holders = holders.len();
        for (name, call, expect) in holders {
            calls.push(call);
            expected.push((name, expect));
        }
        let supplied = calls.len();
        let control = chain.control_import(&calls, empty_import);
        let (block, applied) = chain.author(calls);
        assert_eq!(
            applied.len(),
            supplied,
            "the builder must include every run"
        );
        for ((r, _), (name, expect)) in applied.iter().zip(&expected) {
            assert_designed_outcome(
                r,
                *expect,
                &format!("census j {name} at L'={} KiB", lp / KIB),
            );
        }
        let (imported, heap) = chain.import(&block);
        assert!(
            imported.is_ok(),
            "sweep + holders at L'={} KiB must import",
            lp / KIB
        );
        let net = heap.saturating_sub(empty_import);
        let ratio = f64::from(net) / lp as f64;
        let ctl = f64::from(control) / lp as f64;
        let vm = f64::from(heap.saturating_sub(empty_import).saturating_sub(control)) / lp as f64;
        println!(
            "sweep + {n_holders} holder runs at L'={} KiB: {supplied} xts, \
             import net {:.3} MiB = {ratio:.2} x L' vs empty; same-count control \
             {ctl:.2} x L'; net vs control {vm:.2} x L' (pinned Σ multiple {k})",
            lp / KIB,
            mib(net),
        );
        if ratio > k {
            println!(
                "REPORT AND INVESTIGATE: Σ vs empty at L'={} KiB is {ratio:.2} x L' > {k}",
                lp / KIB
            );
        }
        worst = worst.max(ratio);
        worst_net = worst_net.max(vm);
    }
    println!("census j worst: {worst:.2} x L' vs empty, {worst_net:.2} x L' net vs control");
    assert!(
        worst_net <= k,
        "the VM's Σ, net of a same-count control (a lower bound), exceeds the pin \
         ({worst_net:.2} x L' > {k}) -- STOP and report"
    );
}

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn f_non_vm_baseline_submit_proof() {
    use quantum_validation::AllowedValueSpec;
    let chain = Chain::new();
    // Maximal topology: MaxNodes=5000 u32 nodes, MaxEdges=50000 (u32,u32) edges.
    let nodes: Vec<u32> = (0..5000u32).collect();
    let edges: Vec<(u32, u32)> = (0..50000u32).map(|e| (e % 5000, (e + 1) % 5000)).collect();
    type MaxN = <Runtime as pallet_quantum_pow::Config>::MaxNodes;
    type MaxE = <Runtime as pallet_quantum_pow::Config>::MaxEdges;
    type MaxA = <Runtime as pallet_quantum_pow::Config>::MaxAllowedValues;
    type Meta = pallet_quantum_pow::types::TopologyMeta<
        BoundedVec<u32, MaxN>,
        BoundedVec<(u32, u32), MaxE>,
        BoundedVec<quantum_validation::MilliValue, MaxA>,
        u32,
    >;
    let meta: Meta = pallet_quantum_pow::types::TopologyMeta {
        nodes: BoundedVec::try_from(nodes).unwrap(),
        edges: BoundedVec::try_from(edges).unwrap(),
        allowed_h_values: AllowedValueSpec::IntegerRange { min: -1, max: 1 },
        allowed_j_values: AllowedValueSpec::IntegerRange { min: -1, max: 1 },
        allowed_spin_values: AllowedValueSpec::IntegerRange { min: -1, max: 1 },
        registered_at: 0u32,
    };
    let meta_enc = meta.encode();
    let topology_hash = BlakeTwo256::hash(&meta_enc);
    println!(
        "maximal TopologyMeta: {} B encoded (5000 nodes, 50000 edges)",
        meta_enc.len()
    );

    // Maximal packed proof: MaxSolutions=32 solutions, each MaxNodes=5000 bytes.
    let one_sol: Vec<u8> = vec![0b0101_0101u8; 5000];
    let solutions: Vec<BoundedVec<u8, MaxN>> = (0..32)
        .map(|_| BoundedVec::try_from(one_sol.clone()).unwrap())
        .collect();
    let proof = pallet_quantum_pow::types::QuantumProof {
        topology_hash,
        nonce: sp_core::U256::zero(),
        salt: [0u8; 32],
        solutions: BoundedVec::try_from(solutions).unwrap(),
        device_access_time_us: 0,
    };
    let call: RuntimeCall = pallet_quantum_pow::Call::<Runtime>::submit_proof { proof }.into();
    println!(
        "maximal QuantumProof: call encodes to {} B (32 x 5000-byte packed solutions)",
        call.encode().len()
    );

    // Seed the topology (meta and, since W1-007, its summary) and register the
    // signer as a miner. The weight closure now reads only the fixed-size
    // summary, so the full meta is decoded in dispatch, after the miner,
    // per-block and registration checks: a registered miner is what reaches
    // that decode, the worst case this baseline measures.
    let mut g = chain.genesis.clone();
    g.top.insert(
        pallet_quantum_pow::RegisteredTopologies::<Runtime>::hashed_key_for(topology_hash),
        meta_enc,
    );
    g.top.insert(
        pallet_quantum_pow::TopologySummary::<Runtime>::hashed_key_for(topology_hash),
        pallet_quantum_pow::types::TopologySummaryInfo {
            nodes: meta.nodes.len() as u32,
            edges: meta.edges.len() as u32,
            allowed_h_values: meta.allowed_h_values.clone(),
            allowed_j_values: meta.allowed_j_values.clone(),
        }
        .encode(),
    );
    let mut seeded = TestExternalities::new(g);
    seeded.execute_with(|| {
        pallet_quantum_pow::Pallet::<Runtime>::register_miner(
            quip_protocol_runtime::RuntimeOrigin::signed(account_id_from_public(
                &chain.attacker.public(),
            )),
        )
        .expect("miner registers");
    });
    let sub = Chain {
        executor: Executor::builder().build(),
        genesis: snapshot(&mut seeded),
        genesis_hash: chain.genesis_hash,
        attacker: HybridPair::from_seed_slice(&[0xA7; 32]).unwrap(),
        programs: vec![],
    };
    let (empty, _) = sub.author(vec![]);
    let (_, empty_import) = sub.import(&empty);
    // submit_proof from a registered miner on a registered topology: apply
    // decodes the proof, dispatch decodes the full meta, then the call fails a
    // later check (the topology is not mineable).
    let (block, applied) = sub.author(vec![call]);
    let (_, import_heap) = sub.import(&block);
    println!(
        "submit_proof: author heap {:.3} MiB, {}",
        mib(applied[0].1),
        outcome(&applied[0].0)
    );
    println!(
        "NON-VM BASELINE (importer, meta+proof decode) = {:.3} MiB net (empty baseline {:.3} MiB)",
        mib(import_heap.saturating_sub(empty_import)),
        mib(empty_import)
    );
}

// ---------------------------------------------------------------------------
// Combined end-to-end regression (owner/architect required). ONE importer
// block at the shipped 1 MiB cap carrying, together: the mixed-family order
// sweep (samples across orders + SETLINE/SETQUAD BTreeMap-node runs +
// vec+EQUALITY), the ~2 MiB signed remark (payload 2 MiB + 1 KiB, past the
// 2 MiB payload cliff) applied under a fresh nonce, and
// 2,049-byte-body fillers as far as weight/length allow. Asserts the block
// imports and prints the measured high-water against the pinned budget sum
// (`vm_importer_budget()`).
// Also measures the mixed-family Σ at the shipped L directly (not extrapolated).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "V-A step-0 combined end-to-end: slow, run manually (owner-required)"]
fn g_combined_block_imports_at_cap() {
    let chain = Chain::new();
    let ms = max_steps();
    assert_eq!(
        max_vm_memory(),
        SHIPPED_CAP,
        "combined test pins the shipped 1 MiB cap"
    );
    let l = SHIPPED_CAP;

    // (1) Mixed-family Σ at the shipped L, measured standalone first.
    let sweep = order_sweep(&chain, l, ms);
    let sweep_calls: Vec<RuntimeCall> = sweep.iter().map(|(_, c, _)| c.clone()).collect();
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let (sweep_block, sweep_applied) = chain.author_fast(sweep_calls.clone());
    assert_eq!(
        sweep_applied.len(),
        sweep.len(),
        "every sweep run is included"
    );
    for ((name, _, _), r) in sweep.iter().zip(&sweep_applied) {
        assert_designed_outcome(
            r,
            sweep_designed_outcome(name),
            &format!("census g {name} at L={} KiB", l / KIB),
        );
    }
    let (sweep_ok, sweep_heap) = chain.import(&sweep_block);
    let sweep_net = sweep_heap.saturating_sub(empty_import);
    println!(
        "Σ@L={}KiB: {} runs, importer net {:.3} MiB = {:.2}·L, import {}",
        l / KIB,
        sweep.len(),
        mib(sweep_net),
        f64::from(sweep_net) / l as f64,
        if sweep_ok.is_ok() { "ok" } else { "TRAP" }
    );
    assert!(
        sweep_ok.is_ok(),
        "mixed Σ sweep at the shipped cap must import"
    );

    // (2) Combined block, authored the real way. `Chain::author_fast` drops any
    // extrinsic the builder cannot fit (apply returns an outer Err such as
    // ExhaustsResources; nonces are assigned at apply time, so a drop leaves
    // no future nonce behind) and finalizes to a true header, so whatever it
    // admits is by definition an authorable block that a correct importer
    // must accept. Contents: the full
    // shipped-cap mixed-family sweep (all runs), the ~2 MiB signed remark (fresh
    // nonce) and 2,049-B-body fillers up to the length cap. The remark's
    // payload is 2 MiB + 1 KiB, on census d's worst plateau, past both cliffs
    // where its copies reach the 4 MiB allocator order (A6h); until A8 it was
    // a 2^21+1 opaque body, whose payload sat about 1.7 KB under the cliff.
    // Authored in both orders so neither the remark nor the sweep is privileged.
    let remark = |p: usize| -> RuntimeCall {
        frame_system::Call::<Runtime>::remark {
            remark: vec![0u8; p],
        }
        .into()
    };
    let body0 = opaque_body_len(&chain.sign(remark(0), 0));
    let big_payload = 2 * MIB as usize + 1024;
    let filler_payload = (2049usize).saturating_sub(body0);
    // More fillers than any block can hold, so the author fills the block
    // until the builder refuses (weight or length) and stops after its skip
    // limit: the block is builder-full, not stopped short by the harness.
    let fillers: Vec<RuntimeCall> = (0..1200).map(|_| remark(filler_payload)).collect();
    let big = remark(big_payload);

    // The pinned budget terms, derived from the runtime's shared items (Σ at the
    // cap + the fixed-run allowance + block residency + the non-VM baseline),
    // so a change to any of them flows through here.
    use quip_protocol_runtime::configs::vm_importer_budget;
    let term_sum_mib = vm_importer_budget() as f64 / MIB as f64;
    const HEAP_MIB: f64 = 128.0;

    for (order, calls) in [
        ("remark-first", {
            let mut v = vec![big.clone()];
            v.extend(sweep_calls.clone());
            v.extend(fillers.clone());
            v
        }),
        ("sweep-first", {
            let mut v = sweep_calls.clone();
            v.push(big.clone());
            v.extend(fillers.clone());
            v
        }),
    ] {
        let supplied = calls.len();
        let (block, applied) = chain.author_fast(calls);
        let included = applied.len();
        let dropped = supplied - included;
        let (imported, heap) = chain.import(&block);
        let net = heap.saturating_sub(empty_import);
        let import_status = match &imported {
            Ok(_) => "IMPORTED".to_string(),
            Err(e) => format!(
                "TRAP: {}",
                e.lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>()
            ),
        };
        println!(
            "COMBINED [{order}]: supplied {supplied}, included {included} (dropped {dropped} by the builder), block xts {}, encoded {:.2} MiB; importer high-water {:.3} MiB (net {:.3}); import {import_status}",
            block.extrinsics.len(),
            block.encode().len() as f64 / MIB as f64,
            mib(heap),
            mib(net),
        );
        assert!(
            dropped > 0,
            "[{order}] the builder never refused a filler, so the block is not full"
        );
        // A correct importer must accept a block the builder produced; a trap
        // here would be an author/importer divergence to stop and report.
        assert!(
            imported.is_ok(),
            "[{order}] authored block must import; got {import_status}",
        );
        assert!(
            mib(net) as f64 <= term_sum_mib,
            "[{order}] high-water {:.3} MiB exceeds the pinned budget terms {:.1} MiB -- STOP and report",
            mib(net),
            term_sum_mib,
        );
        assert!(
            mib(heap) as f64 <= HEAP_MIB * 0.75,
            "[{order}] high-water {:.3} MiB leaves under 25% of the {:.0} MiB heap free",
            mib(heap),
            HEAP_MIB,
        );
        // The block carries the ~2 MiB remark and the full sweep alongside as
        // many fillers as the builder accepted, so the remark and the full
        // sweep co-occur in one builder-full, importable block. Proven from
        // the block itself, not from a count the fillers alone could meet:
        // extrinsic 0 is the timestamp inherent, and `applied[i]` is the
        // result of extrinsic i + 1.
        assert_eq!(
            block.extrinsics.len(),
            included + 1,
            "[{order}] the block is the timestamp plus every applied extrinsic"
        );
        let mut vm_runs = Vec::new();
        let mut big_remarks = 0usize;
        for (xt, r) in block.extrinsics[1..].iter().zip(&applied) {
            match &xt.0.function {
                RuntimeCall::Xqvm(pallet_xqvm::Call::execute { .. }) => vm_runs.push(r),
                RuntimeCall::System(frame_system::Call::remark { remark })
                    if remark.len() > MIB as usize =>
                {
                    big_remarks += 1
                }
                _ => {}
            }
        }
        assert_eq!(
            vm_runs.len(),
            sweep.len(),
            "[{order}] every sweep run must be in the block (found {} of {})",
            vm_runs.len(),
            sweep.len(),
        );
        for ((name, _, _), r) in sweep.iter().zip(&vm_runs) {
            assert_designed_outcome(
                r,
                sweep_designed_outcome(name),
                &format!("census g [{order}] {name}"),
            );
        }
        assert_eq!(
            big_remarks, 1,
            "[{order}] the ~2 MiB remark must be in the block"
        );
        let vm_plus_remark = 1 + sweep.len();
        println!(
            "COMBINED [{order}]: remark + full {}-run sweep + {} fillers fit and import; {dropped} calls dropped by the builder",
            sweep.len(),
            included - vm_plus_remark,
        );
    }
}

// ---------------------------------------------------------------------------
// F3 (W1-002 R8): `submit_solution` with a transform program, end to end. One
// order is seeded at genesis through the real extrinsics (job spec with an
// identity transform, registered solver, an Open 5,000-node order); a block of
// maximal `submit_solution`s (QuantumMaxSolutions x QuantumMaxNodes spins, so
// every call runs QuantumMaxSolutions transform VMs and retains the
// `transformed` accumulator) is authored builder-full and imported under
// `WasmExecutor`. This measures the retained term in the importer's heap
// rather than restating it as payload arithmetic.
// ---------------------------------------------------------------------------

/// Copy every top-level key/value of `ext` into a genesis `Storage`.
fn snapshot(ext: &mut TestExternalities) -> Storage {
    let mut storage = Storage::default();
    ext.execute_with(|| {
        let mut key = Vec::new();
        while let Some(next) = sp_io::storage::next_key(&key) {
            let value = sp_io::storage::get(&next).expect("listed key has a value");
            storage.top.insert(next.clone(), value.to_vec());
            key = next;
        }
    });
    storage
}

#[test]
#[ignore = "V-A census: slow, run manually"]
fn h_submit_solution_with_transform_imports() {
    use pallet_quantum_compute_mempool::types;
    use quip_protocol_runtime::RuntimeOrigin;
    type Mempool = pallet_quantum_compute_mempool::Pallet<Runtime>;
    type Cfg = Runtime;
    let chain = Chain::new();
    let max_solutions = <Cfg as pallet_quantum_compute_mempool::Config>::MaxSolutions::get();
    let max_nodes = <Cfg as pallet_quantum_compute_mempool::Config>::MaxNodes::get();

    // Identity transform: echo calldata[0] (the spin VecInt) to output slot 0.
    let mut b = InstructionBuilder::new();
    b.emit_push(0).emit(I::Input { reg: R(0) });
    b.emit_push(0).emit(I::Output { reg: R(0) });
    b.emit(I::Halt {});
    let ident = encode(b);
    let ident_hash = BlakeTwo256::hash(&ident);

    let solver = account_id_from_public(&chain.attacker.public());
    let proposer = account_id_from_public(
        &HybridPair::from_seed_slice(&[0xB0; 32])
            .expect("valid seed")
            .public(),
    );
    let mut ext = TestExternalities::new(chain.genesis.clone());
    let order_id = ext.execute_with(|| {
        let bounded: BoundedVec<u8, <Cfg as pallet_xqvm::Config>::MaxProgramSize> =
            ident.clone().try_into().expect("fits");
        pallet_xqvm::Programs::<Cfg>::insert(ident_hash, bounded);
        let reward = <Cfg as pallet_quantum_compute_mempool::Config>::MinReward::get();
        let _ = <pallet_balances::Pallet<Cfg> as frame_support::traits::Currency<_>>::make_free_balance_be(
            &proposer,
            reward.saturating_mul(10),
        );
        let name: BoundedVec<u8, frame_support::traits::ConstU32<128>> =
            b"census-transform".to_vec().try_into().expect("fits");
        let spec_id = BlakeTwo256::hash_of(&(
            name.clone(),
            types::Formulation::Ising,
            None::<H256>,
            Some(ident_hash),
        ));
        Mempool::register_job_spec(
            RuntimeOrigin::root(),
            proposer.clone(),
            name,
            types::Formulation::Ising,
            None,
            Some(ident_hash),
        )
        .expect("job spec with transform registers");
        Mempool::register_solver(RuntimeOrigin::signed(solver.clone()), types::MinerType::Cpu)
            .expect("solver registers");
        let nodes: Vec<u32> = (0..max_nodes).collect();
        let edges: Vec<(u32, u32)> = (0..max_nodes).map(|i| (i, (i + 1) % max_nodes)).collect();
        let order_id = pallet_quantum_compute_mempool::NextOrderId::<Cfg>::get();
        Mempool::propose_job(
            RuntimeOrigin::signed(proposer.clone()),
            spec_id,
            types::IsingParams {
                nodes: nodes.try_into().expect("nodes fit"),
                edges: edges.try_into().expect("edges fit"),
                h_values: vec![0; max_nodes as usize].try_into().expect("h fit"),
                j_values: vec![-1_000; max_nodes as usize].try_into().expect("j fit"),
                max_energy_milli: None,
                min_diversity_milli: None,
                min_solutions: None,
            },
            reward,
            types::JobMode::Open,
            types::RewardResolution::TopNEqual {
                n: types::MAX_REWARD_WINNERS,
            },
            10,
            5,
            types::ResultDelivery::OnChainOnly,
        )
        .expect("order proposes");
        order_id
    });
    let seeded = Chain {
        executor: Executor::builder().build(),
        genesis: snapshot(&mut ext),
        genesis_hash: chain.genesis_hash,
        attacker: HybridPair::from_seed_slice(&[0xA7; 32]).expect("valid seed"),
        programs: vec![],
    };

    // More calls than a block can hold: the builder decides how many fit.
    let supplied = 64u32;
    // Every call comes from the one census signer, and a repeat submission is
    // accepted only if its best energy strictly improves (`SolutionNotImproved`
    // otherwise). On this ferromagnetic ring (J = -1000) each domain wall costs
    // energy, so call `v` alternates the first 2 * (supplied - v) spins and is
    // all +1 after: later calls have strictly fewer walls, so each one improves.
    let submit = |variant: u32| -> RuntimeCall {
        let frustrated = 2 * (supplied - variant);
        let solutions: Vec<BoundedVec<i8, _>> = (0..max_solutions)
            .map(|_| {
                (0..max_nodes)
                    .map(|i| {
                        if i < frustrated && i % 2 == 1 {
                            -1i8
                        } else {
                            1
                        }
                    })
                    .collect::<Vec<_>>()
                    .try_into()
                    .expect("spins fit")
            })
            .collect();
        pallet_quantum_compute_mempool::Call::<Cfg>::submit_solution {
            order_id,
            solutions: solutions.try_into().expect("solutions fit"),
        }
        .into()
    };
    let (empty, _) = seeded.author(vec![]);
    let (_, empty_import) = seeded.import(&empty);
    let (block, applied, dropped) = seeded.author_with((0..supplied).map(submit).collect(), true);
    for (i, (r, _)) in applied.iter().enumerate() {
        assert!(
            matches!(r, Ok(Ok(()))),
            "submit_solution {i} must succeed, got {}",
            outcome(r)
        );
    }
    assert!(!applied.is_empty(), "at least one submit_solution must fit");
    let author_max = applied.iter().map(|(_, h)| *h).max().unwrap_or_default();
    let (imported, import_heap) = seeded.import(&block);
    println!(
        "F3 submit_solution x{} ({max_solutions} x {max_nodes} spins, identity transform): \
         supplied {supplied}, dropped {dropped}, encoded {:.2} MiB; author apply max {:.3} MiB; \
         importer high-water {:.3} MiB (net of empty block {:.3})",
        applied.len(),
        block.encode().len() as f64 / MIB as f64,
        mib(author_max),
        mib(import_heap),
        mib(import_heap.saturating_sub(empty_import)),
    );
    assert!(
        imported.is_ok(),
        "seeded submit_solution block must import: {imported:?}"
    );
}

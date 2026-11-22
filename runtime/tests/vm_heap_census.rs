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
//! Run: `cargo test -p quip-protocol-runtime --test vm_heap_census \
//!       -- --nocapture --test-threads 1`

use codec::{Decode, Encode};
use frame_support::{dispatch::DispatchClass, traits::Get, BoundedVec};
use quip_protocol_runtime::{
    native_tx_extension, Address, Block, Header, Nonce, Runtime, RuntimeCall, RuntimeGenesisConfig,
    Signature, SignedPayload, TxExtension, UncheckedExtrinsic, SLOT_DURATION, UNIT, WASM_BINARY,
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
/// elements; if `energy != 0`, `ENERGY` against a size-1 model, cloning r1's
/// sample buffer (uncharged) before the size-mismatch fault.
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
///   (W1-011) the most frames a storable program can hold;
/// - inside them the value stack pushed to `VM_STACK_LIMIT` (8,192), then
///   cleared, so the stack `Vec`'s whole doubling history is allocated;
/// - all 256 output slots written (`execute` declares 256);
/// - `NOP`-padded to exactly `MaxProgramSize`, so the stored bytes and the
///   decoded program are maximal.
fn prog_fixed_worst() -> Vec<u8> {
    fixed_worst_program(0, Pile::None)
}

/// xqvm's loop-stack limit (`Vm::LOOP_LIMIT`). Up to xqvm 0.5.0 a back-edge
/// that re-entered a `RANGE`/`ITER` without its `NEXT` pushed another
/// uncharged `LoopFrame` each pass, so only this count bounded the loop stack.
/// xqvm 0.6.0's jump-span phase (W1-011) rejects such a jump at
/// `store_program`, so a storable program holds at most `MaxLoopDepth` frames;
/// the pile-up builders below remain only to check that they are rejected.
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
/// basic block `MaxProgramBlocks` leaves over, so decode's `verifier::scan`
/// builds its largest `targets` (the kept jump table) and `jump_refs`
/// alongside everything else. A `JUMP` followed by a `TARGET` is ONE leader
/// in both the pallet's `ControlFlowShape` and the verifier's dataflow, so an
/// admitted program can carry about 2,048 targets AND 2,047 jumps together.
fn prog_fixed_worst_jumps() -> Vec<u8> {
    let max_blocks = <quip_protocol_runtime::configs::MaxProgramBlocks as Get<u32>>::get() as usize;
    let max_depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get() as usize;
    // The entry block, plus one block per RANGE and per NEXT of the loops,
    // plus one per jump of the chain.
    fixed_worst_program(max_blocks - 1 - 2 * max_depth, Pile::None)
}

/// `prog_fixed_worst_jumps`, plus a loop-frame pile-up to `VM_LOOP_LIMIT`
/// after the stack fill (the chain gives up the blocks the pile-up needs).
fn prog_fixed_worst_pile(pile: Pile) -> Vec<u8> {
    let max_blocks = <quip_protocol_runtime::configs::MaxProgramBlocks as Get<u32>>::get() as usize;
    let max_depth = <quip_protocol_runtime::configs::MaxLoopDepth as Get<u32>>::get() as usize;
    fixed_worst_program(max_blocks - 1 - 2 * max_depth - PILE_BLOCKS, pile)
}

fn fixed_worst_program(jumps: usize, pile: Pile) -> Vec<u8> {
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
        for _ in 0..max_depth {
            b.emit_push(0).emit_push(1).emit(I::Range {});
        }
        for _ in 0..VM_STACK_LIMIT {
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
        for i in 0..=255u8 {
            b.emit_push(i64::from(i)).emit(I::Output { reg: R(i) });
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

/// r4..r255 each a size-2 model with one linear and one quadratic coefficient
/// -- two BTreeMap root leaves (144 B and 184 B on wasm32), so two
/// 256-B-order chunks for 96 B of charge -- and 256 OUTPUT clones of them.
fn prog_holders_models() -> Vec<u8> {
    let mut b = InstructionBuilder::new();
    holders_head(&mut b);
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

/// Charges a holder program makes: 252 registers and 256 OUTPUT clones of
/// `per` bytes each, plus a margin for the INPUT register.
fn holders_charge(per: u64) -> u64 {
    (252 + 256) * per + 64
}

/// The holder runs at emulated cap `l`, each run with 256 output slots:
/// - `holders_samples` in every order its 508 holders fit, with
///   e = chunk/16 + 1;
/// - `holders_models` in the 256-B order.
fn holder_sweep(chain: &Chain, l: u64, ms: u64) -> Vec<RuntimeCall> {
    let mut out = Vec::new();
    // Every order the 508 holders fit: e = chunk/16 + 1 elements, just over
    // half the chunk, charged 8 B each (a sample and its OUTPUT clone).
    let mut chunk = 8u64;
    loop {
        let e = chunk / 16 + 1;
        if holders_charge(8 * e) > l {
            break;
        }
        let steps = (600 * e + 20_000).saturating_mul(5) / 4;
        out.push(chain.execute_with_outputs("holders_samples", &[e as i64], steps.min(ms), 256));
        chunk <<= 1;
    }
    if holders_charge(96) <= l {
        out.push(chain.execute_with_outputs("holders_models", &[], 20_000, 256));
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
    programs: Vec<(&'static str, H256)>,
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
            ("quad", prog_quad()),
            ("halt", prog_halt()),
            ("fixed_worst", prog_fixed_worst()),
            ("fixed_worst_jumps", prog_fixed_worst_jumps()),
            ("holders_samples", prog_holders_samples()),
            ("holders_models", prog_holders_models()),
        ] {
            let program = xqvm::Program::decode(&bytes).expect("decodes");
            assert_eq!(program.encode(), bytes, "{name}: canonical");
            let verdict = xqvm::verifier::verify(&program);
            println!(
                "program {name}: {} bytes, verifier {}",
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
            programs.push((name, hash));
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
            .find(|(n, _)| *n == name)
            .expect("known")
            .1
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
    // xqvm pallet index in the runtime, and the two designed post-allocation
    // fault codes seen in the measured table (stable pallet error indices). A
    // `samples` run with energy=1 computes ENERGY of a 1-variable model against
    // an n-variable sample and faults on the size mismatch; a `vec` run faults
    // in EQUALITY over the built index/coeff vectors.
    const XQVM_PALLET_INDEX: u8 = 8;
    const SAMPLES_ENERGY_FAULT: u8 = 30;
    const VEC_EQUALITY_FAULT: u8 = 19;
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
        out.push((
            format!("samples/o{order}"),
            chain.execute("samples", &[n as i64, count as i64, 1], steps_needed),
            charged,
        ));
        chunk <<= 1;
        order += 1;
    }
    out.push((
        "linear".into(),
        chain.execute("linear", &[(l / 40) as i64], ms),
        40 * (l / 40),
    ));
    out.push((
        "quad".into(),
        chain.execute("quad", &[(l / 56) as i64], ms),
        56 * (l / 56),
    ));
    out.push((
        "vec+equality".into(),
        chain.execute("vec", &[(l / 32 + 1) as i64, 1], ms),
        16 * (l / 32 + 1),
    ));
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
// 4. Decode-peak of an adversarial block, measured on a hand-built header.
//
// This block is assembled directly (header with zero state/extrinsics roots),
// so it is NOT a valid, importable block: `Core_execute_block` rejects it at
// `final_checks`. That is deliberate and does not weaken the measurement --
// the SCALE decode of the `LazyBlock` argument (header + every
// `OpaqueExtrinsic` body) happens before any check runs, so the importer
// allocates the full decode footprint regardless. The high-water therefore
// bounds what even an invalid block an attacker gossips makes an importer
// allocate before it rejects it. It is the block-residency term of the heap
// budget; it is not a claim that this block imports.
// ---------------------------------------------------------------------------

/// Round up to the freeing-bump chunk a Vec<u8> of `len` bytes occupies:
/// next_power_of_two(max(len, 8)) + 8-byte header, clamped at 32 MiB order.
fn chunk_bytes(len: usize) -> u64 {
    let n = (len.max(8) as u64).next_power_of_two().min(32 * MIB);
    n + 8
}

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

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn d_encoded_block_residency_and_largest_extrinsic() {
    use core::mem::size_of;
    let chain = Chain::new();
    println!("size_of::<RuntimeCall>() = {} B", size_of::<RuntimeCall>());

    let max_len = 5usize * 1024 * 1024; // 5 MiB all-class block length bound.
    let remark = |p: usize| -> RuntimeCall {
        frame_system::Call::<Runtime>::remark {
            remark: vec![0u8; p],
        }
        .into()
    };
    // Calibrate payload so the resident OPAQUE BODY is 2^11 + 1 = 2049 B, i.e.
    // just over a power of two -> each body rounds up to a 4096+8 chunk, the
    // worst case for decode rounding. (Correction 1: a body of exactly 2048
    // rounds with zero waste.)
    let body0 = opaque_body_len(&chain.sign(remark(0), 0));
    let filler_payload = (2049usize).saturating_sub(body0);
    let filler_body = opaque_body_len(&chain.sign(remark(filler_payload), 1));
    let per_wire = chain.sign(remark(filler_payload), 1).encode().len();
    println!(
        "empty-remark body {body0} B; padded to body {filler_body} B (wire {per_wire} B); each opaque chunk {} B",
        chunk_bytes(filler_body)
    );
    let count = max_len / per_wire;

    // Case (a): a block of padded fillers at the worst-rounding body. Decode
    // residency does not need distinct signatures -- `Core_execute_block`
    // decodes the whole `Vec<OpaqueExtrinsic>` argument before apply -- so
    // replicate one signed filler and read the peak; apply then traps on the
    // repeated nonce, after the decode peak is realised.
    let one = chain.sign(remark(filler_payload), 1);
    let mut xts = vec![Chain::timestamp(1)];
    xts.extend(std::iter::repeat(one).take(count));
    let block_a = Block {
        header: chain.header(1),
        extrinsics: xts,
    };
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let (_, heap_a) = chain.import(&block_a);
    let model_a = chunk_bytes(block_a.encode().len())
        + block_a
            .extrinsics
            .iter()
            .map(|xt| chunk_bytes(opaque_body_len(xt)))
            .sum::<u64>();
    println!(
        "(a) {count} fillers: encoded block {:.3} MiB; importer peak {:.3} MiB (net {:.3}); analytic {:.3} MiB [input chunk + Σ body chunks]",
        block_a.encode().len() as f64 / MIB as f64,
        mib(heap_a),
        mib(heap_a.saturating_sub(empty_import)),
        model_a as f64 / MIB as f64,
    );

    // Case (b): one large remark (body just over 2 MiB) applied with a FRESH
    // nonce, plus padded fillers to the length cap. The big call is applied
    // first, so its decoded Vec is live while every opaque body is resident:
    // the largest decoded extrinsic is ADDITIVE to the block residency.
    let big_payload = (2 * MIB as usize + 1).saturating_sub(body0);
    let big = chain.sign(remark(big_payload), 0); // fresh nonce 0, applies
    let big_body = opaque_body_len(&big);
    let big_wire = big.encode().len();
    let rest = (max_len - big_wire) / per_wire;
    let filler1 = chain.sign(remark(filler_payload), 1); // applies once, then dups
    let mut xts_b = vec![Chain::timestamp(1), big];
    xts_b.extend(std::iter::repeat(filler1).take(rest));
    let block_b = Block {
        header: chain.header(1),
        extrinsics: xts_b,
    };
    let (_, heap_b) = chain.import(&block_b);
    let model_b = chunk_bytes(block_b.encode().len())
        + block_b
            .extrinsics
            .iter()
            .map(|xt| chunk_bytes(opaque_body_len(xt)))
            .sum::<u64>()
        + chunk_bytes(big_body); // + the large call's decoded Vec during its dispatch
    println!(
        "(b) big body {:.3} MiB + {rest} fillers: encoded block {:.3} MiB; importer peak {:.3} MiB (net {:.3}); analytic {:.3} MiB [input + Σ bodies + largest decoded call]",
        big_body as f64 / MIB as f64,
        block_b.encode().len() as f64 / MIB as f64,
        mib(heap_b),
        mib(heap_b.saturating_sub(empty_import)),
        model_b as f64 / MIB as f64,
    );
    let residency = heap_a.max(heap_b).saturating_sub(empty_import);
    println!(
        "RESIDENCY BOUND = max(measured, analytic) = {:.3} MiB (measured net {:.3}, analytic {:.3})",
        (model_a.max(model_b) as f64 / MIB as f64).max(mib(residency) as f64),
        mib(residency),
        model_a.max(model_b) as f64 / MIB as f64,
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
        .join("tests/fixtures/kimi-spine-control.bin")
}

#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn e_largest_legitimate_run() {
    let chain = Chain::new();
    let ms = max_steps();

    // --- Synthetic model-building run ---
    // This is a generated workload, not a conformance vector. The checked-in
    // `kimi-spine-control.bin` is a raw control-flow STRESS stream (no XQBC
    // header; wrapping it fails verification with StackDepthMismatch), not a
    // runnable program -- decoding it as XQBC panics on BadMagic. So the
    // synthetic run is the seeded `linear` program, which builds an
    // n-variable model and sets n linear coefficients: it verifies, is
    // canonical, and runs to completion on chain.
    let synthetic = prog_linear();
    assert!(
        xqvm::verifier::verify(&xqvm::Program::decode(&synthetic).expect("linear decodes")).is_ok(),
        "the synthetic program must verify"
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

    // Fixture guard: the checked-in kimi-spine-control.bin must be present (fail
    // loud if missing) and is the control-flow stress vector it claims to be --
    // raw code the verifier rejects when wrapped, never a runnable program.
    let fixture = xquad_conformance_fixture();
    let fixture_bytes = std::fs::read(&fixture)
        .unwrap_or_else(|e| panic!("conformance fixture {} unreadable: {e}", fixture.display()));
    assert!(
        !fixture_bytes.is_empty(),
        "conformance fixture must be non-empty"
    );
    assert!(
        xqvm::verifier::verify(&xqvm::Program::new(fixture_bytes)).is_err(),
        "kimi-spine-control.bin is a control-flow stress stream: wrapping it must be \
         rejected by the verifier, not run as a program"
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
            xqvm::verifier::verify(&program).is_ok(),
            "{name}: the verifier admits it"
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
    // The loop-frame pile-ups that set the worst case under xqvm 0.5.0 are no
    // longer storable: 0.6.0's jump-span phase rejects their back-edge
    // (W1-011), so a stored program holds at most MaxLoopDepth frames.
    for (name, bytes) in [
        ("fixed_worst_pile_range", prog_fixed_worst_pile(Pile::Range)),
        ("fixed_worst_pile_iter", prog_fixed_worst_pile(Pile::Iter)),
        ("iter_slices", prog_iter_slices()),
    ] {
        let program = xqvm::Program::decode(&bytes).expect("decodes");
        assert_eq!(
            xqvm::verifier::verify(&program)
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
/// added to the mixed sweep, both swept to every reachable order
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
    let big = remark((2 * MIB as usize + 1).saturating_sub(body0));
    let fillers: Vec<RuntimeCall> = (0..1200)
        .map(|_| remark(2049usize.saturating_sub(body0)))
        .collect();
    // The shipped cap, and 768 KiB (not a power of two, emulated under the
    // compiled cap) for comparison only. Σ is asserted after both are
    // measured, so one run reports every number.
    let mut sigma = Vec::new();
    let mut low_free = Vec::new();
    for l in [SHIPPED_CAP, 3 * MIB / 4] {
        let mut vm_calls: Vec<RuntimeCall> = order_sweep(&chain, l, ms)
            .into_iter()
            .map(|(_, c, _)| c)
            .collect();
        let n_sweep = vm_calls.len();
        // Holder classes (item 19): registers and OUTPUT clones in every order
        // they fit and the 256-B model order; and the value stack plus the
        // loop stack at their limits (census i's legal worst program, which
        // charges far below any cap here). No ITER pile-ups: since xqvm 0.6.0
        // (W1-011) a stored program cannot build one.
        let holders = holder_sweep(&chain, l, ms);
        let n_holders = holders.len();
        vm_calls.extend(holders);
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
        let (vm_block, vm_applied) = chain.author_fast(vm_calls.clone());
        assert_eq!(vm_applied.len(), vm_calls.len(), "every VM run is included");
        let (vm_ok, vm_heap) = chain.import(&vm_block);
        assert!(
            vm_ok.is_ok(),
            "sweep + iter at L={} KiB must import",
            l / KIB
        );
        let vm_net = vm_heap.saturating_sub(empty_import);
        let ratio = f64::from(vm_net) / l as f64;
        println!(
            "Σ+holders@L={}KiB: {n_sweep} sweep + {n_holders} holder + 1 stack runs, importer net {:.3} MiB = {ratio:.4}·L (pinned {k}), import ok",
            l / KIB,
            mib(vm_net),
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
            let (block, applied) = chain.author_fast(calls);
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

/// Non-VM term check (W1-002 R8 item 15): `store_program` decodes AND
/// verifies, and the verifier's memory grows with basic blocks (the
/// `MaxProgramBlocks` docs estimate ~17 MiB in Wasm at 2,048). The budget's
/// non-VM term is sized on `submit_proof` (3.27 MiB, census f). Measures
/// author and importer heap of one `store_program` at `MaxProgramSize` and
/// `MaxProgramBlocks`, for the benchmark's nested shape and census i's
/// programs. Verification runs before the duplicate check, so a program
/// already seeded at genesis still spends the full verify.
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
/// repin): census c's mixed order sweep plus the register and OUTPUT holder
/// runs, at L' = 256 KiB, 512 KiB and 1 MiB, each within the pinned Σ multiple
/// (`VM_SIGMA_ORDERS_PER_L`). Up to xqvm 0.5.0 this test measured `ITER`
/// loop-frame pile-ups (28.65·L' at 256 KiB); 0.6.0's jump-span phase
/// (W1-011) rejects them at `store_program`, which census i asserts.
#[test]
#[ignore = "V-A step-0 evidence/measurement harness: slow, run manually"]
fn j_sigma_at_emulated_caps() {
    let chain = Chain::new();
    let ms = max_steps();
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let k = quip_protocol_runtime::configs::VM_SIGMA_ORDERS_PER_L as f64;
    let mut worst = 0f64;
    for lp in [256 * KIB, 512 * KIB, MIB] {
        let mut calls: Vec<RuntimeCall> = order_sweep(&chain, lp, ms)
            .into_iter()
            .map(|(_, c, _)| c)
            .collect();
        let holders = holder_sweep(&chain, lp, ms);
        let n_holders = holders.len();
        calls.extend(holders);
        let supplied = calls.len();
        let (block, applied) = chain.author(calls);
        assert_eq!(
            applied.len(),
            supplied,
            "the builder must include every run"
        );
        let (imported, heap) = chain.import(&block);
        assert!(
            imported.is_ok(),
            "sweep + holders at L'={} KiB must import",
            lp / KIB
        );
        let net = heap.saturating_sub(empty_import);
        let ratio = f64::from(net) / lp as f64;
        println!(
            "sweep + {n_holders} holder runs at L'={} KiB: {supplied} xts, \
             import net {:.3} MiB = {ratio:.2} x L' (pinned Σ multiple {k})",
            lp / KIB,
            mib(net),
        );
        worst = worst.max(ratio);
    }
    assert!(
        worst <= k,
        "the Σ_orders term is exceeded ({worst:.2} x L' > {k}) -- STOP and report"
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
// vec+EQUALITY), the ~2 MiB signed remark applied under a fresh nonce, and
// 2,049-byte-body fillers as far as weight/length allow. Asserts the block
// imports and prints the measured high-water against the pinned budget sum
// (`vm_importer_budget()`, 70 MiB at the shipped 1 MiB cap).
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
    let (sweep_block, _) = chain.author_fast(sweep_calls.clone());
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
    // shipped-cap mixed-family sweep (all runs), the ~2 MiB-body signed remark
    // (2^21+1 body, fresh nonce) and 2,049-B-body fillers up to the length cap.
    // Authored in both orders so neither the remark nor the sweep is privileged.
    let remark = |p: usize| -> RuntimeCall {
        frame_system::Call::<Runtime>::remark {
            remark: vec![0u8; p],
        }
        .into()
    };
    let body0 = opaque_body_len(&chain.sign(remark(0), 0));
    let big_payload = (2 * MIB as usize + 1).saturating_sub(body0);
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
        // sweep co-occur in one builder-full, importable block.
        let vm_plus_remark = 1 + sweep.len();
        assert!(
            included >= vm_plus_remark,
            "[{order}] the remark and the full {}-run sweep must both fit (included {included})",
            sweep.len(),
        );
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

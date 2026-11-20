//! W1-002 / V-A step 0: measurement harness (WP1-DESIGNS.md rev 3, V-A §1).
//!
//! Not the fix. Runs the compiled Wasm runtime through
//! `sc_executor::WasmExecutor` with the node's default heap, the way the
//! author (one runtime call per extrinsic) and the importer (one
//! `Core_execute_block`) do, and reports `address_space_used` from the
//! executor's own freeing-bump allocator.
//!
//! Runtime 121 filters `Xqvm::store_program`, so programs are written into
//! `pallet_xqvm::Programs` at genesis. `execute` reads only that map.
//!
//! A candidate cap L' is emulated by inputs whose total charge stays <= L': the
//! compiled cap is now `MaxVmMemory` = 2 MiB (W1-002), so a run that charges
//! <= L' <= 2 MiB never reaches the compiled check and allocates exactly what
//! it would under a cap of L'. Emulated L' values therefore stay below 2 MiB
//! with headroom for the k ~ 2 clone overhead; a larger L' would just hit the
//! real cap with `VmMemoryLimitExceeded` and is not a valid per-run measurement.
//!
//! Run: `cargo test -p quip-protocol-runtime --test vm_heap_census \
//!       -- --nocapture --test-threads 1`

use codec::{Decode, Encode};
use frame_support::{traits::Get, BoundedVec};
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

    /// Build block 1 the way a real block builder does, then `finalize_block`
    /// so the header carries the true state/extrinsics roots. Returns the
    /// block and, per INCLUDED signed extrinsic, (apply result, fresh-heap
    /// bytes). An extrinsic whose `apply_extrinsic` returns
    /// `Err(TransactionValidityError)` (e.g. `ExhaustsResources`, or a
    /// future-nonce left by an earlier drop) is omitted, exactly as a real
    /// builder omits it; a dispatch error `Ok(Err(_))` is included (the
    /// extrinsic was valid and paid). Callers that want the dropped count read
    /// it as `signed.len() - applied.len()`.
    fn author(&self, calls: Vec<RuntimeCall>) -> (Block, Vec<(ApplyExtrinsicResult, u32)>) {
        let mut sign_state = self.state();
        let mut state = self.state();
        self.call(
            &mut state,
            "Core_initialize_block",
            &self.header(1).encode(),
        );
        let mut xts = vec![Self::timestamp(1)];
        let (ts_out, _) =
            self.measured(&mut state, "BlockBuilder_apply_extrinsic", &xts[0].encode());
        assert_eq!(
            ApplyExtrinsicResult::decode(&mut &ts_out.expect("ts")[..]).expect("decodes"),
            Ok(Ok(())),
            "timestamp"
        );
        // Sign and apply lazily, stopping at the first builder rejection. Every
        // call is from one account with sequential nonces, so once the builder
        // rejects one (ExhaustsResources / block-length), every later call is a
        // future nonce it would also reject: the included set is the prefix, and
        // signing/applying the rest would only waste work. A dispatch error
        // `Ok(Err(_))` is a valid, paid extrinsic and is kept.
        let mut applied = Vec::new();
        for (n, call) in calls.into_iter().enumerate() {
            let xt = self.sign_in(&mut sign_state, call, n as Nonce);
            let (out, heap) =
                self.measured(&mut state, "BlockBuilder_apply_extrinsic", &xt.encode());
            let out = out.unwrap_or_else(|e| panic!("author traps on apply: {e}"));
            match ApplyExtrinsicResult::decode(&mut &out[..]).expect("decodes") {
                Err(_) => break,
                ok => {
                    applied.push((ok, heap));
                    xts.push(xt);
                }
            }
        }
        drop(sign_state);
        let out = self.call(&mut state, "BlockBuilder_finalize_block", &[]);
        let header = Header::decode(&mut &out[..]).expect("header");
        (
            Block {
                header,
                extrinsics: xts,
            },
            applied,
        )
    }

    /// Like `author`, but applies through the cached executor path (fast: no
    /// per-apply reinstantiation, no per-apply heap measured) and finalizes
    /// through the uncached path (a fresh instance, so the one rejected apply
    /// that stops the loop cannot leave the pooled instance dirty for finalize).
    /// Returns the block and the per-included-extrinsic apply results. Used by
    /// the combined test, which authors the full sweep plus hundreds of
    /// extrinsics and only needs the import high-water, not per-apply heaps.
    fn author_fast(&self, calls: Vec<RuntimeCall>) -> (Block, Vec<ApplyExtrinsicResult>) {
        let mut sign_state = self.state();
        let mut state = self.state();
        self.call(
            &mut state,
            "Core_initialize_block",
            &self.header(1).encode(),
        );
        let mut xts = vec![Self::timestamp(1)];
        self.call(&mut state, "BlockBuilder_apply_extrinsic", &xts[0].encode());
        // A real node block builder reserves weight for the mandatory
        // `on_finalize`/`on_idle` hooks and stops well before the Normal-class
        // budget is full; `BlockBuilder_apply_extrinsic` alone only enforces the
        // per-class budget, so an unbounded loop fills the block so full that
        // `execute_block`'s hooks then trap at import. Model the reservation:
        // stop once the accumulated block weight passes 75 % of `max_block` in
        // either the ref-time or proof-size dimension.
        let max_block =
            state.execute_with(|| <Runtime as frame_system::Config>::BlockWeights::get().max_block);
        let mut applied = Vec::new();
        for (n, call) in calls.into_iter().enumerate() {
            let xt = self.sign_in(&mut sign_state, call, n as Nonce);
            let out = self.call(&mut state, "BlockBuilder_apply_extrinsic", &xt.encode());
            match ApplyExtrinsicResult::decode(&mut &out[..]).expect("decodes") {
                Err(_) => break, // builder full; later same-account nonces also rejected
                ok => {
                    applied.push(ok);
                    xts.push(xt);
                }
            }
            let used =
                state.execute_with(|| frame_system::Pallet::<Runtime>::block_weight().total());
            if used.ref_time().saturating_mul(4) > max_block.ref_time().saturating_mul(3)
                || used.proof_size().saturating_mul(4) > max_block.proof_size().saturating_mul(3)
            {
                break;
            }
        }
        drop(sign_state);
        let (out, _) = self.measured(&mut state, "BlockBuilder_finalize_block", &[]);
        let header = Header::decode(&mut &out.expect("finalize")[..]).expect("header");
        (
            Block {
                header,
                extrinsics: xts,
            },
            applied,
        )
    }

    fn import(&self, block: &Block) -> (Result<Vec<u8>, String>, u32) {
        self.measured(&mut self.state(), "Core_execute_block", &block.encode())
    }

    fn execute(&self, program: &str, calldata: &[i64], step_limit: u64) -> RuntimeCall {
        pallet_xqvm::Call::<Runtime>::execute {
            program_hash: self.program(program),
            calldata: calldata.to_vec().try_into().expect("calldata bound"),
            output_slots: 0,
            step_limit,
        }
        .into()
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
    // (MaxVmMemory = 2 MiB) the whole-block VM growth stays under the 128 MiB
    // heap, so the auditor's attack block -- which trapped every importer at the
    // old 16 MiB cap -- now imports, directly and under every batch wrapper.
    // Reverting MaxVmMemory to 16 MiB fails this test: the block traps on import.
    assert!(
        max_vm_memory() <= 2 * 1024 * 1024,
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
    println!("family,L_MiB,params,charged_B,author_MiB,author_net_MiB,import_MiB,import_net_MiB,import_ratio_to_L,outcome");
    // Emulated caps stay below the compiled 2 MiB cap with headroom for the
    // k ~ 2 clone overhead (matching `c_block_order_sweep_at_emulated_caps`).
    // The old 16 MiB row is dropped: it exceeds the current cap, so it would
    // fault at `VmMemoryLimitExceeded` before reaching its intended allocation
    // and the measured footprint would be meaningless.
    for l in [MIB / 4, MIB / 2, MIB] {
        // Fifth tuple element: the designed outcome (None = success; Some(code)
        // = the exact post-allocation xqvm fault). energy=0 samples, `linear`
        // and `quad` complete; energy=1 samples and `vec` fault after allocating.
        let mut cases: Vec<(String, &str, Vec<i64>, u64, Option<u8>)> = Vec::new();
        for &(n, energy) in &[(l / 8, 0i64), (l / 8, 1), (l / 16 + 1, 1), (l / 32 + 1, 1)] {
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
        for &n in &[l / 32 + 1, l / 16] {
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
                    "{family} ({params}) at L={} MiB: expected success, got {}",
                    l / MIB,
                    outcome(r),
                ),
                Some(code) => assert!(
                    matches!(
                        r,
                        Ok(Err(sp_runtime::DispatchError::Module(m)))
                            if m.index == XQVM_PALLET_INDEX && m.error[0] == code
                    ),
                    "{family} ({params}) at L={} MiB: expected xqvm module error {code}, got {}",
                    l / MIB,
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
                "{family} ({params}) at L={} MiB: footprint {import_net} B < charged/2 \
                 ({} B) -- allocation did not reach its target (early VmMemoryLimitExceeded?)",
                l / MIB,
                charged / 2,
            );

            println!(
                "{family},{},{params},{charged},{:.3},{:.3},{:.3},{:.3},{:.2},{}",
                l / MIB,
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
/// 2 MiB cap.
fn order_sweep(chain: &Chain, l: u64, ms: u64) -> Vec<(String, RuntimeCall, u64)> {
    let mut out = Vec::new();
    let mut chunk = 8u64;
    let mut order = 0u32;
    while chunk <= l {
        let n = if chunk == 8 { 1 } else { chunk / 16 + 1 };
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
// Gap 1: largest LEGITIMATE run. The real inputs the architect named --
// conformance vectors and mempool transforms -- not the adversarial families.
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

    // --- Conformance run: a genuine, canonical program that verifies and runs ---
    // The checked-in `kimi-spine-control.bin` is a raw control-flow STRESS stream
    // (no XQBC header; wrapping it fails verification with StackDepthMismatch),
    // not a runnable conformance program -- decoding it as XQBC panics on
    // BadMagic. So the positive conformance vector is the seeded `linear`
    // program, which builds an n-variable model and sets n linear coefficients:
    // it verifies, is canonical, and runs to completion on chain.
    let conformance = prog_linear();
    assert!(
        xqvm::verifier::verify(&xqvm::Program::decode(&conformance).expect("linear decodes"))
            .is_ok(),
        "conformance program must verify"
    );
    let n: i64 = 4096;
    let (_, applied) = chain.author(vec![chain.execute("linear", &[n], ms)]);
    assert!(
        matches!(applied[0].0, Ok(Ok(()))),
        "conformance program must run successfully on chain: {}",
        outcome(&applied[0].0)
    );
    println!(
        "conformance (linear, n={n}): {} program bytes, author heap {:.3} MiB, ok",
        conformance.len(),
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
        // The `transformed` accumulator holds every solution's output at once
        // (<= QuantumMaxSolutions x QuantumMaxNodes bytes ~ 0.1 MiB) -- the
        // per-extrinsic retained term the MaxVmMemory docs and the pinned budget
        // test account for. Per-run VM residency is capped at
        // MaxTransformVmMemory; a single WasmExecutor address_space_used for the
        // whole `submit_solution` would need the order seeded in genesis and is
        // left to the submit_solution census path.
        println!(
            "real mempool transform: {max_solutions} x {max_nodes} spins ok, {} VM steps, retained ~{} KiB",
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

    // Seed the topology so the weight closure's RegisteredTopologies::get decodes it.
    let mut g = chain.genesis.clone();
    g.top.insert(
        pallet_quantum_pow::RegisteredTopologies::<Runtime>::hashed_key_for(topology_hash),
        meta_enc,
    );
    let sub = Chain {
        executor: Executor::builder().build(),
        genesis: g,
        genesis_hash: chain.genesis_hash,
        attacker: HybridPair::from_seed_slice(&[0xA7; 32]).unwrap(),
        programs: vec![],
    };
    let (empty, _) = sub.author(vec![]);
    let (_, empty_import) = sub.import(&empty);
    // submit_proof from an unregistered miner: the weight closure decodes the
    // full meta, apply decodes the proof, then errors on the Miners check.
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
// block at the shipped 2 MiB cap carrying, together: the mixed-family order
// sweep (samples across orders + SETLINE/SETQUAD BTreeMap-node runs +
// vec+EQUALITY), the ~2 MiB signed remark applied under a fresh nonce, and
// 2,049-byte-body fillers as far as weight/length allow. Asserts the block
// imports and prints the measured high-water against the 87.3 MiB budget sum.
// Also measures the mixed-family Σ at L = 2 MiB directly (not extrapolated).
// ---------------------------------------------------------------------------

#[test]
#[ignore = "V-A step-0 combined end-to-end: slow, run manually (owner-required)"]
fn g_combined_block_imports_at_cap() {
    let chain = Chain::new();
    let ms = max_steps();
    assert_eq!(
        max_vm_memory(),
        2 * 1024 * 1024,
        "combined test pins the shipped 2 MiB cap"
    );

    // (1) Mixed-family Σ at L = 2 MiB, measured standalone first.
    let sweep = order_sweep(&chain, 2 * MIB, ms);
    let sweep_calls: Vec<RuntimeCall> = sweep.iter().map(|(_, c, _)| c.clone()).collect();
    let (empty, _) = chain.author(vec![]);
    let (_, empty_import) = chain.import(&empty);
    let (sweep_block, _) = chain.author_fast(sweep_calls.clone());
    let (sweep_ok, sweep_heap) = chain.import(&sweep_block);
    let sweep_net = sweep_heap.saturating_sub(empty_import);
    println!(
        "Σ@2MiB: {} runs, importer net {:.3} MiB = {:.2}·L (L=2MiB), import {}",
        sweep.len(),
        mib(sweep_net),
        f64::from(sweep_net) / (2 * MIB) as f64,
        if sweep_ok.is_ok() { "ok" } else { "TRAP" }
    );
    assert!(sweep_ok.is_ok(), "mixed Σ sweep at 2 MiB must import");

    // (2) Combined block, authored the real way. `Chain::author` drops any
    // extrinsic the builder cannot fit (apply returns an outer Err such as
    // ExhaustsResources, or a future nonce left by an earlier drop) and
    // finalizes to a true header, so whatever it admits is by definition an
    // authorable block that a correct importer must accept. Contents: the full
    // 2 MiB-cap mixed-family sweep (all runs), the ~2 MiB-body signed remark
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
    // Enough fillers to reach the ~5 MiB length cap even if nothing else is
    // dropped; the author drops the surplus.
    let fillers: Vec<RuntimeCall> = (0..1200).map(|_| remark(filler_payload)).collect();
    let big = remark(big_payload);

    const TERM_SUM_MIB: f64 = 87.3;
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
            "COMBINED [{order}]: supplied {supplied}, included {included} (dropped {dropped} for weight/length), block xts {}, encoded {:.2} MiB; importer high-water {:.3} MiB (net {:.3}); import {import_status}",
            block.extrinsics.len(),
            block.encode().len() as f64 / MIB as f64,
            mib(heap),
            mib(net),
        );
        // A correct importer must accept a realistically-authored block. With
        // the on_finalize weight reserved in author_fast, this holds; a trap
        // here would be an author/importer divergence to stop and report.
        assert!(
            imported.is_ok(),
            "[{order}] authored block must import; got {import_status}",
        );
        assert!(
            mib(net) as f64 <= TERM_SUM_MIB,
            "[{order}] high-water {:.3} MiB exceeds the separately-summed terms {:.1} MiB -- STOP and report",
            mib(net),
            TERM_SUM_MIB,
        );
        assert!(
            mib(heap) as f64 <= HEAP_MIB * 0.75,
            "[{order}] high-water {:.3} MiB leaves under 25% of the {:.0} MiB heap free",
            mib(heap),
            HEAP_MIB,
        );
        // The block carries the ~2 MiB remark AND all `included - 1 - 22`
        // surplus fillers alongside the full 22-run sweep; the dropped ones are
        // surplus fillers beyond the builder's on_finalize weight reserve (the
        // block is weight-bound before the 5 MiB length cap). So the remark and
        // the full sweep DO co-occur in one importable block.
        let vm_plus_remark = 1 + sweep.len();
        assert!(
            included >= vm_plus_remark,
            "[{order}] the remark and the full {}-run sweep must both fit (included {included})",
            sweep.len(),
        );
        println!(
            "COMBINED [{order}]: remark + full {}-run sweep + {} fillers fit and import; {dropped} surplus fillers dropped (weight-bound)",
            sweep.len(),
            included - vm_plus_remark,
        );
    }
}

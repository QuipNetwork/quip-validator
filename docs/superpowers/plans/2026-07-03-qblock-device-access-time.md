# QBlock `device_access_time_us` Implementation Plan (quip-validator)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Miners self-report the compute time spent producing a winning proof (`device_access_time_us`: D-Wave QPU access time for QPU miners, wall-clock for CPU/GPU) inside `submit_proof`, and the value is persisted on the on-chain `QBlock` record.

**Architecture:** Add a trailing `device_access_time_us: u64` field to the three structs the value flows through — `QuantumProof` (extrinsic payload) → `ProofRecord` (`BlockBestProof`, intra-block best) → `QBlock` (persisted per-win record, readable via the existing `qblock_by_id` runtime API). Everything rides the **undeployed** runtime spec 111: the existing v3→v4 `QBlocks` migration is amended to also backfill the new field with 0, and `transaction_version` moves 4→5 because `submit_proof`'s argument encoding changes relative to the deployed spec 110.

**Tech Stack:** Substrate FRAME pallet (`pallets/quantum-pow`), SCALE codec, Rust 2021.

## Global Constraints

- Branch: `v0.2` of `/Users/carback1/Code/quip/quip-validator`. Feature branch off it; GitLab (`origin`) is the only remote that matters for MRs.
- Runtime spec 111 is **not deployed** (chain runs 110, shipped in v0.2.1-rc11). Do NOT bump `spec_version`; amend 111's changes. QuantumPow pallet storage version stays 4 (v4 never shipped either).
- `transaction_version` must move 4 → 5 in the same change (deployed 110 encodes `submit_proof` without the new field).
- Field name is exactly `device_access_time_us` (u64, microseconds). `0` = unreported. No validation/cap on the value — it is self-reported observability, same trust model as `MinerRegistry.participate`'s `budget_seconds`.
- The `BlockWinner` event shape is unchanged (consumers read the field via the `qblock_by_id` runtime API / `QBlocks` storage).
- No LLM co-author lines in commit messages. Imperative mood, ≤72-char subject.
- Zero warnings: `cargo clippy` clean, `cargo fmt` applied.

## Non-Goals (follow-up plans, other repos)

- **quip-protocol (Python miner):** encode the field in `submit_proof` call params, decode the two new trailing `QBlock` fields (`topology_hash` — already in 111 — and `device_access_time_us`), thread the value from `_sum_qpu_access_us` / wall clock into `MiningResult`. Separate plan.
- **dashboard.quip.network:** read the field from the qblock runtime API in the winners plugin and use it as `blocks.mining_time` (derived block-spacing stays the fallback). Separate plan.

---

### Task 1: Field propagation — `QuantumProof` → `ProofRecord` → `QBlock`

**Files:**
- Modify: `pallets/quantum-pow/src/types.rs` (structs `QuantumProof` ~line 28, `ProofRecord` ~line 113, `QBlock` ~line 193)
- Modify: `pallets/quantum-pow/src/lib.rs` (`submit_proof` `ProofRecordOf` construction ~line 965; `on_finalize` `QBlocks::insert` ~line 588)
- Modify: `pallets/quantum-pow/src/tests.rs` (fixtures: `finalize_winner` ~line 197, proof builder `QuantumProof {` ~line 253, per-topology `ProofRecord {` ~line 2206)
- Modify: `pallets/quantum-pow/src/benchmarking.rs` (`types::QuantumProof {` ~line 132)
- Test: `pallets/quantum-pow/src/tests.rs` (new test)

**Interfaces:**
- Consumes: existing `QuantumProof<PackedSolutions>`, `ProofRecord<AccountId, BlockNumber>`, `QBlock<AccountId, Balance, BlockNumber>`.
- Produces: each struct gains trailing `pub device_access_time_us: u64`. Task 2's migration and the follow-up Python/dashboard plans rely on the field being the **last** field of `QBlock` (after `topology_hash`) and the last field of `QuantumProof` (after `solutions`).

- [ ] **Step 1: Write the failing test**

Add to `pallets/quantum-pow/src/tests.rs`, next to the other `finalize_winner`-based tests. It plants a best proof carrying a device time and asserts the finalized `QBlock` persists it:

```rust
#[test]
fn qblock_persists_device_access_time() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        DefaultTopology::<Test>::put(hash);
        System::set_block_number(80);
        BlockBestProof::<Test>::put(ProofRecord {
            miner: 1,
            submitted_at: 80,
            energy_milli: -10_000,
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 1_234_567,
        });
        QuantumPow::on_finalize(80);

        let qblock = QBlocks::<Test>::get(80).expect("qblock stored");
        assert_eq!(qblock.device_access_time_us, 1_234_567);
    });
}
```

(If `registered_topology()` in this file needs a difficulty seeded first, mirror the setup lines of the nearest passing `on_finalize` test — e.g. the per-topology test around line 2200 — rather than inventing new setup.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p pallet-quantum-pow qblock_persists_device_access_time`
Expected: COMPILE ERROR — `ProofRecord` has no field `device_access_time_us` (this is the Rust analog of a failing test).

- [ ] **Step 3: Add the field to the three structs in `types.rs`**

`QuantumProof` (~line 28) — trailing field, after `solutions`:

```rust
pub struct QuantumProof<PackedSolutions> {
    pub topology_hash: H256,
    pub nonce: U256,
    pub salt: [u8; 32],
    pub solutions: PackedSolutions,
    /// Miner-reported compute time spent producing this proof, in
    /// microseconds. QPU miners report the summed D-Wave QPU access time
    /// across the solution's attempts; CPU/GPU miners report wall-clock
    /// mining time. Self-reported observability (same trust model as
    /// `MinerRegistry.participate`'s `budget_seconds`) — the chain cannot
    /// verify it and consensus never reads it. `0` = unreported.
    pub device_access_time_us: u64,
}
```

`ProofRecord` (~line 113) — trailing field, after `topology_hash`:

```rust
    /// Miner-reported compute time from the accepted proof, in microseconds
    /// (see `QuantumProof::device_access_time_us`). Carried here so
    /// `on_finalize` can persist it into `QBlocks` without re-reading the
    /// extrinsic body.
    pub device_access_time_us: u64,
```

`QBlock` (~line 193) — trailing field, after `topology_hash`:

```rust
    /// Miner-reported compute time spent producing the winning proof, in
    /// microseconds — QPU access time for QPU wins, wall clock for CPU/GPU
    /// wins. Copied from the accepted [`ProofRecord`]. Self-reported and
    /// unverifiable; `0` = unreported (including all pre-111 blocks, which
    /// the v4 migration backfills with 0). Wall-clock mining duration
    /// remains derivable from block spacing (`LastProofBlock` deltas), so
    /// no information is lost by carrying compute time here instead.
    pub device_access_time_us: u64,
```

- [ ] **Step 4: Thread the value through `lib.rs`**

In `submit_proof` (~line 965), copy from the proof:

```rust
            let record = ProofRecordOf::<T> {
                miner: who.clone(),
                submitted_at: frame_system::Pallet::<T>::block_number(),
                energy_milli: validation.best_energy_milli,
                salt: proof.salt,
                topology_hash: proof.topology_hash,
                device_access_time_us: proof.device_access_time_us,
            };
```

In `on_finalize` (~line 588), persist into the QBlock:

```rust
            QBlocks::<T>::insert(
                n,
                types::QBlock {
                    miner: record.miner.clone(),
                    salt: record.salt,
                    energy_milli: record.energy_milli,
                    reward,
                    submitted_at: record.submitted_at,
                    difficulty: active,
                    last_proof_block_hash,
                    topology_hash,
                    device_access_time_us: record.device_access_time_us,
                },
            );
```

- [ ] **Step 5: Fix every fixture the compiler flags**

- `tests.rs` `finalize_winner` (~line 197): add `device_access_time_us: 0,` to the `ProofRecord`.
- `tests.rs` proof builder (~line 253): add `device_access_time_us: 0,` to the `QuantumProof`.
- `tests.rs` per-topology test (~line 2206): add `device_access_time_us: 0,` to the `ProofRecord`.
- `benchmarking.rs` (~line 132): add `device_access_time_us: 0,` to the `types::QuantumProof`.
- Let `cargo test -p pallet-quantum-pow` (compile phase) find any remaining sites; fix identically with `device_access_time_us: 0,`.

- [ ] **Step 6: Run the new test and the pallet suite**

Run: `cargo test -p pallet-quantum-pow`
Expected: `qblock_persists_device_access_time` PASSES; **`migration_v3_to_v4_backfills_qblock_topology` FAILS** (the translate in `migration::v4` doesn't produce the new field yet — the compiler flags the `types::QBlock` construction inside the migration; add `device_access_time_us: 0,` there only if needed to compile, but leave its test assertions to Task 2). If the whole suite passes already because you added the field to the migration, that's fine — Task 2 still hardens it.

- [ ] **Step 7: Commit**

```bash
git add pallets/quantum-pow/src/types.rs pallets/quantum-pow/src/lib.rs \
        pallets/quantum-pow/src/tests.rs pallets/quantum-pow/src/benchmarking.rs
git commit -m "feat(quantum-pow): persist miner-reported device_access_time_us on QBlock"
```

---

### Task 2: Fold the backfill into the v3→v4 migration

**Files:**
- Modify: `pallets/quantum-pow/src/lib.rs` (`migration::v4` module ~line 1430; `on_runtime_upgrade` doc comment ~line 435)
- Test: `pallets/quantum-pow/src/tests.rs` (`migration_v3_to_v4_backfills_qblock_topology` ~line 1130)

**Interfaces:**
- Consumes: `types::QBlock` with `device_access_time_us` from Task 1; existing `OldQBlock` decode struct (pre-v4 = **deployed 110** layout — unchanged).
- Produces: `migration::v4::backfill_qblock_fields::<T>()` (renamed from `backfill_topology`) — the only call site is `on_runtime_upgrade`.

- [ ] **Step 1: Extend the migration test (failing first)**

In `migration_v3_to_v4_backfills_qblock_topology` (~line 1130), after the `topology_hash` assertion, add:

```rust
        // Pre-111 blocks carry no self-reported compute time — backfilled 0.
        assert_eq!(migrated.device_access_time_us, 0);
```

Also add, at the top of the test body right after `StorageVersion::new(3).put::<QuantumPow>();`:

```rust
        // A stale pre-111 BlockBestProof would decode-fail post-upgrade
        // (ProofRecord gained a trailing field); v4 kills it like v3 did.
        let old_record_key = frame_support::storage::storage_prefix(b"QuantumPow", b"BlockBestProof");
        frame_support::storage::unhashed::put(&old_record_key, &[7u8; 4]);
```

and after `QuantumPow::on_runtime_upgrade();`:

```rust
        assert!(BlockBestProof::<Test>::get().is_none());
```

- [ ] **Step 2: Run to verify state**

Run: `cargo test -p pallet-quantum-pow migration_v3_to_v4`
Expected: FAIL — either the `device_access_time_us == 0` assertion (if Task 1 left the migration compiling with a different value) or the `BlockBestProof` kill assertion.

- [ ] **Step 3: Amend `migration::v4`**

Rename `backfill_topology` → `backfill_qblock_fields` and extend the translate + add the kill (in `pallets/quantum-pow/src/lib.rs` ~line 1430):

```rust
        /// 3 → 4: re-encode every `QBlocks` entry, backfilling the two fields
        /// added since the deployed layout: `topology_hash` with the default
        /// topology (`H256::zero()` when none is set — blocks won before
        /// per-topology binding were all mined against the default, so this
        /// is the historically-correct value) and `device_access_time_us`
        /// with 0 (pre-111 miners never reported compute time). Also kills
        /// any stale `BlockBestProof`: `ProofRecord` gained a trailing field,
        /// and while the value is always empty across an upgrade boundary
        /// (`on_finalize` take()s it every block) and OptionQuery decode
        /// failure reads as `None`, `kill()` removes all doubt for free —
        /// same reasoning as the v3 step.
        pub(crate) fn backfill_qblock_fields<T: Config>() -> Weight {
            let backfill = DefaultTopology::<T>::get().unwrap_or_default();
            let mut count = 0u64;
            QBlocks::<T>::translate::<OldQBlock<AccountIdOf<T>, BalanceOf<T>, BlockNumberOf<T>>, _>(
                |_block, old| {
                    count = count.saturating_add(1);
                    Some(types::QBlock {
                        miner: old.miner,
                        salt: old.salt,
                        energy_milli: old.energy_milli,
                        reward: old.reward,
                        submitted_at: old.submitted_at,
                        difficulty: old.difficulty,
                        last_proof_block_hash: old.last_proof_block_hash,
                        topology_hash: backfill,
                        device_access_time_us: 0,
                    })
                },
            );
            crate::BlockBestProof::<T>::kill();
            // One read + one write per entry, plus the `DefaultTopology`
            // read and the `BlockBestProof` kill.
            T::DbWeight::get().reads_writes(count.saturating_add(1), count.saturating_add(1))
        }
```

Update the call site in `on_runtime_upgrade` (~line 474):

```rust
            weight = weight.saturating_add(crate::migration::v4::backfill_qblock_fields::<T>());
```

And extend the `on_runtime_upgrade` doc comment's v3→v4 bullet (~line 447) to read:

```rust
        /// v3 → v4: `QBlock` gains trailing `topology_hash` and
        /// `device_access_time_us`. Existing entries were encoded without
        /// them and would otherwise fail to decode (silently reading back as
        /// `None`), so every `QBlocks` value is re-encoded — `topology_hash`
        /// backfilled with the default topology (the only topology mineable
        /// before per-topology binding), `device_access_time_us` with 0
        /// (never reported pre-111). Stale `BlockBestProof` is killed
        /// (`ProofRecord` also changed shape).
```

Note the `OldQBlock` struct is untouched: "pre-v4" *is* the deployed 110 layout, which never had either field.

- [ ] **Step 4: Run the migration tests**

Run: `cargo test -p pallet-quantum-pow migration`
Expected: all four migration tests PASS (`v2_to_v3`, `below_v2`, `v3_to_v4`, `noop_at_v4`).

- [ ] **Step 5: Verify try-runtime hooks still compile**

Run: `cargo check -p pallet-quantum-pow --features try-runtime`
Expected: clean. (`pre_upgrade`/`post_upgrade` count `QBlocks` via `iter_keys`, which decodes only keys — unaffected by the value-shape change.)

- [ ] **Step 6: Commit**

```bash
git add pallets/quantum-pow/src/lib.rs pallets/quantum-pow/src/tests.rs
git commit -m "feat(quantum-pow): backfill device_access_time_us in v4 migration"
```

---

### Task 3: Runtime version bookkeeping — amend the 111 note, bump `transaction_version`

**Files:**
- Modify: `runtime/src/lib.rs` (`RuntimeVersion` const, ~lines 119-136)

**Interfaces:**
- Consumes: nothing from other tasks (documentation + version constants only).
- Produces: `transaction_version: 5`. The Python miner plan targets exactly this: `submit_proof` on spec 111 requires the `device_access_time_us` param and tx_version 5.

- [ ] **Step 1: Amend the spec-111 comment and bump `transaction_version`**

In the `Bumped to 111` comment block, extend the first bullet and replace the closing line. The block currently ends with:

```rust
    // - `submit_proof` weight becomes dimension-scaled (QIP-03): charged
    //   weight now depends on the registered topology's node/edge counts and
    //   the proof's solution count instead of a flat 60M placeholder.
    // No existing call encodings change, so `transaction_version` stays at 4.
    spec_version: 111,
```

Change to:

```rust
    // - `submit_proof` weight becomes dimension-scaled (QIP-03): charged
    //   weight now depends on the registered topology's node/edge counts and
    //   the proof's solution count instead of a flat 60M placeholder.
    // - `QuantumProof` gains a trailing `device_access_time_us: u64`
    //   (miner-reported compute time: QPU access time for QPU wins, wall
    //   clock for CPU/GPU), carried through `ProofRecord` and persisted as a
    //   trailing field on `QBlock` (same v3 → v4 re-encode migration,
    //   backfilled with 0). Read-only runtime API shape change
    //   (`QBlock`/`QBlockWithNonce`) on top of the topology_hash one above.
    // `submit_proof`'s argument encoding changed, so `transaction_version`
    // moves to 5.
    spec_version: 111,
```

And in the same const:

```rust
    transaction_version: 5,
```

- [ ] **Step 2: Build the runtime**

Run: `cargo check -p quip-runtime`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add runtime/src/lib.rs
git commit -m "chore(runtime): note device_access_time_us in 111, bump tx_version to 5"
```

---

### Task 4: Full verification pass

**Files:** none new — verification only.

- [ ] **Step 1: Full workspace test + lints**

Run, in order, expecting each clean/green:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p pallet-quantum-pow
cargo test --workspace
```

If `cargo fmt` flags files you touched, run `cargo fmt --all` and amend nothing — create a `style:` commit.

- [ ] **Step 2: Benchmarks compile**

Run: `cargo check -p pallet-quantum-pow --features runtime-benchmarks`
Expected: clean (the benchmark proof fixture gained the field in Task 1).

- [ ] **Step 3: Commit any stragglers, push branch, open MR against `v0.2`**

```bash
git push -u origin feat/qblock-device-access-time
glab mr create --target-branch v0.2 --title "feat(quantum-pow): device_access_time_us on QBlock (runtime 111)" \
  --description "Adds miner-reported device_access_time_us (u64, µs) to QuantumProof → ProofRecord → QBlock. Rolled into undeployed spec 111: v4 migration backfills 0, transaction_version 4 → 5. QPU miners report D-Wave QPU access time; CPU/GPU report wall clock. Value is self-reported observability — consensus never reads it."
```

---

## Deployment ordering note (for the release that ships 111)

The runtime upgrade and the miner release are coupled: once 111 activates, spec-110 miners can no longer encode `submit_proof` (missing field), and 111-format miners cannot submit to a 110 chain. Same-window deploy: upgrade the runtime, then roll miners — identical to how the 108 `set_difficulty` encoding change shipped. The Python-side plan (follow-up) must also decode the two new trailing `QBlock` fields, since the current `_decode_winning_solution_with_nonce` hard-fails on trailing bytes.

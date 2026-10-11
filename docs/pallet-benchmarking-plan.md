# Pallet Benchmarking Plan

## Context

[MR !57](https://gitlab.com/quip.network/quip-validator/-/merge_requests/57)
adds reference-machine CI jobs that discover benchmarkable pallets, regenerate
their weight files on `node02`, and push the results back to the merge request
branch.

The infrastructure is ready, but several pallet benchmarks must be corrected or
expanded before wholesale regeneration is safe. Passing the pallet benchmark
test suites is not sufficient: those tests use mock runtime constants that
differ from production.

## Scope

This plan covers:

- `pallet_quantum_compute_mempool`
- `pallet_quantum_pow`
- `pallet_miner_registry`
- runtime block and extrinsic execution overhead
- benchmark CI preflight and verification

`pallet_template` already has usable generated weights and requires no further
work.

`pallet_faucet_ops` and `pallet_emission_controller` now have benchmark modules,
`runtime-benchmarks` features, and runtime registrations. Phase 1 adds authority,
fuse, budget, and scheduled issuance storage access, so the previous reused
Balances weight is no longer sufficient. Run the reference-machine job for their
`mint`, `disable`, `accrue`, and `reward` paths before release; initial estimates
are provisional. See `validator-onboarding-phase-1.md`.

`pallet_xqvm` is intentionally out of scope because it is not actively used.
Its execution and per-step weights should be reviewed before the pallet becomes
active.

SDK pallets continue using their upstream weight implementations. Generating
runtime-specific SDK pallet weights remains future work.

Runtime execution overhead is runtime-specific even while SDK dispatchables use
upstream `SubstrateWeight` implementations. The reference-machine workflow now
generates `BlockExecutionWeight` and `ExtrinsicBaseWeight` with `benchmark
overhead`, and the runtime wires those values into its block-weight limits.

## Current Gaps

| Pallet | Gap |
| --- | --- |
| `quantum-compute-mempool` | Five benchmarks fail under production runtime constants because setup uses reward `100`, below the configured minimum of `UNIT`. Its flat weight methods also do not represent topology, solution, ranking, and payout complexity. |
| `quantum-pow` | The pallet is skip-listed. Its benchmark topology has two nodes while production requires at least sixteen. `register_topology` has variable work but a flat weight, and `submit_proof` contains nonlinear `solutions × nodes`, `solutions × edges`, and `solutions² × nodes` work that the normal additive template cannot model directly. |
| `miner-registry` | Missing from the benchmark registry and has no benchmark feature/module. All dispatchable weights are placeholders, while descriptor processing is variable-sized. |
| CI preflight | Mock benchmark tests do not exercise production constants. The reference-machine job can therefore discover setup failures only after a costly release build. |

## Implementation Plan

### 1. Make benchmark setup runtime-safe

For `quantum-compute-mempool`:

- Replace hardcoded rewards with `T::MinReward::get()`.
- Fund benchmark accounts with enough balance for the production minimum,
  reserves, and payouts.
- Keep setup helpers generic over the configured balance type.
- Exercise all eight dispatchables successfully through the production runtime.

For `quantum-pow`:

- Generate connected topologies with at least the configured minimum node count.
- Generate matching edges and packed solutions for the selected dimensions.
- Ensure every benchmark setup follows the production topology and mining
  invariants.

### 2. Establish safe dynamic weight contracts

Before regenerating weights, audit and reduce avoidable nonlinear work in shared
validation code. Repeated linear node lookups can turn topology and energy
validation into `edges × nodes` or `solutions × edges × nodes` work.

For `quantum-compute-mempool`:

- Parameterize `propose_job` for topology and bid-list dimensions.
- Parameterize `submit_solution` for topology and solution dimensions.
- Cover the worst-case top-N ranking path.
- Benchmark `claim_reward` with the maximum supported winner count, including
  all balance transfers and solver updates.
- Use conservative fixed worst-case weights where a required dimension cannot
  be obtained safely before dispatch.
- Add a post-dispatch refund for `submit_solution`: keep the configured maxima
  as the pre-dispatch upper bound, then return the actual weight through
  `DispatchResultWithPostInfo` once the stored topology and transformed-solution
  dimensions are known. Calibrate the refund path on `node02`.

For `quantum-pow`:

- Parameterize `register_topology` for nodes, edges, and allowed-value inputs.
- Keep `submit_proof` dimension-aware.
- Do not replace its nonlinear formula with the additive output produced by
  three ordinary `Linear<>` components.
- Calibrate the nonlinear terms on the reference machine using targeted
  benchmark sweeps, or refactor the validation algorithm into a workload model
  that can be benchmarked and charged safely.
- Preserve the custom `submit_proof` composition during regeneration, either
  through a dedicated generated module or an explicit custom-template path.

### 3. Add missing pallet coverage

For `miner-registry`:

- Add benchmark dependencies, feature wiring, and a benchmark test suite.
- Benchmark `set_descriptor` using the worst relevant V2 descriptor shape,
  including replacement and deposit adjustment.
- Benchmark `clear_descriptor` with a stored descriptor, reserved deposit, and
  participation record.
- Benchmark `participate` with a valid descriptor and candidate qblock.
- Decide whether descriptor weight should use encoded size/components or a
  conservative maximum before changing the `WeightInfo` signature.
- Add the pallet to `define_benchmarks!`.

### 4. Add fast runtime preflight

Add a non-pushing, low-resolution benchmark mode that:

1. Builds the node with `runtime-benchmarks`.
2. Discovers every registered pallet and extrinsic.
3. Executes each benchmark with low `steps` and `repeat` values.
4. Writes generated output to a temporary directory.
5. Verifies that expected weight signatures are preserved.

This preflight must use the production runtime, not only pallet mock runtimes.
The full reference-machine job remains responsible for final measurements.

Add reference-machine CI coverage for
`scripts/run-quantum-pow-sweeps.sh`. The sweep job must:

1. Run on the serialized `node02` benchmark runner, not an ordinary shared
   runner, using the release node built with `runtime-benchmarks`.
2. Execute every fixed-point `submit_proof` sweep used to calibrate the
   nonlinear nodes, edges, solution, and cross-term coefficients.
3. Retain the raw JSON results and a compact min/median/max summary as CI
   artifacts so reviewers can compare calibrations across runs.
4. Share the reference-machine resource lock with weight regeneration so the
   two measurement workloads cannot overlap.
5. Be available alongside the manual weight-regeneration workflow, with any
   scheduled calibration policy documented explicitly.

Completed on 2026-07-27: `benchmark-quantum-pow-sweeps` now builds the release
node with `runtime-benchmarks` on the serialized `node02` bench runner, executes
all six fixed points, and retains the raw JSON files plus
`quantum-pow-sweeps/summary.tsv`. The job is manual on same-repo merge requests;
scheduled execution is explicit opt-in via `QUANTUM_POW_SWEEPS=true`. The sweep
script validates its tooling, output structure, fixed-point dimensions, sample
data, and complete summary before reporting success. It does not write tracked
weight files.

### 5. Regenerate and verify

After the benchmark contracts are complete:

1. Remove pallets from `SKIP_PALLETS` only when their full runtime benchmark
   run succeeds and regenerated signatures compile.
2. Run the complete `50` step, `20` repeat benchmark suite on `node02`.
3. Review proof-size estimates, database read/write counts, regression errors,
   and maximum-call weights.
4. Compile the regenerated runtime.
5. Run:
   - `cargo fmt --check`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test`
   - the release runtime-benchmarks build

### 6. Measure runtime execution overhead

The reference-machine regeneration also runs `benchmark overhead` against the
release benchmark node with the development chain preset and compiled Wasm. It
writes `runtime/src/weights/block_weights.rs` and
`runtime/src/weights/extrinsic_weights.rs`; the benchmark bot includes both in
the same weight commit as pallet results.

The runtime explicitly uses those generated constants as `base_block` and as
the `base_extrinsic` for every dispatch class. Its existing capacity policy is
unchanged: a two-second maximum block, 75% Normal allowance, the remaining 25%
reserved for Operational transactions, and a 10% average initialization
allowance.

The shared-runner preflight executes a one-repeat overhead smoke test with one
extrinsic per block and writes only to its temporary directory. This checks the
CLI/runtime plumbing but is not a source of coefficients. Only the serialized
reference-machine job may update the tracked overhead files.

## Delivery Order

Use small reviewable changes:

1. Runtime-safe mempool and quantum-pow fixtures plus runtime preflight.
2. Mempool complexity/weight model and regenerated weights.
3. Quantum-pow complexity/weight model and regenerated weights.
4. Miner-registry benchmark coverage.
5. Quantum-pow reference-machine sweep CI integration.
6. Final all-pallet reference-machine regeneration and CI cleanup.
7. Reference-machine block and extrinsic overhead generation and runtime wiring.

## Completion Criteria

The work is complete when:

- Every in-scope custom pallet is registered in `define_benchmarks!`.
- Every dispatchable has a benchmark-backed weight contract.
- No in-scope pallet remains in the default skip list.
- Low-resolution runtime preflight succeeds for every registered benchmark.
- The overhead smoke test generates both expected files without modifying the
  tracked runtime weights.
- Reference-machine regeneration updates and commits both runtime overhead
  constants alongside pallet weights.
- Quantum-pow nonlinear sweeps run through CI on `node02`, with raw JSON
  results retained as artifacts.
- Full node02 regeneration succeeds without manual pallet exclusions.
- Regenerated weights compile and all repository checks pass.

## Validator onboarding Phase 2

The runtime now registers admission, Foundation collective/membership and classic
staking benchmarks. `pallets/validator-admission/src/weights.rs` is provisional;
run the reference-machine job before release. The admission snapshot adapter in
`runtime/src/configs/validator_onboarding.rs` additionally reserves a conservative
bounded read/CPU/proof budget for at most 32 accounts; calibrate it against the
maximum approved set with keys and staking ledgers. Upstream governance/staking
weights are baselines, not Quip reference-machine measurements. Nomination
benchmark capacity is a measurement parameter only; the live nomination cap
remains Some(0).


## Permissionless settlement: outstanding merge and release timing gate

`claim_reward` remains **blocked for merge and release pending reference timing**. After
rebasing onto main `7ac712699d3deefeb8cc70edadd75d2e1ae9aca6`, its
`1_705_641_000` ps base and all unrelated measurements are retained from main's
fresh benchmark output (e87f8f0). That claim benchmark whitelisted the proposer,
used smaller topology and equal payouts; it does not establish a timing bound
for the maximal permissionless or dust cases. The earlier pre-rebase timing
`1_737_510_000` is superseded and is not restored. No local diagnostic run is
being represented as reference regeneration.

The public mempool `weights.rs` composes generated `benchmark_weights.rs`,
including main's `finalise_expired_orders(k)` method unchanged. Only settlement's
generated DB accounting is adjusted in both implementations: main's 67 reads /
67 writes become 68 / 68 because the proposer account is no longer whitelisted.
This is an access-model correction pending fresh measurement, **not a full
weight regeneration**. The old lazy-expiry 68 / 69 count no longer applies:
result publication now runs in `on_initialize`, outside the claim. The wrapper
has no permanent +1 DB adjustment: future reference generation writes directly
to `benchmark_weights.rs` and supplies the complete DB counts. The resolver and
preflight protect the public wrapper.

### Proof envelope and integrity checks

The pinned SDK (`845eb9986c7131056b077d14ad7249f9d65ca111`), in
`substrate/utils/frame/benchmarking-cli/src/pallet/writer.rs`, takes the maximum
proof estimate across prefixes. The retained generated claim proof is 644411
bytes. The pre-rebase production-sized preflight recorded 646565, demonstrating
why a maximum across prefixes was insufficient; that observation is historical,
not a measurement of this rebased implementation. Regeneration alone does not
repair that estimator behavior. Settlement uses a maintained 1 MiB proof floor instead;
any larger generated proof estimate is preserved.

The floor covers an additive envelope derived from the runtime's actual storage
metadata (`StorageInfo::max_size` includes the map key/hasher part and the value's
`MaxEncodedLen`), rather than one dominating prefix:

| Storage | Distinct keys | Maximum encoded size per key |
|---|---:|---:|
| JobOrders | 1 | 640946 |
| JobSpecs | 1 | 297 |
| OrderTopSolvers, or the larger ranking representation | 1 | 1305 |
| System Account (proposer + winners) | 33 | 128 |
| Solvers | 32 | 109 |
| OpenOrders (removal included conservatively) | 1 | 24 |

These 69 keys total 650284 bytes. Result publication is hook work, so
`OrderResults` is no longer part of the claim envelope. Charge the SDK's largest u32 map-cardinality
bucket (eight radix-16 layers), plus two additional/global layers, **separately
for every key**, without claiming trie-node sharing:
`(8 + 2) * 15 * 33 = 4950` bytes per key. Conservatively include the whitelisted
16-byte TotalIssuance value and another 4950 bytes. The result is
`650284 + 69 * 4950 + 16 + 4950 = 996800`, below 1048576.

This is a conservative envelope **under the SDK's stated estimation model**, not
an enforced storage-cardinality cap or an unconditional bound for every possible
adversarial trie shape. A protocol requirement for an assumption-free trie bound
needs a separate codec/path analysis. No SDK-wide estimator change is included.

The pallet's integrity hook derives the envelope from configured encoded types
and limits, rejects an insufficient floor or a weight provider that bypasses it,
pins the winner limit underlying the modeled DB counts, and checks the weight
against normal max-extrinsic and block allowances in both dimensions. Runtime
tests additionally check the actual dispatch weight.
Currency, VM or account-hook changes require a fresh access/timing review.

### Lifecycle cases to measure

Measure both `claim_reward` (surviving, well-funded proposer) and the extra
`claim_reward_dust` case. Both settle 32 weighted winners with maximum topology,
bid list and callback endpoint sizes. Both run the expiry hook before the measured
claim and verify that its prepublished result survives settlement. Neither case
is asserted to be slower before reference measurements.

A reserve creates a consumer in the pinned balances implementation. Lowering a
sole-provider reserved account's free balance below ED is rejected with
`ConsumerRemaining`; forcing that state would be an invalid benchmark setup.
The dust case first establishes an independent provider, then leaves ED-1 free
with the reward still reserved. Settlement burns the residual dust, removes the
reserve consumer and releases the balance provider. The independent provider
keeps the System account alive; removing it subsequently reaps the System account.
That last removal is outside settlement and is not claimed as benchmarked work.
The mock's ED=1 exercises zero-dust provider removal; a runtime test with the
production ED verifies actual nonzero dust burning and subsequent reaping.

The H4 Executive failure test also covers a later payout failure: the first
winner's transfer/counter/event roll back together. Hook expiry, the removed
open-order index and the prepublished result remain committed, while the
failed signed transaction's fee, nonce, System failure event and fee event persist.
Removing the lock and retrying at the next nonce settles once. The dispatchable
uses the pinned FRAME macro's `with_storage_layer`; no redundant transaction
wrapper was added.

### Reference procedure, when access and scheduling are authorized

The current `benchmark-weights` CI job always invokes `push-weights.sh`, which
commits and pushes. Do not trigger it while the no-push instruction applies. No verified direct node02 access or no-push settlement CI job was
available during this work. The following is a pending operator procedure, not
a completed reference run:

1. Reserve exclusive use of node02 coordinated with the CI
   `reference-machine` resource lock. Use its normal pinned toolchain/image and
   a temporary checkout of the exact base plus the reviewed uncommitted patch.
2. Record base commit, a full patch hash (including new `benchmark_weights.rs`),
   source/lockfile hashes, SDK revision, Rust version, runner/image identity,
   machine-check evidence and a SHA256 of the fresh benchmark Wasm.
3. Build a release runner and runtime with the normal reference fixture tooling:

```bash
cargo build --release --features runtime-benchmarks -p quip-network-node
```

4. Run both cases into temporary artifacts; do not overwrite tracked weights or
   call `push-weights.sh`:

```bash
benchmark_output=$(mktemp -d)
for settlement_case in claim_reward claim_reward_dust; do
  target/release/quip-network-node benchmark pallet \
    --runtime target/release/wbuild/quip-protocol-runtime/quip_protocol_runtime.wasm \
    --genesis-builder runtime \
    --pallets pallet_quantum_compute_mempool \
    --extrinsic "$settlement_case" --extra \
    --steps 50 --repeat 20 \
    --json-file "$benchmark_output/$settlement_case.json" \
    --output "$benchmark_output/$settlement_case.rs"
done
```

Retain command lines, stdout, raw samples and artifact hashes. Review the timing
and DB envelope across both cases, then update only settlement's generated timing
and access terms from trustworthy evidence while preserving the proof wrapper.
Check that the resulting dispatch weight still fits normal limits; run tests,
strict Clippy, formatting and wrapper guards. Merge and release acceptance remain deferred
until that evidence exists. Local diagnostic builds may skip unrelated Revive
fixtures when `solc` is absent; that is not a substitute for reference provenance.

### Follow-up A timing gate

Strict-improvement submission now decodes the previous bounded `OrderSolutions`
value and uses best-anchored selection. No new storage key is introduced, but
execution cost changes. The retained `submit_solution` measurements are a
baseline, not regeneration of this path. Remeasure both first submissions and
maximal accepted replacements before merge; rejected submissions must also fit
the charged maximum. Quantum-PoW selection is unchanged.

### Follow-up B closure access and proof correction

The current claim model is **69 reads / 69 writes**: the earlier 68/68 plus one
bounded `ProposerOrders` read/write. Refund is **2 reads / 3 writes** under its
caller-account whitelist, up from 1/2. These are source-based corrections to the
retained generated baseline, not fresh measurements. Both closure benchmarks
scan a full proposer index and refund now includes maximum topology. Missing
reserve failure also needs coverage in reference measurement. Reference timing
for both closure paths must be regenerated before merge.

The production claim envelope adds one `ProposerOrders` key (max_size 305 plus
4950 trie bytes) to 996800, giving **1002055**. The refund envelope is
`640946 + 305 + 128 + 24 + 16 + 5 * 4950 = 666169` for JobOrders,
ProposerOrders, System Account, OpenOrders and TotalIssuance. Both use a maintained
**1 MiB** floor and runtime integrity checks against the actual storage metadata
and Normal dispatch limits. Claim/refund do not read stored solutions, poll
results or payees. The same documented trie-model assumptions apply.

### QUI-1512 D: signed destination metadata and submission evidence

Both submit entry points share validation and accepted-write logic. Their model
adds one requirement read and one payee write/delete: 6 reads / 7 writes relative
to the prior 5 / 6 generated model (System bookkeeping follows SDK benchmarking
whitelists). `propose_job_with_payee_requirement` adds one write to the original
proposal. New-call timing compositions are explicitly provisional: no new
reference-machine ref_time is claimed. Regenerate both proposal and submission
APIs, as well as B closure paths, before merge/release.

Submission now reads a previously accepted payload for strict improvement. The
public wrapper sums the actual runtime storage bounds for JobOrders, JobSpecs,
Solvers, OrderSolutions, the larger ranking, both expiry keys, required-chain and
payee keys, and System::Number, adding 4,950 trie bytes per key under the same SDK
model documented above. The production envelope is **792,516 bytes**. Both submit
APIs and both weight providers enforce a 1 MiB proof floor; integrity checks also
verify the maximal charged submission weights against Normal max_extrinsic and
max_total. Closure envelopes remain **1,002,055 claim / 666,169 refund bytes**:
metadata is retained, so neither closure path accesses those new maps.

Benchmark coverage adds calls 8 and 9 and an extra `submit_solution_replacement`
case with a real, full-sized prior accepted payload, full ranking, required payee,
and strictly better replacement. Measure first-submission rescheduling and
replacement independently and use their conservative timing envelope. Selection
and prior-payload decoding changed in A and are not validated by the inherited
reference fit. Run extra cases explicitly; default generated dispatch weights
must not silently omit their cost. Ordinary/dust claim remains a two-case gate.

Native mock benchmark tests, runtime bound checks and wrapper guards are local
correctness evidence, not reference timings or production Wasm preflight. Fresh
Wasm build/preflight and reference-machine measurements are separate required
checks; manual CI regeneration also pushes files and requires separate permission.

The proposal wrapper likewise uses a **686,322-byte** union envelope (JobOrders,
JobSpecs, ProposerOrders, proposer System account, NextOrderId, OpenOrders,
OrdersExpiringAt, OrderPayeeRequirements and System::Number). Both proposal APIs
apply the 1 MiB floor with generic/unit provider and Normal-limit integrity checks.
This covers the additional requirement key together with existing proposal keys.

### Post-regeneration completeness guard

`run-benchmarks.sh` now verifies regenerated mempool output immediately after that
pallet finishes. Both appended calls must have their trait declaration and generic
and unit implementations. Provisional markers and delegation to the old proposal
or submission timing are rejected. This is a structural completeness check; it
does not certify reference hardware, sample quality or the conservative envelope
across first/replacement and ordinary/dust cases.

Run `bash scripts/run-benchmarks.sh --check-regenerated-mempool-guard` for positive
and negative fixtures. To inspect generated repository-template output without
building or changing weights, use
`bash scripts/run-benchmarks.sh --verify-regenerated-mempool <generated-file>`.
The currently committed provisional weights intentionally fail the latter check
until real regeneration replaces them. No manual CI run is implied or authorised
by adding this guard.

# Pallet Benchmarking Plan

## Context

[MR !57](https://gitlab.com/quip.network/quip-protocol-rs/-/merge_requests/57)
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
- benchmark CI preflight and verification

`pallet_template` already has usable generated weights and requires no further
work.

`pallet_faucet_ops` is intentionally out of scope. It is an
operational/development-only pallet rather than a production benchmarking
target, and its single `mint` call reuses the benchmarked
`pallet_balances::WeightInfo::force_set_balance_creating` weight for the same
account-creation/update path. It does not need its own benchmark module,
`runtime-benchmarks` feature, or `define_benchmarks!` registration.

`pallet_xqvm` is intentionally out of scope because it is not actively used.
Its execution and per-step weights should be reviewed before the pallet becomes
active.

SDK pallets continue using their upstream weight implementations. Generating
runtime-specific SDK pallet weights remains future work.

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

## Delivery Order

Use small reviewable changes:

1. Runtime-safe mempool and quantum-pow fixtures plus runtime preflight.
2. Mempool complexity/weight model and regenerated weights.
3. Quantum-pow complexity/weight model and regenerated weights.
4. Miner-registry benchmark coverage.
5. Quantum-pow reference-machine sweep CI integration.
6. Final all-pallet reference-machine regeneration and CI cleanup.

## Completion Criteria

The work is complete when:

- Every in-scope custom pallet is registered in `define_benchmarks!`.
- Every dispatchable has a benchmark-backed weight contract.
- No in-scope pallet remains in the default skip list.
- Low-resolution runtime preflight succeeds for every registered benchmark.
- Quantum-pow nonlinear sweeps run through CI on `node02`, with raw JSON
  results retained as artifacts.
- Full node02 regeneration succeeds without manual pallet exclusions.
- Regenerated weights compile and all repository checks pass.

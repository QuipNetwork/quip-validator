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


## Permissionless settlement: reference coverage and release gate

Reference output in `4b70e3fec36034ffb429fb64efdb83c8ae774c63` measures the
ordinary dispatch benchmarks, including calls 8 and 9. Its ordinary claim base
is **2_367_341_000 ps**, plus 69 reads and 69 writes; the generated claim proof
estimate is 644,411 bytes. Refund measures 2 reads / 3 writes under the caller
whitelist. These are measured baselines, not the earlier modeled corrections.
The old new-call completeness guard passes on that commit. However, its wildcard
run excluded `#[benchmark(extra)]` cases: it did not measure dust settlement or
replacement submissions. It also predates conditional payee writes/events.
**Fresh reference regeneration is still a merge and release gate.**

The public `weights.rs` consumes complete generated `Weight` values, including
each provider's DB costs, using componentwise maxima:

- Claim: ordinary surviving-proposer claim and `claim_reward_dust`.
- Both submission calls: first legacy submission, first payee submission,
  `submit_solution_replacement` with an actual destination change, and
  `submit_solution_clear_payee` through the legacy call on an optional-payee order.

The maintained proof floor is applied after those maxima, preserving a larger
measured proof. There is no extra DB surcharge in the wrapper. The same
composition applies to the runtime generic and unit/RocksDB providers.
Regeneration writes `benchmark_weights.rs`, preserving the maintained wrapper.
Until the reference run, three explicitly marked provisional aliases let the
runner compile against the old generated file. They are **not measured bounds**;
the post-regeneration guard rejects them. Local diagnostic timings are never
copied into tracked weights, and no invented margin substitutes for measurement.

### Proof envelope and integrity checks

The pinned SDK (`845eb9986c7131056b077d14ad7249f9d65ca111`), in
`substrate/utils/frame/benchmarking-cli/src/pallet/writer.rs`, takes the maximum
proof estimate across storage prefixes rather than summing simultaneous keys.
The maintained envelope uses `StorageInfo::max_size` (key/hasher plus bounded
encoded value) for every touched key, without assuming shared trie nodes.

| Claim storage | Distinct keys | Maximum encoded size per key |
| --- | ---: | ---: |
| JobOrders | 1 | 640946 |
| JobSpecs | 1 | 297 |
| OrderTopSolvers, or the larger ranking representation | 1 | 1305 |
| ProposerOrders | 1 | 305 |
| System Account (proposer + winners) | 33 | 128 |
| Solvers | 32 | 109 |
| OpenOrders (removal included conservatively) | 1 | 24 |
| TotalIssuance (including benchmark-whitelisted storage) | 1 | 16 |

These 71 keys total 650,605 encoded bytes. The SDK's largest u32 map-cardinality
bucket (eight radix-16 layers), plus two global layers, costs
`(8 + 2) * 15 * 33 = 4,950` bytes **per key**. The current claim envelope is
`650605 + 71 * 4950 = 1,002,055`, below the **1,048,576-byte (1 MiB)** floor.
Result publication is hook work; claim does not read solutions, poll results or
payee maps. Historical metadata remains stored after closure.

| Family | Production union envelope | Maintained floor |
| --- | ---: | ---: |
| Claim | 1,002,055 | 1,048,576 |
| Refund | 666,169 | 1,048,576 |
| Submission (both APIs) | 792,516 | 1,048,576 |
| Proposal (both APIs) | 686,322 | 1,048,576 |

Refund includes JobOrders, ProposerOrders, proposer System Account, OpenOrders
and TotalIssuance. Submission includes JobOrders, JobSpecs, Solvers,
OrderSolutions, the larger ranking, both expiry keys, OrderPayeeRequirements,
OrderPayees and System::Number. Proposal includes JobOrders, JobSpecs,
ProposerOrders, proposer System Account, NextOrderId, OpenOrders, OrdersExpiringAt,
OrderPayeeRequirements and System::Number. Conditional payee updates add a read
of an already included key, so they do not add another proof key.

These bounds use the SDK estimation model; they are neither an enforced
cardinality cap nor an unconditional proof for every adversarial trie shape.
An assumption-free bound needs separate codec/path analysis. Integrity checks
derive bounds from actual runtime types, pin the winner limit, enforce floors,
and check both dimensions against Normal max_extrinsic and max_total. Runtime
tests check actual dispatch weights. Currency, VM, account-hook or storage-bound
changes require renewed timing, access and proof analysis.

### Required lifecycle coverage

Both claim cases settle 32 weighted winners with maximum topology, bid list,
callback endpoint and proposer-index sizes. Setup runs expiry hooks outside the
measured claim and verifies that prepublished results survive settlement. Neither
case is assumed slower before measurement. A reserved balance is a consumer;
setting a sole-provider account below ED would be invalid. The dust case first
adds an independent provider, leaves ED-1 free, then verifies dust burning and
balance-provider removal. The independent provider retains the System account;
removing it later is outside this benchmark. Mock ED=1 covers zero-dust provider
removal; runtime tests with production ED cover nonzero burning and later reaping.

Submission setup measures first-submission expiry rescheduling and replacement
of a real maximal accepted payload separately, with a full ranking. Replacement
uses a strictly better solution and changes the stored payee; legacy clearing
uses an order with no mandatory destination. Thus event suppression cannot turn
these cases into same-payee/no-event measurements. Maximal topology is 5,000 nodes,
50,000 edges and 20 solutions. Both proposal APIs cover 16 bid miners and eight
miner types. Rejected calls retain state/events and must fit the charged maximum;
correctness tests cover early failures and late diversity/improvement rejection.
Quantum-PoW selection is unchanged.

The signed Executive payout-failure test verifies atomic rollback of earlier
winner transfers, counters and events, while already committed hook expiry and
results remain. Fees and nonce still apply to the failed signed call. Retrying
after the authorised lock owner removes the restriction settles exactly once.
Missing aggregate reserves also roll back settlement/refund. No global order
sweep, forced close, or caller bounty is introduced.

### Reference procedure and checks

The manual `benchmark-weights` job uses the serialized `reference-machine`
resource and runs `scripts/run-benchmarks.sh`, then commits/pushes generated files
through `push-weights.sh`. For MR !100 the approved sequence is implementation,
reviewer/rr review of the exact SHA, push, manual reference regeneration, then
review of bot output and CI. Record the input/output SHAs, pinned toolchain/image,
machine checks, SDK revision, command lines and fresh Wasm hash. A local
`SKIP_PALLET_REVIVE_FIXTURES=1` build is diagnostic evidence, not reference provenance.

Both reference and preflight scripts pass `--extra` for the mempool wildcard.
Reference resolution is 50 steps / 20 repeats by default. The resulting file must
contain the trait declaration and both implementations for each required case;
provisional markers/delegations are rejected. The wrapper guard verifies that
both providers consume all cases through the full-weight maxima. Positive and
negative fixtures exercise missing and unconsumed cases. Structural guards do
not certify hardware, fit quality or adequate sampling: inspect raw results,
DB counts and maximal runtime weights after generation, then rerun tests, strict
Clippy, formatting and runtime integrity checks on the bot's exact output SHA.

```bash
bash scripts/run-benchmarks.sh --check-output-resolver
bash scripts/run-benchmarks.sh --check-regenerated-mempool-guard
bash scripts/check-benchmark-preflight.sh --check-signature-helpers
bash scripts/check-benchmark-preflight.sh --check-wrapper-guard
bash scripts/run-benchmarks.sh --verify-regenerated-mempool pallets/quantum-compute-mempool/src/benchmark_weights.rs
```

The final command intentionally fails until provisional bootstrap aliases are
replaced by actual reference output. Preflight generates into temporary files,
checks the same coverage gate and never modifies tracked weights. Native mock
tests, production Wasm preflight, reference measurements, and deployment/SDK
validation are distinct checks; report each at its actual provenance and SHA.

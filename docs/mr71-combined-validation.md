# !71 combined integration and clarity validation

The complete-state, hook-level upgrade rehearsal passed on the exact disposable
combined checkout below. This evidence belongs to that combined commit and tree,
not to either source branch alone. The clarity branch still needs the parent
integration before it can upgrade the captured state.

## Provenance and review

| Artifact | Exact commit |
| --- | --- |
| Original !71 parent | `c722e308e1954456dd6cd1c0c3e9e3716470879a` |
| Main source | `bc4552a2cc6913bd057ecfdf9abc99197393b55c` |
| Reviewed parent integration | `50164b50236c30eaac9c55a8de106c1267e90c3c` |
| Reviewed clarity stack | `f0fcfffbd4151ba5d51650bc09fde72393603571` |
| Tested combined merge | `387445494facab1e6856e50d1338eb1cd8f69653` |
| Tested combined Git tree | `6105b94cba9b0d6b71ef9e1d436a05dd6fc7b262` |

The combined commit's first parent is the reviewed integration; its second is
the reviewed clarity stack. Only `docs/release.md` conflicted. The resolution
preserves main's mempool/reset history, identifies spec 119 as already deployed,
and scopes the earlier failed rehearsal to the clarity-only candidate. Every
non-release-document change is the union of the two reviewed inputs. In
particular, QuantumPow, quantum-validation, runtime integration tests and the
signing fixture match clarity; the mempool matches the integration. Runtime
`lib.rs` differs from clarity only by main's 505 added regression-test lines.

All three reviewers (`rr`, `reviewer`, `kimi_reviewer`) approved the integration
under `req_20261025_232947_4d445934` and the combined result under
`req_20261025_235345_75b90c7e`. Two independently reproduced the complete
rehearsal, including the final root and weights. The third independently
confirmed the input state root and all 2,706 QBlock keys via pinned-block RPC.
This records review of the code and evidence, not rollout authorization.

## Snapshot and execution

The snapshot was captured read-only at finalized AGLS block **156607**, hash
`0x0df326c87aee42ebfea81e5b2e8761e90af39c9e5a827008ecef905a2d934a6b`.
Its JSON contains 967,080 top-level entries and four default child tries with
three entries each, occupies 246,303,202 bytes, and has SHA-256
`c871a5fc31a2f5706e4e2cb469db6e6503ea59780dedb5eb03568e1148879953`.
Capture timestamp: `2026-10-25T22:59:36.040082+00:00`.

The harness reconstructed the header's complete state root:
`0x5a95746aad6981a8f94899f47a6c0f54457b73db5d178a3155bbe39fe2e507c6`.
It verified the header hash, height and genesis, then executed the captured
on-chain Wasm's `Core_version`, confirming spec 119 / transaction version 7 /
QuantumPowApi v2 without changing state. It substituted the combined candidate's
try-runtime Wasm and executed `TryRuntime_on_runtime_upgrade(All)` through both
native and actual Wasm engines. The combined runtime is spec 120 / tx 8 / API v3.
No snapshot edits, version-only repair or disabled checks were used.

| Pass | Reported ref_time | Reported proof_size | Result |
| --- | ---: | ---: | --- |
| Full upgrade | 1,550,000,000 | 0 | Native and Wasm equal |
| Version-gated rerun | 800,000,000 | 0 | Native and Wasm equal; root unchanged |

Both dimensions were checked against the reported limit
`(2,000,000,000,000, 18,446,744,073,709,551,615)`. The full post-state root was
identical across both engines and both passes:
`0xfdd753a204af655f7fa5a3a5a08fc85f52d26952dacf28f3a2c08ff7db540bd3`.
The rerun proves the version-gated no-op path, not the individual idempotence of
each migration step. These weights are runtime accounting, not measured timing
or proof sizes. The reported zero proof_size is not a zero-proof measurement.

Every one of the 2,706 historical QBlocks retained its original 180-byte raw
value, legacy field meaning and nonce, with zero denoting unmeasured frustration.
Both index directions, cardinalities and contiguous IDs were checked. The
registered graph bytes were unchanged (4,577 nodes and 41,514 edges); dimensions
and hardness `(u32::MAX, u32::MAX, 500)` were backfilled, difficulty became the
original threshold bytes `[4..12]`, and QuantumPow storage advanced from v6 to v7.

Exactly six top-level keys changed; no key was removed:

| Storage item | Change | Post-value bytes |
| --- | --- | ---: |
| `:code` | Candidate try-runtime Wasm installed by the harness | 8,198,274 |
| `System::LastRuntimeUpgrade` | Runtime upgrade record refreshed | 7 |
| QuantumPow storage version | v6 to v7 | 2 |
| `QuantumPow::TopologyHardnessOf` | Existing graph's hardness seeded | 12 |
| `QuantumPow::TopologyDims` | Existing graph's dimensions backfilled | 8 |
| `QuantumPow::Difficulties` | Legacy configuration reduced to its threshold | 8 |

The three map entries use topology hash
`0xcbec1eb4e9dcc7ae343b386902d4dbba19b694a66cebf95bc06bd9260c3270e7`.
The storage names were checked against their SDK `twox_128` prefixes; the full
keys are printed in the rehearsal log. Mempool storage was already v4 and did
not change. Neither QBlock nor registered-topology bytes changed.

## Reproduction and local checks

Run from the exact combined checkout, with the captured JSON kept outside Git:

```bash
unset SKIP_WASM_BUILD
export FORCE_WASM_BUILD=mr71-combined-v1
export WASM_BUILD_WORKSPACE_HINT="$PWD"
export CARGO_TARGET_DIR=/private/tmp/quip-mr71-clarity-target
export CARGO_BUILD_JOBS=4
export QUIP_UPGRADE_SNAPSHOT=/private/tmp/quip-mr71-full-state-156607.json
cargo test --locked -p quip-protocol-runtime --features try-runtime \
  --test runtime_upgrade -- --ignored --exact \
  whole_runtime_upgrade_matches_wasm_on_pinned_state --nocapture
```

The coder's rehearsal passed in 302.77 seconds after a 34.28-second build.
The ignored test must be invoked explicitly; a normal workspace run does not
execute it. Logs retained on the validation host:

- `/private/tmp/quip-mr71-combined-upgrade-20261026.log`.
- `/private/tmp/quip-mr71-combined-workspace-tests-20261026.log`:
  `cargo test --locked --workspace`, 609 passed, zero failed, seven known ignored;
  all seven actual-Wasm parity tests, API-v2 replay, history and signing tests pass.
- `/private/tmp/quip-mr71-combined-clippy-20261026.log`:
  `cargo clippy --locked --workspace --all-targets -- -D warnings`, passed.
- `/private/tmp/quip-mr71-combined-release-20261026.log`:
  `cargo test --locked --release -p quip-protocol-runtime --lib`, 56 passed,
  zero failed. This built the combined runtime in release mode, including Wasm.
- `/private/tmp/quip-mr71-combined-pow-bench-tests-20261026.log`:
  `cargo test --locked --features runtime-benchmarks -p pallet-quantum-pow --lib`,
  198 passed, zero failed, two ignored. These pallet tests exercise benchmark
  fixtures; they are not production-runtime preflight or weight measurements.
- `/private/tmp/quip-mr71-combined-mempool-bench-tests-20261026.log`:
  `cargo test --locked --features runtime-benchmarks -p pallet-quantum-compute-mempool --lib`,
  99 passed, zero failed; the same pallet-test limitation applies.

Formatting, diff and shell-syntax checks, signature helper, both public-wrapper
guards, output resolver, regenerated-mempool guard and tracked mempool generated
weights verification also passed. These files are local logs, not CI artifacts.

## Landing and remaining release obligations

The parent integration and clarity changes are to be submitted as separate draft
MRs targeting !71's branch. Integrate the reviewed main-alignment changes there first;
only then rebase clarity onto the actual updated parent and repeat exact-head
validation. This disposable combined checkout does not update either branch.

The successful hook rehearsal resolves the captured mempool-v4 incompatibility
on the tested combined tree. It does not establish release readiness. Still owed:

- Actual CI and production benchmark preflight on the final integrated runtime.
  The local integration preflight stopped because `solc` was absent, before
  benchmarks ran. CI installs pinned solc 0.8.30 and resolc 1.0.0. Skipping Revive
  fixtures is not a substitute for benchmark execution.
- Required reference-machine weight regeneration on the integrated runtime;
  retained measurements come from the separately reviewed source runtimes.
- A submitted upgrade extrinsic and subsequent authored blocks on a disposable
  development chain, plus compatible companion solver API3/tx8 end-to-end checks.
- Final release/signing checks, a fresh version-collision check and explicit
  release/deployment authorization. The test Wasm above is not a release artifact.

The original clarity-only failure remains useful evidence of why the parent
integration is required; see [the release checklist](release.md#reproducing-the-pinned-whole-runtime-rehearsal).

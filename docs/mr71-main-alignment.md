# !71 parent alignment with main

This integration candidate merges main into the !71 base on a separate branch,
`integration/mr71-main-alignment`, targeting `v0.2.1/generic-graph-difficulty`.
It does not merge main into `refactor/mr71-clarity` or update the existing parent
branch. Both source refs were refreshed before the merge:

- !71: `c722e308e1954456dd6cd1c0c3e9e3716470879a` (first parent).
- main: `bc4552a2cc6913bd057ecfdf9abc99197393b55c` (second parent).

## Why the integration is required

The clarity stack's complete-state rehearsal at finalized AGLS block 156607
reconstructed the header state root and executed the captured on-chain Wasm
(spec 119 / tx 7 / QuantumPowApi v2). Both native and Wasm full upgrade checks
then failed: QuantumComputeMempool was already at storage v4, while !71's base
expected v3. The clarity stack had not modified that pallet.

Main commit `5686afd288ec0fcd63405d4ef505d9e7cfef6954` introduced v4 with
permissionless reward settlement and related mempool changes. This merge brings
that implementation, its migration, tests, public weight wrapper and generated
benchmark cases together. Changing only a storage-version marker would not
restore the deployed behavior.

## Conflict resolutions

- Keep main's shared `do_propose_job` path and both proposal calls, including
  immutable payee requirements, while moving !71's boolean
  `topology_consistency_ok` check into that shared helper. The check preserves
  the same topology acceptance and guard order without allocating diagnostic
  strings on malformed input. A regression test exercises five malformed
  shapes through both calls, verifies `InvalidTopology` with no storage writes,
  and retains the earlier `RewardTooLow` rejection.
- Keep main's mempool public weight wrapper and `benchmark_weights.rs`
  byte-for-byte. This retains the proof floors, worst-case composition across
  settlement/submission paths, extra benchmark cases and regeneration guards.
- Keep !71's QuantumPow generated weights, including its witness benchmarks,
  and its public hand-modelled weight functions. QuantumPow and the validation
  crate are otherwise unchanged from the first parent.
- Keep !71's 2026-10-14 reference-machine generated weights for emission,
  faucet, miner registry, template, validator admission, XQVM and runtime
  overhead. Main's conflicts there were an older 2026-10-12 regeneration;
  main introduced no implementation changes in those pallets in this merge.
- Combine main's generic public-wrapper signature check with !71's three
  QuantumPow signature exceptions (`submit_proof`, `prove_topology_exact`,
  `prove_topology_planar`). Generated signatures are still checked for all
  cases; wrapper guards protect both pallets. Main's mempool output resolver,
  extra-case selection and reference-regeneration checks are retained.

No weight constants were invented or measured locally. Retaining prior
reference measurements is not a claim of fresh reference-machine measurement
of this combined runtime. Production benchmark preflight and reference-machine
weight requirements remain part of the release checks.

## Candidate validation

On 2026-10-26, the resolved integration tree passed `cargo test --locked
--workspace` (579 passed, zero failed, seven intentionally ignored), workspace
Clippy with all targets and warnings denied, formatting and diff checks. The
signature helper, both public-wrapper guards, benchmark output resolver,
regeneration guard and tracked mempool generated-weight verification passed.
The new malformed-topology regression is included in the workspace result.

Production benchmark preflight was attempted with Revive fixtures enabled and
stopped while compiling `pallet-revive-fixtures`: this macOS host has no `solc`
executable. No benchmark ran and no new measurements were produced. The CI
preflight installs checksum-pinned solc 0.8.30 and resolc 1.0.0; its execution
and the separate reference-machine weight job remain outstanding. Skipping
fixtures is not a passing substitute. The local attempt is recorded in
`/private/tmp/quip-mr71-alignment-preflight-20261026.log`.

## Landing and validation dependency

Review this integration candidate independently of the clarity stack. Use a
disposable combined checkout of the reviewed integration and reviewed clarity
commits for preservation tests and the full-state upgrade rehearsal, recording
its exact head/tree. Do not attribute a combined-checkout result to either
branch alone. Clarity remains based on the actual !71 parent until the parent
incorporates the reviewed integration; then rebase it and rerun the checks.

The standalone integration inherits !71's pending release metadata (spec 119 /
tx 8); the clarity stack separately assigns candidate spec 120 and regenerates
the signing fixture. Neither this merge nor retained measurements establish
deployment readiness. Recheck version collisions, complete the successful
combined upgrade rehearsal, reference weights and CI, and identify compatible
miner integration before rollout. Existing-parent pushes, MR merges and
deployment are separate from preparing these draft review artifacts.

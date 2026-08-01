# Quantum PoW Benchmark Review Fixes

## Context

This plan addresses the remaining review findings on
[quip-validator merge request !58](https://gitlab.com/quip.network/quip-validator/-/merge_requests/58).
It is intentionally limited to the benchmark redirect guard, the Quantum PoW
weight envelope, the benchmark preflight CI configuration, removing the
redundant sweep-job `jq` installation, and retaining reproducible sweep
metadata.

The Quantum PoW model shape is derived from the node02 sweep produced by GitLab
job `15552403591` at MR commit `1dfd86e`. Jobs `15618730781` and `15638128172`
are independent holdouts used to size the operational safety buffer. The latter
also verifies the absolute summary-path fix from commit `562e13c`.

## Source data

Each recorded node02 sweep contains 120 samples per calibration point. The
largest recorded maximum for each point is:

| Point | Nodes (`n`) | Edges (`e`) | Solutions (`s`) | Maximum | Job |
| --- | ---: | ---: | ---: | ---: | ---: |
| `minimum` | 16 | 1 | 1 | 68,609 ns | `15638128172` |
| `nodes` | 5,000 | 1 | 1 | 743,905 ns | `15638128172` |
| `edges` | 16 | 50,000 | 1 | 3,030,034 ns | `15638128172` |
| `solution_nodes` | 5,000 | 1 | 32 | 4,806,971 ns | `15618730781` |
| `solution_edges` | 16 | 50,000 | 32 | 29,224,348 ns | `15552403591` |
| `worst_case` | 5,000 | 50,000 | 32 | 145,685,019 ns | `15638128172` |

The complete 18-row evidence set is checked in at
`pallets/quantum-pow/testdata/node02-submit-proof-sweeps.tsv`. Normal unit tests
enforce the deterministic calibration target against it. Each new sweep job
also runs an environment-driven test against its own `summary.tsv` to enforce a
separate live safety floor.

## Required changes

### 1. Make the generated-weight redirect self-protecting

`scripts/run-benchmarks.sh` currently identifies `pallet_quantum_pow` by name and
redirects FRAME output to `benchmark_weights.rs`. If that branch is removed,
FRAME overwrites the public `weights.rs` wrapper. The current preflight only
compares method signatures, so a generated `submit_proof(n, e, s)` implementation
can replace the nonlinear wrapper without being detected.

Implement the following:

- Resolve the output by convention:
  - default to `src/weights.rs`;
  - use `src/benchmark_weights.rs` when that file exists.
- Add a preflight invariant that independently verifies the Quantum PoW public
  wrapper still contains its nonlinear model and delegates the generated base
  weight to `benchmark_weights`.
- Add focused, build-free regression coverage proving:
  - the real wrapper passes;
  - a signature-compatible plain FRAME output file fails;
  - the resolver selects `benchmark_weights.rs` for Quantum PoW.

### 2. Recalibrate the public `submit_proof` envelope

The committed model undercharges the node02 worst case because it does not
capture the combined high-node/high-edge cost. Replace it with:

```text
W = generated_base
  + database_weight
  + K1 * n
  + K2 * e
  + K3 * s * n
  + K4 * s * e
  + K5 * s² * n
  + K6 * n * e
```

Use these coefficients:

| Coefficient | Before drift follow-up | Planned |
| --- | ---: | ---: |
| `K1_NODE` | 1,200 | 1,200 |
| `K2_EDGE` | 2,400 | 2,400 |
| `K3_SOLUTION_NODE` | 6,000 | 6,000 |
| `K4_SOLUTION_EDGE` | 17,000 | 18,500 |
| `K5_SOLUTION_NODE_QUADRATIC` | 1,200 | 1,200 |
| `K6_NODE_EDGE` | 500 | 535 |

With the MR-head generated zero-dimension base and database weight of
`5,036,336,000` reference-time picoseconds, the planned model covers every
recorded observation by at least 20%. This target leaves useful room above the
10% live-sweep floor after node02's observed approximately 3% day-to-day drift.
The two holdout runs are not used to re-identify the formula shape; their maxima
only size the safety buffer around the already-derived model.

| Point | Planned charge | Largest observed maximum | Margin |
| --- | ---: | ---: | ---: |
| `minimum` | 5.036500 ms | 0.068609 ms | substantial |
| `nodes` | 5.081032 ms | 0.743905 ms | substantial |
| `edges` | 6.509470 ms | 3.030034 ms | substantial |
| `solution_nodes` | 12.149605 ms | 4.806971 ms | substantial |
| `solution_edges` | 35.207088 ms | 29.224348 ms | 20.47% |
| `worst_case` | 175.616336 ms | 145.685019 ms | 20.55% |

The checked-in documentation must describe `n * e` as a conservative empirical
proxy for the measured combined topology cost. It must not claim the verifier
has literal `O(n * e)` complexity: its topology index uses `BTreeMap`
construction and lookups, making its structural behavior closer to
`n log n + e log n`.

Implementation details:

- Update `pallets/quantum-pow/src/weights.rs` with the coefficient and provenance
  changes.
- Update the formula documentation in `pallets/quantum-pow/src/lib.rs`.
- Add a normal
  `weight_covers_recorded_node02_sweeps_with_twenty_percent_target` test over
  the checked-in 18-row fixture.
- Keep an ignored `weight_covers_executed_sweep_with_ten_percent_floor` test
  that reads the path supplied through `QUANTUM_POW_SWEEP_SUMMARY`, requires the
  exact six points, and compares the committed formula with each freshly
  observed maximum using the 10% operational floor.
- Retain or add formula-component, monotonicity, and saturation checks. At
  minimum, check solution sizes `1..=32` at the minimum and maximum topology
  bounds.
- Do not hand-edit generated `benchmark_weights.rs`.

Job `15552403591` remains the same-run calibration source. Jobs `15618730781`
and `15638128172` are holdout confirmation and buffer-sizing evidence, not
independent coefficient-identification data. The 20% recorded target and 10%
fresh-sweep floor are deliberately different: routine host variation should not
make CI flaky, while a fresh measurement crossing the 10% floor must still fail.

### 3. Fix benchmark preflight timeout and rules

Change only the `benchmark-preflight` job:

- increase `timeout` from `60m` to `90m`;
- run for every merge request pipeline, including forks;
- add an explicit final `when: never` fallback.

The intended rules are:

```yaml
rules:
  - if: '$CI_PIPELINE_SOURCE == "merge_request_event"'
  - when: never
```

Do not add a same-project condition or a `changes:` filter. Do not alter the
rules for `benchmark-weights` or `benchmark-quantum-pow-sweeps`.

### 4. Use the `jq` already provided by the toolchain image

The node02 toolchain image already contains `jq`, so the sweeps job must not
modify the image at runtime:

- In `.gitlab-ci.yml`, remove the `benchmark-quantum-pow-sweeps`
  `before_script` that runs `apt-get update`, installs `jq`, and removes the apt
  lists.
- Update the adjacent job comment so it states that `jq` is supplied by the
  toolchain image.
- Keep the hard `command -v "$JQ"` check in
  `scripts/run-quantum-pow-sweeps.sh`. The job must still fail before a sweep if
  the selected image stops providing `jq`.
- Do not change the sweeps job's runner tags, rules, resource group, timeout, or
  manual/scheduled behavior.

### 5. Retain reproducibility metadata with every sweep

Extend `scripts/run-quantum-pow-sweeps.sh` to write
`quantum-pow-sweeps/metadata.json` after all six raw results and `summary.tsv`
have passed their existing validation. Generate it with `jq -n -S`, write it to
a temporary file, validate it, and atomically rename it into place so a failed
or partial run cannot leave metadata that appears complete.

Use the following stable schema and field names:

```json
{
  "schema_version": 1,
  "gitlab": {
    "job_id": "15552403591",
    "job_url": "https://gitlab.example/jobs/15552403591",
    "commit_sha": "1dfd86e...",
    "pipeline_id": "12345"
  },
  "container": {
    "image_ref": "registry.example/toolchain:tag",
    "image_tag": "tag",
    "image_digest": null
  },
  "toolchain": {
    "rustc_vv": "rustc 1.x...\n..."
  },
  "host": {
    "hostname": "node02",
    "cpu_model": "...",
    "kernel": "...",
    "os": {
      "id": "...",
      "version_id": "...",
      "pretty_name": "..."
    }
  },
  "sweep": {
    "pallet": "pallet_quantum_pow",
    "extrinsic": "submit_proof",
    "steps": 2,
    "repeat": 20,
    "min_duration": 0,
    "point_count": 6,
    "points": [
      {
        "name": "minimum",
        "nodes": 16,
        "edges": 1,
        "solutions": 1,
        "sample_count": 120
      }
    ]
  }
}
```

Implementation details:

- Populate `gitlab.job_id`, `gitlab.job_url`, `gitlab.commit_sha`, and
  `gitlab.pipeline_id` from the corresponding `CI_*` variables. Preserve the
  keys with JSON `null` values for local runs where GitLab context is absent.
- Populate `container.image_ref` from the effective CI image variable and
  `container.image_tag` from its explicit tag, using JSON `null` when the
  reference is digest-only or no tag is available. Populate
  `container.image_digest` when the runner or pipeline exposes a digest, and
  otherwise retain the key with a JSON `null` value. Do not add registry or
  Docker-daemon access solely to discover a digest.
- Capture the complete `rustc -vV` output, `hostname`, the first Linux CPU model
  from `/proc/cpuinfo`, `uname` kernel information, and the stable
  `/etc/os-release` identity fields. Use explicit fallbacks that preserve the
  schema with `null` when a value is unavailable.
- Derive `sweep.points[*].sample_count` from each validated raw JSON result,
  rather than assuming it from `REPEAT`. Build the point list from the same
  dimensions used by the benchmark loop so metadata and execution cannot drift.
- Do not add a wall-clock generation timestamp. The CI job identity already
  identifies the run, while omitting a new clock value makes repeated local
  validation deterministic.
- In `.gitlab-ci.yml`, keep the existing one-year artifact retention and ensure
  `metadata.json` is retained alongside the six raw JSON files and
  `summary.tsv`. After the sweep script succeeds, run the ignored envelope test
  with
  `QUANTUM_POW_SWEEP_SUMMARY=$CI_PROJECT_DIR/quantum-pow-sweeps/summary.tsv`;
  a new undercharged measurement must fail the job before artifacts are accepted.
  The existing `quantum-pow-sweeps/*.json` path may cover `metadata.json`, but
  CI validation must assert that the file appears in the retained artifact.

### 6. Harden post-sweep verification and failure semantics

Job `15618730781` completed all measurements but failed before checking the
envelope because the test received a checkout-relative summary path while Cargo
ran its test binary from the pallet directory. Commit `562e13c` corrected the
path, and job `15638128172` then exercised all six rows successfully.

- Keep the absolute `$CI_PROJECT_DIR` path and add an explicit readability check
  before invoking Cargo so path failures are diagnosed before test startup.
- Require the live summary's exact header, the exact six point names and
  dimensions, nonzero sample counts, ordered min/median/max values, and no
  duplicate or unexpected rows. A simple six-row counter is insufficient.
- Keep manual MR sweeps optional with `allow_failure: true`; otherwise an
  unplayed reference-machine job would block an ordinary merge.
- Make opted-in scheduled sweeps explicitly `allow_failure: false`. A scheduled
  calibration is monitoring and must alert when tooling, hardware, or the 10%
  live envelope floor regresses.
- Document this deliberate rule split next to the job and verify it through
  GitLab CI lint/compiled-rule inspection.

## Validation

- Run `bash -n` on all changed shell scripts.
- Run the focused resolver and wrapper-invariant regression tests.
- Run Quantum PoW unit tests, including formula-component, monotonicity, and
  saturation cases.
- Run the deterministic recorded-sweep test and the ignored live-floor test
  against all three retained node02 summaries. Confirm the parser rejects a
  duplicate, missing, unexpected, dimension-mismatched, or unordered row, and
  fails when any observed maximum exceeds the applicable margin.
- Exercise the sweep script with a temporary output directory and a controlled
  benchmark stub, confirming it still fails when `jq` is unavailable and
  succeeds with the image-provided `jq`.
- Validate `metadata.json` with `jq -e`: require the schema version and every
  named key, correct scalar types or documented `null` fallbacks, exactly six
  sweep points, and sample counts that match the corresponding raw JSON files.
- In CI context, additionally require nonempty job ID, job URL, commit SHA, and
  container image reference fields. Confirm the commit SHA is the current
  `CI_COMMIT_SHA`; allow the image digest to remain `null` when the runner does
  not expose one.
- Confirm the sweeps job does not run `apt-get`, its `jq` availability check
  still executes before benchmarking, and its retained one-year artifact
  contains `metadata.json`, the six raw JSON files, and `summary.tsv`.
- Run the normal low-resolution benchmark preflight when practical.
- Validate `.gitlab-ci.yml` syntax and use GitLab CI Lint.
- Confirm the compiled job matrix includes `benchmark-preflight` for same-project
  and fork merge requests, and excludes branch pushes, tags, releases, and
  schedules.
- Confirm `benchmark-quantum-pow-sweeps` is optional on same-project merge
  requests and hard-failing only on explicitly enabled schedules.
- Run `git diff --check`.

## Out of scope

- Moving benchmark jobs away from node02; that work remains tracked separately
  by QUI-951.
- Regenerating `benchmark_weights.rs`.
- Running a new calibration sweep.
- Adding a `changes:` optimization to preflight rules.
- Unrelated revive or extrinsic-signing work.

## Completion criteria

The change is ready for review when all six required fixes are implemented: the
recorded three-run fixture demonstrates at least a 20% calibration envelope;
the redirect cannot silently overwrite the public wrapper; the compiled CI
rules match the intended matrices; the sweeps job performs no runtime `jq`
installation while retaining its availability check; every successful sweep
retains a schema-valid `metadata.json` whose sample counts match all six raw
result files; and the absolute-path live test enforces a 10% floor over the
exact six-point summary. The runner placement and every other agreed MR !58
scope item must remain unchanged. Pedant should then review the implementation
and its validation evidence.

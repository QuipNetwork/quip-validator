#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/run-benchmarks.sh
#
# Regenerates pallet weight files and the runtime's block/extrinsic execution
# overhead by running the FRAME benchmarks. Meant to run on the benchmark
# reference machine (the CI `benchmark-weights` job), but runs anywhere for a
# local pre-flight — just don't commit weights measured off-reference hardware.
#
# What it does:
#   1. Builds the node with `runtime-benchmarks` and the local-chain EIP-155
#      configuration required by the benchmark CLI's default chain preset.
#   2. Derives the pallet list from `benchmark pallet --list` — the runtime's
#      define_benchmarks! registry — so a newly registered pallet is picked up
#      with no change here.
#   3. Regenerates weights for every listed pallet that has a matching in-repo
#      crate directory (pallet_foo_bar -> pallets/foo-bar). Pallets without
#      one (frame_system, pallet_balances, ...) use upstream SubstrateWeight
#      and are skipped.
#   4. Measures the runtime-specific empty-block and System::remark overhead
#      into runtime/src/weights/ using compiled Wasm, then mirrors those
#      measurements into runtime-testnet/src/weights/ for runtime parity.
#
# Environment overrides:
#   STEPS / REPEAT   benchmark resolution (default 50 / 20, the settings the
#                    existing generated weights were produced with)
#   OVERHEAD_WARMUP / OVERHEAD_REPEAT
#                    execution-overhead resolution (default 10 / 100)
#   SKIP_PALLETS     optional space-separated pallets to skip (default empty)

STEPS="${STEPS:-50}"
REPEAT="${REPEAT:-20}"
OVERHEAD_WARMUP="${OVERHEAD_WARMUP:-10}"
OVERHEAD_REPEAT="${OVERHEAD_REPEAT:-100}"
SKIP_PALLETS="${SKIP_PALLETS:-}"

resolve_weight_output() {
  local pallet_dir="$1"

  if [ -f "$pallet_dir/src/benchmark_weights.rs" ]; then
    printf '%s\n' "$pallet_dir/src/benchmark_weights.rs"
  else
    printf '%s\n' "$pallet_dir/src/weights.rs"
  fi
}

check_output_resolver() {
  local temp_dir
  local ordinary_dir
  local wrapped_dir

  temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/quip-weight-resolver.XXXXXX")"
  trap 'rm -rf "$temp_dir"' RETURN
  ordinary_dir="$temp_dir/ordinary"
  wrapped_dir="$temp_dir/wrapped"
  mkdir -p "$ordinary_dir/src" "$wrapped_dir/src"
  touch "$wrapped_dir/src/benchmark_weights.rs"

  if [ "$(resolve_weight_output "$ordinary_dir")" != "$ordinary_dir/src/weights.rs" ]; then
    echo "ERROR: ordinary pallet did not resolve to weights.rs" >&2
    return 1
  fi
  if [ "$(resolve_weight_output "$wrapped_dir")" != \
    "$wrapped_dir/src/benchmark_weights.rs" ]; then
    echo "ERROR: wrapped pallet did not resolve to benchmark_weights.rs" >&2
    return 1
  fi
  if [ "$(resolve_weight_output pallets/quantum-pow)" != \
    "pallets/quantum-pow/src/benchmark_weights.rs" ]; then
    echo "ERROR: Quantum PoW did not resolve to benchmark_weights.rs" >&2
    return 1
  fi

  if [ "$(resolve_weight_output pallets/quantum-compute-mempool)" != \
    "pallets/quantum-compute-mempool/src/benchmark_weights.rs" ]; then
    echo "ERROR: Quantum Compute Mempool did not resolve to benchmark_weights.rs" >&2
    exit 1
  fi

  echo "Weight output resolver checks passed."
}

verify_regenerated_mempool_weights() {
  local file="$1"
  local name
  local count

  if [ ! -f "$file" ]; then
    echo "ERROR: regenerated mempool weights missing: $file" >&2
    return 1
  fi
  # The repository template emits one trait declaration and two implementations.
  # Catch incomplete/stale regeneration even if an old composition still compiles.
  for name in propose_job_with_payee_requirement submit_solution submit_solution_with_payee submit_solution_replacement submit_solution_clear_payee claim_reward claim_reward_dust; do
    count="$(awk -v name="$name" '$0 ~ "fn[[:space:]]+" name "[[:space:]]*\\(" {n++} END {print n+0}' "$file")"
    if [ "$count" -ne 3 ]; then
      echo "ERROR: regenerated $name must have a trait declaration and both implementations" >&2
      return 1
    fi
  done
  if grep -Eiq 'provisional|pending regeneration|NOT new measured timings|as[[:space:]]+WeightInfo>::' "$file"; then
    echo "ERROR: regenerated mempool weights retain provisional compositions or markers" >&2
    return 1
  fi
}

check_regenerated_mempool_guard() {
  local temp_dir
  local valid
  local invalid
  local name

  temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/quip-regenerated-guard.XXXXXX")"
  trap 'rm -rf "$temp_dir"' RETURN
  valid="$temp_dir/generated.rs"
  invalid="$temp_dir/incomplete.rs"
  # Synthetic syntax fixture only; these are not benchmark measurements.
  : > "$valid"
  for name in propose_job_with_payee_requirement submit_solution submit_solution_with_payee submit_solution_replacement submit_solution_clear_payee claim_reward claim_reward_dust; do
    printf 'fn %s() -> Weight;\nfn %s() -> Weight {}\nfn %s() -> Weight {}\n' "$name" "$name" "$name" >> "$valid"
  done
  verify_regenerated_mempool_weights "$valid"
  for name in propose_job_with_payee_requirement submit_solution submit_solution_with_payee submit_solution_replacement submit_solution_clear_payee claim_reward claim_reward_dust; do
    awk -v name="$name" '$0 ~ "fn " name && !removed {removed=1; next} {print}' "$valid" > "$invalid"
    if verify_regenerated_mempool_weights "$invalid" >/dev/null 2>&1; then
      echo "ERROR: incomplete $name passed regeneration guard" >&2
      return 1
    fi
  done
  cp "$valid" "$invalid"
  printf '%s\n' '// Provisional composition, timing pending regeneration.' >> "$invalid"
  if verify_regenerated_mempool_weights "$invalid" >/dev/null 2>&1; then
    echo "ERROR: provisional markers passed regeneration guard" >&2
    return 1
  fi
  cp "$valid" "$invalid"
  printf '%s\n' '<Self as WeightInfo>::submit_solution(n, e, s)' >> "$invalid"
  if verify_regenerated_mempool_weights "$invalid" >/dev/null 2>&1; then
    echo "ERROR: delegated timing passed regeneration guard" >&2
    return 1
  fi
  echo "Post-regeneration mempool weight guard checks passed."
}

case "${1:-}" in
  --check-output-resolver)
    check_output_resolver
    exit 0
    ;;
  --check-regenerated-mempool-guard)
    check_regenerated_mempool_guard
    exit 0
    ;;
  --verify-regenerated-mempool)
    if [ "$#" -ne 2 ]; then
      echo "Usage: $0 --verify-regenerated-mempool <generated-file>" >&2
      exit 2
    fi
    verify_regenerated_mempool_weights "$2"
    exit 0
    ;;
esac

if [ "$#" -ne 0 ]; then
  echo "Usage: $0 [--check-output-resolver|--check-regenerated-mempool-guard|--verify-regenerated-mempool <file>]" >&2
  exit 2
fi

echo "== Building node with runtime-benchmarks (this is the slow part) =="
cargo build --release --features runtime-benchmarks -p quip-network-node

BIN="${CARGO_TARGET_DIR:-target}/release/quip-network-node"

# `benchmark pallet --list` prints CSV rows of "<pallet>, <benchmark>".
# First column, deduplicated, header dropped.
mapfile -t pallets < <(
  "$BIN" benchmark pallet --list \
    | awk -F', ' '/^[a-z0-9_]+, /{print $1}' \
    | grep -v '^pallet$' \
    | sort -u
)

if [ "${#pallets[@]}" -eq 0 ]; then
  echo "ERROR: derived no pallets from 'benchmark pallet --list' — output format change?" >&2
  exit 1
fi

echo "== Benchmarkable pallets: ${pallets[*]} =="

for pallet in "${pallets[@]}"; do
  case " $SKIP_PALLETS " in
    *" $pallet "*)
      echo "-- $pallet: SKIPPED (SKIP_PALLETS)"
      continue
      ;;
  esac

  dir="pallets/$(echo "${pallet#pallet_}" | tr '_' '-')"
  if [ ! -d "$dir" ]; then
    echo "-- $pallet: no in-repo crate at $dir, skipping (upstream weights)"
    continue
  fi

  # Pallets with a public wrapper keep ordinary FRAME output in the dedicated
  # benchmark module. This convention prevents regeneration from overwriting
  # wrapper-only composition such as quantum-pow's nonlinear submit_proof.
  output="$(resolve_weight_output "$dir")"

  extra_args=()
  if [ "$pallet" = "pallet_quantum_compute_mempool" ]; then
    # Wildcard alone excludes lifecycle cases that the maintained wrapper consumes.
    extra_args=(--extra)
  fi
  echo "== Benchmarking $pallet -> $output =="
  # --template is required: the CLI's built-in template emits the
  # runtime-style file (pub struct WeightInfo implementing
  # <crate>::WeightInfo), which does not compile inside a pallet crate.
  # .maintain/frame-weight-template.hbs emits the in-pallet convention
  # (pub trait WeightInfo + SubstrateWeight<T> + () impls) this repo uses.
  "$BIN" benchmark pallet \
    --pallet "$pallet" \
    --extrinsic '*' "${extra_args[@]}" \
    --steps "$STEPS" \
    --repeat "$REPEAT" \
    --template .maintain/frame-weight-template.hbs \
    --output "$output"

  if [ "$pallet" = "pallet_quantum_compute_mempool" ]; then
    verify_regenerated_mempool_weights "$output"
    bash scripts/check-benchmark-preflight.sh --check-wrapper-guard
  fi
done

echo "== Benchmarking runtime execution overhead -> runtime/src/weights =="
"$BIN" benchmark overhead \
  --dev \
  --wasm-execution compiled \
  --warmup "$OVERHEAD_WARMUP" \
  --repeat "$OVERHEAD_REPEAT" \
  --weight-path runtime/src/weights

# The runtime parity gate requires the same base-cost measurements, including
# generated headers, in both runtimes. Do not benchmark these independently.
cp runtime/src/weights/block_weights.rs runtime-testnet/src/weights/block_weights.rs
cp runtime/src/weights/extrinsic_weights.rs runtime-testnet/src/weights/extrinsic_weights.rs

echo "== Running 'cargo fmt'"
cargo fmt
bash scripts/check-runtime-parity.sh

echo "== Done. Regenerated files: =="
git diff --stat -- \
  'pallets/*/src/weights.rs' \
  'pallets/*/src/benchmark_weights.rs' \
  'runtime/src/weights/block_weights.rs' \
  'runtime/src/weights/extrinsic_weights.rs' \
  'runtime-testnet/src/weights/block_weights.rs' \
  'runtime-testnet/src/weights/extrinsic_weights.rs' || true

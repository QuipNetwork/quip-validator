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
#      into runtime/src/weights/ using compiled Wasm.
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

  echo "Weight output resolver checks passed."
}

if [ "${1:-}" = "--check-output-resolver" ]; then
  check_output_resolver
  exit 0
fi

if [ "$#" -ne 0 ]; then
  echo "Usage: $0 [--check-output-resolver]" >&2
  exit 2
fi

echo "== Building node with runtime-benchmarks (this is the slow part) =="
cargo build --release --features runtime-benchmarks,dev-chain-id -p quip-network-node

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

  echo "== Benchmarking $pallet -> $output =="
  # --template is required: the CLI's built-in template emits the
  # runtime-style file (pub struct WeightInfo implementing
  # <crate>::WeightInfo), which does not compile inside a pallet crate.
  # .maintain/frame-weight-template.hbs emits the in-pallet convention
  # (pub trait WeightInfo + SubstrateWeight<T> + () impls) this repo uses.
  "$BIN" benchmark pallet \
    --pallet "$pallet" \
    --extrinsic '*' \
    --steps "$STEPS" \
    --repeat "$REPEAT" \
    --template .maintain/frame-weight-template.hbs \
    --output "$output"
done

echo "== Benchmarking runtime execution overhead -> runtime/src/weights =="
"$BIN" benchmark overhead \
  --dev \
  --wasm-execution compiled \
  --warmup "$OVERHEAD_WARMUP" \
  --repeat "$OVERHEAD_REPEAT" \
  --weight-path runtime/src/weights

echo "== Done. Regenerated files: =="
git diff --stat -- \
  'pallets/*/src/weights.rs' \
  'pallets/*/src/benchmark_weights.rs' \
  'runtime/src/weights/block_weights.rs' \
  'runtime/src/weights/extrinsic_weights.rs' || true

echo "== Running 'cargo fmt'"
cargo fmt
#!/usr/bin/env bash
set -euo pipefail

# Fast, non-pushing production-runtime benchmark preflight.
#
# This deliberately uses a debug node and low benchmark resolution: the goal
# is to exercise every benchmark against the production runtime constants, not
# to produce reference-machine measurements. Generated files live only in a
# temporary directory; tracked weights are never overwritten.
#
# Environment overrides:
#   STEPS / REPEAT   benchmark resolution (default 2 / 1)

STEPS="${STEPS:-2}"
REPEAT="${REPEAT:-1}"

normalize_signature() {
  tr -d '[:space:]' | sed 's/,)/)/g'
}

extract_signature() {
  local name="$1"
  local file="$2"

  awk -v name="$name" '
    {
      if (!capturing) {
        pattern = "fn[[:space:]]+" name "[[:space:]]*\\("
        if (!match($0, pattern)) {
          next
        }
        capturing = 1
        signature = substr($0, RSTART)
      } else {
        signature = signature "\n" $0
      }

      terminator = index(signature, ";")
      if (terminator > 0) {
        print substr(signature, 1, terminator)
        exit
      }
    }
  ' "$file"
}

check_signature_helpers() {
  local single_line
  local wrapped

  single_line="$(
    extract_signature example <(
      printf '%s\n' 'fn example(nodes: u32, edges: u32) -> Weight;'
    ) | normalize_signature
  )"
  wrapped="$(
    extract_signature example <(
      printf '%s\n' \
        'fn example(' \
        '    nodes: u32,' \
        '    edges: u32,' \
        ') -> Weight;'
    ) | normalize_signature
  )"

  if [ -z "$single_line" ] || [ "$single_line" != "$wrapped" ]; then
    echo "ERROR: signature helper check failed" >&2
    echo "  single-line: $single_line" >&2
    echo "  wrapped:     $wrapped" >&2
    exit 1
  fi

  echo "Signature extraction/normalization checks passed."
}

check_quantum_pow_wrapper() {
  local file="$1"
  local normalized
  local required

  if [ ! -f "$file" ]; then
    echo "ERROR: Quantum PoW public weights wrapper not found at $file" >&2
    return 1
  fi

  normalized="$(tr -d '[:space:]' < "$file")"
  for required in \
    'usecrate::benchmark_weights' \
    'constSUBMIT_PROOF_K1_NODE:' \
    'constSUBMIT_PROOF_K2_EDGE:' \
    'constSUBMIT_PROOF_K3_SOLUTION_NODE:' \
    'constSUBMIT_PROOF_K4_SOLUTION_EDGE:' \
    'constSUBMIT_PROOF_K5_SOLUTION_SQ_NODE:' \
    'constSUBMIT_PROOF_K6_NODE_EDGE:' \
    'fnsubmit_proof_dimension_weight(' \
    '.saturating_add(solution_node_cost)' \
    '.saturating_add(solution_edge_cost)' \
    '.saturating_add(solution_squared_cost)' \
    '.saturating_add(node_edge_cost)' \
    'asBenchmarkWeightInfo>::submit_proof(0,0,0)' \
    '.saturating_add(submit_proof_dimension_weight(n,e,s))'; do
    if [[ "$normalized" != *"$required"* ]]; then
      echo "ERROR: Quantum PoW public weights wrapper lost invariant '$required'" >&2
      return 1
    fi
  done
}

check_wrapper_guard() {
  local temp_dir
  local plain_generated

  check_quantum_pow_wrapper pallets/quantum-pow/src/weights.rs

  temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/quip-wrapper-guard.XXXXXX")"
  trap 'rm -rf "$temp_dir"' RETURN
  plain_generated="$temp_dir/weights.rs"
  printf '%s\n' \
    'use frame_support::weights::Weight;' \
    'pub trait WeightInfo {' \
    '    fn submit_proof(n: u32, e: u32, s: u32) -> Weight;' \
    '}' \
    > "$plain_generated"

  if check_quantum_pow_wrapper "$plain_generated" >/dev/null 2>&1; then
    echo "ERROR: signature-compatible plain FRAME output passed the wrapper guard" >&2
    return 1
  fi

  echo "Quantum PoW public wrapper guard checks passed."
}

case "${1:-}" in
  --check-signature-helpers)
    check_signature_helpers
    exit 0
    ;;
  --check-wrapper-guard)
    check_wrapper_guard
    exit 0
    ;;
  '')
    ;;
  *)
    echo "Usage: $0 [--check-signature-helpers|--check-wrapper-guard]" >&2
    exit 2
    ;;
esac

check_quantum_pow_wrapper pallets/quantum-pow/src/weights.rs

echo "== Building debug node with runtime-benchmarks =="
cargo build --features runtime-benchmarks -p quip-network-node

BIN="${CARGO_TARGET_DIR:-target}/debug/quip-network-node"
OUTPUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/quip-benchmark-preflight.XXXXXX")"
trap 'rm -rf "$OUTPUT_DIR"' EXIT

LIST_FILE="$OUTPUT_DIR/benchmarks.csv"
"$BIN" benchmark pallet --list > "$LIST_FILE"

mapfile -t pallets < <(
  awk -F', ' '/^[a-z0-9_]+, / && $1 != "pallet" {print $1}' "$LIST_FILE" \
    | sort -u
)

benchmark_count="$(
  awk -F', ' '/^[a-z0-9_]+, / && $1 != "pallet" {count++} END {print count + 0}' \
    "$LIST_FILE"
)"

if [ "${#pallets[@]}" -eq 0 ] || [ "$benchmark_count" -eq 0 ]; then
  echo "ERROR: discovered no runtime benchmarks" >&2
  exit 1
fi

echo "== Discovered $benchmark_count benchmarks across ${#pallets[@]} pallets =="

for pallet in "${pallets[@]}"; do
  output="$OUTPUT_DIR/$pallet.rs"
  echo "-- $pallet"
  "$BIN" benchmark pallet \
    --pallet "$pallet" \
    --extrinsic '*' \
    --steps "$STEPS" \
    --repeat "$REPEAT" \
    --min-duration 0 \
    --template .maintain/frame-weight-template.hbs \
    --output "$output"

  pallet_dir="pallets/$(echo "${pallet#pallet_}" | tr '_' '-')"
  tracked_weights="$pallet_dir/src/weights.rs"
  public_weights="$tracked_weights"
  if [ "$pallet" = "pallet_quantum_pow" ]; then
    tracked_weights="$pallet_dir/src/benchmark_weights.rs"
  fi
  if [ ! -f "$tracked_weights" ]; then
    continue
  fi

  while IFS='|' read -r _ extrinsic; do
    tracked_signature="$(extract_signature "$extrinsic" "$tracked_weights")"
    generated_signature="$(extract_signature "$extrinsic" "$output")"

    if [ -z "$tracked_signature" ] || [ -z "$generated_signature" ]; then
      echo "ERROR: missing WeightInfo signature for $pallet::$extrinsic" >&2
      exit 1
    fi

    if [ "$(printf '%s' "$tracked_signature" | normalize_signature)" != \
      "$(printf '%s' "$generated_signature" | normalize_signature)" ]; then
      echo "ERROR: generated signature differs for $pallet::$extrinsic" >&2
      echo "  tracked:   $tracked_signature" >&2
      echo "  generated: $generated_signature" >&2
      exit 1
    fi

    if [ "$pallet" = "pallet_quantum_pow" ]; then
      public_signature="$(extract_signature "$extrinsic" "$public_weights")"
      if [ -z "$public_signature" ] || \
        [ "$(printf '%s' "$public_signature" | normalize_signature)" != \
          "$(printf '%s' "$generated_signature" | normalize_signature)" ]; then
        echo "ERROR: public nonlinear wrapper signature differs for $pallet::$extrinsic" >&2
        echo "  public:    $public_signature" >&2
        echo "  generated: $generated_signature" >&2
        exit 1
      fi
    fi
  done < <(
    awk -F', ' -v pallet="$pallet" \
      '$1 == pallet {print $1 "|" $2}' \
      "$LIST_FILE"
  )
done

echo "== Production-runtime benchmark preflight passed =="
echo "Executed $benchmark_count benchmarks without modifying tracked weights."

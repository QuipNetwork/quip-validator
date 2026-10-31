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

check_quantum_compute_mempool_wrapper() {
  local file="$1"
  local normalized
  local required

  if [ ! -f "$file" ]; then
    echo "ERROR: Quantum Compute Mempool public weights wrapper not found at $file" >&2
    return 1
  fi
  normalized="$(normalize_signature < "$file")"
  for required in \
    'usecrate::benchmark_weights' \
    'pubconstCLAIM_REWARD_PROOF_FLOOR:u64=1024*1024;' \
    'fnwith_claim_reward_proof_floor(' \
    'weight.max(Weight::from_parts(0,CLAIM_REWARD_PROOF_FLOOR))' \
    'claim_reward_weight::<benchmark_weights::SubstrateWeight<T>>()' \
    'claim_reward_weight::<()>()' \
    'with_claim_reward_proof_floor(W::claim_reward().max(W::claim_reward_dust()))' \
    'pubconstRECLAIM_ORDER_PROOF_FLOOR:u64=1024*1024;' \
    'weight.max(Weight::from_parts(0,RECLAIM_ORDER_PROOF_FLOOR))' \
    'with_reclaim_order_proof_floor(<benchmark_weights::SubstrateWeight<T>asBenchmarkWeightInfo>::reclaim_order())' \
    'with_reclaim_order_proof_floor(<()asBenchmarkWeightInfo>::reclaim_order())' \
    'pubconstPROPOSE_JOB_PROOF_FLOOR:u64=1024*1024;' \
    'weight.max(Weight::from_parts(0,PROPOSE_JOB_PROOF_FLOOR))' \
    'with_propose_job_proof_floor(<benchmark_weights::SubstrateWeight<T>asBenchmarkWeightInfo>::propose_job(' \
    'with_propose_job_proof_floor(<()asBenchmarkWeightInfo>::propose_job(' \
    'with_propose_job_proof_floor(<benchmark_weights::SubstrateWeight<T>asBenchmarkWeightInfo>::propose_job_with_payee_requirement(' \
    'with_propose_job_proof_floor(<()asBenchmarkWeightInfo>::propose_job_with_payee_requirement(' \
    'pubconstSUBMIT_SOLUTION_PROOF_FLOOR:u64=1024*1024;' \
    'weight.max(Weight::from_parts(0,SUBMIT_SOLUTION_PROOF_FLOOR))' \
    'with_submit_solution_proof_floor(W::submit_solution(n,e,s).max(W::submit_solution_with_payee(n,e,s)).max(W::submit_solution_replacement(n,e,s)).max(W::submit_solution_clear_payee(n,e,s)))' \
    'fnsubmit_solution(n:u32,e:u32,s:u32)->Weight{submit_solution_weight::<benchmark_weights::SubstrateWeight<T>>(n,e,s)}' \
    'fnsubmit_solution_with_payee(n:u32,e:u32,s:u32)->Weight{submit_solution_weight::<benchmark_weights::SubstrateWeight<T>>(n,e,s)}' \
    'fnsubmit_solution(n:u32,e:u32,s:u32)->Weight{submit_solution_weight::<()>(n,e,s)}' \
    'fnsubmit_solution_with_payee(n:u32,e:u32,s:u32)->Weight{submit_solution_weight::<()>(n,e,s)}'; do
    if [[ "$normalized" != *"$required"* ]]; then
      echo "ERROR: Quantum Compute Mempool public wrapper lost invariant '$required'" >&2
      return 1
    fi
  done
}

check_wrapper_guard() {
  local temp_dir
  local plain_generated

  check_quantum_pow_wrapper pallets/quantum-pow/src/weights.rs
  check_quantum_compute_mempool_wrapper pallets/quantum-compute-mempool/src/weights.rs

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

  if check_quantum_compute_mempool_wrapper pallets/quantum-compute-mempool/src/benchmark_weights.rs >/dev/null 2>&1; then
    echo "ERROR: generated mempool weights passed the public wrapper guard" >&2
    return 1
  fi

  local case_name
  for case_name in claim_reward_dust submit_solution_replacement submit_solution_clear_payee; do
    sed "s/$case_name/unused_case/g" pallets/quantum-compute-mempool/src/weights.rs > "$plain_generated"
    if check_quantum_compute_mempool_wrapper "$plain_generated" >/dev/null 2>&1; then
      echo "ERROR: unconsumed $case_name passed the wrapper guard" >&2
      return 1
    fi
  done

  echo "Quantum PoW and Quantum Compute Mempool public wrapper guard checks passed."
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
check_quantum_compute_mempool_wrapper pallets/quantum-compute-mempool/src/weights.rs

python3 scripts/check-vesting-weights.py --self-test

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
  template_args=(--template .maintain/frame-weight-template.hbs)
  if [ "$pallet" = "pallet_vesting" ]; then
    template_args=()
  fi
  extra_args=()
  if [ "$pallet" = "pallet_quantum_compute_mempool" ]; then
    extra_args=(--extra)
  fi
  echo "-- $pallet"
  "$BIN" benchmark pallet \
    --pallet "$pallet" \
    --extrinsic '*' "${extra_args[@]}" \
    --steps "$STEPS" \
    --repeat "$REPEAT" \
    --min-duration 0 \
    "${template_args[@]}" \
    --output "$output"

  if [ "$pallet" = "pallet_quantum_compute_mempool" ]; then
    bash scripts/run-benchmarks.sh --verify-regenerated-mempool "$output"
  fi

  if [ "$pallet" = "pallet_vesting" ]; then
    # Check all nine external-trait signatures, including while the tracked
    # module still aliases explicitly provisional upstream weights.
    python3 scripts/check-vesting-weights.py "$output"
    continue
  fi

  pallet_dir="pallets/$(echo "${pallet#pallet_}" | tr '_' '-')"
  tracked_weights="$pallet_dir/src/weights.rs"
  public_weights="$tracked_weights"
  if [ -f "$pallet_dir/src/benchmark_weights.rs" ]; then
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

    if [ "$public_weights" != "$tracked_weights" ]; then
      public_signature="$(extract_signature "$extrinsic" "$public_weights")"
      if [ -z "$public_signature" ] || \
        [ "$(printf '%s' "$public_signature" | normalize_signature)" != \
          "$(printf '%s' "$generated_signature" | normalize_signature)" ]; then
        echo "ERROR: public wrapper signature differs for $pallet::$extrinsic" >&2
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

OVERHEAD_OUTPUT_DIR="$OUTPUT_DIR/overhead"
mkdir -p "$OVERHEAD_OUTPUT_DIR"

echo "== Checking runtime execution-overhead benchmark =="
"$BIN" benchmark overhead \
  --dev \
  --wasm-execution compiled \
  --warmup 1 \
  --repeat 1 \
  --max-ext-per-block 1 \
  --weight-path "$OVERHEAD_OUTPUT_DIR"

for output in block_weights.rs extrinsic_weights.rs; do
  if [ ! -s "$OVERHEAD_OUTPUT_DIR/$output" ]; then
    echo "ERROR: overhead benchmark did not generate $output" >&2
    exit 1
  fi
done

echo "== Production-runtime benchmark preflight passed =="
echo "Executed $benchmark_count pallet benchmarks and the overhead smoke test without modifying tracked weights."

#!/usr/bin/env bash
set -euo pipefail

# Targeted fixed-point sweeps for quantum-pow's linear submit_proof model.
# Run this on the same reference host used for weight generation. Each JSON
# file retains the raw samples needed to validate the n/e terms; summary.tsv
# provides a deterministic compact view for CI and local review, and
# metadata.json records the run context needed for reproduction.
#
# A proof carries exactly one configuration (see `QuantumProof::solutions`),
# so the public weight is a hand-derived linear model over the topology:
#
#   BASE + Kn*n + Ke*e
#
# Use these artifacts to:
# - compare observed maxima with the committed charge and catch undercharging;
# - track median/max/spread against a baseline from the same reference host;
# - localize regressions to node-, edge-, or interaction-heavy work;
# - support release reviews and deliberate, conservatively margined recalibration.
#
# Do not derive and commit weights from summary.tsv alone. The four points
# validate the charged envelope only. Sweeps do not replace FRAME generation
# of base time, proof size, or DB accounting. The safe workflow is: measure on
# the reference host, compare raw samples and current coverage, derive any
# change from measurements plus code analysis, regenerate the FRAME base
# separately, then rerun these points as holdout validation before committing.
#
# The benchmark floors the edge count at `n + 4` (five fundamental cycles, so
# a gauge-trivial draw is a 2^-5 event, not a coin flip) and floors the node
# count where a simple graph can carry `MaxEdges`, keeping the two components
# independent. Every point below respects both floors; see
# `pallets/quantum-pow/src/benchmarking.rs` for the derivations.

REPEAT="${REPEAT:-20}"
BIN="${BIN:-${CARGO_TARGET_DIR:-target}/release/quip-network-node}"
OUTPUT_DIR="${OUTPUT_DIR:-quantum-pow-sweeps}"
JQ="${JQ:-jq}"

case "$REPEAT" in
  ''|*[!0-9]*|0)
    echo "ERROR: REPEAT must be a positive integer, got '$REPEAT'" >&2
    exit 1
    ;;
esac

if [ ! -x "$BIN" ]; then
  echo "ERROR: benchmark node not found at $BIN" >&2
  echo "Build it with: cargo build --release --features runtime-benchmarks -p quip-network-node" >&2
  exit 1
fi

if ! command -v "$JQ" >/dev/null 2>&1; then
  echo "ERROR: summary tool '$JQ' not found; install jq or set JQ to its executable path" >&2
  exit 1
fi

os_release_value() {
  local key="$1"

  if [ ! -r /etc/os-release ]; then
    return
  fi

  awk -F= -v key="$key" '
    $1 == key {
      value = substr($0, index($0, "=") + 1)
      sub(/^"/, "", value)
      sub(/"$/, "", value)
      print value
      exit
    }
  ' /etc/os-release
}

mkdir -p "$OUTPUT_DIR"
SUMMARY_FILE="$OUTPUT_DIR/summary.tsv"
METADATA_FILE="$OUTPUT_DIR/metadata.json"
rm -f "$METADATA_FILE"

points=(
  # Practical floor: the node floor keeping `e` independent of `n`
  # (`n(n-1)/2 >= MaxEdges` first holds at 317) and its path-plus-five-cycles
  # edge floor.
  "minimum:317,321"
  # Large-node slope with edges floored at `n + 4`.
  "nodes:5000,5004"
  # Large-edge slope with nodes floored: 317 nodes carry 50_086 simple edges,
  # so `MaxEdges` clears the simple-graph cap.
  "edges:317,50000"
  # Aggregate maximum; catches interaction costs missed by isolated axes.
  "worst_case:5000,50000"
)

printf 'point\tnodes\tedges\tsamples\tmin_ns\tmedian_ns\tmax_ns\n' > "$SUMMARY_FILE"

for entry in "${points[@]}"; do
  name="${entry%%:*}"
  dimensions="${entry#*:}"
  IFS=',' read -r nodes edges <<< "$dimensions"
  output="$OUTPUT_DIR/$name.json"
  rm -f "$output"

  echo "== submit_proof $name ($dimensions) =="
  "$BIN" benchmark pallet \
    --pallet pallet_quantum_pow \
    --extrinsic submit_proof \
    --steps 2 \
    --repeat "$REPEAT" \
    --min-duration 0 \
    --low "$dimensions" \
    --high "$dimensions" \
    --no-median-slopes \
    --no-min-squares \
    --json-file "$output"

  if [ ! -s "$output" ]; then
    echo "ERROR: benchmark produced no JSON for '$name' at $output" >&2
    exit 1
  fi

  if ! stats="$(
    "$JQ" -er \
      --argjson nodes "$nodes" \
      --argjson edges "$edges" \
      --argjson minimum_samples "$REPEAT" '
        if type != "array" or length != 1 then
          error("expected exactly one benchmark result")
        elif .[0].pallet != "pallet_quantum_pow"
          or .[0].benchmark != "submit_proof" then
          error("unexpected pallet or benchmark")
        elif (.[0].time_results | type) != "array"
          or (.[0].time_results | length) < $minimum_samples then
          error("missing benchmark samples")
        elif any(.[0].time_results[];
          (.extrinsic_time | type) != "number" or .extrinsic_time < 0) then
          error("invalid extrinsic_time sample")
        elif any(.[0].time_results[];
          (.components | type) != "array"
          or (
            [.components[] | {key: .[0], value: .[1]}]
            | from_entries
          )
            != {"n": $nodes, "e": $edges}) then
          error("sample dimensions do not match the requested fixed point")
        else
          [.[0].time_results[].extrinsic_time] | sort
          | [
              length,
              .[0],
              # Deterministic upper median when the sample count is even.
              .[(length / 2 | floor)],
              .[-1]
            ]
          | @tsv
        end
      ' "$output"
  )"; then
    echo "ERROR: invalid benchmark JSON for '$name' at $output" >&2
    exit 1
  fi

  printf '%s\t%s\t%s\t%s\n' \
    "$name" "$nodes" "$edges" "$stats" >> "$SUMMARY_FILE"
  printf '%s\n' "$stats" \
    | awk -F '\t' '{printf "samples=%s min_ns=%s median_ns=%s max_ns=%s\n", $1, $2, $3, $4}'
done

expected_lines=$((${#points[@]} + 1))
actual_lines="$(wc -l < "$SUMMARY_FILE" | tr -d '[:space:]')"
if [ "$actual_lines" -ne "$expected_lines" ]; then
  echo "ERROR: summary has $actual_lines lines; expected $expected_lines" >&2
  exit 1
fi

points_json="$(
  tail -n +2 "$SUMMARY_FILE" \
    | "$JQ" -Rsc '
        split("\n")
        | map(
            select(length > 0)
            | split("\t")
            | {
                name: .[0],
                nodes: (.[1] | tonumber),
                edges: (.[2] | tonumber),
                sample_count: (.[3] | tonumber)
              }
          )
      '
)"

rustc_vv=""
if command -v rustc >/dev/null 2>&1; then
  rustc_vv="$(rustc -vV 2>/dev/null || true)"
fi

hostname_value="$(hostname 2>/dev/null || true)"
cpu_model=""
if [ -r /proc/cpuinfo ]; then
  cpu_model="$(
    awk -F: '
      /^(model name|Hardware)[[:space:]]*:/ {
        value = $2
        sub(/^[[:space:]]+/, "", value)
        print value
        exit
      }
    ' /proc/cpuinfo
  )"
elif command -v sysctl >/dev/null 2>&1; then
  cpu_model="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || true)"
fi

kernel_value="$(uname -srvmo 2>/dev/null || uname -a 2>/dev/null || true)"
os_id="$(os_release_value ID)"
os_version_id="$(os_release_value VERSION_ID)"
os_pretty_name="$(os_release_value PRETTY_NAME)"

image_ref="${CI_JOB_IMAGE:-${TOOLCHAIN_IMAGE:-}}"
image_tag=""
if [[ -n "$image_ref" && "$image_ref" != *@* ]]; then
  image_name="${image_ref##*/}"
  if [[ "$image_name" == *:* ]]; then
    image_tag="${image_name##*:}"
  fi
fi
image_digest="${CI_JOB_IMAGE_DIGEST:-${TOOLCHAIN_IMAGE_DIGEST:-}}"

METADATA_TMP="$(mktemp "$OUTPUT_DIR/.metadata.json.XXXXXX")"
cleanup_metadata_tmp() {
  if [ -n "${METADATA_TMP:-}" ]; then
    rm -f -- "$METADATA_TMP"
  fi
}
trap cleanup_metadata_tmp EXIT

"$JQ" -nS \
  --arg job_id "${CI_JOB_ID:-}" \
  --arg job_url "${CI_JOB_URL:-}" \
  --arg commit_sha "${CI_COMMIT_SHA:-}" \
  --arg pipeline_id "${CI_PIPELINE_ID:-}" \
  --arg image_ref "$image_ref" \
  --arg image_tag "$image_tag" \
  --arg image_digest "$image_digest" \
  --arg rustc_vv "$rustc_vv" \
  --arg hostname "$hostname_value" \
  --arg cpu_model "$cpu_model" \
  --arg kernel "$kernel_value" \
  --arg os_id "$os_id" \
  --arg os_version_id "$os_version_id" \
  --arg os_pretty_name "$os_pretty_name" \
  --argjson repeat "$REPEAT" \
  --argjson points "$points_json" '
    def nullable: if . == "" then null else . end;
    {
      schema_version: 1,
      gitlab: {
        job_id: ($job_id | nullable),
        job_url: ($job_url | nullable),
        commit_sha: ($commit_sha | nullable),
        pipeline_id: ($pipeline_id | nullable)
      },
      container: {
        image_ref: ($image_ref | nullable),
        image_tag: ($image_tag | nullable),
        image_digest: ($image_digest | nullable)
      },
      toolchain: {
        rustc_vv: ($rustc_vv | nullable)
      },
      host: {
        hostname: ($hostname | nullable),
        cpu_model: ($cpu_model | nullable),
        kernel: ($kernel | nullable),
        os: {
          id: ($os_id | nullable),
          version_id: ($os_version_id | nullable),
          pretty_name: ($os_pretty_name | nullable)
        }
      },
      sweep: {
        pallet: "pallet_quantum_pow",
        extrinsic: "submit_proof",
        steps: 2,
        repeat: $repeat,
        min_duration: 0,
        point_count: ($points | length),
        points: $points
      }
    }
  ' > "$METADATA_TMP"

if ! "$JQ" -e '
  def optional_string: . == null or type == "string";
  .schema_version == 1
    and (.gitlab | has("job_id") and has("job_url")
      and has("commit_sha") and has("pipeline_id"))
    and all(.gitlab[]; optional_string)
    and (.container | has("image_ref") and has("image_tag")
      and has("image_digest"))
    and all(.container[]; optional_string)
    and (.toolchain.rustc_vv | optional_string)
    and (.host.hostname | optional_string)
    and (.host.cpu_model | optional_string)
    and (.host.kernel | optional_string)
    and (.host.os | has("id") and has("version_id") and has("pretty_name"))
    and all(.host.os[]; optional_string)
    and .sweep.pallet == "pallet_quantum_pow"
    and .sweep.extrinsic == "submit_proof"
    and .sweep.steps == 2
    and (.sweep.repeat | type == "number" and . > 0)
    and .sweep.min_duration == 0
    and .sweep.point_count == 4
    and (.sweep.points | type == "array" and length == 4)
    and ([.sweep.points[].name] | unique | length == 4)
    and all(.sweep.points[];
      (.name | type) == "string"
      and (.nodes | type) == "number"
      and (.edges | type) == "number"
      and (.sample_count | type) == "number"
      and .sample_count > 0)
' "$METADATA_TMP" >/dev/null; then
  echo "ERROR: generated sweep metadata failed schema validation" >&2
  exit 1
fi

if [ -n "${CI:-}" ]; then
  if ! "$JQ" -e \
    --arg commit_sha "${CI_COMMIT_SHA:-}" '
      (.gitlab.job_id | type == "string" and length > 0)
        and (.gitlab.job_url | type == "string" and length > 0)
        and (.gitlab.commit_sha == $commit_sha and ($commit_sha | length) > 0)
        and (.container.image_ref | type == "string" and length > 0)
    ' "$METADATA_TMP" >/dev/null; then
    echo "ERROR: generated sweep metadata is missing required CI context" >&2
    exit 1
  fi
fi

while IFS=$'\t' read -r name _nodes _edges samples _rest; do
  if [ "$name" = "point" ]; then
    continue
  fi
  metadata_samples="$(
    "$JQ" -er --arg name "$name" \
      '.sweep.points[] | select(.name == $name) | .sample_count' \
      "$METADATA_TMP"
  )"
  raw_samples="$("$JQ" -er '.[0].time_results | length' "$OUTPUT_DIR/$name.json")"
  if [ "$metadata_samples" -ne "$samples" ] || [ "$metadata_samples" -ne "$raw_samples" ]; then
    echo "ERROR: metadata sample count mismatch for '$name'" >&2
    exit 1
  fi
done < "$SUMMARY_FILE"

mv "$METADATA_TMP" "$METADATA_FILE"
METADATA_TMP=""
trap - EXIT

echo "Raw sweep results, summary, and metadata written to $OUTPUT_DIR"

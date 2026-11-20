#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/check-weights-staleness.sh
#
# Warns when a merge request changes a pallet's benchmarkable surface without
# regenerating that pallet's weights. `benchmark-weights` is a manual job by
# design (it costs a serialized run on the reference machine), so forgetting it
# is otherwise silent: the MR merges with stale weights and the drift only
# surfaces later.
#
# This is a path-level heuristic, not an analysis. It cannot tell a comment
# change from a dispatch-path change, so it warns on some merge requests that
# genuinely need nothing. That is the intended trade — it is a reminder, not a
# gate. The calling job sets `allow_failure: true`, so a warning is yellow.
#
# Classification:
#   touched    pallets/<name>/src/{lib,benchmarking}.rs changed
#   refreshed  pallets/<name>/src/{weights,benchmark_weights}.rs changed
#   stale      touched minus refreshed
#
# `touched` is filtered to pallets that actually have a benchmarking.rs in the
# tree. This excludes evm-chain-id, which carries no weights. Faucet and
# emission-controller now have benchmarks and are included automatically.
# Reading the tree rather than a hardcoded list means a pallet starts being
# covered the moment it gains benchmarks.
#
# quantum-pow's generated output is benchmark_weights.rs, not weights.rs, hence
# both names in the `refreshed` pattern.
#
# Required environment (CI-provided):
#   CI_MERGE_REQUEST_DIFF_BASE_SHA   merge-request pipelines only
#
# Exits 0 when clean, 1 when at least one pallet looks stale.

# Placeholder sentinel scan (W1-008). The path heuristic below counts any
# weights.rs edit as "refreshed", so a pallet that ships a generated weights.rs
# still carrying an unmeasured placeholder coefficient would pass as green while
# its production weight is wrong. A weights.rs containing BENCHMARK-PLACEHOLDER
# is therefore treated as stale regardless of the diff. The reference machine's
# `benchmark-weights` job overwrites the file and removes the sentinel, so this
# clears automatically once real weights land.
PALLETS_ROOT="${WEIGHTS_STALENESS_PALLETS_ROOT:-pallets}"
SENTINEL='BENCHMARK-PLACEHOLDER'

scan_placeholders() {
  local found="" w
  for w in "$PALLETS_ROOT"/*/src/weights.rs; do
    [ -f "$w" ] || continue
    if grep -q "$SENTINEL" "$w"; then
      found+="$(basename "$(dirname "$(dirname "$w")")")"$'\n'
    fi
  done
  printf '%s' "$found" | sed '/^$/d' | sort -u
}

# `--placeholders-only`: scan the tree for placeholder weights and exit, without
# needing a merge-request diff. Used by the mutation-control test.
if [ "${1:-}" = "--placeholders-only" ]; then
  placeholder_stale="$(scan_placeholders)"
  if [ -n "$placeholder_stale" ]; then
    echo "STALE (placeholder weights): these pallets ship an unmeasured ${SENTINEL}:"
    printf '%s\n' "$placeholder_stale" | sed 's/^/  - /'
    echo
    echo "Play the manual 'benchmark-weights' job to regenerate real weights and drop the sentinel."
    exit 1
  fi
  echo "OK: no pallet ships a ${SENTINEL} weight."
  exit 0
fi

base="${CI_MERGE_REQUEST_DIFF_BASE_SHA:?not a merge-request pipeline (needs GIT_DEPTH 0)}"

changed="$(git diff --name-only "$base"...HEAD)"

# Note the s#...# delimiter: using `|` collides with the \| alternation and
# silently matches nothing.
pallets_matching() {
  printf '%s\n' "$changed" | sed -n "$1" | sed '/^$/d' | sort -u
}

touched_all="$(pallets_matching 's#^pallets/\([^/]*\)/src/\(lib\|benchmarking\)\.rs$#\1#p')"
refreshed="$(pallets_matching 's#^pallets/\([^/]*\)/src/\(weights\|benchmark_weights\)\.rs$#\1#p')"

touched=""
unbenchmarked=""
while IFS= read -r pallet; do
  [ -n "$pallet" ] || continue
  if [ -f "pallets/$pallet/src/benchmarking.rs" ]; then
    touched+="$pallet"$'\n'
  else
    unbenchmarked+="$pallet"$'\n'
  fi
done <<<"$touched_all"

stale="$(comm -23 \
  <(printf '%s' "$touched" | sed '/^$/d') \
  <(printf '%s\n' "$refreshed" | sed '/^$/d'))"

if [ -n "$unbenchmarked" ]; then
  echo "Ignoring pallets with no benchmarking.rs (nothing regenerates their weights):"
  printf '%s' "$unbenchmarked" | sed 's/^/  - /'
  echo
fi

if printf '%s\n' "$changed" | grep -qx 'runtime/src/benchmarks.rs'; then
  echo "NOTE: runtime/src/benchmarks.rs changed — the registered pallet set may have moved."
  echo
fi

placeholder_stale="$(scan_placeholders)"

if [ -z "$stale" ] && [ -z "$placeholder_stale" ]; then
  echo "OK: no pallet changed its benchmarkable surface without refreshed weights, and none ships a placeholder weight."
  exit 0
fi

if [ -n "$stale" ]; then
  echo "WARNING: these pallets changed lib.rs/benchmarking.rs but their weights were not regenerated:"
  printf '%s\n' "$stale" | sed 's/^/  - /'
  echo
fi

if [ -n "$placeholder_stale" ]; then
  echo "WARNING: these pallets ship an unmeasured ${SENTINEL} weight (W1-008):"
  printf '%s\n' "$placeholder_stale" | sed 's/^/  - /'
  echo
fi

echo "If the change can affect weights, play the manual 'benchmark-weights' job on this MR."
echo "If it cannot (comments, tests, internal refactor with no dispatch-path change), ignore this."
exit 1

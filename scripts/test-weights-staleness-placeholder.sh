#!/usr/bin/env bash
# Mutation control for the BENCHMARK-PLACEHOLDER staleness scan added to
# check-weights-staleness.sh (W1-008 part 2). It proves the scan reports stale
# with the sentinel present and clean without it, so a broken scan (always-pass
# or always-fail) flips one assertion.
#
# Disposable fixtures, pointed at via WEIGHTS_STALENESS_PALLETS_ROOT, keep this
# stable after the reference machine regenerates the real weights and the
# sentinel disappears from the tree. Working trees are never mutated.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
script="$repo_root/scripts/check-weights-staleness.sh"

fail() { echo "FAIL: $1" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/demo/src"

run() { WEIGHTS_STALENESS_PALLETS_ROOT="$tmp" bash "$script" --placeholders-only 2>&1; }

# With the sentinel: the scan must report stale (exit 1) and name the pallet.
cat > "$tmp/demo/src/weights.rs" <<'RS'
// BENCHMARK-PLACEHOLDER(W1-008)
pub fn x() {}
RS
set +e
out="$(run)"; code=$?
set -e
[ "$code" -eq 1 ] || fail "sentinel present: expected exit 1, got $code (output: $out)"
printf '%s\n' "$out" | grep -q 'demo' || fail "sentinel present: pallet 'demo' not reported"

# Without the sentinel: the scan must pass (exit 0).
cat > "$tmp/demo/src/weights.rs" <<'RS'
pub fn x() {}
RS
set +e
out="$(run)"; code=$?
set -e
[ "$code" -eq 0 ] || fail "sentinel absent: expected exit 0, got $code (output: $out)"

echo "OK: placeholder scan reports stale with the sentinel and clean without it."

#!/usr/bin/env bash
# Self-test for scripts/prune-ci-cache.sh. Builds a fake /ci-cache volume in a
# temp dir, puts a fake `cargo` on PATH that records which target it was asked
# to sweep (no real sweep runs), and checks the slot rules:
#   (a) a slot held under a shared in-use lock is neither swept nor removed;
#   (b) a stale, unlocked slot is removed;
#   (c) another job's fresh slot is not swept;
#   (d) the job's own slot is swept;
# plus abandoned/half-created slots, grandfathering, the per-slot sweep
# interval, and an own slot still locked by another holder.
#
# Needs Linux tools (flock, GNU stat/touch), like the script it tests. In CI
# their absence is a failure; elsewhere the test reports SKIP.

# check() takes its condition as a string and evals it later, so the single
# quotes around those conditions defer expansion on purpose.
# shellcheck disable=SC2016
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prune="$here/prune-ci-cache.sh"

if ! command -v flock >/dev/null || ! stat -c %Y / >/dev/null 2>&1; then
  if [[ -n "${CI:-}" ]]; then
    echo "test-prune-ci-cache: FAIL: flock and GNU stat are required" >&2
    exit 1
  fi
  echo "test-prune-ci-cache: SKIP: needs flock and GNU stat (Linux)"
  exit 0
fi

tmp="$(mktemp -d)"
holders=()
cleanup() {
  for pid in "${holders[@]}"; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$tmp"
}
trap cleanup EXIT

root="$tmp/ci-cache"
project="$tmp/project"
bin="$tmp/bin"
sweeps="$tmp/sweeps.log"
mkdir -p "$root" "$project" "$bin"
touch "$project/Cargo.toml" "$sweeps"
cat >"$bin/cargo" <<EOF
#!/usr/bin/env bash
echo "\$CARGO_TARGET_DIR" >>"$sweeps"
EOF
chmod +x "$bin/cargo"

days_ago() { echo "@$(($(date +%s) - $1 * 86400))"; }

# slot ID JOB AGE_DAYS: a slot with a target/ stamped AGE_DAYS ago.
slot() {
  mkdir -p "$root/$1/$2/target"
  touch -d "$(days_ago "$3")" "$root/$1/$2/target/.last-used"
}

# hold PATH: hold a shared lock on PATH's in-use lock until cleanup.
hold() {
  flock --shared "$1/.in-use.lock" sleep 600 &
  holders+=("$!")
  # Wait until the holder really has the lock.
  for _ in $(seq 50); do
    if ! flock --nonblock --exclusive "$1/.in-use.lock" true; then return 0; fi
    sleep 0.1
  done
  echo "test-prune-ci-cache: could not establish the lock holder" >&2
  exit 1
}

run() {
  PATH="$bin:$PATH" CI_CACHE_ROOT="$root" CI_CACHE_PROJECT_DIR="$project" \
    CARGO_TARGET_DIR="$root/1/own/target" bash "$prune" >>"$tmp/prune.log" 2>&1
}

fails=0
check() {
  if eval "$2"; then
    echo "ok   - $1"
  else
    echo "FAIL - $1"
    fails=$((fails + 1))
  fi
}
swept() { grep -qxF "$root/$1/target" "$sweeps"; }

slot 1 own 0
slot 2 held 30
hold "$root/2/held"
slot 3 stale 30
slot 4 fresh 0
mkdir -p "$root/5/abandoned" && touch -d "$(days_ago 30)" "$root/5/abandoned"
mkdir -p "$root/6/unstamped/target"
mkdir -p "$root/7/half-new"

run

check "(d) the job's own slot is swept" 'swept 1/own'
check "(a) a stale slot held under a shared lock is not removed" '[[ -d "$root/2/held/target" ]]'
check "(a) a slot held under a shared lock is not swept" '! swept 2/held'
check "(b) a stale, unlocked slot is removed" '[[ ! -e "$root/3/stale" ]]'
check "(c) another job's fresh slot is kept" '[[ -d "$root/4/fresh/target" ]]'
check "(c) another job's fresh slot is not swept" '! swept 4/fresh'
check "an old slot with no target/ is removed" '[[ ! -e "$root/5/abandoned" ]]'
check "an unstamped slot is grandfathered (kept and stamped)" \
  '[[ -e "$root/6/unstamped/target/.last-used" ]]'
check "a fresh half-created slot is kept" '[[ -d "$root/7/half-new" ]]'
check "only the own slot was swept" '[[ "$(wc -l <"$sweeps")" -eq 1 ]]'

# Inside the per-slot interval the own slot is not swept again.
run
check "the own slot is not re-swept inside the interval" '[[ "$(wc -l <"$sweeps")" -eq 1 ]]'

# Once the interval has passed, an own slot still locked by another holder
# (for example a leaked background process) is skipped, not swept.
touch -d "$(days_ago 2)" "$root/1/own/.last-sweep"
hold "$root/1/own"
run
check "an own slot locked by another holder is not swept" '[[ "$(wc -l <"$sweeps")" -eq 1 ]]'
check "the skip is logged" 'grep -q "still locked by another holder" "$tmp/prune.log"'

if ((fails)); then
  echo "--- prune log ---"
  cat "$tmp/prune.log"
  echo "test-prune-ci-cache: $fails check(s) failed" >&2
  exit 1
fi
echo "test-prune-ci-cache: all checks passed"

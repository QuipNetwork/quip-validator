#!/usr/bin/env bash
# Self-test for scripts/prune-ci-cache.sh. Builds a fake /ci-cache volume in a
# temp dir, puts a fake `cargo` on PATH that records which target it was asked
# to sweep (no real sweep runs), and checks the slot rules:
#   (a) a slot held under a shared in-use lock is neither swept nor removed;
#   (b) a stale, unlocked slot is removed;
#   (c) another job's fresh slot is not swept;
#   (d) the job's own slot is swept;
# plus abandoned/half-created slots, grandfathering, the per-slot sweep
# interval, an own slot still locked by another holder, and a missing
# cargo-sweep, the capacity report (volume, mount, slot sizes within a time
# budget), and rule 3, the opt-in volume budget (LRU whole-slot eviction:
# over/under budget, locked, own, recent, df failure, device mismatch, an
# unparseable df, an invalid or unset budget, and a realistic volume pushed
# over budget by trees that are not our slots). The fake volume mirrors a real host: the shared CARGO_HOME,
# a bench-style target tree, the scan's own lock and stamp, an unmarked
# directory under a numeric id, and a stamped job tree outside the numeric
# layout all sit beside the slots, and all must survive.
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
# release PID: stop a lock holder and the command it runs (flock(1) forks its
# command, and the child may hold the lock too).
release() {
  pkill -P "$1" 2>/dev/null || true
  kill "$1" 2>/dev/null || true
}
cleanup() {
  for pid in "${holders[@]}"; do release "$pid"; done
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
[[ " \$* " == *" --version "* ]] && exit 0
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
  # Detached from our stdout/stderr so a caller piping this test is not held
  # open by the sleeping holder.
  flock --shared "$1/.in-use.lock" sleep 600 </dev/null >/dev/null 2>&1 &
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
    CI_CACHE_DU_BUDGET="${du_budget:-180}" \
    CARGO_TARGET_DIR="$root/1/own/target" bash "$prune" >>"$tmp/prune.log" 2>&1
}
logged() { grep -qF "prune-ci-cache: $1" "$tmp/prune.log"; }

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
# A half-created slot this cache made (it carries the lock file the
# before_script opens), abandoned long ago.
mkdir -p "$root/5/abandoned" && touch "$root/5/abandoned/.in-use.lock"
touch -d "$(days_ago 30)" "$root/5/abandoned"
mkdir -p "$root/6/unstamped/target"
mkdir -p "$root/7/half-new" && touch "$root/7/half-new/.in-use.lock"
# Everything else on a real host volume, all old, none of it a slot:
# the shared CARGO_HOME ...
for d in registry git bin binstall advisory-dbs; do
  mkdir -p "$root/cargo-home/$d" && touch -d "$(days_ago 30)" "$root/cargo-home/$d"
done
touch -d "$(days_ago 30)" "$root/cargo-home"
# ... a bench-style target tree at the top level ...
mkdir -p "$root/target/release" "$root/target/debug"
touch -d "$(days_ago 30)" "$root/target/release" "$root/target/debug"
# ... an unmarked directory under a numeric id (not made by this cache) ...
mkdir -p "$root/8/unmarked" && touch -d "$(days_ago 30)" "$root/8/unmarked"
# ... a stale, stamped job tree outside the numeric layout ...
mkdir -p "$root/bench/benchmark-weights/target"
touch -d "$(days_ago 30)" "$root/bench/benchmark-weights/target/.last-used"
# ... and the scan's own lock and interval stamp, due for a scan.
touch "$root/.prune.lock"
touch -d "$(days_ago 2)" "$root/.last-prune"

run

check "(d) the job's own slot is swept" 'swept 1/own'
check "(a) a stale slot held under a shared lock is not removed" '[[ -d "$root/2/held/target" ]]'
check "(a) a slot held under a shared lock is not swept" '! swept 2/held'
check "(b) a stale, unlocked slot is removed" '[[ ! -e "$root/3/stale" ]]'
check "(c) another job's fresh slot is kept" '[[ -d "$root/4/fresh/target" ]]'
check "(c) another job's fresh slot is not swept" '! swept 4/fresh'
check "an old marked slot with no target/ is removed" '[[ ! -e "$root/5/abandoned" ]]'
check "an unstamped slot is grandfathered (kept and stamped)" \
  '[[ -e "$root/6/unstamped/target/.last-used" ]]'
check "a fresh half-created slot is kept" '[[ -d "$root/7/half-new" ]]'
for d in registry git bin binstall advisory-dbs; do
  check "the shared CARGO_HOME $d survives" "[[ -d \"\$root/cargo-home/$d\" ]]"
done
check "a bench-style target tree survives" \
  '[[ -d "$root/target/release" && -d "$root/target/debug" ]]'
check "an unmarked directory under a numeric id survives" '[[ -d "$root/8/unmarked" ]]'
check "a stamped job tree outside the numeric layout survives" \
  '[[ -d "$root/bench/benchmark-weights/target" ]]'
check "the scan's own lock and stamp survive" \
  '[[ -e "$root/.prune.lock" && -e "$root/.last-prune" ]]'
check "only the own slot was swept" '[[ "$(wc -l <"$sweeps")" -eq 1 ]]'

# Capacity report: reading only, nothing above depends on it.
check "the volume's df line is reported" 'logged "volume: "'
check "the mount entry is reported" 'logged "mount: "'
check "the own slot's size is reported before and after the sweep" \
  'logged "own slot size before sweep: " && logged "own slot size after sweep: "'
for s in 1/own 2/held 4/fresh 6/unstamped 7/half-new 8/unmarked; do
  check "the scan reports the size of numeric slot $s" "logged \"size: \$root/$s \""
done
check "the scan reports the shared CARGO_HOME's size" 'logged "size: $root/cargo-home "'
check "no size is reported outside the numeric layout" \
  '! grep -qE "size: $root/(bench|target|cargo-home/)" "$tmp/prune.log"'
# shellcheck disable=SC2034 # used inside check's eval'd condition
sz='[0-9][0-9.,]*[A-Za-z]*'
check "a measured size is a size on disk and apparent, not an error" \
  'grep -qE "size: $root/1/own $sz on disk, $sz apparent\$" "$tmp/prune.log" \
    && grep -qE "own slot size after sweep: $sz on disk, $sz apparent\$" "$tmp/prune.log"'

# A spent budget stops the size report and says so; the scan still finishes.
: >"$tmp/prune.log"
touch -d "$(days_ago 2)" "$root/.last-prune"
du_budget=0 run
check "a spent size budget is logged" 'logged "size budget of 0s spent after 0 report(s)"'
check "a spent size budget reports no slot size" '! logged "size: "'
check "the scan still records its interval" \
  '[[ "$(($(date +%s) - $(stat -c %Y "$root/.last-prune")))" -lt 60 ]]'

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

# A missing cargo-sweep (say, after CARGO_HOME/bin was wiped) costs the sweep,
# never the job: the script still exits 0 and says why it skipped.
cat >"$bin/cargo" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == "sweep" ]] && { echo "error: no such command: \`sweep\`" >&2; exit 101; }
exit 0
EOF
release "${holders[-1]}"
touch -d "$(days_ago 2)" "$root/1/own/.last-sweep"
if run; then
  check "a missing cargo-sweep does not fail the prune" true
else
  check "a missing cargo-sweep does not fail the prune" false
fi
check "a missing cargo-sweep is logged" 'grep -q "cargo-sweep is not available" "$tmp/prune.log"'
check "nothing was swept without cargo-sweep" '[[ "$(wc -l <"$sweeps")" -eq 1 ]]'

# --- Rule 3: the volume budget ---------------------------------------------
# Each scenario gets a fresh volume. A fake `df` reports a 2,000,000 KiB
# volume whose usage is the sum of `.fake-kb` files under it, so removing a
# slot lowers the usage the next `df` reports; a fake mount table names the
# same device. Sizes are KiB of pretend data, not real files.
broot="$tmp/budget/ci-cache"
fakebin="$tmp/fakebin"
mkdir -p "$fakebin"
cat >"$fakebin/df" <<EOF
#!/usr/bin/env bash
[[ -e "$tmp/df.fail" ]] && exit 1
if [[ -e "$tmp/df.garbage" ]]; then echo "Filesystem stuff"; echo "nonsense here"; exit 0; fi
used=0
while IFS= read -r f; do used=\$((used + \$(cat "\$f"))); done < <(find "$broot" -name .fake-kb)
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "fakepool/srv 2000000 \$used \$((2000000 - used)) \$((used / 20000))% $broot"
EOF
chmod +x "$fakebin/df"

# bslot ID JOB AGE_SECONDS KIB: a marked slot last used AGE_SECONDS ago.
bslot() {
  mkdir -p "$broot/$1/$2/target"
  echo "$4" >"$broot/$1/$2/target/.fake-kb"
  touch -d "@$(($(date +%s) - $3))" "$broot/$1/$2/target/.last-used"
}
# tree PATH KIB: anything else on the volume that takes space.
tree() {
  mkdir -p "$broot/$1"
  echo "$2" >"$broot/$1/.fake-kb"
}
budget_volume() {
  for pid in "${holders[@]}"; do release "$pid"; done
  holders=()
  rm -rf "$tmp/budget" "$tmp/df.fail" "$tmp/df.garbage"
  mkdir -p "$broot"
  echo "fakepool/srv $broot zfs rw,noatime 0 0" >"$tmp/mounts"
  day=86400
  bslot 1 own 0 300000            # the job's own slot
  bslot 2 old $((10 * day)) 200000 # least recently used candidate
  bslot 3 mid $((5 * day)) 200000  # next
  bslot 4 held $((20 * day)) 200000
  hold "$broot/4/held"             # oldest, but in use
  bslot 5 recent 1800 200000       # used 30 minutes ago
  tree cargo-home/registry 100000  # the shared CARGO_HOME
  tree 6/unmarked 100000           # numeric, but not ours (no stamp)
  mkdir -p "$broot/7/unstamped/target" && echo 50000 >"$broot/7/unstamped/target/.fake-kb"
  # Old, so that only the marker and layout gates keep these.
  touch -d "@$(($(date +%s) - 30 * day))" "$broot/6/unmarked" "$broot/7/unstamped" \
    "$broot/7/unstamped/target"
  mkdir -p "$broot/bench/weights/target" && echo 50000 >"$broot/bench/weights/target/.fake-kb"
  touch -d "@$(($(date +%s) - 30 * day))" "$broot/bench/weights/target/.last-used"
  # A fresh orphan-scan stamp keeps rule 2 out of these scenarios.
  touch "$broot/.prune.lock" "$broot/.last-prune"
  : >"$tmp/budget.log"
}
# Total pretend usage: 1,400,000 KiB of 2,000,000 = 70%.
brun() {
  PATH="$fakebin:$bin:$PATH" CI_CACHE_ROOT="$broot" CI_CACHE_PROJECT_DIR="$project" \
    CI_CACHE_MOUNTS="$tmp/mounts" CARGO_TARGET_DIR="$broot/1/own/target" \
    "$@" bash "$prune" >>"$tmp/budget.log" 2>&1
}
blogged() { grep -qF "prune-ci-cache: volume budget: $1" "$tmp/budget.log"; }
survives() { [[ -d "$broot/$1" ]]; }

# Over budget: LRU eviction stops once under it, and skips every guarded slot.
budget_volume
brun env CI_CACHE_VOLUME_BUDGET=55
check "budget: the least recently used unlocked slot is removed" '! survives 2/old'
check "budget: eviction continues in LRU order while over budget" '! survives 3/mid'
check "budget: eviction stops once under budget" 'blogged "$broot is 50% used, budget 55%"'
check "budget: a slot whose lock is held is kept" 'survives 4/held/target'
check "budget: the held slot is logged and skipped" 'blogged "$broot/4/held is in use (lock held), skipping"'
check "budget: the own slot is kept" 'survives 1/own/target'
check "budget: a slot used within the idle window is kept" 'survives 5/recent/target'
check "budget: an unmarked numeric directory is kept" 'survives 6/unmarked'
check "budget: an unstamped numeric target/ is kept" 'survives 7/unstamped/target'
check "budget: a stamped tree outside the numeric layout is kept" 'survives bench/weights/target'
check "budget: the shared CARGO_HOME is kept" 'survives cargo-home/registry'

# With no idle window every slot is old enough: the own slot is still kept.
budget_volume
brun env CI_CACHE_VOLUME_BUDGET=10 CI_CACHE_BUDGET_MIN_IDLE=0
check "budget: with no idle window a recent slot may go" '! survives 5/recent'
check "budget: with no idle window the own slot is still kept" 'survives 1/own/target'
check "budget: with no idle window the held slot is still kept" 'survives 4/held/target'

# Under budget: nothing is removed.
budget_volume
brun env CI_CACHE_VOLUME_BUDGET=80
check "budget: under budget nothing is removed" 'survives 2/old && survives 3/mid'
check "budget: under budget the usage is logged" 'blogged "$broot is 70% used, budget 80%"'

# Unset (the default in this MR): the rule is inert even far over budget.
budget_volume
brun env -u CI_CACHE_VOLUME_BUDGET
check "budget: unset means inert" 'survives 2/old && survives 3/mid && ! grep -q "volume budget" "$tmp/budget.log"'

# An invalid budget removes nothing.
for bad in 0 100 abc 50%; do
  budget_volume
  brun env CI_CACHE_VOLUME_BUDGET="$bad"
  check "budget: invalid value '$bad' removes nothing" \
    'survives 2/old && blogged "CI_CACHE_VOLUME_BUDGET=$bad is not a percentage 1-99; removing nothing"'
done

# Fail closed: df failing, df unparseable, df's device not the mount entry's.
budget_volume
touch "$tmp/df.fail"
brun env CI_CACHE_VOLUME_BUDGET=10
check "budget: a failing df removes nothing" 'survives 2/old && blogged "cannot read a usage"'
budget_volume
touch "$tmp/df.garbage"
brun env CI_CACHE_VOLUME_BUDGET=10
check "budget: an unparseable df removes nothing" 'survives 2/old && blogged "cannot read a usage"'
budget_volume
echo "otherpool/srv $broot zfs rw,noatime 0 0" >"$tmp/mounts"
brun env CI_CACHE_VOLUME_BUDGET=10
check "budget: a df device that is not the mount entry's removes nothing" \
  'survives 2/old && blogged "cannot read a usage"'
budget_volume
echo "fakepool/srv $tmp/budget zfs rw,noatime 0 0" >"$tmp/mounts"
brun env CI_CACHE_VOLUME_BUDGET=10
check "budget: a df mount point that is not the mount entry's removes nothing" \
  'survives 2/old && blogged "cannot read a usage"'

# A realistic volume pushed over budget by what is not ours: every candidate
# goes, nothing outside our slots does, and the run ends over budget, logged.
budget_volume
tree cargo-home/git 300000   # 1,900,000 KiB in all: 95%
tree target/release 200000
brun env CI_CACHE_VOLUME_BUDGET=50
check "budget (realistic): the eligible slots are removed" '! survives 2/old && ! survives 3/mid'
check "budget (realistic): nothing outside our slots is removed" \
  'survives cargo-home/registry && survives cargo-home/git && survives target/release \
    && survives 6/unmarked && survives 7/unstamped/target && survives bench/weights/target'
check "budget (realistic): guarded slots survive" \
  'survives 1/own/target && survives 4/held/target && survives 5/recent/target'
check "budget (realistic): the run ends over budget and says so" \
  'blogged "$broot/5/recent and every later candidate were used within 7200s; keeping them" \
    && blogged "still over budget; no removable slot is left"'

if ((fails)); then
  echo "--- prune log ---"
  cat "$tmp/prune.log"
  echo "test-prune-ci-cache: $fails check(s) failed" >&2
  exit 1
fi
echo "test-prune-ci-cache: all checks passed"

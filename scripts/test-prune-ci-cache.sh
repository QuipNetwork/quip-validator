#!/usr/bin/env bash
# Self-test for scripts/prune-ci-cache.sh. Builds a fake /ci-cache volume in a
# temp dir, puts a fake `cargo` on PATH that records which target it was asked
# to sweep (no real sweep runs), and checks the slot rules:
#   (a) a slot held under a shared in-use lock is neither swept nor evicted;
#   (b) a stale, unlocked slot has its target/ evicted, and the slot directory
#       and its .in-use stay (the prune never removes or renames either);
#   (c) another job's fresh slot is not swept;
#   (d) the job's own slot is swept;
# plus abandoned/half-created slots, grandfathering, the per-slot sweep
# interval, an own slot still locked by another holder, and a missing
# cargo-sweep, the capacity report (volume, mount, slot sizes within a time
# budget), eviction of target/ only (a job that starts mid-delete gets a fresh
# target/, and a waiter that locks the way main's c63fd2f template does --
# without an inode re-check -- still holds the live .in-use afterwards),
# the stamp re-check under the lock (rules 2 and 3), the template's lock step
# (scripts/ci-cache-lock.sh: wait, inode retry, fail closed), and rule 3, the
# opt-in volume budget (one LRU whole-slot removal per run, decided from the
# removed slot's size: over/under budget, a lagging df, locked, own, recent,
# recently built, df failure, device mismatch, an unparseable df, strict
# parsing of --volume-budget, the environment ignored, and a realistic volume
# pushed over budget by trees that are not our slots). The lock is main's
# `.in-use`, so holders that lock the way main's jobs do are honoured. The fake volume mirrors a real host: the shared CARGO_HOME,
# a bench-style target tree, the scan's own lock and stamp, an unmarked
# directory under a numeric id, and a stamped job tree outside the numeric
# layout all sit beside the slots, and all must survive.
#
# Needs Linux tools (flock, GNU stat/touch), like the script it tests. In CI
# their absence is a failure; elsewhere the test reports SKIP.

# check() takes its condition as a string and evals it later, so the single
# quotes around those conditions defer expansion on purpose (SC2016), and
# variables read only inside those conditions look unused (SC2034).
# shellcheck disable=SC2016,SC2034
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

# hold PATH [MODE]: hold a lock (default shared, as a job does) on PATH's
# `.in-use` -- main's lock file and this branch's -- until cleanup.
hold() {
  # Detached from our stdout/stderr so a caller piping this test is not held
  # open by the sleeping holder.
  flock "--${2:-shared}" "$1/.in-use" sleep 600 </dev/null >/dev/null 2>&1 &
  holders+=("$!")
  # Wait until the holder really has the lock.
  for _ in $(seq 50); do
    if ! flock --nonblock --exclusive "$1/.in-use" true; then return 0; fi
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
# (The earlier lock name, still recognised as a marker.)
mkdir -p "$root/5/abandoned" && touch "$root/5/abandoned/.in-use.lock"
touch -d "$(days_ago 30)" "$root/5/abandoned"
mkdir -p "$root/9/abandoned-now" && touch "$root/9/abandoned-now/.in-use"
touch -d "$(days_ago 30)" "$root/9/abandoned-now"
mkdir -p "$root/6/unstamped/target"
mkdir -p "$root/7/half-new" && touch "$root/7/half-new/.in-use"
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
check "(a) a stale slot held under a shared lock is not evicted" '[[ -d "$root/2/held/target" ]]'
check "(a) a slot held under a shared lock is not swept" '! swept 2/held'
check "(b) a stale, unlocked slot has its target/ evicted" '[[ ! -e "$root/3/stale/target" ]]'
check "(b) ... and keeps its directory and .in-use" \
  '[[ -d "$root/3/stale" && -e "$root/3/stale/.in-use" ]]'
check "(c) another job's fresh slot is kept" '[[ -d "$root/4/fresh/target" ]]'
check "(c) another job's fresh slot is not swept" '! swept 4/fresh'
check "a slot with no target/ has nothing to evict and is left as it is" \
  '[[ -e "$root/5/abandoned/.in-use.lock" && -e "$root/9/abandoned-now/.in-use" ]]'
check "eviction leaves no trash behind" \
  '! compgen -G "$root/*/*/.prune-trash.*" >/dev/null && [[ ! -e "$root/.trash" ]]'
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

# --- Shared fixtures for the budget, race and lock-step scenarios -------------
# Each scenario gets a fresh volume. A fake `df` reports a 2,000,000 KiB
# volume whose usage is the sum of `.fake-kb` files under it (or a frozen
# figure, to model a df that lags the delete); a fake `du` reports the same
# pretend sizes; a fake mount table names the same device. Sizes are KiB of
# pretend data, not real files.
broot="$tmp/budget/ci-cache"
fakebin="$tmp/fakebin"
mkdir -p "$fakebin"
cat >"$fakebin/df" <<EOF
#!/usr/bin/env bash
[[ -e "$tmp/df.fail" ]] && exit 1
if [[ -e "$tmp/df.garbage" ]]; then echo "Filesystem stuff"; echo "nonsense here"; exit 0; fi
if [[ -e "$tmp/df.frozen" ]]; then used=\$(cat "$tmp/df.frozen"); else
  used=0
  while IFS= read -r f; do used=\$((used + \$(cat "\$f"))); done < <(find "$broot" -name .fake-kb)
fi
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "fakepool/srv 2000000 \$used \$((2000000 - used)) \$((used / 20000))% $broot"
EOF
cat >"$fakebin/du" <<'EOF'
#!/usr/bin/env bash
path="${*: -1}"
kb=0
while IFS= read -r f; do kb=$((kb + $(cat "$f"))); done < <(find "$path" -name .fake-kb 2>/dev/null)
if [[ " $* " == *" -sk "* ]]; then printf '%s\t%s\n' "$kb" "$path"; else printf '%sK\t%s\n' "$kb" "$path"; fi
EOF
chmod +x "$fakebin/df" "$fakebin/du"

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
  rm -rf "$tmp/budget" "$tmp/df.fail" "$tmp/df.garbage" "$tmp/df.frozen" "$tmp/refresh.list"
  mkdir -p "$broot"
  echo "fakepool/srv $broot zfs rw,noatime 0 0" >"$tmp/mounts"
  day=86400
  bslot 1 own 0 300000            # the job's own slot
  bslot 2 old $((10 * day)) 200000 # least recently used candidate
  bslot 3 mid $((5 * day)) 200000  # next
  bslot 4 held $((20 * day)) 200000
  hold "$broot/4/held"             # oldest, but in use (main-style shared hold)
  bslot 5 recent 1800 200000       # used 30 minutes ago
  tree cargo-home/registry 100000  # the shared CARGO_HOME
  tree 6/unmarked 100000           # numeric, but not ours (no stamp)
  mkdir -p "$broot/7/unstamped/target" && echo 50000 >"$broot/7/unstamped/target/.fake-kb"
  # Old, so that only the marker and layout gates keep these.
  touch -d "@$(($(date +%s) - 30 * day))" "$broot/6/unmarked" "$broot/7/unstamped" \
    "$broot/7/unstamped/target"
  mkdir -p "$broot/bench/weights/target" && echo 50000 >"$broot/bench/weights/target/.fake-kb"
  touch -d "@$(($(date +%s) - 30 * day))" "$broot/bench/weights/target/.last-used"
  # A fresh orphan-scan stamp keeps rule 2 out of the budget scenarios.
  touch "$broot/.prune.lock" "$broot/.last-prune"
  : >"$tmp/budget.log"
}
# Total pretend usage: 1,400,000 KiB of 2,000,000 = 70%.
benv=()
bpath=""
# brun [prune args...]: one prune of the fake volume; extra environment in
# the array `benv`, extra PATH entries (wrappers) in `bpath`.
brun() {
  env "${benv[@]}" PATH="$bpath$fakebin:$bin:$PATH" CI_CACHE_ROOT="$broot" \
    CI_CACHE_PROJECT_DIR="$project" CI_CACHE_MOUNTS="$tmp/mounts" \
    CARGO_TARGET_DIR="$broot/1/own/target" \
    bash "$prune" "$@" >>"$tmp/budget.log" 2>&1
}
blogged() { grep -qF "prune-ci-cache: volume budget: $1" "$tmp/budget.log"; }
survives() { [[ -d "$broot/$1" ]]; }
# evicted SLOT: its target/ is gone, its directory and .in-use stay.
evicted() { [[ -d "$broot/$1" && ! -e "$broot/$1/target" ]]; }
# kept SLOT: its target/ is still there.
kept() { [[ -d "$broot/$1/target" ]]; }
# runs N ARGS...: N prunes in a row (rule 3 removes at most one slot per run).
runs() {
  local n="$1"
  shift
  for _ in $(seq "$n"); do brun "$@"; done
}

# --- Rule 3: the volume budget ---------------------------------------------
# Over budget: one LRU removal per run, decided from the removed slot's size.
budget_volume
brun --volume-budget 55
check "budget: one run removes exactly one slot, the least recently used unlocked one" \
  'evicted 2/old && kept 3/mid'
check "budget: the projection uses the removed slot's size" \
  'blogged "projected 60% after evicting $broot/2/old/target; still over budget, the next prune continues"'
brun --volume-budget 55
check "budget: the next run continues in LRU order" 'evicted 3/mid'
check "budget: and stops once under budget" \
  'blogged "projected 50% after evicting $broot/3/mid/target; under budget"'
brun --volume-budget 55
check "budget: under budget nothing more is removed" \
  'survives 4/held/target && survives 5/recent/target && blogged "$broot is 50% used, budget 55%"'
check "budget: a slot locked the way main's jobs lock (.in-use, shared) is kept" \
  'survives 4/held/target && blogged "$broot/4/held is in use (lock held), skipping"'
check "budget: the own slot is kept" 'survives 1/own/target'
check "budget: a slot used within the idle window is kept" 'survives 5/recent/target'
check "budget: an unmarked numeric directory is kept" 'survives 6/unmarked'
check "budget: an unstamped numeric target/ is kept" 'survives 7/unstamped/target'
check "budget: a stamped tree outside the numeric layout is kept" 'survives bench/weights/target'
check "budget: the shared CARGO_HOME is kept" 'survives cargo-home/registry'
check "budget: eviction leaves no trash behind" \
  '! compgen -G "$broot/*/*/.prune-trash.*" >/dev/null'

# A df that lags the delete (ZFS frees at txg sync): still one removal per
# run, and the projection comes from the slot's size, not from df.
budget_volume
echo 1400000 >"$tmp/df.frozen"
brun --volume-budget 50
check "budget (lagging df): one run removes one slot only" 'evicted 2/old && kept 3/mid'
check "budget (lagging df): the projection ignores the stale df" \
  'blogged "projected 60% after evicting $broot/2/old/target; still over budget"'

# Recently built: a stale stamp but a fresh cargo fingerprint is in use.
budget_volume
mkdir -p "$broot/2/old/target/debug/.fingerprint/dep-0123"
touch "$broot/2/old/target/debug/.fingerprint/dep-0123/lib-dep"
mkdir -p "$broot/3/mid/target/debug/.fingerprint/dep-4567"
touch -d "@$(($(date +%s) - 3 * day))" "$broot/3/mid/target/debug/.fingerprint/dep-4567"
brun --volume-budget 50
check "budget: a slot with fresh cargo fingerprints is kept (activity probe)" \
  'survives 2/old/target && blogged "$broot/2/old has cargo fingerprints newer than 7200s; keeping it"'
check "budget: old fingerprints do not protect a slot" 'evicted 3/mid'

# With no idle window every slot is old enough: own and held are still kept.
budget_volume
benv=(CI_CACHE_BUDGET_MIN_IDLE=0)
runs 6 --volume-budget 10
benv=()
check "budget: with no idle window a recent slot may go" 'evicted 5/recent'
check "budget: with no idle window the own slot is still kept" 'survives 1/own/target'
check "budget: with no idle window the held slot is still kept" 'survives 4/held/target'

# Under budget: nothing is removed.
budget_volume
brun --volume-budget 80
check "budget: under budget nothing is removed" 'kept 2/old && kept 3/mid'
check "budget: under budget the usage is logged" 'blogged "$broot is 70% used, budget 80%"'

# No argument (this MR's YAML): inert, and the environment cannot switch it on.
budget_volume
brun
check "budget: without --volume-budget the rule is inert" \
  'kept 2/old && ! grep -q "volume budget" "$tmp/budget.log"'
budget_volume
benv=(CI_CACHE_VOLUME_BUDGET=10)
brun
benv=()
check "budget: CI_CACHE_VOLUME_BUDGET is ignored (a pipeline variable cannot switch it on)" \
  'kept 2/old && grep -q "CI_CACHE_VOLUME_BUDGET is ignored" "$tmp/budget.log" \
    && ! grep -q "volume budget: $broot" "$tmp/budget.log"'

# Strict parsing: only 1-99 written in decimal without a leading zero.
for bad in 0 100 abc 50% 010 08 077 "" " 80" +80 1e2; do
  budget_volume
  brun --volume-budget "$bad"
  check "budget: '$bad' is not a budget; nothing is removed" \
    'kept 2/old && blogged "'"'"'$bad'"'"' is not a percentage 1-99; removing nothing"'
done
budget_volume
brun --volume-budget
check "budget: a missing value is not a budget" \
  'kept 2/old && blogged "'"''"' is not a percentage 1-99; removing nothing"'
budget_volume
brun --volume-budget=9
check "budget: --volume-budget=9 is read as 9 percent" \
  'blogged "$broot is 70% used, budget 9%" && evicted 2/old'

# Fail closed: df failing, df unparseable, df's device not the mount entry's.
budget_volume
touch "$tmp/df.fail"
brun --volume-budget 10
check "budget: a failing df removes nothing" 'kept 2/old && blogged "cannot read a usage"'
budget_volume
touch "$tmp/df.garbage"
brun --volume-budget 10
check "budget: an unparseable df removes nothing" 'kept 2/old && blogged "cannot read a usage"'
budget_volume
echo "otherpool/srv $broot zfs rw,noatime 0 0" >"$tmp/mounts"
brun --volume-budget 10
check "budget: a df device that is not the mount entry's removes nothing" \
  'kept 2/old && blogged "cannot read a usage"'
budget_volume
echo "fakepool/srv $tmp/budget zfs rw,noatime 0 0" >"$tmp/mounts"
brun --volume-budget 10
check "budget: a df mount point that is not the mount entry's removes nothing" \
  'kept 2/old && blogged "cannot read a usage"'

# A realistic volume pushed over budget by what is not ours: every candidate
# goes (one per run), nothing outside our slots does, and the run that finds
# no candidate left says so.
budget_volume
tree cargo-home/git 300000 # 1,900,000 KiB in all: 95%
tree target/release 200000
runs 4 --volume-budget 50
check "budget (realistic): the eligible slots are removed" 'evicted 2/old && evicted 3/mid'
check "budget (realistic): nothing outside our slots is removed" \
  'survives cargo-home/registry && survives cargo-home/git && survives target/release \
    && survives 6/unmarked && survives 7/unstamped/target && survives bench/weights/target'
check "budget (realistic): guarded slots survive" \
  'survives 1/own/target && survives 4/held/target && survives 5/recent/target'
check "budget (realistic): the run ends over budget and says so" \
  'blogged "$broot/5/recent was used within 7200s; keeping it" \
    && blogged "still over budget; no removable slot is left"'

# --- The re-check under the lock (F5): a job that stamps between the scan
# and the lock keeps its slot. A flock wrapper refreshes the listed stamps
# just before every exclusive lock the prune takes.
real_flock="$(command -v flock)"
refreshbin="$tmp/refreshbin"
mkdir -p "$refreshbin"
cat >"$refreshbin/flock" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *" --exclusive "* && -e "$tmp/refresh.list" ]]; then
  while IFS= read -r stamp; do touch "\$stamp"; done <"$tmp/refresh.list"
fi
exec "$real_flock" "\$@"
EOF
chmod +x "$refreshbin/flock"

budget_volume
# Without the held slot (whose lock the scan would take first), the refresh
# lands on stale's own lock: after its pre-check, before its re-check.
release "${holders[-1]}"
rm -rf "$broot/4/held"
bslot 8 stale $((30 * day)) 10000
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune" # rule 2 due
echo "$broot/8/stale/target/.last-used" >"$tmp/refresh.list"
bpath="$refreshbin:"
brun
bpath=""
check "rule 2: a slot stamped between the scan and the lock survives" \
  'survives 8/stale/target && ! grep -q "evicting the target/ of unused slot $broot/8/stale" "$tmp/budget.log"'
budget_volume
bslot 8 stale $((30 * day)) 10000
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune"
brun
check "rule 2 (control): without the refresh the stale slot's target/ is evicted" 'evicted 8/stale'

budget_volume
# Without the held slot, old is the first candidate, so the refresh lands on
# old's own lock: after its pre-check, before its re-check.
release "${holders[-1]}"
rm -rf "$broot/4/held"
echo "$broot/2/old/target/.last-used" >"$tmp/refresh.list"
bpath="$refreshbin:"
brun --volume-budget 50
bpath=""
check "rule 3: a slot stamped between the scan and the lock survives" \
  'survives 2/old/target && evicted 3/mid'

# --- Eviction never touches the slot directory or .in-use (F1, item 7) ------
# A slow `rm` that deletes a directory's `.in-use` first (if it has one) and
# the rest two seconds later: the order that let a job lock a new `.in-use`
# inside a slot still being deleted. The prune now deletes only the evicted
# target/ (renamed aside first), so the job keeps the slot's one live lock and
# builds into a fresh target/.
slowbin="$tmp/slowbin"
mkdir -p "$slowbin"
real_rm="$(command -v rm)"
real_mv="$(command -v mv)"
cat >"$slowbin/rm" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do [[ -e "\$a/.in-use" ]] && "$real_rm" -f "\$a/.in-use"; done
touch "$tmp/rm.unlinked" # any lock in the doomed tree is gone; the rest is not
sleep 2
exec "$real_rm" "\$@"
EOF
chmod +x "$slowbin/rm"
lockstep="$here/ci-cache-lock.sh"
inode() { stat -c %d:%i "$1" 2>/dev/null || echo missing; }

budget_volume
bslot 8 victim $((30 * day)) 10000
touch "$broot/8/victim/.in-use"
victim_lock="$(inode "$broot/8/victim/.in-use")"
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune"
bpath="$slowbin:"
brun &
prune_pid=$!
bpath=""
# Start the job only once the delete is under way: the window claude's
# simulation found.
rm -f "$tmp/rm.unlinked"
for _ in $(seq 200); do
  [[ -e "$tmp/rm.unlinked" ]] && break
  sleep 0.05
done
CARGO_TARGET_DIR="$broot/8/victim/target" CI_CACHE_LOCK_WAIT=20 \
  bash -c '. "$1"; echo built >"$CARGO_TARGET_DIR/marker"; sleep 4' _ "$lockstep" \
  >"$tmp/job.log" 2>&1 &
job_pid=$!
wait "$prune_pid" || true
job_status=0
wait "$job_pid" || job_status=$?
check "evict: a job that starts mid-delete locks and builds normally" \
  '((job_status == 0)) && grep -q "holding shared in-use lock" "$tmp/job.log"'
check "evict: ... into a fresh target/ that survives the delete" \
  '[[ -e "$broot/8/victim/target/marker" ]]'
check "evict: the slot's .in-use is the same file throughout" \
  '[[ "$(inode "$broot/8/victim/.in-use")" == "$victim_lock" ]]'
check "evict: the trash is emptied" '! compgen -G "$broot/8/victim/.prune-trash.*" >/dev/null'

# Leftover trash of a killed prune: deleted at the next scan, but only an
# exact `.prune-trash.<pid>.<epoch>` inside a numeric-layout slot.
budget_volume
mkdir -p "$broot/9/x/.prune-trash.123.1700000000/target"
echo 5 >"$broot/9/x/.prune-trash.123.1700000000/target/x"
mkdir -p "$broot/9/x/.prune-trash.notapid" "$broot/9/x/.prune-trash.1.2.extra" \
  "$broot/cargo-home/registry/.prune-trash.1.2" "$broot/bench/weights/.prune-trash.1.2"
touch "$broot/9/x/.in-use"
brun
check "evict: an exact leftover trash name in a numeric slot is deleted" \
  '[[ ! -e "$broot/9/x/.prune-trash.123.1700000000" ]]'
check "evict: near-miss trash names are left alone" \
  '[[ -d "$broot/9/x/.prune-trash.notapid" && -d "$broot/9/x/.prune-trash.1.2.extra" ]]'
check "evict: trash-shaped names outside the numeric layout are left alone" \
  '[[ -d "$broot/cargo-home/registry/.prune-trash.1.2" && -d "$broot/bench/weights/.prune-trash.1.2" ]]'
check "evict: the slot and its .in-use survive the leftover cleanup" '[[ -e "$broot/9/x/.in-use" ]]'

# Regression (item 7): a waiter that locks EXACTLY the way main's c63fd2f
# template does -- no inode re-check -- starts while the prune holds the
# slot exclusively. Afterwards it must hold the slot's LIVE .in-use, so a
# later prune sees the slot as held. (Whole-slot rename, eb0f409, left it
# holding the trashed file.) A slow `mv` keeps the prune inside its exclusive
# section long enough for the waiter to queue on the lock.
slowmv="$tmp/slowmv"
mkdir -p "$slowmv"
cat >"$slowmv/mv" <<EOF
#!/usr/bin/env bash
touch "$tmp/mv.started" # the prune holds the slot's lock exclusively now
sleep 2
exec "$real_mv" "\$@"
EOF
chmod +x "$slowmv/mv"
cat >"$tmp/mainwaiter.sh" <<'EOF'
# origin/main .gitlab-ci.yml:202-209 (c63fd2f), verbatim:
slot_dir="${CARGO_TARGET_DIR%/target}"
mkdir -p "$slot_dir"
exec 9>"$slot_dir/.in-use"
if ! flock --shared --wait 1200 9; then
  echo "ERROR: a cache prune held $slot_dir for 20 minutes" >&2
  exit 1
fi
# Test instrumentation: report the locked file, then hold the lock.
stat -L -c %d:%i /dev/fd/9 >"$WAITER_INODE"
sleep 8
EOF
# main_waiter_race SLOT PRUNE-ARGS...: run one prune with the slow mv, start
# main's waiter on SLOT once the prune holds it, and wait for the prune.
main_waiter_race() {
  local slot="$1"
  shift
  rm -f "$tmp/mv.started" "$tmp/waiter.inode"
  bpath="$slowmv:"
  brun "$@" &
  local prune_pid=$!
  bpath=""
  for _ in $(seq 200); do
    [[ -e "$tmp/mv.started" ]] && break
    sleep 0.05
  done
  CARGO_TARGET_DIR="$broot/$slot/target" WAITER_INODE="$tmp/waiter.inode" \
    bash "$tmp/mainwaiter.sh" >/dev/null 2>&1 &
  waiter_pid=$!
  wait "$prune_pid" || true
  for _ in $(seq 200); do
    [[ -s "$tmp/waiter.inode" ]] && break
    sleep 0.05
  done
}

budget_volume
bslot 8 victim $((30 * day)) 10000
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune" # rule 2 due
main_waiter_race 8/victim
check "main waiter (rule 2): the prune evicted the stale target/" 'evicted 8/victim'
check "main waiter (rule 2): the waiter holds the slot's live .in-use" \
  '[[ "$(cat "$tmp/waiter.inode")" == "$(inode "$broot/8/victim/.in-use")" ]]'
bslot 8 victim $((30 * day)) 10000 # its build, stamped long ago
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune"
brun
check "main waiter (rule 2): a later prune sees the slot as held" \
  'kept 8/victim && grep -q "stale slot $broot/8/victim is in use (lock held), skipping" "$tmp/budget.log"'
wait "$waiter_pid" || true

budget_volume
main_waiter_race 2/old --volume-budget 50
check "main waiter (rule 3): the prune evicted the LRU target/" 'evicted 2/old'
check "main waiter (rule 3): the waiter holds the slot's live .in-use" \
  '[[ "$(cat "$tmp/waiter.inode")" == "$(inode "$broot/2/old/.in-use")" ]]'
bslot 2 old $((10 * day)) 200000
brun --volume-budget 50
check "main waiter (rule 3): a later prune sees the slot as held" \
  'kept 2/old && blogged "$broot/2/old is in use (lock held), skipping"'
wait "$waiter_pid" || true

# --- Physical containment (item 9) -------------------------------------------
# `mv`, `rm -rf` and cargo-sweep follow symlinks in intermediate path
# components, so names alone prove nothing. Slot-shaped trees OUTSIDE the
# physical <root>/<n>/<slot> -- with a stale target/ and an exact trash name
# -- are reached through a symlinked numeric dir, a symlinked slot, and a slot
# symlinked into CARGO_HOME. Nothing there may be renamed, deleted or created;
# the skip is logged; normal slots still prune.
outside="$tmp/outside"
outside_shapes() { # DIR: a slot-shaped tree with a stale target/ and leftover trash
  mkdir -p "$1/target" "$1/.prune-trash.123.1700000000"
  echo 1 >"$1/target/keep"
  echo 1 >"$1/.prune-trash.123.1700000000/keep"
  touch -d "@$(($(date +%s) - 30 * day))" "$1/target/.last-used"
}
untouched() { # DIR: the outside tree is exactly as planted, and gained no lock file
  [[ -e "$1/target/keep" && -e "$1/.prune-trash.123.1700000000/keep" && ! -e "$1/.in-use" ]]
}
contained_volume() {
  budget_volume
  rm -rf "$outside"
  mkdir -p "$outside"
  bslot 8 stale $((30 * day)) 10000 # a normal stale slot, which must still be evicted
  touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune" # rule 2 due
}

contained_volume
outside_shapes "$outside/job"
ln -s "$outside" "$broot/9"
brun --volume-budget 10
check "containment: a symlinked numeric dir is never entered" 'untouched "$outside/job"'
check "containment: ... and the skip is logged" \
  'grep -q "$broot/9/job is not physically inside" "$tmp/budget.log"'
check "containment: other slots still prune" 'evicted 8/stale'

contained_volume
outside_shapes "$outside/slot"
mkdir -p "$broot/9"
ln -s "$outside/slot" "$broot/9/job"
brun --volume-budget 10
check "containment: a symlinked slot is never entered" 'untouched "$outside/slot"'
check "containment: ... and the skip is logged" \
  'grep -q "$broot/9/job is not physically inside" "$tmp/budget.log"'
check "containment: other slots still prune (symlinked slot)" 'evicted 8/stale'

contained_volume
mkdir -p "$broot/cargo-home/.prune-trash.1.2" "$broot/9"
echo 1 >"$broot/cargo-home/.prune-trash.1.2/keep"
ln -s "$broot/cargo-home" "$broot/9/job"
brun --volume-budget 10
check "containment: a slot symlinked into CARGO_HOME deletes nothing there" \
  '[[ -e "$broot/cargo-home/.prune-trash.1.2/keep" && -d "$broot/cargo-home/registry" ]]'
check "containment: ... and creates no lock file there" '[[ ! -e "$broot/cargo-home/.in-use" ]]'
check "containment: other slots still prune (CARGO_HOME link)" 'evicted 8/stale'

# A swap between the scan and the lock: a flock wrapper replaces a real stale
# slot with a symlink to an outside tree just before the prune locks it. The
# pre-lock check saw a real slot; only the re-check under the lock can catch it.
swapbin="$tmp/swapbin"
mkdir -p "$swapbin"
cat >"$swapbin/flock" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *" --exclusive "* && -e "$tmp/swap.spec" ]]; then
  read -r slot dest <"$tmp/swap.spec"
  if [[ -d "\$slot" && ! -L "\$slot" ]]; then
    "$real_mv" "\$slot" "$tmp/swapped.\$\$"
    ln -s "\$dest" "\$slot"
  fi
fi
exec "$real_flock" "\$@"
EOF
chmod +x "$swapbin/flock"
contained_volume
# Without the held slot and 8/stale (whose locks the scan would take first),
# the swap lands on 8/swap's own lock: after its pre-lock check, before the
# re-check under the lock.
release "${holders[-1]}"
rm -rf "$broot/4/held" "$broot/8/stale"
bslot 8 swap $((30 * day)) 10000
outside_shapes "$outside/swap"
echo "$broot/8/swap $outside/swap" >"$tmp/swap.spec"
bpath="$swapbin:"
brun
bpath=""
rm -f "$tmp/swap.spec"
check "containment: a slot swapped for a symlink between scan and lock is not evicted" \
  'untouched_target() { [[ -e "$outside/swap/target/keep" ]]; }; untouched_target'
check "containment: ... the re-check under the lock logs it" \
  'grep -q "$broot/8/swap is not physically inside" "$tmp/budget.log"'

# --- The template's lock step (scripts/ci-cache-lock.sh) ----------------------
lroot="$tmp/lock/ci-cache"
lockjob() { # lockjob SLOT [ENV...] -- run the lock step and a short job body
  local slot="$1"
  shift
  env "$@" CARGO_TARGET_DIR="$lroot/$slot/target" \
    bash -c '. "$1"; echo built >"$CARGO_TARGET_DIR/marker"; sleep 2' _ "$lockstep"
}
check "lock step: the template sources it" \
  'grep -qxF "    - . ./scripts/ci-cache-lock.sh" "$here/../.gitlab-ci.yml"'
mkdir -p "$lroot"
lockjob 0/fresh >"$tmp/lock.log" 2>&1 || true
check "lock step: a fresh slot is locked, stamped and built into" \
  'grep -q "holding shared in-use lock on fd 9" "$tmp/lock.log" \
    && [[ -e "$lroot/0/fresh/target/.last-used" && -e "$lroot/0/fresh/target/marker" ]]'

# A prune renames the slot away while the job waits for it: the job retries
# on a fresh slot rather than build into the trashed one.
mkdir -p "$lroot/0/moved/target"
hold "$lroot/0/moved" exclusive
lockjob 0/moved CI_CACHE_LOCK_WAIT=20 >"$tmp/lock.log" 2>&1 &
job_pid=$!
sleep 0.5
mv "$lroot/0/moved" "$lroot/trashed"
release "${holders[-1]}"
wait "$job_pid" || true
check "lock step: a slot renamed while waiting is retried" \
  'grep -q "was replaced while locking (attempt 1), retrying" "$tmp/lock.log" \
    && grep -q "holding shared in-use lock on fd 9" "$tmp/lock.log"'
check "lock step: the job builds into the fresh slot, not the trashed one" \
  '[[ -e "$lroot/0/moved/target/marker" && ! -e "$lroot/trashed/target/marker" ]]'

# Three failed inode checks: fail closed, never run unlocked.
badstat="$tmp/badstat"
mkdir -p "$badstat"
cat >"$badstat/stat" <<'EOF'
#!/usr/bin/env bash
echo "$RANDOM:$RANDOM"
EOF
chmod +x "$badstat/stat"
status=0
lockjob 0/unlockable PATH="$badstat:$PATH" >"$tmp/lock.log" 2>&1 || status=$?
check "lock step: three failed inode checks fail the job" \
  '((status != 0)) && grep -q "could not lock cache slot .* in 3 attempts" "$tmp/lock.log"'
check "lock step: ... and never claims the lock or builds" \
  '! grep -q "holding shared" "$tmp/lock.log" && [[ ! -e "$lroot/0/unlockable/target/marker" ]]'

# A prune that holds the slot past the wait fails the job, as main's does.
mkdir -p "$lroot/0/busy/target"
hold "$lroot/0/busy" exclusive
status=0
lockjob 0/busy CI_CACHE_LOCK_WAIT=1 >"$tmp/lock.log" 2>&1 || status=$?
check "lock step: a slot held past the wait fails the job" \
  '((status != 0)) && grep -q "a cache prune held .* for 1s" "$tmp/lock.log"'

if ((fails)); then
  echo "--- prune log ---"
  cat "$tmp/prune.log"
  echo "test-prune-ci-cache: $fails check(s) failed" >&2
  exit 1
fi
echo "test-prune-ci-cache: all checks passed"

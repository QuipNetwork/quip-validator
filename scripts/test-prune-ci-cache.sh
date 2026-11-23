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

# Hermetic: no CI_CACHE_* setting is inherited. A CI job exports its own
# (cargo-test's CI_CACHE_MAX_SIZE is 140GiB), and every knob a check depends
# on is set explicitly where it is used, never read from the caller.
unset "${!CI_CACHE_@}"

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

# Physical: the prune compares slots against `pwd -P`, and a temp dir behind a
# symlink (macOS /var -> /private/var) would read as outside the root.
tmp="$(cd -P "$(mktemp -d)" && pwd -P)"
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
# The target cargo would sweep: a relative CARGO_TARGET_DIR is resolved
# against cargo's own working directory, as cargo metadata does.
case "\$CARGO_TARGET_DIR" in
  /*) echo "\$CARGO_TARGET_DIR" >>"$sweeps" ;;
  *) echo "\$(pwd -P)/\$CARGO_TARGET_DIR" >>"$sweeps" ;;
esac
# How it was invoked: cwd, the raw variable, and the arguments.
echo "cwd=\$(pwd -P) CARGO_TARGET_DIR=\$CARGO_TARGET_DIR args=\$*" >>"$tmp/cargo.calls"
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

# Every CI_CACHE_* setting the checks depend on is set here, not inherited: the
# CI job running this test exports its own (cargo-test's CI_CACHE_MAX_SIZE is
# 140GiB), which a check on the sweep arguments would otherwise see.
rpath="" # extra PATH entries (hooks) for run()
run() {
  PATH="$rpath$bin:$PATH" CI_CACHE_ROOT="$root" CI_CACHE_PROJECT_DIR="$project" \
    CI_CACHE_MAX_SIZE=50GiB \
    CI_CACHE_DU_BUDGET="${du_budget:-180}" CI_CACHE_DU_TIMEOUT="${du_timeout:-60}" \
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
# lcheck: a check that needs Linux semantics a macOS shim does not give: the
# device/inode `stat -L` reports for /dev/fd/N (the lock step's inode check,
# main's waiter) and flock persistence across short-lived helpers. Tagged so
# local and CI counts explain themselves; CI (Linux) runs them for real.
linux_checks=0
lcheck() {
  linux_checks=$((linux_checks + 1))
  check "[linux] $1" "$2"
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
check "(d) ... by cargo-sweep run from the verified slot, with a relative target" \
  'grep -qxF "cwd=$root/1/own CARGO_TARGET_DIR=target args=sweep --maxsize 50GiB $project" "$tmp/cargo.calls"'
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

# Item 17: the own slot renamed away within the volume, with a fresh directory
# put at its path, while the sweep takes its exclusive lock. The re-check under
# the lock reads the cwd with getcwd (/bin/pwd -P) and refuses to sweep; the
# builtin `pwd -P` would re-resolve the path to the fresh directory and pass.
release "${holders[-1]}"
touch -d "$(days_ago 2)" "$root/1/own/.last-sweep"
ownmovebin="$tmp/ownmovebin"
mkdir -p "$ownmovebin"
cat >"$ownmovebin/flock" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *" --exclusive "* && -e "$tmp/ownmove.spec" ]]; then
  rm -f "$tmp/ownmove.spec"
  mv "$root/1/own" "$root/1/.moved-own"
  mkdir -p "$root/1/own/target"
fi
exec "$(command -v flock)" "\$@"
EOF
chmod +x "$ownmovebin/flock"
touch "$tmp/ownmove.spec"
sweeps_before="$(wc -l <"$sweeps")"
rpath="$ownmovebin:"
run
rpath=""
check "an own slot moved while the sweep locks it is not swept" \
  '[[ ! -e "$tmp/ownmove.spec" && "$(wc -l <"$sweeps")" -eq "$sweeps_before" ]] \
    && logged "own slot $root/1/own moved while locking; not sweeping it"'
rm -rf "$root/1/own"
mv "$root/1/.moved-own" "$root/1/own"

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

# Item 18: one size budget for every measurement in a run. A du that fails, or
# hangs past CI_CACHE_DU_TIMEOUT, costs only the size report: the prune exits
# 0, logs "unknown", skips the own slot's second measurement, and stops
# measuring once CI_CACHE_DU_BUDGET is spent.
dubin="$tmp/dubin"
mkdir -p "$dubin"
cat >"$dubin/du" <<EOF
#!/usr/bin/env bash
# exec, so timeout(1) kills the sleeper itself, as it would a real du.
[[ -e "$tmp/du.hang" ]] && exec sleep 30
exit 1
EOF
chmod +x "$dubin/du"
# The recording cargo again (the section above replaced it with one that
# lacks the sweep subcommand), so rule 1 sweeps and measures its slot.
cat >"$bin/cargo" <<EOF
#!/usr/bin/env bash
[[ " \$* " == *" --version "* ]] && exit 0
echo "\$(pwd -P)/\$CARGO_TARGET_DIR" >>"$sweeps"
EOF
touch -d "$(days_ago 2)" "$root/1/own/.last-sweep" "$root/.last-prune"
: >"$tmp/prune.log"
status=0
rpath="$dubin:"
run || status=$?
rpath=""
check "du budget: a failing du does not fail the prune" '((status == 0))'
check "du budget: ... the own slot's size is unknown, and not measured twice" \
  'logged "own slot size before sweep: unknown (du failed" \
    && logged "own slot size after sweep: not measured (the size before was unknown)"'
check "du budget: ... the scan reports unknown sizes and still finishes" \
  'logged "size: $root/2/held unknown (du failed" \
    && [[ "$(($(date +%s) - $(stat -c %Y "$root/.last-prune")))" -lt 60 ]]'

touch -d "$(days_ago 2)" "$root/1/own/.last-sweep" "$root/.last-prune"
touch "$tmp/du.hang"
: >"$tmp/prune.log"
status=0
started=$SECONDS
rpath="$dubin:"
du_budget=3 du_timeout=1 run || status=$?
rpath=""
elapsed=$((SECONDS - started))
rm -f "$tmp/du.hang"
check "du budget: a hanging du is cut off and the whole run stays inside the budget" \
  '((status == 0 && elapsed < 15)) && logged "own slot size before sweep: unknown (du failed or took over 1s)" \
    && logged "size budget of 3s spent after"'

# du_size's zero-limit guard. With CI_CACHE_DU_BUDGET equal to
# CI_CACHE_DU_TIMEOUT, the own slot's first (on-disk) du is given the whole
# budget and hangs through it, so the second (apparent-size) du is left 0s.
# The guard must answer "size budget spent" without running du: `timeout 0`
# means NO limit, so a du started with it would hang the run.
# The first du's limit is 1s or 2s: the budget's deadline is taken from
# bash's SECONDS, which can tick between setting it and that du. Either way
# nothing is left for the second du, so the check accepts both.
touch -d "$(days_ago 2)" "$root/1/own/.last-sweep" "$root/.last-prune"
touch "$tmp/du.hang"
: >"$tmp/prune.log"
status=0
started=$SECONDS
rpath="$dubin:"
du_budget=2 du_timeout=2 run || status=$?
rpath=""
elapsed=$((SECONDS - started))
rm -f "$tmp/du.hang"
check "du budget: with the budget spent, the next du is not started (timeout 0 is no limit)" \
  '((status == 0 && elapsed < 15)) \
    && grep -Eq "prune-ci-cache: own slot size before sweep: unknown \(du failed or took over [12]s\) on disk, unknown \(size budget spent\) apparent" "$tmp/prune.log"'
# Back to the cargo without a sweep subcommand that the sections below were
# written against (with it, rule 1 takes no exclusive lock first).
cat >"$bin/cargo" <<'EOF'
#!/usr/bin/env bash
[[ "$1" == "sweep" ]] && { echo "error: no such command: \`sweep\`" >&2; exit 101; }
exit 0
EOF

# --- Shared fixtures for the budget, race and lock-step scenarios -------------
# Each scenario gets a fresh volume. A fake `df` reports a 2,000,000 KiB
# volume whose usage is the sum of `.fake-kb` files under it (or a frozen
# figure, to model a df that lags the delete); a fake `du` reports the same
# pretend sizes; a fake mount table names the same device. Sizes are KiB of
# pretend data, not real files.
broot="$tmp/budget/ci-cache"
real_mv_hook="$(command -v mv)"
real_rm_hook="$(command -v rm)"
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
[[ -e "@TMP@/du.fail" ]] && exit 1
# A slot marked .du-fail cannot be measured (path relative or absolute).
[[ "$path" == */target && -e "${path%/target}/.du-fail" ]] && exit 1
# Trace: the physical directory every du call resolves to.
if [[ -e "@TMP@/trace.on" ]]; then
  echo "du $(cd -P "$(dirname "$path")" 2>/dev/null && pwd -P)/$(basename "$path")" >>"@TMP@/trace.log"
fi
# Lock probe: is the slot whose target/ is measured locked by the prune now?
if [[ -e "@TMP@/du.probe" && "$path" == */target ]]; then
  exec {probe_fd}>>"${path%/target}/.in-use"
  if flock --nonblock --shared "$probe_fd"; then state=free; else state=held; fi
  exec {probe_fd}>&-
  # Log the physical path measured (the prune passes a relative one).
  echo "$state $(cd -P "$(dirname "$path")" && pwd -P)/$(basename "$path")" >>"@TMP@/du.probe.log"
fi
kb=0
while IFS= read -r f; do kb=$((kb + $(cat "$f"))); done < <(find "$path" -name .fake-kb 2>/dev/null)
if [[ " $* " == *" -sk "* ]]; then printf '%s\t%s\n' "$kb" "$path"; else printf '%sK\t%s\n' "$kb" "$path"; fi
EOF
sed -i.bak "s|@TMP@|$tmp|g" "$fakebin/du" && rm -f "$fakebin/du.bak"
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
  rm -rf "$tmp/budget" "$tmp/df.fail" "$tmp/df.garbage" "$tmp/df.frozen" "$tmp/refresh.list" \
    "$tmp/du.fail" "$tmp/du.probe" "$tmp/du.probe.log" "$tmp/mv.args" "$tmp/trace.on" \
    "$tmp/trace.log" "$tmp/postswap.spec"
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
broot_arg="" # CI_CACHE_ROOT as given, when it should differ from $broot
# brun [prune args...]: one prune of the fake volume; extra environment in
# the array `benv`, extra PATH entries (wrappers) in `bpath`.
brun() {
  env "${benv[@]}" PATH="$bpath$fakebin:$bin:$PATH" CI_CACHE_ROOT="${broot_arg:-$broot}" \
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
  'survives 2/old/target && blogged "$broot/2/old has cargo fingerprints newer than 7200s or could not be probed; keeping it"'
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
# A fresh .last-sweep keeps rule 1 from sweeping, so its exclusive lock on the
# own slot does not fire the wrapper before rule 3 has looked at anything.
touch "$broot/1/own/.last-sweep"
echo "$broot/2/old/target/.last-used" >"$tmp/refresh.list"
bpath="$refreshbin:"
brun --volume-budget 50
bpath=""
check "rule 3: a slot stamped between the scan and the lock survives" \
  'survives 2/old/target && evicted 3/mid'
check "rule 3: ... caught by the re-check under the lock, not the pre-check" \
  '! blogged "$broot/2/old was used within"'

# Item 13 (a): the same for the activity probe. A fingerprint that is old at
# the pre-check is refreshed just before the lock; only the re-check sees it.
budget_volume
release "${holders[-1]}"
rm -rf "$broot/4/held"
touch "$broot/1/own/.last-sweep"
mkdir -p "$broot/2/old/target/debug/.fingerprint/dep-0123"
touch -d "@$(($(date +%s) - 3 * day))" "$broot/2/old/target/debug/.fingerprint/dep-0123/lib-dep" \
  "$broot/2/old/target/debug/.fingerprint/dep-0123"
echo "$broot/2/old/target/debug/.fingerprint/dep-0123/lib-dep" >"$tmp/refresh.list"
bpath="$refreshbin:"
brun --volume-budget 50
bpath=""
check "rule 3: a slot built between the scan and the lock survives (activity re-check)" \
  'survives 2/old/target && evicted 3/mid && ! blogged "$broot/2/old has cargo fingerprints"'

# Item 13 (F3): rule 2 runs the activity probe under the lock. A job on a
# template without the lock step builds in a slot idle for 30 days: it takes
# no lock and stamps nothing, so only cargo's fingerprints show it.
budget_volume
release "${holders[-1]}"
rm -rf "$broot/4/held"
touch "$broot/1/own/.last-sweep"
bslot 8 stale $((30 * day)) 10000
mkdir -p "$broot/8/stale/target/debug/.fingerprint/dep-89ab"
touch -d "@$(($(date +%s) - 3 * day))" "$broot/8/stale/target/debug/.fingerprint/dep-89ab/lib-dep" \
  "$broot/8/stale/target/debug/.fingerprint/dep-89ab"
echo "$broot/8/stale/target/debug/.fingerprint/dep-89ab/lib-dep" >"$tmp/refresh.list"
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune" # rule 2 due
bpath="$refreshbin:"
brun
bpath=""
check "rule 2: a stale slot with fresh cargo fingerprints survives (activity probe)" \
  'survives 8/stale/target && grep -q "stale slot $broot/8/stale has cargo fingerprints newer than 7200s or could not be probed; keeping it" "$tmp/budget.log"'

# Item 13 (b): rule 3 measures the candidate before locking it, and a
# measurement that fails evicts nothing. (Measuring first means a held slot is
# measured too, before its lock is tried; the held slot is removed here so the
# failure lands on 2/old.)
budget_volume
release "${holders[-1]}"
rm -rf "$broot/4/held"
touch "$tmp/du.fail"
brun --volume-budget 50
check "budget: a failing du evicts nothing" \
  'kept 2/old && kept 3/mid && blogged "cannot measure $broot/2/old/target; skipping it"'
# Item 14: an unmeasurable candidate is skipped, not a stop: the oldest
# eligible slot cannot be measured, so the next one is evicted instead.
budget_volume
touch "$broot/2/old/.du-fail"
brun --volume-budget 55
check "budget: an unmeasurable candidate is skipped and the next one evicted" \
  'kept 2/old && evicted 3/mid && blogged "cannot measure $broot/2/old/target; skipping it"'
# Item 18: rule 3 measures inside the same budget; once it is spent nothing
# more is measured or evicted, and the run says so.
budget_volume
benv=(CI_CACHE_DU_BUDGET=0)
brun --volume-budget 55
benv=()
check "budget: with the size budget spent, rule 3 measures and evicts nothing" \
  'kept 2/old && kept 3/mid && blogged "the size budget of 0s is spent; evicting nothing more this run"'
check "budget: ... and the closing line blames the spent budget, not a lack of slots" \
  'blogged "still over budget; the size budget was spent before a slot could be measured" \
    && ! blogged "no removable slot is left"'
budget_volume
touch "$tmp/du.probe"
brun --volume-budget 55
check "budget: the candidate is measured before its lock is taken" \
  'evicted 2/old && grep -qxF "free $broot/2/old/target" "$tmp/du.probe.log" \
    && ! grep -q "^held " "$tmp/du.probe.log"'

# Item 13 (c): the eviction is a rename to $slot/.prune-trash.<pid>.<epoch>,
# inside the slot, exactly.
mvlogbin="$tmp/mvlogbin"
mkdir -p "$mvlogbin"
cat >"$mvlogbin/mv" <<EOF
#!/usr/bin/env bash
echo "\$(pwd -P) \$*" >>"$tmp/mv.args"
exec "$real_mv_hook" "\$@"
EOF
chmod +x "$mvlogbin/mv"
trash_named_in_slot() { # SLOT: the one mv, run in SLOT, renamed target to .prune-trash.N.N
  local cwd tflag dashdash src dest extra
  [[ "$(wc -l <"$tmp/mv.args")" -eq 1 ]] || return 1
  read -r cwd tflag dashdash src dest extra <"$tmp/mv.args"
  [[ -z "$extra" && "$cwd" == "$broot/$1" && "$tflag" == -T && "$dashdash" == -- && "$src" == target &&
    "$dest" =~ ^\.prune-trash\.[0-9]+\.[0-9]+$ ]]
}
budget_volume
bpath="$mvlogbin:"
brun --volume-budget 55
bpath=""
check "budget: the evicted target/ is renamed inside its slot, to the exact trash name" \
  'evicted 2/old && trash_named_in_slot 2/old'

# Item 13: a trailing slash on CI_CACHE_ROOT is normalised, so slots match and
# the evicted target/ is recognised as trash and deleted.
budget_volume
broot_arg="$broot/"
brun --volume-budget 55
broot_arg=""
check "budget: CI_CACHE_ROOT with a trailing slash prunes normally and leaks no trash" \
  'evicted 2/old && ! compgen -G "$broot/*/*/.prune-trash.*" >/dev/null \
    && ! grep -q "not physically inside" "$tmp/budget.log"'

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
lcheck "evict: a job that starts mid-delete locks and builds normally" \
  '((job_status == 0)) && grep -q "holding shared in-use lock" "$tmp/job.log"'
lcheck "evict: ... into a fresh target/ that survives the delete" \
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
# Test instrumentation: report the locked file, then hold the lock until the
# test has made its checks and signals the release (a fixed sleep raced a slow
# host). Bounded at 120 s so a broken test cannot hang the suite.
stat -L -c %d:%i /dev/fd/9 >"$WAITER_INODE"
for _ in $(seq 1200); do
  [[ -e "$WAITER_RELEASE" ]] && break
  sleep 0.1
done
EOF
# main_waiter_race SLOT PRUNE-ARGS...: run one prune with the slow mv, start
# main's waiter on SLOT once the prune holds it, and wait for the prune.
main_waiter_race() {
  local slot="$1"
  shift
  rm -f "$tmp/mv.started" "$tmp/waiter.inode" "$tmp/waiter.release"
  bpath="$slowmv:"
  brun "$@" &
  local prune_pid=$!
  bpath=""
  for _ in $(seq 200); do
    [[ -e "$tmp/mv.started" ]] && break
    sleep 0.05
  done
  CARGO_TARGET_DIR="$broot/$slot/target" WAITER_INODE="$tmp/waiter.inode" \
    WAITER_RELEASE="$tmp/waiter.release" bash "$tmp/mainwaiter.sh" >/dev/null 2>&1 &
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
lcheck "main waiter (rule 2): the waiter holds the slot's live .in-use" \
  '[[ "$(cat "$tmp/waiter.inode")" == "$(inode "$broot/8/victim/.in-use")" ]]'
bslot 8 victim $((30 * day)) 10000 # its build, stamped long ago
touch -d "@$(($(date +%s) - 2 * day))" "$broot/.last-prune"
brun
check "main waiter (rule 2): a later prune sees the slot as held" \
  'kept 8/victim && grep -q "stale slot $broot/8/victim is in use (lock held), skipping" "$tmp/budget.log"'
touch "$tmp/waiter.release"
wait "$waiter_pid" || true

budget_volume
main_waiter_race 2/old --volume-budget 50
check "main waiter (rule 3): the prune evicted the LRU target/" 'evicted 2/old'
lcheck "main waiter (rule 3): the waiter holds the slot's live .in-use" \
  '[[ "$(cat "$tmp/waiter.inode")" == "$(inode "$broot/2/old/.in-use")" ]]'
bslot 2 old $((10 * day)) 200000
brun --volume-budget 50
check "main waiter (rule 3): a later prune sees the slot as held" \
  'kept 2/old && blogged "$broot/2/old is in use (lock held), skipping"'
touch "$tmp/waiter.release"
wait "$waiter_pid" || true

# Item 12 (F1): a job that waits on its slot's lock THROUGH a rule-3 eviction
# wakes holding the live lock with target/ renamed away. The lock step must
# recreate target/ after the lock, so the job stamps, builds and exits 0
# under the template shell's `set -eo pipefail`.
budget_volume
rm -f "$tmp/mv.started"
bpath="$slowmv:"
brun --volume-budget 50 &
prune_pid=$!
bpath=""
for _ in $(seq 200); do
  [[ -e "$tmp/mv.started" ]] && break
  sleep 0.05
done
CARGO_TARGET_DIR="$broot/2/old/target" CI_CACHE_LOCK_WAIT=20 \
  bash -c 'set -eo pipefail; . "$1"; echo built >"$CARGO_TARGET_DIR/marker"' _ "$lockstep" \
  >"$tmp/job.log" 2>&1 &
job_pid=$!
wait "$prune_pid" || true
job_status=0
wait "$job_pid" || job_status=$?
check "lock step: the prune evicted the waiting job's target/" \
  'blogged "evicting the target/ of least recently used slot $broot/2/old "'
lcheck "lock step: a job that waited through the eviction exits 0 holding the lock" \
  '((job_status == 0)) && grep -q "holding shared in-use lock" "$tmp/job.log"'
lcheck "lock step: ... into a fresh, stamped target/" \
  '[[ -e "$broot/2/old/target/.last-used" && -e "$broot/2/old/target/marker" ]]'

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

# A slot whose target/ is a symlink out of the volume: skipped, not measured.
# (`mv` of the link would rename only the link, but `du` and the activity
# probe's `find` would read through it, so the decision would rest on data
# outside the volume.) Nothing outside is renamed or deleted, and the link
# itself stays.
contained_volume
outside_shapes "$outside/tgt"
mkdir -p "$broot/9/linked"
ln -s "$outside/tgt/target" "$broot/9/linked/target"
brun --volume-budget 10
check "containment: a symlinked target/ leaves the outside tree untouched" \
  'untouched "$outside/tgt" && [[ -e "$outside/tgt/target/.last-used" ]]'
check "containment: ... the slot is skipped, its link kept, and the skip logged" \
  '[[ -L "$broot/9/linked/target" ]] && grep -q "$broot/9/linked has a symlinked target/" "$tmp/budget.log"'
check "containment: other slots still prune (symlinked target/)" 'evicted 8/stale'

# Item 16: STATIC leaf refusals. A symlinked or non-regular ./.in-use or
# target/.last-used is refused by anchor_slot (with its own log line), so
# nothing is opened, created or touched through it, and the slot is skipped.
mtime() { stat -c %Y "$1"; }
contained_volume
mkdir -p "$outside"
ln -s "$outside/absent-lock" "$broot/8/stale/.in-use" # dangling: >> would create it
brun --volume-budget 10
check "leaves: a symlinked .in-use is refused and nothing is created through it" \
  '[[ ! -e "$outside/absent-lock" ]] && kept 8/stale \
    && grep -q "$broot/8/stale has a symlinked target/, or a symlinked or non-regular .in-use" "$tmp/budget.log"'

contained_volume
mkdir -p "$broot/8/stale/.in-use" # a directory, not a regular file
brun --volume-budget 10
check "leaves: a non-regular .in-use is refused" \
  'kept 8/stale && grep -q "$broot/8/stale has a symlinked target/, or a symlinked or non-regular" "$tmp/budget.log"'

contained_volume
mkdir -p "$outside"
echo keep >"$outside/stamp"
touch -d "@$(($(date +%s) - 30 * day))" "$outside/stamp"
stamp_before="$(mtime "$outside/stamp")"
rm -f "$broot/8/stale/target/.last-used"
ln -s "$outside/stamp" "$broot/8/stale/target/.last-used"
brun --volume-budget 10
check "leaves: a symlinked stamp is refused and the outside file is not touched" \
  '[[ "$(mtime "$outside/stamp")" == "$stamp_before" && "$(cat "$outside/stamp")" == keep ]] && kept 8/stale \
    && grep -q "$broot/8/stale has a symlinked target/, or a symlinked or non-regular" "$tmp/budget.log"'

contained_volume
rm -f "$broot/8/stale/target/.last-used"
mkdir -p "$broot/8/stale/target/.last-used" # a directory, not a regular file
brun --volume-budget 10
check "leaves: a non-regular stamp is refused" \
  'kept 8/stale && grep -q "$broot/8/stale has a symlinked target/, or a symlinked or non-regular" "$tmp/budget.log"'

# The activity probe fails closed: a `find` that exits non-zero (a shim that
# fails only the fingerprint probe) must keep the stale slot and log why, not
# read as "no recent fingerprint" and evict it.
findfailbin="$tmp/findfailbin"
mkdir -p "$findfailbin"
real_find="$(command -v find)"
cat >"$findfailbin/find" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *".fingerprint"* ]]; then
  echo "find: simulated failure" >&2
  exit 1
fi
exec "$real_find" "\$@"
EOF
chmod +x "$findfailbin/find"
contained_volume
bpath="$findfailbin:"
brun --volume-budget 10
bpath=""
check "activity probe: a failing find keeps the stale slot (fails closed)" \
  'kept 8/stale && grep -q "/8/stale: the activity probe.s find failed (exit 1); treating the slot as in use" "$tmp/budget.log"'

# .last-sweep: rule 1 reads its mtime and touches it after a sweep. A dangling
# symlink there would read as "never swept", and the touch would create the
# outside file; a link to an old outside file would be read and then touched.
# anchor_slot refuses both, so the own slot is skipped and nothing outside is
# created or touched.
contained_volume
mkdir -p "$outside"
rm -f "$broot/1/own/.last-sweep"
ln -s "$outside/absent-sweep" "$broot/1/own/.last-sweep"
brun --volume-budget 10
check "leaves: a dangling .last-sweep is refused and nothing is created through it" \
  '[[ ! -e "$outside/absent-sweep" && -L "$broot/1/own/.last-sweep" ]] \
    && grep -q "$broot/1/own has a symlinked target/, or a symlinked or non-regular .in-use, .last-sweep" "$tmp/budget.log"'

contained_volume
mkdir -p "$outside"
echo keep >"$outside/sweep"
touch -d "@$(($(date +%s) - 30 * day))" "$outside/sweep"
sweep_before="$(mtime "$outside/sweep")"
rm -f "$broot/1/own/.last-sweep"
ln -s "$outside/sweep" "$broot/1/own/.last-sweep"
brun --volume-budget 10
check "leaves: an old-mtime .last-sweep link is refused and the outside file is not touched" \
  '[[ "$(mtime "$outside/sweep")" == "$sweep_before" && "$(cat "$outside/sweep")" == keep ]] \
    && grep -q "$broot/1/own has a symlinked target/, or a symlinked or non-regular" "$tmp/budget.log"'

# The root control files: a symlinked .prune.lock or .last-prune refuses the
# whole scan (rules 2 and 3); the outside file is neither truncated, created
# nor touched. An existing regular .prune.lock is opened read-only: never
# truncated.
contained_volume
mkdir -p "$outside"
echo keep >"$outside/scanlock"
rm -f "$broot/.prune.lock"
ln -s "$outside/scanlock" "$broot/.prune.lock"
brun --volume-budget 10
check "leaves: a symlinked .prune.lock refuses the scan and is not truncated" \
  '[[ "$(cat "$outside/scanlock")" == keep ]] && kept 8/stale \
    && grep -q "is a symlink or not a regular file; skipping the orphan scan and the volume budget" "$tmp/budget.log"'

contained_volume
mkdir -p "$outside"
echo keep >"$outside/lastprune"
touch -d "@$(($(date +%s) - 30 * day))" "$outside/lastprune"
lp_before="$(mtime "$outside/lastprune")"
rm -f "$broot/.last-prune"
ln -s "$outside/lastprune" "$broot/.last-prune"
brun --volume-budget 10
check "leaves: a symlinked .last-prune refuses the scan and is not touched" \
  '[[ "$(mtime "$outside/lastprune")" == "$lp_before" ]] && kept 8/stale \
    && grep -q "is a symlink or not a regular file; skipping the orphan scan and the volume budget" "$tmp/budget.log"'

contained_volume
echo keep >"$broot/.prune.lock"
brun
check "leaves: an existing .prune.lock is opened without truncation" \
  '[[ "$(cat "$broot/.prune.lock")" == keep ]] && evicted 8/stale'

# .last-prune turned into a symlink DURING the scan (a flock wrapper swaps it
# when rule 2 locks a slot): the re-check right before the touch refuses it.
lpbin="$tmp/lpbin"
mkdir -p "$lpbin"
cat >"$lpbin/flock" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *" --exclusive "* && -e "$tmp/lp.spec" ]]; then
  rm -f "$broot/.last-prune" "$tmp/lp.spec"
  ln -s "$outside/lastprune2" "$broot/.last-prune"
fi
exec "$real_flock" "\$@"
EOF
chmod +x "$lpbin/flock"
contained_volume
mkdir -p "$outside"
echo keep >"$outside/lastprune2"
touch -d "@$(($(date +%s) - 30 * day))" "$outside/lastprune2"
lp2_before="$(mtime "$outside/lastprune2")"
touch "$broot/1/own/.last-sweep" # rule 1 takes no exclusive lock first
touch "$tmp/lp.spec"
bpath="$lpbin:"
brun
bpath=""
check "leaves: a .last-prune swapped for a symlink mid-scan is not touched" \
  '[[ ! -e "$tmp/lp.spec" && "$(mtime "$outside/lastprune2")" == "$lp2_before" ]] \
    && grep -q "became a symlink or not a regular file; not recording the scan" "$tmp/budget.log"'

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
  'grep -qF "$broot/8/swap is not physically inside $broot/<n>/<slot> (moved while locking)" "$tmp/budget.log"'

# Item 17a: the verified slot renamed away WITHIN the volume, with a fresh real
# directory put at its path, while the prune takes the slot's lock (claude's
# recheck_moved). Only getcwd (/bin/pwd -P) sees the move; the builtin
# `pwd -P` re-resolves the path string to the fresh directory and passes.
movebin="$tmp/movebin"
mkdir -p "$movebin"
cat >"$movebin/flock" <<EOF
#!/usr/bin/env bash
if [[ " \$* " == *" --exclusive "* && -e "$tmp/move.spec" ]]; then
  read -r slot dest <"$tmp/move.spec"
  rm -f "$tmp/move.spec"
  "$real_mv" "\$slot" "\$dest"
  mkdir -p "\$slot/target"
  echo 1 >"\$slot/target/fresh"
fi
exec "$real_flock" "\$@"
EOF
chmod +x "$movebin/flock"
contained_volume
release "${holders[-1]}"
rm -rf "$broot/4/held"
touch "$broot/1/own/.last-sweep"
echo "$broot/8/stale $broot/8/.moved-stale" >"$tmp/move.spec"
bpath="$movebin:"
brun
bpath=""
check "anchored: a slot moved within the volume while locking is refused" \
  '[[ ! -e "$tmp/move.spec" ]] \
    && grep -qF "$broot/8/stale is not physically inside $broot/<n>/<slot> (moved while locking)" "$tmp/budget.log"'
check "anchored: ... neither the moved slot nor the fresh one is evicted" \
  '[[ -d "$broot/8/.moved-stale/target" && -e "$broot/8/stale/target/fresh" ]]'

# Item 17c: a symlinked target/ with NO stamp outside. anchor_slot's `! -L
# target` is what stops rule 2 from grandfathering it -- a `touch` that would
# create target/.last-used outside the volume.
contained_volume
mkdir -p "$outside/nostamp" "$broot/9/nostamp"
ln -s "$outside/nostamp" "$broot/9/nostamp/target"
brun
check "anchored: a symlinked target/ with no stamp gets none created outside" \
  '[[ ! -e "$outside/nostamp/.last-used" ]] \
    && grep -q "$broot/9/nostamp has a symlinked target/" "$tmp/budget.log"'

# Item 17b: a symlink to an outside directory planted at the trash name, just
# before the eviction's rename (claude's trash_dest). `mv -T` refuses a
# directory onto it; a plain `mv` would move target/ INTO the outside dir.
plantbin="$tmp/plantbin"
mkdir -p "$plantbin"
cat >"$plantbin/mv" <<EOF
#!/usr/bin/env bash
dest="\${*: -1}"
if [[ -e "$tmp/plant.spec" && "\$(basename "\$dest")" == .prune-trash.* ]]; then
  rm -f "$tmp/plant.spec"
  ln -s "$outside/planted" "\$dest"
fi
exec "$real_mv" "\$@"
EOF
chmod +x "$plantbin/mv"
contained_volume
mkdir -p "$outside/planted"
echo 1 >"$broot/8/stale/target/artifact"
touch "$tmp/plant.spec"
bpath="$plantbin:"
brun
bpath=""
check "anchored: a planted trash-name link fails the eviction closed" \
  '[[ ! -e "$tmp/plant.spec" && -e "$broot/8/stale/target/artifact" ]] \
    && [[ -z "$(ls -A "$outside/planted")" ]] \
    && grep -q "cannot move $broot/8/stale/target aside; not evicting it" "$tmp/budget.log"'

# --- Item 14: anchored operations. A hook swaps the slot for a symlink to an
# outside tree AFTER the prune verified it and just before one operation runs
# (mv of target/, rm of the trash, rm of a leftover, du of target/). The
# operation must act on the verified directory (moved within the volume), and
# nothing outside may change. Path-based operations fail these.
postbin="$tmp/postbin"
mkdir -p "$postbin"
cat >"$postbin/hook" <<EOF
#!/usr/bin/env bash
me="\$(basename "\$0")"
if [[ -e "$tmp/postswap.spec" ]]; then
  read -r want slot dest <"$tmp/postswap.spec"
  hit=0
  name=""
  # The operand this op acts on, and the physical directory it resolves in:
  # the hook fires only for an op on the spec's slot, not an earlier one.
  arg=""
  if [[ "\$me" == "\$want" ]]; then
    case "\$me" in
      mv) for a in "\$@"; do [[ "\$a" == target || "\$a" == */target ]] && arg="\$a"; done ;;
      rm) for a in "\$@"; do [[ "\$a" == *.prune-trash.* ]] && arg="\$a"; done ;;
      du) [[ "\${*: -1}" == *target ]] && arg="\${*: -1}" ;;
    esac
  fi
  if [[ -n "\$arg" && "\$(cd -P "\$(dirname "\$arg")" 2>/dev/null && pwd -P)" == "\$slot" ]]; then
    hit=1
    [[ "\$me" == rm ]] && name="\${arg##*/}"
  fi
  if ((hit)); then
    if [[ -n "\$name" ]]; then mkdir -p "\$dest/\$name" && echo 1 >"\$dest/\$name/keep"; fi
    "$real_mv_hook" "\$slot" "$broot/moved.\$\$" # within the volume
    ln -s "\$dest" "\$slot"
    "$real_rm_hook" -f "$tmp/postswap.spec"
  fi
fi
case "\$me" in
  mv) exec "$real_mv_hook" "\$@" ;;
  rm) exec "$real_rm_hook" "\$@" ;;
  du) exec "$fakebin/du" "\$@" ;;
esac
EOF
chmod +x "$postbin/hook"
for t in mv rm du; do ln -sf hook "$postbin/$t"; done
untouched_tree() { # DIR NAME...: DIR still has each NAME/keep, and no lock file
  local d="$1"
  shift
  for n in "$@"; do [[ -e "$d/$n/keep" ]] || return 1; done
  [[ ! -e "$d/.in-use" ]]
}

contained_volume
outside_shapes "$outside/post"
echo "mv $broot/8/stale $outside/post" >"$tmp/postswap.spec"
bpath="$postbin:"
brun
bpath=""
check "anchored: a slot swapped just before the eviction's mv leaves the outside tree untouched" \
  '[[ ! -e "$tmp/postswap.spec" ]] && untouched_tree "$outside/post" target .prune-trash.123.1700000000'
check "anchored: ... the verified directory's target/ was evicted instead" \
  '! compgen -G "$broot/moved.*/target" >/dev/null && compgen -G "$broot/moved.*" >/dev/null'

contained_volume
mkdir -p "$outside/post2"
echo "rm $broot/8/stale $outside/post2" >"$tmp/postswap.spec"
bpath="$postbin:"
brun
bpath=""
check "anchored: a slot swapped just before the trash delete deletes nothing outside" \
  '[[ ! -e "$tmp/postswap.spec" ]] && compgen -G "$outside/post2/.prune-trash.*/keep" >/dev/null'
check "anchored: ... the verified directory's trash was deleted instead" \
  '! compgen -G "$broot/moved.*/.prune-trash.*" >/dev/null'

budget_volume
mkdir -p "$broot/9/x/.prune-trash.123.1700000000" "$outside/post3"
touch "$broot/9/x/.in-use"
echo "rm $broot/9/x $outside/post3" >"$tmp/postswap.spec"
bpath="$postbin:"
brun
bpath=""
check "anchored: a slot swapped just before a leftover delete deletes nothing outside" \
  '[[ ! -e "$tmp/postswap.spec" && -e "$outside/post3/.prune-trash.123.1700000000/keep" ]]'
check "anchored: ... the verified directory's leftover was deleted instead" \
  '! compgen -G "$broot/moved.*/.prune-trash.*" >/dev/null'

budget_volume
rm -rf "$outside"
mkdir -p "$outside/post4/target"
echo 900000 >"$outside/post4/target/.fake-kb"
touch "$tmp/trace.on"
echo "du $broot/2/old $outside/post4" >"$tmp/postswap.spec"
bpath="$postbin:"
brun --volume-budget 55
bpath=""
check "anchored: a slot swapped just before its du is measured from the verified directory" \
  '[[ ! -e "$tmp/postswap.spec" ]] && ! grep -qF "$outside" "$tmp/trace.log"'
check "anchored: ... and the outside tree is untouched" \
  '[[ -e "$outside/post4/target/.fake-kb" && ! -e "$outside/post4/.in-use" ]]'

# Reads through a symlinked numeric parent: rule 3's candidate stamp and the
# capacity report's du are taken from inside verified slots only, so nothing
# outside is stat'ed or measured, and the skip is logged.
statbin="$tmp/statbin"
mkdir -p "$statbin"
real_stat="$(command -v stat)"
cat >"$statbin/stat" <<EOF
#!/usr/bin/env bash
f="\${*: -1}"
echo "stat \$(cd -P "\$(dirname "\$f")" 2>/dev/null && pwd -P)/\$(basename "\$f")" >>"$tmp/trace.log"
exec "$real_stat" "\$@"
EOF
chmod +x "$statbin/stat"
contained_volume
outside_shapes "$outside/job"
echo 50000 >"$outside/job/target/.fake-kb"
ln -s "$outside" "$broot/9"
touch "$tmp/trace.on"
bpath="$statbin:"
brun --volume-budget 10
bpath=""
check "anchored: nothing outside is stat'ed or measured through a symlinked numeric dir" \
  '[[ -s "$tmp/trace.log" ]] && ! grep -qF "$outside" "$tmp/trace.log"'
check "anchored: ... the skip is logged and other slots still prune" \
  'grep -q "$broot/9/job is not physically inside" "$tmp/budget.log" && evicted 8/stale'

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
lcheck "lock step: a fresh slot is locked, stamped and built into" \
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
lcheck "lock step: a slot renamed while waiting is retried" \
  'grep -q "was replaced while locking (attempt 1), retrying" "$tmp/lock.log" \
    && grep -q "holding shared in-use lock on fd 9" "$tmp/lock.log"'
lcheck "lock step: the job builds into the fresh slot, not the trashed one" \
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
  echo "test-prune-ci-cache: $fails check(s) failed ($linux_checks tagged [linux]: on a non-Linux host, shim limits can fail those)" >&2
  exit 1
fi
echo "test-prune-ci-cache: all checks passed ($linux_checks tagged [linux]: they need Linux /dev/fd and flock semantics)"

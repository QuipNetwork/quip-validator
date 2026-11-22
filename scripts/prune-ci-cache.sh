#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/prune-ci-cache.sh [--dry-run] [--volume-budget N] [extra cargo-sweep args]
#
# Prunes the host cache volume that .cargo-host-cache mounts at /ci-cache. It
# runs from that template's after_script, so it rides along with the jobs that
# fill the volume on every host in the amd64 pool.
#
# Layout: a slot is /ci-cache/<CI_CONCURRENT_ID>/<CI_JOB_NAME>, holding the
# job's target/ and the lock file `.in-use`. CI_CONCURRENT_ID is numeric, and
# only directories under a numeric first level are ever treated as slots: the
# volume also holds the shared CARGO_HOME (/ci-cache/cargo-home), which is not
# a slot.
#
# Locking. While a job's before_script and script run, the job holds a SHARED
# flock on $slot/.in-use (scripts/ci-cache-lock.sh, sourced by the template's
# before_script). That is the file and the semantics main has used since
# c63fd2f, so this prune and main's see each other's holders on every slot.
# This script only ever takes the lock EXCLUSIVELY and without waiting, so it
# never touches a slot a job is using.
#
# Mixed rollout. Jobs on main and on the other branches that carry a lock take
# `.in-use` too. Branches without any lock (and anything cut before the lock
# existed) use the same slot paths unprotected; for those only the stamps
# below and, for rule 3, the activity probe stand between a slot and removal.
# The start stamp (target/.last-used, written when the lock is taken) exists
# only for jobs that run THIS template; main's jobs stamp only when their own
# prune runs at the end of the job.
#
# Eviction INVARIANT: this script never removes or renames a slot directory or
# its `.in-use`. Their inodes stay stable for every waiter -- ours, which
# re-checks the inode, and main's (c63fd2f), which does not: a job that opened
# `.in-use` and is waiting for the prune wakes up holding the live lock.
# What is evicted is the slot's target/ only: under the exclusive lock it is
# renamed to `$slot/.prune-trash.<pid>.<epoch>` (same directory, so the rename
# is atomic), the lock is released, and only then is the trash deleted. A job
# that starts meanwhile builds into a fresh target/. An emptied slot (the
# directory and a 0-byte `.in-use`) stays; that costs nothing. Leftover trash
# from a prune that was killed is deleted at the next scan, and only on an
# exact name match inside a numeric-layout slot.
#
# Merging with main (c63fd2f): this script, scripts/ci-cache-lock.sh and the
# .cargo-host-cache template replace main's. The lock is the same: `.in-use`,
# shared with `--wait 1200` for jobs, exclusive without waiting for the prune.
# The sweep is NOT a superset of main's: main sweeps every free slot to the
# running job's ceiling, this one sweeps only the job's OWN slot (rule 1), on a
# per-slot interval. That is deliberate -- do not restore main's all-slots
# sweep, which would shrink cargo-test's 140 GiB slot to another job's
# ceiling. Main's node-build and xqvm-onchain-test cache jobs need their own
# CI_CACHE_MAX_SIZE when they come under this template.
#
# Physical containment: the prune must never act outside the volume. A path
# check followed by a path-based operation is a race (a writer that ignores
# the lock can swap a slot for a symlink in between), so every read or write
# of a slot runs in a subshell that `cd -P`s into the slot, verifies that
# `pwd -P` is exactly <real cache root>/<digits>/<name> (anchor_slot), and
# then uses relative paths only: the lock on ./.in-use, the stamp and
# activity re-checks, the rename of ./target, the trash delete, leftover
# deletes, `du -P`, `find -P`, `stat`, and cargo-sweep (run with a relative
# CARGO_TARGET_DIR, which cargo resolves against this cwd). The cwd is
# re-verified under the exclusive lock. /ci-cache is its own mount, so a
# verified directory renamed afterwards stays inside the volume, and `mv`/`rm`
# never follow a symlink in their final component. A slot that fails
# verification is skipped, logged and counted as a failure; nothing aborts
# the run. If the cache root cannot be resolved, nothing is swept or removed.
#
# Threat model. The jobs use the docker executor: the runner mounts
# /ci-cache into every job container (runner-level `volumes`), and this prune
# runs in after_script as an unprivileged job container with exactly the
# mounts every other job on that runner has. ASSUMED: every job on a runner
# gets the same volume set. What is defended, and how:
#   - STATIC or accidental symlinks and special files are refused, never
#     followed: a symlinked numeric dir, slot or target/ (anchor_slot); a
#     symlinked or non-regular ./.in-use or target/.last-used (anchor_slot,
#     and again under the lock); a symlinked or non-regular root .prune.lock
#     or .last-prune (the whole scan is skipped). A refusal skips and logs;
#     nothing is deleted on refusal.
#   - Lock files are opened read-only, never truncated; an absent one is
#     created with noclobber (O_EXCL), which refuses any existing name.
#   - ACCEPTED RESIDUAL: a concurrent writer that ignores the lock could swap
#     a leaf (./.in-use, target/, the stamp, the root control files) for a
#     symlink between its check and its use, or swap a parent between cargo's
#     getcwd and cargo-sweep's deletes. That writer must already be able to
#     write /ci-cache, which lets it delete or poison anything there
#     directly; redirecting this prune reaches nothing beyond that volume and
#     this job's own ephemeral container. Closing it would need no-follow
#     openat(2) handling, which shell cannot do cleanly, so race-hardening
#     deliberately stops at this line.
#
# Three rules: two for two kinds of garbage, one for the volume as a whole.
#
#   1. Stale artifacts inside the job's OWN slot. `cargo sweep --maxsize`
#      evicts least-recently-used first, but only while the slot is over the
#      ceiling, which is in cargo-sweep's LOGICAL bytes (not the compressed
#      on-disk size a ZFS volume reports). Each cached job sets its own
#      CI_CACHE_MAX_SIZE about 1.3x above its measured logical working set, so
#      a sweep only removes superseded artifacts and never the warm set the
#      next run needs. cargo-sweep (0.8.0) ranks artifacts by the ACCESS time
#      (atime) of their fingerprint files, for --maxsize and --time alike.
#      There is deliberately NO age rule (--time): the hosts differ (finney
#      mounts the volume noatime, lovelace relatime), and with noatime the
#      oldest atimes in a warm slot are the stable dependencies worth keeping.
#      The mount options are logged on every run. Only $CARGO_TARGET_DIR is
#      swept -- never another job's slot, which may be mid-build or mid-test --
#      at most once per CI_CACHE_INTERVAL per slot, and only if the exclusive
#      lock is free (by after_script the job shell that held the shared lock
#      has exited). cargo-sweep never touches target/*/incremental, which this
#      rule therefore does not bound (CARGO_INCREMENTAL=0 would avoid it, but
#      it slows a warm rebuild by about two thirds, so it is not set).
#   2. Slots nothing uses any more: left by a renamed job, a reduced
#      concurrency limit, or a job that died before cleaning up. `--maxsize`
#      never reclaims one, because an abandoned slot sits under the ceiling
#      forever. A slot's target/ is evicted only when BOTH its age exceeds
#      CI_CACHE_ORPHAN_DAYS (the target/.last-used stamp each job refreshes)
#      AND the exclusive lock is free; the age is checked again under the lock.
#      A slot with no target/ has nothing to evict and is left as it is. This
#      scan runs once per host per CI_CACHE_INTERVAL and never two at once.
#   3. A volume over its budget (opt-in: `--volume-budget N`, an integer
#      percentage 1-99 of the volume's on-disk size as `df` reports it). It is
#      a COMMAND-LINE argument written in .gitlab-ci.yml, and no environment
#      variable can set it: project, group and pipeline variables override
#      YAML variables, so an environment switch could not be held off. The
#      argument is absent in this MR; add it at the merge to main, once the
#      branches that use these slots without a lock are gone.
#      Ceilings bound each slot, not their number, so the volume is bounded
#      separately: while usage is above the budget, the target/ of at most ONE
#      slot per run is evicted, the least recently used by its
#      target/.last-used stamp.
#      Only marked, numeric slots are candidates (a stamped target/ under
#      /ci-cache/<digits>/<job>); never the own slot, never a slot whose lock
#      is held, never one used within CI_CACHE_BUDGET_MIN_IDLE -- by its stamp
#      OR by any cargo fingerprint under target/ newer than that (cargo
#      rewrites those throughout a build, so this probe works without any
#      lock). Both checks are repeated under the lock. Whether the run left the
#      volume under budget is computed as df's usage minus the evicted
#      target/'s on-disk size, not by re-reading df (ZFS frees lag the delete). Every
#      failure removes nothing: df failing or printing something unparseable,
#      or df's device and mount point not matching the mount entry for the
#      cache root. A candidate whose target/ cannot be measured is skipped
#      (logged, counted as a failure) and the next is tried, so one
#      unmeasurable slot cannot starve the rule. Rule 3 runs on every cache
#      job's prune, so the volume converges across jobs.
#
# A slot that has a target/ but no stamp predates the stamp and is
# grandfathered: it is stamped and kept, rather than reading the missing stamp
# as "unused" and deleting every warm cache on the volume the first time this
# runs.
#
# Pass --dry-run to report what would be removed without removing it.
#
# Environment overrides:
#   CI_CACHE_ROOT          volume root (default /ci-cache)
#   CI_CACHE_MAX_SIZE      per-slot ceiling, cargo-sweep syntax (default 50GiB)
#   CI_CACHE_ORPHAN_DAYS   age at which an unused slot is removed (default 14)
#   CI_CACHE_INTERVAL      seconds between sweeps of one slot, and between
#                          orphan scans on one host (default 86400)
#   CI_CACHE_PROJECT_DIR   checkout cargo-sweep reads the manifest from
#                          (default: the repository holding this script)
#   CI_CACHE_DU_TIMEOUT    seconds one size measurement may take (default 60)
#   CI_CACHE_DU_BUDGET     seconds all slot-size reports in one orphan scan may
#                          take together, so after_script stays inside its
#                          runner limit (default 180)
#   CI_CACHE_BUDGET_MIN_IDLE  seconds since a slot's last use before rule 3 may
#                          remove it (default 7200)
#   CI_CACHE_MOUNTS        mount table to read (default /proc/self/mounts)
#
# Reporting (no effect on what is removed): every run logs the volume's `df`
# line and its mount entry (device and options, including the atime policy);
# sizes are reported twice, on disk and apparent (logical; ZFS compresses);
# the own-slot sweep logs the slot's size before and after; the orphan scan
# logs the size of every numeric slot and of the shared CARGO_HOME, within
# CI_CACHE_DU_BUDGET.

cache_root="${CI_CACHE_ROOT:-/ci-cache}"
# Without trailing slashes: every slot path below is built as
# "$cache_root/<n>/<slot>" and compared against it, so `/ci-cache/` would
# match nothing (and an evicted target/ would never be recognised as trash).
while [[ "$cache_root" == */ && "$cache_root" != / ]]; do cache_root="${cache_root%/}"; done
max_size="${CI_CACHE_MAX_SIZE:-50GiB}"
orphan_days="${CI_CACHE_ORPHAN_DAYS:-14}"
interval="${CI_CACHE_INTERVAL:-86400}"
du_timeout="${CI_CACHE_DU_TIMEOUT:-60}"
du_budget="${CI_CACHE_DU_BUDGET:-180}"
budget_min_idle="${CI_CACHE_BUDGET_MIN_IDLE:-7200}"
mounts="${CI_CACHE_MOUNTS:-/proc/self/mounts}"

stamp_name=".last-used"
lock_name=".in-use"
sweep_name=".last-sweep"
trash_prefix=".prune-trash"

log() { echo "prune-ci-cache: $*"; }

dry_run=0
budget_arg=""
budget_given=0
sweep_args=()
while (($#)); do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --volume-budget)
      budget_given=1
      budget_arg="${2-}"
      (($# >= 2)) && shift
      ;;
    --volume-budget=*)
      budget_given=1
      budget_arg="${1#--volume-budget=}"
      ;;
    *) sweep_args+=("$1") ;;
  esac
  shift
done

if [[ ! -d "$cache_root" ]]; then
  log "$cache_root is not mounted, nothing to do"
  exit 0
fi

if [[ -n "${CI_CACHE_VOLUME_BUDGET:-}" ]]; then
  log "CI_CACHE_VOLUME_BUDGET is ignored; the volume budget is only the --volume-budget argument"
fi

# cargo-sweep runs `cargo metadata`, so it needs a manifest. It will not accept
# a slot directory: those hold a bare target/ with no Cargo.toml beside it.
# Point it at the checkout and name the slot through CARGO_TARGET_DIR instead.
project_dir="${CI_CACHE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
sweep=1
if [[ ! -f "$project_dir/Cargo.toml" ]]; then
  echo "prune-ci-cache: no manifest at $project_dir, removing unused slots only" >&2
  sweep=0
fi
# cargo-sweep may be missing (for example after a wiped CARGO_HOME/bin). That
# must cost the sweep, never the job.
if ((sweep)) && ! cargo sweep --version >/dev/null 2>&1; then
  echo "prune-ci-cache: cargo-sweep is not available, skipping the own-slot sweep" >&2
  sweep=0
fi

# du_size DIR SECONDS [DU-FLAG]: DIR's size as `du -sh` prints it, or why it
# is not known. Bounded by SECONDS; never fails.
du_size() {
  local out size
  if ! command -v timeout >/dev/null 2>&1; then
    echo "unknown (timeout(1) is not available)"
  elif out="$(timeout "$2" du -P -sh ${3:+"$3"} "$1" 2>/dev/null)" && read -r size _ <<<"$out" \
    && [[ -n "$size" ]]; then
    echo "$size"
  else
    echo "unknown (du failed or took over ${2}s)"
  fi
}

# size_of DIR [SECONDS]: DIR's on-disk and apparent (logical) sizes. ZFS
# compresses, so the two differ; cargo-sweep's ceiling is in logical bytes.
# Each measurement is bounded by SECONDS (default CI_CACHE_DU_TIMEOUT).
size_of() {
  local limit="${2:-$du_timeout}"
  echo "$(du_size "$1" "$limit") on disk, $(du_size "$1" "$limit" --apparent-size) apparent"
}

# disk_kib DIR: DIR's on-disk size in KiB (df's unit), or fail.
disk_kib() {
  local out kib
  command -v timeout >/dev/null 2>&1 || return 1
  out="$(timeout "$du_timeout" du -P -sk "$1" 2>/dev/null)" || return 1
  read -r kib _ <<<"$out"
  [[ "$kib" =~ ^[0-9]+$ ]] || return 1
  echo "$kib"
}

# report_volume: log the volume's df line and the mount entry that holds it.
# Never fails.
report_volume() {
  local line
  if line="$(df -hP "$cache_root" 2>/dev/null | tail -n +2)" && [[ -n "$line" ]]; then
    log "volume: $line"
  else
    log "volume: df failed for $cache_root"
  fi
  if line="$(mount_entry)"; then
    log "mount: $line"
  else
    log "mount: no entry holding $cache_root in $mounts"
  fi
}

# mount_entry: the mount table line for the longest mount point that holds
# the cache root. Fails when the table is unreadable or has no such entry.
mount_entry() {
  local line
  [[ -r "$mounts" ]] || return 1
  line="$(awk -v root="$cache_root" '
    { m = $2; if (m == "/" || index(root "/", m "/") == 1) {
        if (length(m) >= length(best)) { best = m; entry = $0 } } }
    END { print entry }' "$mounts" 2>/dev/null)" || return 1
  [[ -n "$line" ]] || return 1
  echo "$line"
}

# volume_kib: print "USED TOTAL" in KiB for the cache volume from `df -P`.
# Fails -- so that rule 3 removes nothing -- if df fails, its line does not
# parse, or its device and mount point differ from the mount entry holding
# the cache root.
volume_kib() {
  local line fs used avail mnt entry dev mpoint
  line="$(df -P "$cache_root" 2>/dev/null | tail -n +2)" || return 1
  read -r fs _ used avail _ mnt <<<"$line" || return 1
  [[ "$used" =~ ^[0-9]+$ && "$avail" =~ ^[0-9]+$ && -n "$fs" && -n "$mnt" ]] || return 1
  ((used + avail > 0)) || return 1
  entry="$(mount_entry)" || return 1
  read -r dev mpoint _ <<<"$entry"
  [[ "$dev" == "$fs" && "$mpoint" == "$mnt" ]] || return 1
  echo "$used $((used + avail))"
}

# percent USED TOTAL: whole percent, rounded up.
percent() {
  echo $((($1 * 100 + $2 - 1) / $2))
}

# --- Slot operations: anchored on a verified cwd --------------------------
# A path check followed by a path-based operation is a race: anything that
# can write the volume can swap the slot for a symlink between the two. So
# every read or write of a slot runs in a SUBSHELL that first `cd -P`s into
# the slot and verifies the physical cwd (anchor_slot), then uses only
# relative paths. Swapping the slot's path afterwards cannot redirect them:
# the cwd is the verified directory's inode, and /ci-cache is its own mount,
# so that directory can only be renamed within the volume. `mv` and `rm` do
# not follow a symlink in the final component, and `du -P` and `find -P`
# follow none, so a target/ swapped for a link after the check is renamed or
# removed as a link, and never read through.

# The cache root, resolved once. Empty when it cannot be resolved: then
# anchor_slot fails for everything and nothing is swept or removed.
root_real="$(realpath -e "$cache_root" 2>/dev/null || true)"
if [[ -n "$root_real" ]]; then
  root_real="$(CDPATH='' cd -P -- "$root_real" && pwd -P)"
else
  log "cannot resolve $cache_root with realpath -e; nothing will be swept or removed"
fi

# slot_rel SLOT: "<digits>/<name>" when SLOT is spelled
# <cache root>/<digits>/<name>; fails otherwise.
slot_rel() {
  local n_dir
  n_dir="$(dirname "$1")"
  [[ "$(dirname "$n_dir")" == "$cache_root" && "$(basename "$n_dir")" =~ ^[0-9]+$ ]] || return 1
  echo "$(basename "$n_dir")/$(basename "$1")"
}

# plain_or_absent PATH: PATH is absent, or a regular file that is not a
# symlink. The leaves this script opens, touches or stats must be one or the
# other: a STATIC symlinked or special leaf is refused, never followed.
plain_or_absent() {
  [[ ! -L "$1" ]] && { [[ ! -e "$1" ]] || [[ -f "$1" ]]; }
}

# leaves_ok: (anchored) ./target is not a symlink, and ./.in-use and
# ./target/.last-used are each absent or a regular, non-symlink file.
leaves_ok() {
  [[ ! -L target ]] && plain_or_absent "$lock_name" && plain_or_absent "target/$stamp_name"
}

# anchor_slot SLOT: call in a subshell. `cd -P` into SLOT and verify that the
# physical cwd is exactly <real cache root>/<digits>/<name> -- no symlink in
# any component -- and that the leaves are sound (leaves_ok). Sets
# $anchored_at. Logs (to stderr, so a captured subshell's output stays clean)
# and returns 1 otherwise.
anchor_slot() {
  local rel
  if [[ -z "$root_real" ]] || ! rel="$(slot_rel "$1")" ||
    ! CDPATH='' cd -P -- "$1" 2>/dev/null || [[ "$(pwd -P)" != "$root_real/$rel" ]]; then
    log "$1 is not physically inside $cache_root/<n>/<slot> (symlink or unresolvable); skipping it" >&2
    return 1
  fi
  if ! leaves_ok; then
    log "$1 has a symlinked target/, or a symlinked or non-regular $lock_name or target/$stamp_name; skipping it" >&2
    return 1
  fi
  anchored_at="$root_real/$rel"
}

# still_anchored: (anchored) the cwd is still where anchor_slot verified it,
# and the leaves are still sound. Re-checked under the exclusive lock.
still_anchored() {
  [[ "$(pwd -P)" == "$anchored_at" ]] && leaves_ok
}

# older_than FILE SECONDS: true when FILE's mtime is at least SECONDS ago.
older_than() {
  (($(date +%s) - $(stat -c %Y "$1") >= $2))
}

# recently_built: (anchored) true when any cargo fingerprint under ./target is
# newer than CI_CACHE_BUDGET_MIN_IDLE. Cargo rewrites these throughout a
# build, so this sees activity whatever lock (if any) the builder took.
recently_built() {
  local ref hit
  ref="$(mktemp)" || return 0 # cannot tell: treat as in use
  touch -d "@$(($(date +%s) - budget_min_idle))" "$ref"
  hit="$(find -P ./target -mindepth 3 -maxdepth 5 -path '*/.fingerprint/*' -newer "$ref" \
    -print -quit 2>/dev/null || true)"
  rm -f "$ref"
  [[ -n "$hit" ]]
}

# open_lock PATH: open PATH read-only on a fresh fd, left in $lock_fd. An
# absent PATH is created first with `noclobber` (O_EXCL, which refuses any
# existing name, a dangling symlink included); an existing one is opened
# without O_CREAT or truncation. PATH must be a regular, non-symlink file
# immediately before the open (checked; a swap between the check and the open
# is the accepted residual, see the header). flock needs no write access.
open_lock() {
  [[ -e "$1" || -L "$1" ]] || (set -o noclobber && : >"$1") 2>/dev/null || true
  if [[ -L "$1" || ! -f "$1" ]]; then
    return 1
  fi
  exec {lock_fd}<"$1"
}

# try_exclusive DIR: open DIR's lock on a fresh fd and take it exclusively
# without waiting. On success the fd number is left in $held_fd (the caller
# closes it); on failure the fd is closed and the function returns 1.
try_exclusive() {
  local fd
  open_lock "$1/$lock_name" || return 1
  fd=$lock_fd
  if flock --nonblock --exclusive "$fd"; then
    held_fd=$fd
    return 0
  fi
  exec {fd}>&-
  return 1
}

release() {
  exec {held_fd}>&-
}

# is_trash PATH: PATH is spelled <cache root>/<digits>/<slot>/.prune-trash.<pid>.<epoch>
# -- the only shape this script ever deletes as trash.
is_trash() {
  [[ "$(basename "$1")" =~ ^\.prune-trash\.[0-9]+\.[0-9]+$ ]] && slot_rel "$(dirname "$1")" >/dev/null
}

# slot_size SLOT [SECONDS]: (subshell) the slot's sizes, measured from inside it.
slot_size() (
  anchor_slot "$1" || exit 1
  size_of . "${2:-$du_timeout}"
)

# target_kib SLOT: (subshell) SLOT/target's on-disk KiB, measured from inside.
target_kib() (
  anchor_slot "$1" || exit 1
  disk_kib ./target
)

# target_size SLOT: (subshell) SLOT/target's sizes, measured from inside.
target_size() (
  anchor_slot "$1" || exit 1
  size_of ./target
)

# target_stamp SLOT: (subshell) the mtime of SLOT/target/.last-used, or
# nothing when the slot has no stamped target/.
target_stamp() (
  anchor_slot "$1" || exit 1
  [[ -d target && -e "target/$stamp_name" ]] || exit 0
  stat -c %Y "target/$stamp_name"
)

# orphan_state SLOT: (subshell) none | grandfathered | fresh | stale, for
# rule 2's pre-check. A stamp-less target/ is stamped here (grandfathered).
orphan_state() (
  anchor_slot "$1" || exit 1
  [[ -d target ]] || { echo none; exit 0; }
  if [[ ! -e "target/$stamp_name" ]]; then
    ((dry_run)) || touch "target/$stamp_name"
    echo grandfathered
  elif older_than "target/$stamp_name" "$orphan_seconds"; then
    echo stale
  else
    echo fresh
  fi
)

# budget_idle SLOT: (subshell) rule 3's pre-check. 0 idle; 2 used within
# CI_CACHE_BUDGET_MIN_IDLE by its stamp; 3 by a cargo fingerprint; 1 failed.
budget_idle() (
  anchor_slot "$1" || exit 1
  older_than "target/$stamp_name" "$budget_min_idle" || exit 2
  if recently_built; then exit 3; fi
  exit 0
)

# evict_slot RULE SLOT: (subshell) anchored on SLOT: take its exclusive lock
# on ./.in-use, re-check the cwd and (for RULE orphan or budget) the stamp and
# the activity probe under it, rename ./target to
# ./.prune-trash.<pid>.<epoch> (same directory: atomic), release the lock,
# then delete the trash -- the lock, the re-checks, the rename and the delete
# all act on the one verified directory. The slot directory and its .in-use
# are never touched. Exit: 0 evicted (or would be, on a dry run); 5 evicted
# but the delete failed (the next scan retries); 3 lock held; 4 kept by a
# re-check; 1 failed (logged).
evict_slot() (
  rule="$1"
  slot="$2"
  anchor_slot "$slot" || exit 1
  try_exclusive . || exit 3
  if ! still_anchored; then
    log "$slot is not physically inside $cache_root/<n>/<slot> (moved while locking); skipping it"
    exit 1
  fi
  [[ -d target ]] || exit 4
  if [[ "$rule" == orphan ]]; then
    older_than "target/$stamp_name" "$orphan_seconds" || exit 4
    # A job on a template without the lock step takes no lock and does not
    # stamp, so cargo's fingerprints are the only sign it is building here.
    if recently_built; then
      log "stale slot $slot has cargo fingerprints newer than ${budget_min_idle}s; keeping it"
      exit 4
    fi
    log "evicting the target/ of unused slot $slot"
  else
    older_than "target/$stamp_name" "$budget_min_idle" || exit 4
    if recently_built; then exit 4; fi
    log "volume budget: evicting the target/ of least recently used slot $slot ($evict_size)"
  fi
  if ((dry_run)); then
    [[ "$rule" == orphan ]] && { du -P -sh ./target || true; }
    exit 0
  fi
  trash="$trash_prefix.$$.$(date +%s)"
  if ! mv -- target "$trash"; then
    log "cannot move $slot/target aside; not evicting it"
    exit 1
  fi
  release
  if ! rm -rf -- "./$trash"; then
    log "could not delete $slot/$trash (the next scan retries)"
    exit 5
  fi
  exit 0
)

# delete_leftover SLOT NAME: (subshell) delete SLOT/NAME, an exact trash name
# left by a prune that was killed between an eviction and its delete.
delete_leftover() (
  anchor_slot "$1" || exit 1
  [[ "$2" =~ ^\.prune-trash\.[0-9]+\.[0-9]+$ ]] || exit 1
  [[ -e "./$2" || -L "./$2" ]] || exit 0
  log "deleting leftover $1/$2"
  rm -rf -- "./$2" || {
    log "could not delete $1/$2 (the next scan retries)"
    exit 1
  }
)

# own_stamp / own_sweep: rule 1, anchored on the own slot.
own_stamp() (
  anchor_slot "$own_slot" || exit 1
  touch "target/$stamp_name"
)
# Exit: 0 swept (or skipped inside the interval, logged); 3 lock held; 1 failed.
own_sweep() (
  anchor_slot "$own_slot" || exit 1
  if [[ -f "$sweep_name" ]] && ! older_than "$sweep_name" "$interval"; then
    log "own slot $own_slot swept less than ${interval}s ago, skipping sweep"
    exit 0
  fi
  try_exclusive . || exit 3
  if ! still_anchored; then
    log "own slot $own_slot moved while locking; not sweeping it"
    exit 1
  fi
  log "sweeping own slot $CARGO_TARGET_DIR to $max_size"
  log "own slot size before sweep: $(size_of .)"
  if ((dry_run)); then
    log "(dry run) would run cargo sweep --maxsize $max_size"
    exit 0
  fi
  # cargo-sweep's path argument is the PROJECT (it runs `cargo metadata` on
  # its manifest and sweeps the reported target directory). Run from this
  # verified cwd with a RELATIVE CARGO_TARGET_DIR, cargo resolves the target
  # against its own working directory -- this one, by inode -- rather than a
  # path string computed earlier (checked against cargo-sweep 0.8.0's source
  # and `cargo metadata`; the self-test records the cwd and the variable).
  status=0
  if CARGO_TARGET_DIR=target \
    cargo sweep --maxsize "$max_size" "${sweep_args[@]}" "$project_dir"; then
    touch "$sweep_name"
  else
    status=1
  fi
  log "own slot size after sweep: $(size_of .)"
  exit "$status"
)

failed=0
own_slot=""

report_volume

# --- Rule 1: the job's own slot -------------------------------------------
if [[ -n "${CARGO_TARGET_DIR:-}" && -d "$CARGO_TARGET_DIR" ]]; then
  own_slot="$(cd "$CARGO_TARGET_DIR/.." && pwd)"
  # cargo-sweep and `touch` follow symlinks: a slot or target/ that is one is
  # neither stamped nor swept.
  if [[ "$(basename "$CARGO_TARGET_DIR")" != target ]] || ! (anchor_slot "$own_slot"); then
    log "own slot $own_slot is not a physical slot; not stamping or sweeping it"
    failed=1
    sweep=0
  else
    # Mark the slot this job just used. Directory modification times cannot
    # stand in for this: creating a file under target/debug does not touch
    # the slot directory above it, so an actively built slot still looks
    # untouched.
    ((dry_run)) || own_stamp || failed=1
    if ((sweep)); then
      status=0
      own_sweep || status=$?
      if ((status == 3)); then
        log "own slot $own_slot is still locked by another holder, skipping sweep"
      elif ((status != 0)); then
        failed=1
      fi
    fi
  fi
fi

# --- Rules 2 and 3 run under the per-host scan lock -----------------------
# The scan lock and interval are separate files: other prunes (main's) open
# the lock with `>`, which truncates it, so its modification time cannot carry
# the interval. This one opens it read-only (open_lock).
scan_lock="$cache_root/.prune.lock"
last_scan="$cache_root/.last-prune"
have_scan_lock=0
# Both control files are leaves at the root: a symlinked or non-regular one
# refuses the whole scan rather than be opened or touched through.
if ! plain_or_absent "$scan_lock" || ! plain_or_absent "$last_scan"; then
  log "$scan_lock or $last_scan is a symlink or not a regular file; skipping the orphan scan and the volume budget"
  failed=1
elif ! open_lock "$scan_lock"; then
  log "cannot open $scan_lock as a regular file; skipping the orphan scan and the volume budget"
  failed=1
elif scan_fd=$lock_fd && flock --nonblock "$scan_fd"; then
  have_scan_lock=1
  # Leftovers of a prune that was killed between an eviction and its delete:
  # exact trash names inside numeric-layout slots only.
  if ((dry_run == 0)); then
    for leftover in "$cache_root"/*/*/"$trash_prefix".*; do
      [[ -e "$leftover" || -L "$leftover" ]] || continue
      is_trash "$leftover" || continue
      delete_leftover "$(dirname "$leftover")" "$(basename "$leftover")" || failed=1
    done
  fi
else
  log "another prune holds the scan lock, skipping the orphan scan and the volume budget"
fi

# --- Rule 2: orphaned slots, once per host per interval --------------------
orphan_seconds=$((orphan_days * 86400))
if ((have_scan_lock == 0)); then
  :
elif ((dry_run == 0)) && [[ -f "$last_scan" ]] && ! older_than "$last_scan" "$interval"; then
  log "orphan scan ran less than ${interval}s ago, skipping"
else
  for slot in "$cache_root"/*/*; do
    [[ -d "$slot" ]] || continue
    # Only /ci-cache/<CI_CONCURRENT_ID>/<CI_JOB_NAME> is a slot. Everything else
    # on the volume -- above all the shared CARGO_HOME at /ci-cache/cargo-home,
    # whose registry/git/bin children look like target-less slots -- is not,
    # and must never be swept or removed.
    [[ "$(basename "$(dirname "$slot")")" =~ ^[0-9]+$ ]] || continue
    [[ "$(cd "$slot" && pwd)" == "$own_slot" ]] && continue
    if ! state="$(orphan_state "$slot")"; then
      failed=1
      continue
    fi
    case "$state" in
      grandfathered)
        log "grandfathered unstamped slot $slot"
        continue
        ;;
      stale) ;;
      *) continue ;;
    esac
    status=0
    evict_slot orphan "$slot" || status=$?
    case "$status" in
      0 | 4) ;;
      3) log "stale slot $slot is in use (lock held), skipping" ;;
      *) failed=1 ;;
    esac
  done
  # Size report for capacity planning: every numeric slot left after the scan,
  # then the shared CARGO_HOME, inside one time budget. Reading only, and each
  # slot is measured from inside it.
  deadline=$((SECONDS + du_budget))
  reported=0
  for dir in "$cache_root"/*/* "$cache_root/cargo-home"; do
    [[ -d "$dir" ]] || continue
    if [[ "$dir" != "$cache_root/cargo-home" ]]; then
      [[ "$(basename "$(dirname "$dir")")" =~ ^[0-9]+$ ]] || continue
    fi
    left=$((deadline - SECONDS))
    if ((left <= 0)); then
      log "size budget of ${du_budget}s spent after $reported report(s); the rest are not reported"
      break
    fi
    ((left < du_timeout)) || left=$du_timeout
    if [[ "$dir" == "$cache_root/cargo-home" ]]; then
      size="$(size_of "$dir" "$left")"
    elif ! size="$(slot_size "$dir" "$left")"; then
      failed=1
      continue
    fi
    log "size: $dir $size"
    reported=$((reported + 1))
  done
  # Record the interval only on a scan that reached the end. after_script has
  # its own runner timeout, and a scan killed part way through must be retried
  # by the next job rather than counted as done. Re-checked right before the
  # touch, which would follow a symlink.
  if ((dry_run == 0)); then
    if plain_or_absent "$last_scan"; then
      touch "$last_scan"
    else
      log "$last_scan became a symlink or not a regular file; not recording the scan"
      failed=1
    fi
  fi
fi

# --- Rule 3: the volume budget (opt-in, --volume-budget N) ----------------
# Runs on every prune that holds the scan lock, independently of the scan
# interval: it is one `df` while the volume is under budget.
if ((budget_given)) && ((have_scan_lock)); then
  if ! [[ "$budget_arg" =~ ^[1-9][0-9]?$ ]]; then
    log "volume budget: '$budget_arg' is not a percentage 1-99; removing nothing"
  else
    budget=$((10#$budget_arg))
    if ! read -r used total < <(volume_kib); then
      log "volume budget: cannot read a usage for $cache_root that matches its mount entry; removing nothing"
    else
      use="$(percent "$used" "$total")"
      log "volume budget: $cache_root is ${use}% used, budget ${budget}%"
      if ((use > budget)); then
        # Candidates: stamped, numeric slots only, least recently used first.
        # Each stamp is read from inside its slot.
        stamped=()
        for slot in "$cache_root"/*/*; do
          [[ -d "$slot" ]] || continue
          [[ "$(basename "$(dirname "$slot")")" =~ ^[0-9]+$ ]] || continue
          [[ "$(cd "$slot" && pwd)" == "$own_slot" ]] && continue
          if ! stamp="$(target_stamp "$slot")"; then
            failed=1
            continue
          fi
          [[ -n "$stamp" ]] && stamped+=("$stamp $slot")
        done
        candidates=()
        if ((${#stamped[@]})); then
          while IFS= read -r line; do
            candidates+=("${line#* }")
          done < <(printf '%s\n' "${stamped[@]}" | sort -n)
        fi
        removed=""
        for slot in "${candidates[@]}"; do
          # Judge each candidate on its own stamp: the list was sorted before
          # any lock was taken, and a job may have stamped since.
          status=0
          budget_idle "$slot" || status=$?
          case "$status" in
            0) ;;
            2)
              log "volume budget: $slot was used within ${budget_min_idle}s; keeping it"
              continue
              ;;
            3)
              log "volume budget: $slot has cargo fingerprints newer than ${budget_min_idle}s; keeping it"
              continue
              ;;
            *)
              failed=1
              continue
              ;;
          esac
          # Measure before the lock: `du` may take CI_CACHE_DU_TIMEOUT per
          # call, and a job waiting on this slot's lock would wait for it. A
          # slot that cannot be measured is skipped, never evicted, and the
          # next candidate is tried: one unmeasurable slot must not starve
          # the rule.
          if ! slot_kib="$(target_kib "$slot")"; then
            log "volume budget: cannot measure $slot/target; skipping it"
            failed=1
            continue
          fi
          evict_size="$(target_size "$slot")" || evict_size="size unknown"
          status=0
          evict_slot budget "$slot" || status=$?
          case "$status" in
            0 | 5) ;;
            3)
              log "volume budget: $slot is in use (lock held), skipping"
              continue
              ;;
            4) continue ;;
            *)
              failed=1
              continue
              ;;
          esac
          ((status == 5)) && failed=1
          removed="$slot"
          if ((dry_run == 0)); then
            # Decide from the evicted target/'s size, not a df re-read: ZFS
            # frees lag the delete. At most one eviction per run.
            projected="$(percent "$((used > slot_kib ? used - slot_kib : 0))" "$total")"
            if ((projected > budget)); then
              log "volume budget: projected ${projected}% after evicting $slot/target; still over budget, the next prune continues"
            else
              log "volume budget: projected ${projected}% after evicting $slot/target; under budget"
            fi
          fi
          break
        done
        if [[ -z "$removed" ]]; then
          log "volume budget: still over budget; no removable slot is left"
        fi
      fi
    fi
  fi
fi

# A prune failure must not red a green build. Report it loudly instead: a
# silent skip here is how the volume fills up without anyone noticing.
if ((failed)); then
  echo "prune-ci-cache: a sweep, move or delete failed, see above" >&2
fi
exit 0

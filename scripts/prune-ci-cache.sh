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
# volume also holds the shared CARGO_HOME (/ci-cache/cargo-home) and this
# script's own `.trash`, neither of which is a slot.
#
# Locking. While a job's before_script and script run, the job holds a SHARED
# flock on $slot/.in-use (scripts/ci-cache-lock.sh, sourced by the template's
# before_script). That is the file and the semantics main has used since
# c63fd2f, so this prune and main's see each other's holders on every slot.
# This script only ever takes the lock EXCLUSIVELY and without waiting, so it
# never touches a slot a job is using. `.in-use.lock`, the name an earlier
# version of this branch used, is still recognised as a marker of a slot this
# cache created, but nothing locks on it any more.
#
# Mixed rollout. Jobs on main and on the other branches that carry a lock take
# `.in-use` too. Branches without any lock (and anything cut before the lock
# existed) use the same slot paths unprotected; for those only the stamps
# below and, for rule 3, the activity probe stand between a slot and removal.
# The start stamp (target/.last-used, written when the lock is taken) exists
# only for jobs that run THIS template; main's jobs stamp only when their own
# prune runs at the end of the job.
#
# Removal is by rename: under the exclusive lock a slot is moved aside to
# $cache_root/.trash/ (same filesystem, atomic), the lock is released, and only
# then is the trashed copy deleted. A job that was waiting on the old lock
# wakes up, sees that its fd no longer names the slot's lock, and retries on a
# fresh slot; a job that starts meanwhile creates a fresh slot. Deleting in
# place would let a job lock a new `.in-use` while `rm` was still emptying
# target/. Leftovers in .trash (from a prune that was killed) are deleted at
# the start of the next scan.
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
#      forever. A slot is removed whole only when BOTH its age exceeds
#      CI_CACHE_ORPHAN_DAYS (the target/.last-used stamp each job refreshes)
#      AND the exclusive lock is free; the age is checked again under the lock.
#      A directory with no target/ is removed only if it carries one of this
#      cache's markers (`.in-use`, `.in-use.lock` or `.last-used`), aged by its
#      own mtime: what this cache created is allow-listed, and anything
#      unmarked is never touched. This scan runs once per host per
#      CI_CACHE_INTERVAL and never two at once.
#   3. A volume over its budget (opt-in: `--volume-budget N`, an integer
#      percentage 1-99 of the volume's on-disk size as `df` reports it). It is
#      a COMMAND-LINE argument written in .gitlab-ci.yml, and no environment
#      variable can set it: project, group and pipeline variables override
#      YAML variables, so an environment switch could not be held off. The
#      argument is absent in this MR; add it at the merge to main, once the
#      branches that use these slots without a lock are gone.
#      Ceilings bound each slot, not their number, so the volume is bounded
#      separately: while usage is above the budget, at most ONE whole slot per
#      run is removed, the least recently used by its target/.last-used stamp.
#      Only marked, numeric slots are candidates (a stamped target/ under
#      /ci-cache/<digits>/<job>); never the own slot, never a slot whose lock
#      is held, never one used within CI_CACHE_BUDGET_MIN_IDLE -- by its stamp
#      OR by any cargo fingerprint under target/ newer than that (cargo
#      rewrites those throughout a build, so this probe works without any
#      lock). Both checks are repeated under the lock. Whether the run left the
#      volume under budget is computed as df's usage minus the removed slot's
#      on-disk size, not by re-reading df (ZFS frees lag the delete). Every
#      failure removes nothing: df failing or printing something unparseable,
#      or df's device and mount point not matching the mount entry for the
#      cache root. Rule 3 runs on every cache job's prune, so the volume
#      converges across jobs.
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
max_size="${CI_CACHE_MAX_SIZE:-50GiB}"
orphan_days="${CI_CACHE_ORPHAN_DAYS:-14}"
interval="${CI_CACHE_INTERVAL:-86400}"
du_timeout="${CI_CACHE_DU_TIMEOUT:-60}"
du_budget="${CI_CACHE_DU_BUDGET:-180}"
budget_min_idle="${CI_CACHE_BUDGET_MIN_IDLE:-7200}"
mounts="${CI_CACHE_MOUNTS:-/proc/self/mounts}"

stamp_name=".last-used"
lock_name=".in-use"
legacy_lock_name=".in-use.lock"
sweep_name=".last-sweep"
trash="$cache_root/.trash"

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
  elif out="$(timeout "$2" du -sh ${3:+"$3"} "$1" 2>/dev/null)" && read -r size _ <<<"$out" \
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
  out="$(timeout "$du_timeout" du -sk "$1" 2>/dev/null)" || return 1
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

# older_than FILE SECONDS: true when FILE's mtime is at least SECONDS ago.
older_than() {
  (($(date +%s) - $(stat -c %Y "$1") >= $2))
}

# recently_built SLOT: true when any cargo fingerprint under SLOT/target is
# newer than CI_CACHE_BUDGET_MIN_IDLE. Cargo rewrites these throughout a
# build, so this sees activity whatever lock (if any) the builder took.
recently_built() {
  local ref hit
  ref="$(mktemp)" || return 0 # cannot tell: treat as in use
  touch -d "@$(($(date +%s) - budget_min_idle))" "$ref"
  hit="$(find "$1/target" -mindepth 3 -maxdepth 5 -path '*/.fingerprint/*' -newer "$ref" \
    -print -quit 2>/dev/null || true)"
  rm -f "$ref"
  [[ -n "$hit" ]]
}

# try_exclusive SLOT: open SLOT's lock on a fresh fd and take it exclusively
# without waiting. On success the fd number is left in $held_fd (the caller
# closes it); on failure the fd is closed and the function returns 1.
try_exclusive() {
  local fd
  exec {fd}>>"$1/$lock_name"
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

# to_trash SLOT: with SLOT's exclusive lock held, move it aside atomically.
# Prints the trash path; on failure logs, sets failed=1 and returns 1.
to_trash() {
  local dest
  dest="$trash/$(basename "$(dirname "$1")").$(basename "$1").$$.$RANDOM"
  if ! mkdir -p "$trash"; then
    log "cannot create $trash; not removing $1" >&2
    return 1
  fi
  if ! mv "$1" "$dest"; then
    log "cannot move $1 to $trash; not removing it" >&2
    return 1
  fi
  echo "$dest"
}

# delete_trashed PATH: delete a trashed slot (after its lock was released).
delete_trashed() {
  rm -rf "$1" || {
    log "could not delete $1 (the next scan retries)"
    failed=1
  }
}

failed=0
own_slot=""

report_volume

# --- Rule 1: the job's own slot -------------------------------------------
if [[ -n "${CARGO_TARGET_DIR:-}" && -d "$CARGO_TARGET_DIR" ]]; then
  own_slot="$(cd "$CARGO_TARGET_DIR/.." && pwd)"
  # Mark the slot this job just used. Directory modification times cannot
  # stand in for this: creating a file under target/debug does not touch the
  # slot directory above it, so an actively built slot still looks untouched.
  ((dry_run)) || touch "$CARGO_TARGET_DIR/$stamp_name"

  if ((sweep)); then
    if [[ -f "$own_slot/$sweep_name" ]] && ! older_than "$own_slot/$sweep_name" "$interval"; then
      log "own slot $own_slot swept less than ${interval}s ago, skipping sweep"
    elif try_exclusive "$own_slot"; then
      log "sweeping own slot $CARGO_TARGET_DIR to $max_size"
      log "own slot size before sweep: $(size_of "$own_slot")"
      if ((dry_run)); then
        log "(dry run) would run cargo sweep --maxsize $max_size"
      elif CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
        cargo sweep --maxsize "$max_size" "${sweep_args[@]}" "$project_dir"; then
        touch "$own_slot/$sweep_name"
      else
        failed=1
      fi
      ((dry_run)) || log "own slot size after sweep: $(size_of "$own_slot")"
      release
    else
      log "own slot $own_slot is still locked by another holder, skipping sweep"
    fi
  fi
fi

# --- Rules 2 and 3 run under the per-host scan lock -----------------------
# The scan lock and interval are separate files: opening the lock truncates
# it, which would reset a modification time read back from the lock itself.
scan_lock="$cache_root/.prune.lock"
last_scan="$cache_root/.last-prune"
exec {scan_fd}>"$scan_lock"
have_scan_lock=0
if flock --nonblock "$scan_fd"; then
  have_scan_lock=1
  # Leftovers of a prune that was killed between a rename and its delete.
  if [[ -d "$trash" ]] && ((dry_run == 0)); then
    for leftover in "$trash"/* "$trash"/.[!.]*; do
      [[ -e "$leftover" ]] || continue
      log "deleting leftover $leftover"
      delete_trashed "$leftover"
    done
  fi
else
  log "another prune holds the scan lock, skipping the orphan scan and the volume budget"
fi

# --- Rule 2: orphaned slots, once per host per interval --------------------
if ((have_scan_lock == 0)); then
  :
elif ((dry_run == 0)) && [[ -f "$last_scan" ]] && ! older_than "$last_scan" "$interval"; then
  log "orphan scan ran less than ${interval}s ago, skipping"
else
  orphan_seconds=$((orphan_days * 86400))
  for slot in "$cache_root"/*/*; do
    [[ -d "$slot" ]] || continue
    # Only /ci-cache/<CI_CONCURRENT_ID>/<CI_JOB_NAME> is a slot. Everything else
    # on the volume -- above all the shared CARGO_HOME at /ci-cache/cargo-home,
    # whose registry/git/bin children look like target-less slots -- is not,
    # and must never be swept or removed.
    [[ "$(basename "$(dirname "$slot")")" =~ ^[0-9]+$ ]] || continue
    [[ "$(cd "$slot" && pwd)" == "$own_slot" ]] && continue
    target="$slot/target"

    if [[ -d "$target" ]]; then
      age_of="$target/$stamp_name"
      if [[ ! -e "$age_of" ]]; then
        ((dry_run)) || touch "$age_of"
        log "grandfathered unstamped slot $slot"
        continue
      fi
    else
      # No target/: remove it only if this cache made it. A half-created slot
      # carries the lock file the template's before_script opens; anything
      # without one of our markers is not ours and is left alone.
      if [[ ! -e "$slot/$lock_name" && ! -e "$slot/$legacy_lock_name" &&
        ! -e "$slot/$stamp_name" ]]; then
        log "leaving unmarked directory $slot alone"
        continue
      fi
      age_of="$slot"
    fi

    older_than "$age_of" "$orphan_seconds" || continue

    if ! try_exclusive "$slot"; then
      log "stale slot $slot is in use (lock held), skipping"
      continue
    fi
    # Re-check under the lock: a job that started between the first check and
    # the lock stamps target/ right after taking its shared hold. Read only
    # that stamp here -- opening the lock may itself have created the lock
    # file, which refreshes the slot directory's own mtime.
    if [[ -e "$target/$stamp_name" ]] && ! older_than "$target/$stamp_name" "$orphan_seconds"; then
      release
      continue
    fi
    log "removing unused slot $slot"
    if ((dry_run)); then
      du -sh "$slot" || true
      release
      continue
    fi
    trashed=""
    trashed="$(to_trash "$slot")" || failed=1
    release
    [[ -n "$trashed" ]] && delete_trashed "$trashed"
  done
  # Size report for capacity planning: every numeric slot left after the scan,
  # then the shared CARGO_HOME, inside one time budget. Reading only.
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
    log "size: $dir $(size_of "$dir" "$left")"
    reported=$((reported + 1))
  done
  # Record the interval only on a scan that reached the end. after_script has
  # its own runner timeout, and a scan killed part way through must be retried
  # by the next job rather than counted as done.
  ((dry_run)) || touch "$last_scan"
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
        # Candidates: marked, numeric slots only, least recently used first.
        candidates=()
        while IFS= read -r line; do
          candidates+=("${line#* }")
        done < <(
          for slot in "$cache_root"/*/*; do
            [[ -d "$slot/target" && -e "$slot/target/$stamp_name" ]] || continue
            [[ "$(basename "$(dirname "$slot")")" =~ ^[0-9]+$ ]] || continue
            [[ "$(cd "$slot" && pwd)" == "$own_slot" ]] && continue
            echo "$(stat -c %Y "$slot/target/$stamp_name") $slot"
          done | sort -n
        )
        removed=""
        for slot in "${candidates[@]}"; do
          # Judge each candidate on its own stamp: the list was sorted before
          # any lock was taken, and a job may have stamped since.
          if ! older_than "$slot/target/$stamp_name" "$budget_min_idle"; then
            log "volume budget: $slot was used within ${budget_min_idle}s; keeping it"
            continue
          fi
          if recently_built "$slot"; then
            log "volume budget: $slot has cargo fingerprints newer than ${budget_min_idle}s; keeping it"
            continue
          fi
          if ! try_exclusive "$slot"; then
            log "volume budget: $slot is in use (lock held), skipping"
            continue
          fi
          # Re-check under the lock: a job that started meanwhile stamps first.
          if ! older_than "$slot/target/$stamp_name" "$budget_min_idle" || recently_built "$slot"; then
            release
            continue
          fi
          if ! slot_kib="$(disk_kib "$slot")"; then
            log "volume budget: cannot measure $slot; removing nothing"
            release
            break
          fi
          log "volume budget: removing least recently used slot $slot ($(size_of "$slot"))"
          if ((dry_run)); then
            release
            removed="$slot"
            break
          fi
          trashed=""
          trashed="$(to_trash "$slot")" || failed=1
          release
          if [[ -n "$trashed" ]]; then
            delete_trashed "$trashed"
            removed="$slot"
            # Decide from the removed slot's size, not a df re-read: ZFS frees
            # lag the delete. At most one removal per run.
            projected="$(percent "$((used > slot_kib ? used - slot_kib : 0))" "$total")"
            if ((projected > budget)); then
              log "volume budget: projected ${projected}% after removing $slot; still over budget, the next prune continues"
            else
              log "volume budget: projected ${projected}% after removing $slot; under budget"
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

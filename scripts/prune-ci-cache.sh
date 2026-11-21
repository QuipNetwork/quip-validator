#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/prune-ci-cache.sh [extra cargo-sweep args]
#
# Prunes the host cache volume that .cargo-host-cache mounts at /ci-cache. It
# runs from that template's after_script, so it rides along with the jobs that
# fill the volume on every host in the amd64 pool.
#
# Layout: a slot is /ci-cache/<CI_CONCURRENT_ID>/<CI_JOB_NAME>, holding the
# job's target/ and an `.in-use.lock` file. CI_CONCURRENT_ID is numeric, and
# only directories under a numeric first level are ever treated as slots: the
# volume also holds the shared CARGO_HOME (/ci-cache/cargo-home), which this
# script must never touch. While a job's before_script and
# script run, the job holds a SHARED flock on its slot's lock (taken in the
# .cargo-host-cache before_script, on a file descriptor of the job shell). Any
# number of holders can share it; this script only ever takes it EXCLUSIVELY
# and without waiting, so it can never touch a slot a job is using.
#
# The lock protects only jobs that run this .cargo-host-cache template. Until
# it reaches main, jobs on main and other branches share the same slots
# without taking it; for them the 14-day stamp (refreshed at job start by the
# template's before_script, and at job end here) is the only protection.
#
# Two rules, for two kinds of garbage:
#
#   1. Stale artifacts inside the job's OWN slot. `cargo sweep --maxsize`
#      evicts least-recently-used first, but only while the slot is over the
#      ceiling. cargo-sweep (0.8.0) ranks artifacts by the ACCESS time (atime)
#      of their fingerprint files, for --maxsize and --time alike, so whether an
#      age rule is safe depends on the volume's atime mount option: with
#      noatime the oldest atimes in a warm slot are the stable dependencies
#      worth keeping. The mount options are logged on every run (below) to
#      settle that. Only $CARGO_TARGET_DIR is swept -- never
#      another job's slot, which may be mid-build or mid-test -- at most once
#      per CI_CACHE_INTERVAL per slot, and only if the exclusive lock is free
#      (by after_script the job shell that held the shared lock has exited).
#   2. Slots nothing uses any more: left by a renamed job, a reduced
#      concurrency limit, or a job that died before cleaning up. `--maxsize`
#      never reclaims one, because an abandoned slot sits under the ceiling
#      forever. A slot is removed whole only when BOTH its age exceeds
#      CI_CACHE_ORPHAN_DAYS (the target/.last-used stamp each job refreshes)
#      AND the exclusive lock is free. Otherwise it is skipped and logged.
#      A directory with no target/ is removed only if it carries one of this
#      script's own markers (`.in-use.lock` or `.last-used`), aged by its own
#      mtime: what this cache created is allow-listed, and anything unmarked
#      is never touched. This scan runs once per host per
#      CI_CACHE_INTERVAL and never two at once.
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
#
# Reporting (no effect on what is removed): every run logs the volume's `df`
# line and its mount entry (device and options, including the atime policy);
# the own-slot sweep logs the slot's size before and after; the orphan scan
# logs the size of every numeric slot and of the shared CARGO_HOME, within
# CI_CACHE_DU_BUDGET. These are the inputs for sizing the per-job ceilings
# against the volume.

cache_root="${CI_CACHE_ROOT:-/ci-cache}"
max_size="${CI_CACHE_MAX_SIZE:-50GiB}"
orphan_days="${CI_CACHE_ORPHAN_DAYS:-14}"
interval="${CI_CACHE_INTERVAL:-86400}"
du_timeout="${CI_CACHE_DU_TIMEOUT:-60}"
du_budget="${CI_CACHE_DU_BUDGET:-180}"

stamp_name=".last-used"
lock_name=".in-use.lock"
sweep_name=".last-sweep"

dry_run=0
sweep_args=()
for arg in "$@"; do
  if [[ "$arg" == "--dry-run" ]]; then
    dry_run=1
  else
    sweep_args+=("$arg")
  fi
done

log() { echo "prune-ci-cache: $*"; }

if [[ ! -d "$cache_root" ]]; then
  log "$cache_root is not mounted, nothing to do"
  exit 0
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

# size_of DIR [SECONDS]: DIR's size as `du -sh` prints it, or why it is not
# known. Bounded by SECONDS (default CI_CACHE_DU_TIMEOUT); never fails.
size_of() {
  local limit="${2:-$du_timeout}" out size
  if ! command -v timeout >/dev/null 2>&1; then
    echo "unknown (timeout(1) is not available)"
  elif out="$(timeout "$limit" du -sh "$1" 2>/dev/null)" && read -r size _ <<<"$out" \
    && [[ -n "$size" ]]; then
    echo "$size"
  else
    echo "unknown (du failed or took over ${limit}s)"
  fi
}

# report_volume: log the volume's df line and the mount entry that holds it
# (the longest mount point that is a prefix of the cache root). Never fails.
report_volume() {
  local line
  if line="$(df -hP "$cache_root" 2>/dev/null | tail -n +2)" && [[ -n "$line" ]]; then
    log "volume: $line"
  else
    log "volume: df failed for $cache_root"
  fi
  if [[ -r /proc/self/mounts ]]; then
    line="$(awk -v root="$cache_root" '
      { m = $2; if (m == "/" || index(root "/", m "/") == 1) {
          if (length(m) >= length(best)) { best = m; entry = $0 } } }
      END { print entry }' /proc/self/mounts 2>/dev/null || true)"
    log "mount: ${line:-no entry holding $cache_root in /proc/self/mounts}"
  else
    log "mount: /proc/self/mounts is not readable"
  fi
}

# older_than FILE SECONDS: true when FILE's mtime is at least SECONDS ago.
older_than() {
  (($(date +%s) - $(stat -c %Y "$1") >= $2))
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

# --- Rule 2: orphaned slots, once per host per interval --------------------
# The scan lock and interval are separate files: opening the lock truncates
# it, which would reset a modification time read back from the lock itself.
scan_lock="$cache_root/.prune.lock"
last_scan="$cache_root/.last-prune"
exec {scan_fd}>"$scan_lock"
if ! flock --nonblock "$scan_fd"; then
  log "another prune holds the scan lock, skipping the orphan scan"
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
      if [[ ! -e "$slot/$lock_name" && ! -e "$slot/$stamp_name" ]]; then
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
    else
      rm -rf "$slot"
    fi
    release
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

# A prune failure must not red a green build. Report it loudly instead: a
# silent skip here is how the volume fills up without anyone noticing.
if ((failed)); then
  echo "prune-ci-cache: the own-slot sweep failed, see above" >&2
fi
exit 0

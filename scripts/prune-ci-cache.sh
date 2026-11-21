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
# job's target/ and an `.in-use.lock` file. While a job's before_script and
# script run, the job holds a SHARED flock on its slot's lock (taken in the
# .cargo-host-cache before_script, on a file descriptor of the job shell). Any
# number of holders can share it; this script only ever takes it EXCLUSIVELY
# and without waiting, so it can never touch a slot a job is using.
#
# Two rules, for two kinds of garbage:
#
#   1. Stale artifacts inside the job's OWN slot. `cargo sweep --maxsize`
#      evicts oldest-first, but only while the slot is over the ceiling. An age
#      rule does not work here: cargo never updates an artifact's modification
#      time when it reuses it, so the oldest files in a warm slot are the stable
#      dependencies worth keeping. Only $CARGO_TARGET_DIR is swept -- never
#      another job's slot, which may be mid-build or mid-test -- at most once
#      per CI_CACHE_INTERVAL per slot, and only if the exclusive lock is free
#      (by after_script the job shell that held the shared lock has exited).
#   2. Slots nothing uses any more: left by a renamed job, a reduced
#      concurrency limit, or a job that died before cleaning up, including
#      half-created slots with no target/. `--maxsize` never reclaims one,
#      because an abandoned slot sits under the ceiling forever. A slot is
#      removed whole only when BOTH its age exceeds CI_CACHE_ORPHAN_DAYS (the
#      target/.last-used stamp each job refreshes; the slot directory's own
#      mtime when it has no target/) AND the exclusive lock is free. Otherwise
#      it is skipped and logged. This scan runs once per host per
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

cache_root="${CI_CACHE_ROOT:-/ci-cache}"
max_size="${CI_CACHE_MAX_SIZE:-50GiB}"
orphan_days="${CI_CACHE_ORPHAN_DAYS:-14}"
interval="${CI_CACHE_INTERVAL:-86400}"

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
      if ((dry_run)); then
        log "(dry run) would run cargo sweep --maxsize $max_size"
      elif CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
        cargo sweep --maxsize "$max_size" "${sweep_args[@]}" "$project_dir"; then
        touch "$own_slot/$sweep_name"
      else
        failed=1
      fi
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
      # Half-created or abandoned: no target/, so no stamp to read.
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

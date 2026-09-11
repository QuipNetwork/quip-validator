#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/prune-ci-cache.sh [extra cargo-sweep args]
#
# Prunes the host cache volume that .cargo-host-cache mounts at /ci-cache.
# See docs/ci-cache.md for the volume layout and the host setup.
#
# The jobs that fill this volume land on every host in the amd64 pool, so the
# prune rides along with them instead of running as a scheduled job. A
# scheduled job only ever reaches one runner.
#
# Two kinds of garbage need two different rules:
#
#   1. Stale artifacts inside a slot that is still in use. `cargo sweep
#      --maxsize` evicts oldest-first, but only while that slot is over the
#      ceiling. An age rule does not work here: cargo never updates an
#      artifact's modification time when it reuses it, so the oldest files in
#      a warm slot are the stable dependencies worth keeping.
#   2. A slot nothing writes to any more, left by a renamed job or a reduced
#      concurrency limit. `--maxsize` never reclaims one, because an abandoned
#      slot sits under the ceiling forever. Each run stamps the slot it used,
#      and a slot whose stamp goes stale is removed whole.
#
# Pass --dry-run to report what would be removed without removing it. Use that
# on a host before trusting the ceiling.
#
# Environment overrides:
#   CI_CACHE_ROOT          volume root (default /ci-cache)
#   CI_CACHE_MAX_SIZE      per-slot ceiling, cargo-sweep syntax (default 50GiB)
#   CI_CACHE_ORPHAN_DAYS   age at which an unused slot is removed (default 14)
#   CI_CACHE_INTERVAL      seconds between prunes on one host (default 86400)
#   CI_CACHE_PROJECT_DIR   checkout cargo-sweep reads the manifest from
#                          (default: the repository holding this script)

cache_root="${CI_CACHE_ROOT:-/ci-cache}"
max_size="${CI_CACHE_MAX_SIZE:-50GiB}"
orphan_days="${CI_CACHE_ORPHAN_DAYS:-14}"
interval="${CI_CACHE_INTERVAL:-86400}"

stamp_name=".last-used"

dry_run=0
if [[ " $* " == *" --dry-run "* ]]; then
  dry_run=1
fi

if [[ ! -d "$cache_root" ]]; then
  echo "prune-ci-cache: $cache_root is not mounted, nothing to do"
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

# Mark the slot this job just used. Directory modification times cannot stand
# in for this: creating a file under target/debug does not touch the slot
# directory above it, so an actively built slot still looks untouched.
if ((dry_run == 0)) && [[ -n "${CARGO_TARGET_DIR:-}" ]] &&
  [[ -d "$CARGO_TARGET_DIR" ]]; then
  touch "$CARGO_TARGET_DIR/$stamp_name"
fi

# One prune per host per interval, and never two at once. A job that loses the
# lock or arrives inside the interval exits without waiting. The interval is
# tracked in its own file: opening the lock truncates it, which would reset a
# modification time read back from the lock itself.
lock="$cache_root/.prune.lock"
last_prune="$cache_root/.last-prune"
exec {lock_fd}>"$lock"
if ! flock --nonblock "$lock_fd"; then
  echo "prune-ci-cache: another prune holds the lock, skipping"
  exit 0
fi

# A dry run reports rather than changes, so the interval must not silence it.
if ((dry_run == 0)) && [[ -f "$last_prune" ]] &&
  (($(date +%s) - $(stat -c %Y "$last_prune") < interval)); then
  echo "prune-ci-cache: pruned less than ${interval}s ago, skipping"
  exit 0
fi

failed=0

for slot in "$cache_root"/*/*; do
  target="$slot/target"
  [[ -d "$target" ]] || continue

  # A slot that predates this script carries no stamp. Grandfather it in
  # rather than reading a missing stamp as "unused" — that would delete every
  # warm cache on the volume the first time this runs.
  stamp="$target/$stamp_name"
  if [[ ! -e "$stamp" ]]; then
    ((dry_run)) || touch "$stamp"
    continue
  fi

  if [[ -z "$(find "$stamp" -mtime "-$orphan_days" 2>/dev/null)" ]]; then
    echo "prune-ci-cache: removing unused slot $slot"
    if ((dry_run)); then
      du -sh "$slot"
    else
      rm -rf "$slot"
    fi
    continue
  fi

  ((sweep)) || continue

  echo "prune-ci-cache: sweeping $target to $max_size"
  CARGO_TARGET_DIR="$target" \
    cargo sweep --maxsize "$max_size" "$@" "$project_dir" || failed=1
done

# Record the interval only on a run that reached the end. after_script has its
# own runner timeout, and a prune killed part way through must be retried by
# the next job rather than counted as done. The lock is what stops those
# retries from piling up.
((dry_run)) || touch "$last_prune"

# A prune failure must not red a green build. Report it loudly instead: a
# silent skip here is how the volume fills up without anyone noticing.
if ((failed)); then
  echo "prune-ci-cache: one or more sweeps failed, see above" >&2
fi
exit 0

# shellcheck shell=bash
# Sourced (not run) by the .cargo-host-cache before_script:
#
#   . ./scripts/ci-cache-lock.sh
#
# Takes a SHARED flock on this job's cache slot lock, $slot/.in-use, on file
# descriptor 9 of the job shell. GitLab runs before_script and script in one
# shell, so the lock is held until the script ends, including on failure or
# cancellation (the container dies and the kernel releases it); after_script
# runs in a new shell without it, so the job's own prune can still sweep the
# slot. A fixed fd rather than bash's {var}> form, so this also works if the
# runner falls back to sh.
#
# `.in-use` and the flags (`--shared --wait 1200`) are main's (c63fd2f), so
# the prune of every branch that carries a lock sees every holder on the same
# slot paths. scripts/prune-ci-cache.sh only ever takes this lock
# EXCLUSIVELY and without waiting; a job that finds it held waits up to
# CI_CACHE_LOCK_WAIT seconds (default 1200) and then fails, as main's does.
#
# This branch's prune never removes or renames a slot directory or its
# `.in-use` (it evicts target/ only), so the locked file is always the live
# one. The inode check guards against anything else that replaces the file --
# main's prune, for instance, deletes a whole slot -- by retrying on a fresh
# slot. A job that cannot lock its live slot in three attempts fails rather
# than build into an unprotected slot.

ci_cache_slot="${CARGO_TARGET_DIR%/target}"
ci_cache_locked=0
for ci_cache_attempt in 1 2 3; do
  mkdir -p "$CARGO_TARGET_DIR"
  exec 9>>"$ci_cache_slot/.in-use"
  if ! flock --shared --wait "${CI_CACHE_LOCK_WAIT:-1200}" 9; then
    echo "ERROR: a cache prune held $ci_cache_slot for ${CI_CACHE_LOCK_WAIT:-1200}s" >&2
    exit 1
  fi
  if [ "$(stat -L -c %d:%i /dev/fd/9)" = "$(stat -c %d:%i "$ci_cache_slot/.in-use" 2>/dev/null)" ]; then
    ci_cache_locked=1
    break
  fi
  exec 9>&-
  echo "cache slot $ci_cache_slot was replaced while locking (attempt $ci_cache_attempt), retrying"
done
if [ "$ci_cache_locked" != 1 ]; then
  echo "ERROR: could not lock cache slot $ci_cache_slot in 3 attempts" >&2
  exit 1
fi
# Stamp the slot at job start: rule 2 and rule 3 of the prune read this stamp.
# (Main's template does not stamp at start; its jobs are protected by the
# lock alone.)
touch "$CARGO_TARGET_DIR/.last-used"
echo "cache slot $ci_cache_slot: holding shared in-use lock on fd 9"

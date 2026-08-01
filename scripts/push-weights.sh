#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/push-weights.sh
#
# CI companion to run-benchmarks.sh: commits regenerated weight files back to
# the merge request's source branch as the bench bot. Only meaningful inside
# the `benchmark-weights` CI job (merge_request_event pipelines on this repo);
# exits early when there is nothing to push.
#
# Safety properties:
#   * No-op when the benchmarks produced no weight changes.
#   * Aborts (job fails, nothing pushed) if the source branch moved while the
#     benchmarks ran — re-run the job on the new head instead.
#   * The bot commit intentionally does NOT carry [skip ci]: the retriggered
#     pipeline is the compile check for the regenerated files. No trigger
#     loop: the bench job in that new pipeline is `when: manual` again.
#
# Required environment (CI-provided except the token):
#   BENCH_PUSH_TOKEN                        project access token (bench-bot,
#                                           write_repository), masked CI var
#   CI_MERGE_REQUEST_SOURCE_BRANCH_NAME / CI_SERVER_HOST / CI_PROJECT_PATH

if git diff --quiet -- \
  'pallets/*/src/weights.rs' \
  'pallets/*/src/benchmark_weights.rs'; then
  echo "No weight changes — nothing to push."
  exit 0
fi

: "${BENCH_PUSH_TOKEN:?BENCH_PUSH_TOKEN not set (project CI/CD variable)}"
: "${CI_MERGE_REQUEST_SOURCE_BRANCH_NAME:?not in a merge_request_event pipeline}"
: "${CI_SERVER_HOST:?}" "${CI_PROJECT_PATH:?}"

git fetch origin "$CI_MERGE_REQUEST_SOURCE_BRANCH_NAME"

# In branch pipelines CI_COMMIT_SHA is the source-branch head; in merged-results
# pipelines it is an ephemeral merge commit and the branch head is exposed as
# CI_MERGE_REQUEST_SOURCE_BRANCH_SHA instead.
expected="${CI_MERGE_REQUEST_SOURCE_BRANCH_SHA:-$CI_COMMIT_SHA}"
actual="$(git rev-parse FETCH_HEAD)"
if [ "$actual" != "$expected" ]; then
  echo "ERROR: source branch moved during the benchmark run" >&2
  echo "  benchmarked: $expected" >&2
  echo "  branch now:  $actual" >&2
  echo "Re-run the benchmark job on the new head." >&2
  exit 1
fi

# Move onto the real branch ref; the regenerated weights ride along in the
# working tree (guard above guarantees FETCH_HEAD == the benchmarked commit).
git checkout -B "$CI_MERGE_REQUEST_SOURCE_BRANCH_NAME" FETCH_HEAD

git config user.name  "quip-bench-bot"
git config user.email "ops@postquant.xyz"
git add 'pallets/*/src/weights.rs' 'pallets/*/src/benchmark_weights.rs'
git commit -m "chore: regenerate weights on reference machine"

git push \
  "https://bench-bot:${BENCH_PUSH_TOKEN}@${CI_SERVER_HOST}/${CI_PROJECT_PATH}.git" \
  "HEAD:$CI_MERGE_REQUEST_SOURCE_BRANCH_NAME"

echo "Pushed regenerated weights to $CI_MERGE_REQUEST_SOURCE_BRANCH_NAME."

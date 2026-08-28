#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   scripts/test-xqvm-onchain.sh
#
# Boots a --dev node and runs the pallet-xqvm on-chain suite against it:
# a PQ-signed store_program + execute round trip, the dispatch errors that
# guard the step and allocation budgets, and a replay of the XQuad
# conformance vectors that assert a step count.
#
# The conformance vectors are not vendored. They are read from the xquad
# repository at the tag matching the `xqvm` version this workspace pins, so
# the assertions track the toolchain instead of drifting from a copy. Set
# XQUAD_DIR to reuse an existing checkout instead of cloning.
#
# Prerequisites (the CI job installs these; see .gitlab-ci.yml):
#   make wasm-signer
#   npm ci --prefix js/quip-signer && npm run build --prefix js/quip-signer
#   cargo build -p quip-network-node

node_binary="${QUIP_NODE_BINARY:-target/debug/quip-network-node}"
node_log="$(mktemp)"
work_dir="$(mktemp -d)"
node_pid=""

cleanup() {
  if [[ -n "$node_pid" ]] && kill -0 "$node_pid" 2>/dev/null; then
    kill "$node_pid"
    wait "$node_pid" 2>/dev/null || true
  fi
  rm -rf "$node_log" "$work_dir"
}
trap cleanup EXIT

# The pinned xqvm version is the single source of truth for which toolchain
# the vectors come from: same crate, same tag, no second place to update.
xqvm_version="$(
  sed -n 's/^xqvm = { version = "\([^"]*\)".*/\1/p' Cargo.toml
)"
if [[ -z "$xqvm_version" ]]; then
  echo "ERROR: could not read the pinned xqvm version from Cargo.toml" >&2
  exit 1
fi
echo "== Pinned xqvm: ${xqvm_version} =="

if [[ -n "${XQUAD_DIR:-}" ]]; then
  xquad_dir="$XQUAD_DIR"
  echo "== Using xquad checkout at ${xquad_dir} =="
else
  xquad_dir="$work_dir/xquad"
  echo "== Cloning xquad at v${xqvm_version} =="
  git clone --quiet --depth 1 --branch "v${xqvm_version}" \
    https://gitlab.com/quip.network/xquad.git "$xquad_dir"
fi

vectors_src="$xquad_dir/conformance/vectors"
if [[ ! -d "$vectors_src" ]]; then
  echo "ERROR: no conformance vectors at $vectors_src" >&2
  exit 1
fi

echo "== Building the assembler =="
cargo build --release --manifest-path "$xquad_dir/Cargo.toml" -p xqcli
xquad_bin="$(cargo metadata --format-version 1 --no-deps \
  --manifest-path "$xquad_dir/Cargo.toml" |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release/xquad"

# Assemble every vector that pins a step count. Those are the ones whose
# expectations this suite can check through the chain: outputs and
# steps_used are what `ProgramExecuted` carries.
vectors_out="$work_dir/vectors"
mkdir -p "$vectors_out"

count=0
while IFS= read -r expected; do
  dir="$(dirname "$expected")"
  name="$(basename "$dir")"

  "$xquad_bin" asm "$dir/program.xqasm" -o "$vectors_out/$name.xqb" >/dev/null
  cp "$expected" "$vectors_out/$name.expected.json"
  cp "$dir/inputs.json" "$vectors_out/$name.inputs.json"
  count=$((count + 1))
done < <(grep -rl '"steps"' "$vectors_src" --include=expected.json)

if [[ "$count" -eq 0 ]]; then
  echo "ERROR: no step-asserting vectors found — did the vector format change?" >&2
  exit 1
fi
echo "== Assembled $count step-asserting vectors =="

# Control programs for the budget guards. Both are statically valid, so
# they store; what must stop them is the runtime bound, not the verifier.
cat >"$work_dir/runaway.xqasm" <<'PROGRAM'
TARGET .0
NOP
JUMP .0
PROGRAM
"$xquad_bin" asm "$work_dir/runaway.xqasm" -o "$vectors_out/runaway.xqb" >/dev/null

# One variable past the runtime's 16 MiB MaxVmMemory, at ~2.1M steps — an
# order of magnitude inside MaxStepLimit, so the allocation budget is what
# has to refuse it.
cat >"$work_dir/alloc.xqasm" <<'PROGRAM'
PUSH 2097153
BSMX r0
HALT
PROGRAM
"$xquad_bin" asm "$work_dir/alloc.xqasm" -o "$vectors_out/alloc_over_budget.xqb" >/dev/null

echo "== Starting the node =="
"$node_binary" --dev --tmp --no-prometheus >"$node_log" 2>&1 &
node_pid="$!"

ready=false
for _ in $(seq 1 120); do
  if ! kill -0 "$node_pid" 2>/dev/null; then
    cat "$node_log"
    echo "ERROR: node exited during startup" >&2
    exit 1
  fi

  response="$(curl --silent --show-error \
    --header 'content-type: application/json' \
    --data '{"id":1,"jsonrpc":"2.0","method":"system_health","params":[]}' \
    http://127.0.0.1:9944 2>/dev/null || true)"

  if [[ "$response" == *'"result"'* ]]; then
    ready=true
    break
  fi

  sleep 1
done

if [[ "$ready" != true ]]; then
  cat "$node_log"
  echo "ERROR: node did not become ready" >&2
  exit 1
fi

echo "== Running the on-chain suite =="
if ! QUIP_WS_URL=ws://127.0.0.1:9944 \
  VECTORS_DIR="$vectors_out" \
  QUIP_REPO="$PWD" \
  node js/quip-signer/test/xqvm-onchain-smoke.mjs; then
  echo "--- node log ---"
  cat "$node_log"
  exit 1
fi

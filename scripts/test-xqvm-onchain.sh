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
#
# In CI this runs inside browser-signer-test, which already performs all of
# the above for its own suite; see .gitlab-ci.yml for why they share a job.

# CI sets CARGO_TARGET_DIR to a host volume outside the project dir, so the
# node is resolved through it, the same way test-browser-signer-integration.sh
# does; a plain `cargo build` on a laptop lands in target/ as before.
node_binary="${QUIP_NODE_BINARY:-${CARGO_TARGET_DIR:-target}/debug/quip-network-node}"
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

# The pinned xqvm version names the toolchain the vectors come from: same
# crate, same tag. The requirement is an exact pin (`=0.5.0`), so the
# leading `=` is stripped to get the version the tag is named after.
xqvm_version="$(
  sed -n 's/^xqvm = { version = "=\{0,1\}\([^"]*\)".*/\1/p' Cargo.toml
)"
if [[ -z "$xqvm_version" ]]; then
  echo "ERROR: could not read the pinned xqvm version from Cargo.toml" >&2
  exit 1
fi
echo "== Pinned xqvm: ${xqvm_version} =="

# A tag can be moved; a commit cannot. This is the commit crates.io records
# for the published xqvm ${xqvm_version} crate (its `.cargo_vcs_info.json`),
# so the vectors are asserted to come from exactly the source the pallet
# links against. Update it together with the version pin in Cargo.toml.
xquad_commit="a99f7d2e8956b70453dc6d5e4c1b1705a27d9802"

if [[ -n "${XQUAD_DIR:-}" ]]; then
  xquad_dir="$XQUAD_DIR"
  echo "== Using xquad checkout at ${xquad_dir} =="
else
  xquad_dir="$work_dir/xquad"
  echo "== Cloning xquad at v${xqvm_version} =="
  git clone --quiet --depth 1 --branch "v${xqvm_version}" \
    https://gitlab.com/quip.network/xquad.git "$xquad_dir"
fi

# Checked for an existing checkout too: the vectors come from whatever HEAD
# is, and a stale or experimental xquad tree would make the suite assert
# against the wrong toolchain while reporting success.
actual_commit="$(git -C "$xquad_dir" rev-parse HEAD)"
if [[ "$actual_commit" != "$xquad_commit" ]]; then
  echo "ERROR: xquad checkout at ${xquad_dir} is ${actual_commit}," \
    "expected ${xquad_commit} (v${xqvm_version})" >&2
  exit 1
fi

xquad_dir="$(cd "$xquad_dir" && pwd)"
vectors_src="$xquad_dir/xqvm/tests/vectors"
if [[ ! -d "$vectors_src" ]]; then
  echo "ERROR: no conformance vectors at $vectors_src" >&2
  exit 1
fi

echo "== Building the assembler =="
# The 0.5.0 toolchain removed xqcli. Use its assembler library directly in
# a temporary helper so this gate still needs only Cargo, not a Python CLI
# environment. The path points at the exact source commit checked above.
assembler_dir="$work_dir/assembler"
mkdir -p "$assembler_dir/src"
# Escape the path for a TOML basic string (including spaces/quoted paths).
assembler_path="${xquad_dir//\\/\\\\}/xqasm"
assembler_path="${assembler_path//\"/\\\"}"
cat >"$assembler_dir/Cargo.toml" <<MANIFEST
[package]
name = "quip-xqvm-assemble"
version = "0.0.0"
edition = "2024"
publish = false
[workspace]
[dependencies]
xqasm = { path = "$assembler_path" }
MANIFEST
cat >"$assembler_dir/src/main.rs" <<'RUST'
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    assert_eq!(args.len(), 3, "usage: quip-xqvm-assemble INPUT OUTPUT");
    let source = std::fs::read_to_string(&args[1]).expect("read assembly");
    let program = xqasm::assemble_source(&source).expect("assemble program");
    std::fs::write(&args[2], program.encode()).expect("write bytecode");
}
RUST
cargo build --manifest-path "$assembler_dir/Cargo.toml" \
  --target-dir "$assembler_dir/target"
xquad_bin="$assembler_dir/target/debug/quip-xqvm-assemble"

# Assemble every vector that pins a step count. Those are the ones whose
# expectations this suite can check through the chain: outputs and
# steps_used are what `ProgramExecuted` carries.
vectors_out="$work_dir/vectors"
mkdir -p "$vectors_out"

count=0
while IFS= read -r expected; do
  dir="$(dirname "$expected")"
  name="$(basename "$dir")"

  "$xquad_bin" "$dir/program.xqasm" "$vectors_out/$name.xqb" >/dev/null
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
"$xquad_bin" "$work_dir/runaway.xqasm" "$vectors_out/runaway.xqb" >/dev/null

# One variable past the runtime's 16 MiB MaxVmMemory, at ~2.1M steps — an
# order of magnitude inside MaxStepLimit, so the allocation budget is what
# has to refuse it.
cat >"$work_dir/alloc.xqasm" <<'PROGRAM'
PUSH 2097153
BSMX r0
HALT
PROGRAM
"$xquad_bin" "$work_dir/alloc.xqasm" "$vectors_out/alloc_over_budget.xqb" >/dev/null

echo "== Starting the node =="
"$node_binary" --dev --tmp --no-prometheus >"$node_log" 2>&1 &
node_pid="$!"

# 300s, not the 120s a laptop needs. A CI runner starts this node while
# still compiling the rest of the pipeline, and the node lost that race on
# !75: it had printed only its startup banner when the window closed.
ready=false
for _ in $(seq 1 300); do
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

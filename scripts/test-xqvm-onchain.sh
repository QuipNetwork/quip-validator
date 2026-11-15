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
# repository at the commit resolved by the locked `xqvm` dependency, so
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

# Resolve both draft git pins and their eventual registry release replacement.
# Registry provenance comes from the published crate's .cargo_vcs_info.json;
# no manually maintained commit constant can silently outlive a version bump.
cargo metadata --locked --format-version 1 > "$work_dir/cargo-metadata.json"
source_info="$(node scripts/resolve-xqvm-source.mjs "$work_dir/cargo-metadata.json")"
IFS=$'\t' read -r xqvm_version xquad_repository xquad_commit xquad_ref <<< "$source_info"
echo "== Pinned xqvm: ${xqvm_version} at ${xquad_commit} =="

if [[ -n "${XQUAD_DIR:-}" ]]; then
  xquad_dir="$XQUAD_DIR"
  echo "== Using xquad checkout at ${xquad_dir} =="
else
  xquad_dir="$work_dir/xquad"
  echo "== Fetching xquad at ${xquad_ref} =="
  git init --quiet "$xquad_dir"
  git -C "$xquad_dir" remote add origin "$xquad_repository"
  git -C "$xquad_dir" fetch --quiet --depth 1 origin "$xquad_ref"
  git -C "$xquad_dir" checkout --quiet --detach FETCH_HEAD
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

vectors_src="$xquad_dir/xqvm/tests/vectors"
if [[ ! -d "$vectors_src" ]]; then
  echo "ERROR: no conformance vectors at $vectors_src" >&2
  exit 1
fi

# Use a disposable copy even with XQUAD_DIR: build only committed source and
# never add the assembly adapter to a caller's checkout.
if [[ -n "${XQUAD_DIR:-}" ]]; then
  mkdir -p "$work_dir/xquad"
  git -C "$xquad_dir" archive "$xquad_commit" | tar -x -C "$work_dir/xquad"
  xquad_dir="$work_dir/xquad"
  vectors_src="$xquad_dir/xqvm/tests/vectors"
fi

echo "== Building the assembler =="
# XQuad no longer ships xqcli. Add a tiny adapter to the disposable checkout
# so its assembler and lockfile supply the exact bytecode used by the vectors.
mkdir -p "$xquad_dir/xqasm/examples"
cat > "$xquad_dir/xqasm/examples/quip_onchain_assemble.rs" <<'RUST'
//! Adapter for validator conformance: assemble and check the chain memory budget.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or("missing mode")?;
    let input = args.next().ok_or("missing input path")?;
    if mode == "assemble" {
        let output = args.next().ok_or("missing output path")?;
        let program = xqasm::assemble_source(&std::fs::read_to_string(input)?)?;
        std::fs::write(output, program.encode())?;
    } else if mode == "check-memory" {
        let inputs_path = args.next().ok_or("missing vector inputs")?;
        let memory_limit: u64 = args.next().ok_or("missing memory limit")?.parse()?;
        let step_limit: u64 = args.next().ok_or("missing step limit")?.parse()?;
        let inputs: serde_json::Value = serde_json::from_slice(&std::fs::read(inputs_path)?)?;
        let calldata: Vec<i64> = serde_json::from_value(inputs["calldata"].clone())?;
        let output_slots = inputs["output_slots"].as_u64().unwrap_or(16);
        let program = xqvm::Program::decode(&std::fs::read(input)?)?;
        let mut vm = xqvm::Vm::new();
        vm.set_memory_limit(memory_limit)
            .set_step_limit(step_limit)
            .set_output_slots(usize::try_from(output_slots)?)
            .set_calldata(calldata.into_iter().map(xqvm::RegVal::Int).collect());
        // Successful vectors can exceed the chain's smaller memory budget.
        // Only that fault is an accepted change from their recorded success.
        let outcome = match vm.run(&program) {
            Ok(()) => "ok",
            Err(xqvm::Error::MemoryLimitExceeded { .. }) => "memory-limit",
            Err(error) => return Err(error.into()),
        };
        use std::io::Write;
        writeln!(std::io::stdout(), "{outcome}")?;
    } else {
        return Err("unknown mode".into());
    }
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    Ok(())
}
RUST
xquad_target="$work_dir/xquad-target"
CARGO_TARGET_DIR="$xquad_target" cargo build --locked --release --manifest-path "$xquad_dir/Cargo.toml" -p xqasm --example quip_onchain_assemble
xquad_bin="$xquad_target/release/examples/quip_onchain_assemble"

# Assemble every vector that pins a step count. Those are the ones whose
# expectations this suite can check through the chain: outputs and
# steps_used are what `ProgramExecuted` carries.
vectors_out="$work_dir/vectors"
mkdir -p "$vectors_out"

count=0
while IFS= read -r expected; do
  dir="$(dirname "$expected")"
  name="$(basename "$dir")"

  "$xquad_bin" assemble "$dir/program.xqasm" "$vectors_out/$name.xqb" >/dev/null
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
"$xquad_bin" assemble "$work_dir/runaway.xqasm" "$vectors_out/runaway.xqb" >/dev/null

# One variable past the runtime's 16 MiB MaxVmMemory, at ~2.1M steps — an
# order of magnitude inside MaxStepLimit, so the allocation budget is what
# has to refuse it.
cat >"$work_dir/alloc.xqasm" <<'PROGRAM'
PUSH 2097153
BSMX r0
HALT
PROGRAM
"$xquad_bin" assemble "$work_dir/alloc.xqasm" "$vectors_out/alloc_over_budget.xqb" >/dev/null

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
  XQVM_REFERENCE_BINARY="$xquad_bin" \
  QUIP_REPO="$PWD" \
  node js/quip-signer/test/xqvm-onchain-smoke.mjs; then
  echo "--- node log ---"
  cat "$node_log"
  exit 1
fi

#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_ROOT="${QUIP_LOCAL3_DIR:-${TMPDIR:-/tmp}/quip-local3}"
LOG_DIR="${STATE_ROOT}/logs"
BIN="${ROOT_DIR}/target/debug/quip-network-node"

NODE1_KEY="000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
NODE2_KEY="1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100"
NODE3_KEY="f0e1d2c3b4a5968778695a4b3c2d1e0ff1e2d3c4b5a69788796a5b4c3d2e1f00"
NODE1_PEER_ID="12D3KooWA4Xop1JaT3MHxwYMkCepYsv4iPVopMXwCz5iHYdBfeSB"
BOOTNODE="/ip4/127.0.0.1/tcp/30333/p2p/${NODE1_PEER_ID}"

mkdir -p "${LOG_DIR}" \
  "${STATE_ROOT}/node1" \
  "${STATE_ROOT}/node2" \
  "${STATE_ROOT}/node3"

cleanup() {
  local exit_code=$?
  if [[ -n "${TAIL_PID:-}" ]]; then
    kill "${TAIL_PID}" 2>/dev/null || true
  fi
  for pid in "${NODE1_PID:-}" "${NODE2_PID:-}" "${NODE3_PID:-}"; do
    if [[ -n "${pid}" ]]; then
      kill "${pid}" 2>/dev/null || true
    fi
  done
  wait || true
  exit "${exit_code}"
}

trap cleanup INT TERM EXIT

echo "Building quip-network-node..."
cargo build -p quip-network-node --manifest-path "${ROOT_DIR}/Cargo.toml"

echo "Logs will be written to ${LOG_DIR}"
echo "State directories live under ${STATE_ROOT}"

"${BIN}" \
  --chain local3 \
  --base-path "${STATE_ROOT}/node1" \
  --node-key "${NODE1_KEY}" \
  --listen-addr /ip4/127.0.0.1/tcp/30333 \
  --rpc-port 9944 \
  --prometheus-port 9615 \
  --alice \
  --validator \
  > "${LOG_DIR}/node1.log" 2>&1 &
NODE1_PID=$!

sleep 2

"${BIN}" \
  --chain local3 \
  --base-path "${STATE_ROOT}/node2" \
  --node-key "${NODE2_KEY}" \
  --listen-addr /ip4/127.0.0.1/tcp/30334 \
  --rpc-port 9945 \
  --prometheus-port 9616 \
  --bootnodes "${BOOTNODE}" \
  --bob \
  --validator \
  > "${LOG_DIR}/node2.log" 2>&1 &
NODE2_PID=$!

"${BIN}" \
  --chain local3 \
  --base-path "${STATE_ROOT}/node3" \
  --node-key "${NODE3_KEY}" \
  --listen-addr /ip4/127.0.0.1/tcp/30335 \
  --rpc-port 9946 \
  --prometheus-port 9617 \
  --bootnodes "${BOOTNODE}" \
  --charlie \
  --validator \
  --unsafe-rpc-external \
  > "${LOG_DIR}/node3.log" 2>&1 &
NODE3_PID=$!

echo "Started local3 network:"
echo "  node1 pid=${NODE1_PID} rpc=9944 p2p=30333 log=${LOG_DIR}/node1.log"
echo "  node2 pid=${NODE2_PID} rpc=9945 p2p=30334 log=${LOG_DIR}/node2.log"
echo "  node3 pid=${NODE3_PID} rpc=9946 p2p=30335 log=${LOG_DIR}/node3.log"
echo
echo "Press Ctrl-C to stop all nodes."

tail -F \
  "${LOG_DIR}/node1.log" \
  "${LOG_DIR}/node2.log" \
  "${LOG_DIR}/node3.log" &
TAIL_PID=$!

wait "${NODE1_PID}" "${NODE2_PID}" "${NODE3_PID}"

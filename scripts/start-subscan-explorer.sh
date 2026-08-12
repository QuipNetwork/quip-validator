#!/usr/bin/env bash
set -euo pipefail

# Starts the local Substrate development explorer stack (Subscan spike):
#   1. quip-network-node --dev running NATIVELY (debug build with dev-chain-id,
#      archive pruning so subscan-essentials can backfill history)
#   2. subscan-essentials in Docker (MySQL + Redis + API :4399 + subscribe +
#      worker), indexing directly from the node's websocket RPC
#   3. the subscan-essentials React UI in Docker (http://localhost:3100)
#
# This stack is independent from the Blockscout EVM explorer
# (scripts/start-evm-explorer.sh): no eth-rpc sidecar, different ports.
#
# Runs attached: Ctrl-C stops the node and the containers. Chain state lives
# under STATE_ROOT; the subscan MySQL/Redis volumes persist across runs until
# `make subscan-explorer-down` removes them.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_ROOT="${QUIP_SUBSCAN_DEV_DIR:-${TMPDIR:-/tmp}/quip-subscan-dev}"
LOG_DIR="${STATE_ROOT}/logs"
BIN="${ROOT_DIR}/target/debug/quip-network-node"
COMPOSE_FILE="${ROOT_DIR}/docker-compose.subscan-explorer.yml"

NODE_RPC_PORT="${QUIP_NODE_RPC_PORT:-9944}"
SUBSCAN_API_PORT="${QUIP_SUBSCAN_API_PORT:-4399}"
SUBSCAN_UI_PORT="${QUIP_SUBSCAN_UI_PORT:-3100}"

mkdir -p "${LOG_DIR}" "${STATE_ROOT}/node"

cleanup() {
  local exit_code=$?
  for pid in "${TAIL_PID:-}" "${LOGS_PID:-}"; do
    if [[ -n "${pid}" ]]; then
      kill "${pid}" 2>/dev/null || true
    fi
  done
  if [[ -n "${NODE_PID:-}" ]]; then
    kill "${NODE_PID}" 2>/dev/null || true
  fi
  if [[ -n "${COMPOSE_STARTED:-}" ]]; then
    docker compose -f "${COMPOSE_FILE}" stop >/dev/null 2>&1 || true
  fi
  wait || true
  exit "${exit_code}"
}

trap cleanup INT TERM EXIT

echo "Building quip-network-node (debug, dev-chain-id)..."
cargo build -p quip-network-node --features dev-chain-id --manifest-path "${ROOT_DIR}/Cargo.toml"

echo "Logs will be written to ${LOG_DIR}"
echo "Chain state lives under ${STATE_ROOT}/node"

"${BIN}" \
  --dev \
  --base-path "${STATE_ROOT}/node" \
  --state-pruning=archive \
  --blocks-pruning=archive \
  --rpc-port "${NODE_RPC_PORT}" \
  --unsafe-rpc-external \
  --rpc-methods=unsafe \
  --rpc-cors=all \
  > "${LOG_DIR}/node.log" 2>&1 &
NODE_PID=$!

echo "Waiting for the node RPC on port ${NODE_RPC_PORT}..."
until curl --fail --silent \
  --header 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","method":"system_chain","params":[],"id":1}' \
  "http://127.0.0.1:${NODE_RPC_PORT}" > /dev/null 2>&1; do
  if ! kill -0 "${NODE_PID}" 2>/dev/null; then
    echo "Node exited during startup; see ${LOG_DIR}/node.log" >&2
    exit 1
  fi
  sleep 1
done

echo "Starting subscan-essentials (builds images from pinned commits on first run)..."
docker compose -f "${COMPOSE_FILE}" up -d --build --wait
COMPOSE_STARTED=1

echo
echo "Subscan dev stack is up:"
echo "  node pid=${NODE_PID} substrate-rpc=ws://localhost:${NODE_RPC_PORT} log=${LOG_DIR}/node.log"
echo "  subscan api=http://localhost:${SUBSCAN_API_PORT} ui=http://localhost:${SUBSCAN_UI_PORT}"
echo
echo "Press Ctrl-C to stop the node and all containers."

tail -F "${LOG_DIR}/node.log" &
TAIL_PID=$!

docker compose -f "${COMPOSE_FILE}" logs -f &
LOGS_PID=$!

wait "${NODE_PID}"

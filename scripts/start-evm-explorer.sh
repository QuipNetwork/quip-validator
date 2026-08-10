#!/usr/bin/env bash
set -euo pipefail

# Starts the local EVM development stack:
#   1. quip-network-node --dev running NATIVELY (debug build with dev-chain-id,
#      Chain ID 1337, archive pruning so the sidecar and Blockscout can
#      backfill history)
#   2. the pinned pallet-revive Ethereum JSON-RPC sidecar in Docker
#      (http://localhost:8545)
#   3. Blockscout in Docker (UI http://localhost:3000, API :4000), indexing
#      from the sidecar
#
# Runs attached: Ctrl-C stops the node and the containers. Chain state lives
# under STATE_ROOT; Blockscout's Postgres volume persists across runs until
# `make evm-explorer-down` removes it.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_ROOT="${QUIP_EVM_DEV_DIR:-${TMPDIR:-/tmp}/quip-evm-dev}"
LOG_DIR="${STATE_ROOT}/logs"
BIN="${ROOT_DIR}/target/debug/quip-network-node"
COMPOSE_FILE="${ROOT_DIR}/docker-compose.evm-explorer.yml"

NODE_RPC_PORT="${QUIP_NODE_RPC_PORT:-9944}"
ETH_RPC_PORT="${QUIP_ETH_RPC_PORT:-8545}"
BLOCKSCOUT_PORT="${QUIP_BLOCKSCOUT_PORT:-3000}"
BLOCKSCOUT_API_PORT="${QUIP_BLOCKSCOUT_API_PORT:-4000}"

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

echo "Starting eth-rpc sidecar and Blockscout..."
docker compose -f "${COMPOSE_FILE}" up -d --wait
COMPOSE_STARTED=1

echo
echo "EVM dev stack is up:"
echo "  node pid=${NODE_PID} substrate-rpc=ws://localhost:${NODE_RPC_PORT} log=${LOG_DIR}/node.log"
echo "  ethereum-rpc=http://localhost:${ETH_RPC_PORT} (EIP-155 chain id 1337)"
echo "  blockscout ui=http://localhost:${BLOCKSCOUT_PORT} api=http://localhost:${BLOCKSCOUT_API_PORT}"
echo
echo "Press Ctrl-C to stop the node and all containers."

tail -F "${LOG_DIR}/node.log" &
TAIL_PID=$!

docker compose -f "${COMPOSE_FILE}" logs -f &
LOGS_PID=$!

wait "${NODE_PID}"

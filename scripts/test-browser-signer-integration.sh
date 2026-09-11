#!/usr/bin/env bash
set -euo pipefail

# CI sets CARGO_TARGET_DIR to a host volume outside the project dir, so the
# binary is not under ./target. Honour it for the default; an explicit
# QUIP_NODE_BINARY still wins.
node_binary="${QUIP_NODE_BINARY:-${CARGO_TARGET_DIR:-target}/debug/quip-network-node}"
node_log="$(mktemp)"
node_pid=""

# Sandboxed macOS shells may not expose the login keychain to
# rustls-native-certs even though the system PEM bundle is readable.
if [[ -z "${SSL_CERT_FILE:-}" ]] &&
  [[ "$(uname -s)" == "Darwin" ]] &&
  [[ -f /etc/ssl/cert.pem ]]; then
  export SSL_CERT_FILE=/etc/ssl/cert.pem
fi

cleanup() {
  if [[ -n "$node_pid" ]] && kill -0 "$node_pid" 2>/dev/null; then
    kill "$node_pid"
    wait "$node_pid" 2>/dev/null || true
  fi
  rm -f "$node_log"
}
trap cleanup EXIT

"$node_binary" --dev --tmp --no-prometheus >"$node_log" 2>&1 &
node_pid="$!"

# The node builds genesis and sets up the wasm runtime before it answers RPC.
# In a debug build, on a CI runner also hosting cargo-test, that has been
# measured at 106 seconds against a bound that allowed only 120.
#
# That bound counted *iterations*, not seconds: each pass costs a sleep plus
# however long curl takes to fail, so the real budget shrank precisely when
# the machine was slow. A wall-clock deadline says what it means. 300s leaves
# headroom over the observed worst case while still failing a node that is
# genuinely hung rather than merely slow.
ready=false
deadline=$((SECONDS + 300))
while ((SECONDS < deadline)); do
  if ! kill -0 "$node_pid" 2>/dev/null; then
    cat "$node_log"
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
  exit 1
fi

npm run test:integration --prefix js/quip-signer

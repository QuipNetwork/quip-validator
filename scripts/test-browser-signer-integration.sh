#!/usr/bin/env bash
set -euo pipefail

node_binary="${QUIP_NODE_BINARY:-target/debug/quip-network-node}"
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

ready=false
for _ in $(seq 1 120); do
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

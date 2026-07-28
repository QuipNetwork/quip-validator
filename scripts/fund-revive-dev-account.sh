#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
helper="$repo_root/js/quip-signer/test/revive-fund-account.mjs"
signer_dist="$repo_root/js/quip-signer/dist/index.js"
wasm_js="$repo_root/js/quip-transaction-crypto-wasm/quip_transaction_crypto_wasm.js"
wasm_binary="$repo_root/js/quip-transaction-crypto-wasm/quip_transaction_crypto_wasm_bg.wasm"

if (($# != 0)); then
  echo "usage: scripts/fund-revive-dev-account.sh" >&2
  echo "configure endpoints with QUIP_WS_URL, QUIP_HTTP_URL, and REVIVE_RPC_URL" >&2
  exit 2
fi

if ! command -v node >/dev/null 2>&1; then
  echo "node is required to run the revive funding helper" >&2
  exit 1
fi

for required_file in "$helper" "$signer_dist" "$wasm_js" "$wasm_binary"; do
  if [[ ! -f "$required_file" ]]; then
    echo "required generated file is missing: $required_file" >&2
    echo "run 'make wasm-signer' and 'npm run build --prefix js/quip-signer'" >&2
    exit 1
  fi
done

export QUIP_WS_URL="${QUIP_WS_URL:-ws://127.0.0.1:9944}"
export QUIP_HTTP_URL="${QUIP_HTTP_URL:-http://127.0.0.1:9944}"
export REVIVE_RPC_URL="${REVIVE_RPC_URL:-${ETH_RPC_URL:-http://127.0.0.1:8545}}"

exec node "$helper"

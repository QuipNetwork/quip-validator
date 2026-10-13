# Metadata consumer fixtures

These are shared inputs for QUI-901's Rust, Apps and REST conformance tests.
They are fixture evidence, not a claim that every consumer already passes.

| Directory | Execution runtime | Cases |
| --- | --- | --- |
| `aglais-117-118` | Archived production 117 and 118, tx 7 | Upgrade block 330109, first following block 330110, transfers 591863 and 591865 |
| `local-119` | Non-benchmark runtime 119, tx 7 | Finalized development-chain transfer from the checked-in H4 fixture account |

Each manifest records the genesis, canonical block hash, parent hash, execution
and post-state runtime versions, runtime code hashes, build provenance where
known, metadata version, every captured file's SHA-256 and exact RPC query.
SCALE files contain raw bytes: metadata starts with `meta` followed by `0x10`;
events contain the encoded `System.Events` vector. They have no RPC hex or
`OpaqueMetadata` length wrapper. JSON files contain the RPC result only.

Metadata is queried at **parent(block)**. Block 330109 installs runtime 118 but
executes under 117; its events and extrinsics therefore use 117 metadata. Block
330110 uses 118. The real boundary blocks also happen to decode under both
versions; a negative wrong-metadata test must exercise a changed call (118 adds
`Xqvm.remove_program` and `evict_program`), not assume these blocks fail.

The manifests' extrinsic expectations come from an independent, deliberately
narrow wire parser: V5 bare or V4 signed with `MultiAddress::Id`, the H4 envelope
(929 public-key bytes + 731 signature-field bytes), and current native extension
layout. They pin order, hashes, signer, era, nonce, tip, metadata-hash mode, call
indexes and bytes. Timestamp and balance-transfer values are also recorded.
Consumer tests must derive their values from metadata, not use this parser as
their decoder. The large upgrade call is retained in full in the block fixture;
its expected call and envelope are represented by hashes to avoid duplication.

The archived blocks, events and metadata were independently fetched on
2026-10-13 from the RPC URL in the manifest. Block bodies match rr's earlier
scratch captures. Their execution artifact hashes, rather than an assumed
source revision or build flags, identify the deployed runtimes. This is a small
representative subset of rr's 100-block survey, not that entire survey.

The local node was built at source `e5de2ddcb347565d15b75df823a116b67f5b602d`
using `SKIP_PALLET_REVIVE_FIXTURES=1 cargo build --offline -p quip-network-node`
with default features and no `runtime-benchmarks`. This is a development-chain
capture of the ordinary runtime configuration, not release packaging evidence.
Its `:code` SHA-256 equals the supplied Wasm artifact's SHA-256, and the V16 API
list has no `Benchmark`. It does not reuse rr's benchmark-built spec-119 blob.
Local hashes, timestamps, signatures and block numbers change on regeneration.
The recorded transfer is 10,000,000,000,000 base units at extrinsic index 1;
the preceding extrinsic is a bare V5 timestamp. The sender uses the seed in
[`hybrid-signing.json`](../../polkadotjs/fixtures/hybrid-signing.json).

## Verify existing fixtures

Run from the repository root; Python uses only its standard library:

```sh
python3 scripts/metadata-conformance/fixtures.py verify docs/fixtures/metadata-conformance/aglais-117-118
python3 scripts/metadata-conformance/fixtures.py verify docs/fixtures/metadata-conformance/local-119

# Optional read-only comparison against an archive RPC:
python3 scripts/metadata-conformance/fixtures.py verify docs/fixtures/metadata-conformance/aglais-117-118 \
  --rpc https://bootnode-2.aglais.quip.network:20049/rpc
```

Verification fails on missing files, changed hashes, wrong genesis, metadata
state selection, runtime versions, extrinsic expectations or RPC differences.
Local fixtures remain verifiable offline after the disposable node stops.

The stock `subxt-core 0.44.3` consumer assertions run with:

```sh
SKIP_PALLET_REVIVE_FIXTURES=1 cargo test --offline -p quip-protocol-runtime --test metadata_v16
```

They reconstruct and hash each header, decode all 25 extrinsics and every event
using the paired metadata, and check signer/public/signature bytes, call fields,
all twelve extensions, and per-extrinsic outcomes. The archived failed
`MinerRegistry.participate` calls remain in the blocks and must retain their
`ExtrinsicFailed` events. Three transfers must have matching `Balances.Transfer`
events at their own extrinsic index. Synthetic calls added in 118 exercise the
negative historical-metadata case. This is offline consumer coverage; the
online stock-subxt smoke below and Apps/REST acceptance are separate checks.

## Capture again

Capture requires a new output directory and refuses non-finalized blocks.

```sh
python3 scripts/metadata-conformance/fixtures.py capture /tmp/quip-archived-fixtures \
  --rpc https://bootnode-2.aglais.quip.network:20049/rpc \
  --blocks 330109 330110 591863 591865 --kind archived-live
```

For local regeneration, build the node and signer using the normal repository
instructions (`make wasm-signer` if the WASM signer is absent), then:

```sh
SKIP_PALLET_REVIVE_FIXTURES=1 cargo build --offline -p quip-network-node
npm run build --prefix js/quip-signer
target/debug/quip-network-node --dev --tmp --state-pruning archive \
  --rpc-port 19944 --port 30377 --no-prometheus --rpc-methods Unsafe
```

In another terminal, run the producer. It permits only a localhost Development
chain running non-benchmark runtime 119/tx 7, funds the fixture account using
the existing development Alice seed, and checks finalized success and transfer
events. It uses the existing WASM/`QuipSigner` implementation. Its V4 assembly is
producer code only; the consumer tests must not use handwritten Quip types.

```sh
QUIP_WS_URL=ws://127.0.0.1:19944 node js/quip-signer/test/produce-conformance-transfer.mjs
# Substitute the finalized block number printed by the producer:
python3 scripts/metadata-conformance/fixtures.py capture /tmp/quip-local-fixtures \
  --rpc http://127.0.0.1:19944 --blocks BLOCK_NUMBER --kind local-non-benchmark \
  --source-commit "$(git rev-parse HEAD)" --build-features std \
  --wasm target/debug/wbuild/quip-protocol-runtime/quip_protocol_runtime.wasm
```

Stop the disposable node after capture. Do not submit fixture transactions to
the archived live chain. The producer's transfer amount covers creation of the
destination account; repeat runs may use higher account nonces.

## Online stock-subxt smoke

With the disposable node above still running, save a fresh producer result and
pass it to the read-only consumer example:

```sh
QUIP_WS_URL=ws://127.0.0.1:19944 node js/quip-signer/test/produce-conformance-transfer.mjs > /tmp/quip-transfer.json
QUIP_WS_URL=ws://127.0.0.1:19944 SKIP_PALLET_REVIVE_FIXTURES=1 \
  cargo run --offline -p quip-protocol-runtime --example metadata_online -- \
  /tmp/quip-transfer.json target/debug/wbuild/quip-protocol-runtime/quip_protocol_runtime.wasm
```

The example pins stock `subxt 0.44.3` and uses
`OnlineClient<SubstrateConfig>` with default metadata negotiation, high-level
block/extrinsic access, and both block and per-extrinsic event access. No Quip
decoder types or metadata overrides are registered. It requires a numeric
loopback WebSocket address, a Development chain, runtime 119/tx 7, V16 metadata
without the Benchmark API, and exact equality between the supplied Wasm and
the executing `:code`. A 60-second deadline bounds RPC work. The jsonrpsee
transport response limit is raised from 10 MiB to 32 MiB for the debug Wasm's
approximately 15 MB hex-encoded `:code` response; metadata decoding stays stock.

It verifies finality and canonical block/header hashes, raw body byte parity
and order, the V5 bare timestamp followed by the V4 signed fixture transfer,
dynamic call fields, signer and H4 envelope, all twelve transaction extensions,
and exactly one success outcome per extrinsic. The transfer event must match
the sender, destination and amount at index 1; every event must dynamically
decode and reproduce the complete storage bytes in order. Output records the
block, genesis, parent metadata state, runtime/metadata hashes and checked counts.

This deliberately isolated smoke rejects other transactions in the target block.
Subxt 0.44.3's high-level block API uses client-cached metadata, so the example
also requires parent/post-state metadata and code to match and checks the
negotiated metadata against the parent registry. It does not claim that this
API automatically selects historical metadata across an upgrade. The offline
tests above cover that case explicitly. Apps and actual REST-service acceptance
remain independent gates.

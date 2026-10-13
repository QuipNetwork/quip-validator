# Metadata consumer fixtures

These are shared inputs for QUI-901's Rust, Apps and REST conformance tests.
See the [reviewed consumer matrix](../../metadata-consumer-compatibility.md) for
passing configurations and explicit stock failures.

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
This cap is specific to the provenance check, not a requirement for ordinary
block decoding.

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

## Actual Polkadot REST API 0.3.2 service

Stock **v0.3.2**, commit `283555b8e8e9cf488b11e9dbc1ba5f12344f5319`,
is **not conformant**: it decodes the mixed block but unconditionally drops the
first signature byte in the HTTP serializer. The H4 envelope becomes 1,659
bytes instead of 1,660. This is a consumer serializer defect, not a missing
Quip type registration. No runtime or signing-format change is required.

The local supported configuration for the tested block/event/parse routes is
that exact upstream commit plus these patches, applied in order:

1. [`polkadot-rest-api-0.3.2-signature.patch`](../../../scripts/metadata-conformance/polkadot-rest-api-0.3.2-signature.patch)
2. [`polkadot-rest-api-0.3.2-transaction-parse.patch`](../../../scripts/metadata-conformance/polkadot-rest-api-0.3.2-transaction-parse.patch)

The second patch reuses the same metadata-aware helper for transaction parsing;
it does not submit transactions or change signing/fee estimation. Its full
parse-path regression checks a real Asset Hub signature and all three standard
signature variants (synthetic payloads test decoding, not signature validity).

The patch uses the decoded signature's metadata type: enum signatures retain
the established REST contract of omitting the discriminant; composite/array
signatures retain all bytes. Transparent wrappers are followed with a bounded
traversal. It contains regressions for composite first bytes 0/1/2, arrays,
Ed25519/Sr25519/ECDSA enum payloads, a wrapped enum, and the existing real Asset
Hub signed fixture. Neither patch adds Quip-specific types or branches. They
are local reproducible artifacts, not an upstream release or deployment.

Start the disposable non-benchmark node and produce/capture a fresh transfer
as above, retaining the node while testing. Use a new capture directory, for
example `/tmp/quip-rest-fixture`. Keep the validator path for applying the patch:

```sh
QUIP_VALIDATOR_ROOT="$PWD"
git clone --branch v0.3.2 --depth 1 https://github.com/paritytech/polkadot-rest-api.git /tmp/quip-rest-api-0.3.2
cd /tmp/quip-rest-api-0.3.2
test "$(git rev-parse HEAD)" = 283555b8e8e9cf488b11e9dbc1ba5f12344f5319
cargo build --locked --bin polkadot-rest-api
SAS_SUBSTRATE_URL=ws://127.0.0.1:19944 SAS_EXPRESS_BIND_HOST=127.0.0.1 \
  SAS_EXPRESS_PORT=18015 target/debug/polkadot-rest-api
```

From the validator root in another terminal, test the **running HTTP service**:

```sh
# Must fail on stock: parse/block must retain the complete signature envelope.
node js/quip-signer/test/rest-conformance.mjs /tmp/quip-rest-fixture
# Exit zero here means the exact known bug was reproduced, NOT conformance.
QUIP_REST_EVIDENCE=/tmp/quip-rest-stock \
  node js/quip-signer/test/rest-conformance.mjs /tmp/quip-rest-fixture --expect-stock-truncation
```

Stop the stock service. In its checkout, apply both patches in order and rebuild:

```sh
git apply --check "$QUIP_VALIDATOR_ROOT/scripts/metadata-conformance/polkadot-rest-api-0.3.2-signature.patch"
git apply "$QUIP_VALIDATOR_ROOT/scripts/metadata-conformance/polkadot-rest-api-0.3.2-signature.patch"
git apply --check "$QUIP_VALIDATOR_ROOT/scripts/metadata-conformance/polkadot-rest-api-0.3.2-transaction-parse.patch"
git apply "$QUIP_VALIDATOR_ROOT/scripts/metadata-conformance/polkadot-rest-api-0.3.2-transaction-parse.patch"
cargo test --locked -p polkadot-rest-api --lib utils::extrinsic_decode::tests
cargo test --locked -p polkadot-rest-api --lib handlers::transaction::parse::tests
cargo build --locked --bin polkadot-rest-api
SAS_SUBSTRATE_URL=ws://127.0.0.1:19944 SAS_EXPRESS_BIND_HOST=127.0.0.1 \
  SAS_EXPRESS_PORT=18015 target/debug/polkadot-rest-api
```

Back in the validator root, the identical strict test must now pass:

```sh
QUIP_REST_EVIDENCE=/tmp/quip-rest-patched \
  node js/quip-signer/test/rest-conformance.mjs /tmp/quip-rest-fixture
```

The script uses stock `@polkadot/types 16.5.6` and `@polkadot/util-crypto 14.0.3`
from `js/quip-signer` as an independent metadata-only oracle. It checks fixture
hashes/provenance, the block header, raw body count/order/bytes, both per-index
routes, timestamp and transfer arguments, signer, nonce/tip/era, the complete
signature bytes, fee/outcome fields and **every event field and phase**. HTTP
routes are `/v1/blocks/{hash}` with default fees enabled, each
`/v1/blocks/{hash}/extrinsics/{index}`, `/v1/blocks/{hash}/extrinsics-raw`, and
`/v1/pallets/system/storage/events?at={hash}`. It also POSTs each raw extrinsic as
`{"tx":"0x..."}` to `/v1/transaction/parse` and checks the entire envelope, signer,
call/arguments, hash, signedness, era, nonce and tip against the block response
and raw oracle. Bare parse responses must omit signature/nonce/tip. This endpoint
only decodes and does not execute or submit the input. Missing/partial decoding fails.
`QUIP_REST_URL` permits a numeric localhost HTTP service (default port 18015);
`QUIP_REST_EVIDENCE` optionally saves raw responses and a hashed report.

The original block-only gate run is recorded in
[`rest-0.3.2-results.json`](rest-0.3.2-results.json): one V5 bare timestamp, one
V4 signed transfer, and 14 events, on the same finalized block before/after the
patch. Strict stock execution failed; explicit stock-reproduction mode passed;
patched strict execution passed with 1,660 signature bytes. The stock-reproduction
mode also fails against the patched service, preventing a silent false positive.
The results identify upstream commit, patch hash, runtime build and response
hashes; localhost addresses and generated block hashes are run-specific. Reproduce
with a fresh producer/capture, rather than expecting this temporary chain to persist.

The follow-on [parse results](rest-0.3.2-parse-results.json) pin both patch hashes
and a new block-11 run (same ordinary runtime artifact and source `a15e024`). With
only the first patch, blocks return 1,660 bytes but parse returns 1,659. Strict
mode fails; `--expect-parse-truncation` proves exactly that residual bug and
reports NOT CONFORMANT for parse. With both patches, strict mode passes all
block/event/parse assertions, and that known-failure mode fails. Five parse
unit tests (including all three standard signature variants), six extrinsic
unit tests and strict library/test clippy pass. This follow-on does not rewrite
the historical block-only results.

This is an actual local HTTP-service gate, not a deployed-service audit. It does
not claim REST historical upgrade-state selection, arbitrary event variants,
V5 General support, or exposure of extensions absent from the REST JSON schema.
The broader historical and failure-event fixtures are covered by the separate
subxt and Apps tests. No types bundle was needed for this supported configuration.
Stop both disposable processes after testing/review.

The known gap outside these tested routes is `/v1/node/transaction-pool` fee and
priority handling (also the `rc/node` variant): its hand parser assumes 64/65-byte
standard signature payloads. This path is unsupported for hybrid envelopes until
fixed/tested; its risk is source-identified, not live-reproduced. Transaction
parsing uses current metadata only; the relay-chain parse variant shares the
helper but was not HTTP-tested. The tested transfers have tip zero. See the
[final matrix and follow-up scope](../../metadata-consumer-compatibility.md#rest-endpoint-limits-and-remaining-work).

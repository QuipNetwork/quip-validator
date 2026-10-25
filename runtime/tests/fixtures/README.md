# Historical QBlock fixture

`agls-qblocks-v2-153726.json` contains three public AGLS storage records captured
read-only by RR on 2026-10-25. The JSON records the endpoint, genesis, finalized
block hash/height, runtime versions, exact storage keys, raw values, hashes and
independently decoded fields. Its SHA-256 is
`a22e5c00f2026309801cc05100694dcb19aadc90bd073b0ad3e7600d06ce8598`.

RR decoded the values in Python using the QBlock field order at main commit
`bc4552a2cc6913bd057ecfdf9abc99197393b55c`. The Rust test cross-checks that evidence
with the actual runtime `AccountId`, `Balance` and `BlockNumber` aliases and the
production `StoredQBlock` decoder and `QBlocks<Runtime>` storage map.

The pinned chain reported spec 119 / transaction version 7, QuantumPowApi v2 and
QuantumPow storage version 6. These version domains are distinct: the test covers
raw storage conversion, not runtime API `changed_in` dispatch. The historical
threshold is preserved as recorded; old admission was strict `energy < D`.
Backfilled frustration zero means unmeasured for these records and can also be
a genuine measured value in newer records.

The three values are 180 bytes each. This is not evidence that all historical
QBlocks have that length, nor is it a whole-state migration test. The first win
at block 1006 carries the genesis sentinel; blocks 1017 and 153634 cover later
records. The wrapper supports the 180-byte historical and 184-byte current
layouts for these runtime types. It validates exact consumption, not corruption
of arbitrary scalar fields; truncation to another supported length cannot in
general be identified without a format tag.

Run the native compatibility test from the repository root:

```sh
SKIP_WASM_BUILD=1 cargo test --locked -p quip-protocol-runtime --test qblock_history
```

This deliberately skips building runtime Wasm; it establishes neither Wasm
execution parity nor successful runtime release/benchmark builds.

## Partial storage-v6 migration fixture

`agls-topology-153726.scale` is the complete raw `RegisteredTopologies` value
fetched read-only on 2026-10-25 at the same finalized block. Its 350,456 bytes
have SHA-256 `d7ff6acd88cc31a81f2c17e164756b7041f8dc5fac159af1bee5e20498ba0f8d`,
matching RR's earlier capture. `agls-migration-153726.json` records its exact key,
independently decoded dimensions/specifications and the small storage values
and absences fetched at that block. A Python SCALE-compact/struct decoder also
verified canonical node/edge order, one connected component and cycle rank
36,938. The graph has 4,577 nodes and 41,514 edges and was registered at block 87.

The migration test combines this graph and frozen round/difficulty state with
the three sampled QBlocks. The counter and indices are **synthetic for those
three records**; the transient best-proof sentinel and candidate salt are also
test inputs. This is a partial-state fixture, not a full-chain upgrade rehearsal.
It checks the runtime's storage-v6-to-v7 hook, unchanged historical bytes and
meaning, preserved round and generated instance, seeded hardness/dimensions,
and a version-gated rerun that leaves the state root unchanged. That rerun
does not establish idempotence of individual migration steps. Every sampled
history value must decode.

Enable the production pre/post-upgrade checks and the negative malformed-history
test with:

```sh
SKIP_WASM_BUILD=1 cargo test --locked -p quip-protocol-runtime --test qblock_history --features try-runtime
```

The negative case keeps the key count unchanged but supplies an unsupported
181-byte record, proving the postcheck rejects an undecodable value rather than
accepting key-count equality alone. These migration tests run natively.

## API-v2 response fixtures

`agls-api-v2-153726.json` captures public `state_call` responses fetched read-only
on 2026-10-25 at the same pinned block. Its SHA-256 is
`a75214537e26a5c59ab3fbc44ff7471360344fb14e84bb1475737dc77ba22dec`.
Each entry records the exact export name, SCALE input, response length and
SHA-256. Small responses are inline; both mining-snapshot requests share
`agls-mining-snapshot-api-v2-153726.scale` (350,533 bytes, SHA-256
`a0a0419ddd0259a01e2b065f9569b680eb590fe20dea861644c0d7356e6e72d2`).
The capture also includes `Core_version` bytes and the independently queried
`state_getRuntimeVersion` result: spec 119 / transaction 7 / QuantumPowApi v2.
QBlock id 1 was resolved through `QuantumPowApi_qblock_id_by_block(1006)`.

To reproduce a response, send JSON-RPC `state_call` to the recorded endpoint
with parameters `[entry.method, entry.input, at_block_hash]`. Decode the hex
result and compare its length/hash to the entry. The snapshot is stored as
binary to avoid embedding a 700 KB hex string in JSON. Neither fetching nor
running the tests submits transactions.

`quantum_pow_api_versions.rs` constructs the actual generated runtime API
client over a `CallApiAt` adapter that matches the exact block, export and
argument bytes before returning a captured response. All seven
`#[changed_in(3)]` methods are called through their generated
`*_before_version_3` names. Their wire names retain the original exports;
there are no `before_version` Wasm exports. Full re-encoding checks ensure the
returned value accounts for every response byte. Historical QBlock and graph
fields are cross-checked against the separate storage fixtures, and the nonce
is pinned to the captured API result.

Clients must inspect the reported API version and choose the matching method.
The pinned SDK's generated client uses `Decode`, not `DecodeAll`, and
`changed_in` does not select a decoder automatically. The negative test shows
that calling the modern difficulty method against the old response silently
interprets its first eight bytes as the wrong threshold. This characterizes
the client contract; it does not change SDK decoding or implement a miner's
version-selection policy.

Run offline from the repository root:

```sh
SKIP_WASM_BUILD=1 cargo test --locked -p quip-protocol-runtime --test quantum_pow_api_versions
```

These are generated-client routing/decoding tests using responses actually
served by the pinned historical runtime. They do not locally execute its old
Wasm, exercise the current runtime's API exports, or rehearse a full upgrade.
The adapter uses the pinned SDK's macro-support backend/extension types only
to satisfy `CallApiAt`; it intentionally has no storage backend.

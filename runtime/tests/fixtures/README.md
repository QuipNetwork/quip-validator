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
and an idempotent second upgrade. Every sampled history value must decode.

Enable the production pre/post-upgrade checks and the negative malformed-history
test with:

```sh
SKIP_WASM_BUILD=1 cargo test --locked -p quip-protocol-runtime --test qblock_history --features try-runtime
```

The negative case keeps the key count unchanged but supplies an unsupported
181-byte record, proving the postcheck rejects an undecodable value rather than
accepting key-count equality alone. These migration tests run natively.

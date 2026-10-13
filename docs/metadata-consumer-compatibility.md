# Metadata consumer compatibility

QUI-901 results, verified on **2026-10-13**. The required consumers pass in the
configurations below without Quip-specific type registration. **No `@quip/types`
package is required for this matrix**, and none was created or published.
The Apps and REST patches correct decoder/serializer assumptions; a type bundle
would not fix those bugs.

The runtime publishes **V16** through `state_getMetadata` and retains versioned
metadata **[14, 15, 16]**. These were existing runtime features: this work adds
consumer evidence, not another rollout from runtime 115 to 116. Local acceptance
used ordinary, non-benchmark runtime **119 / transaction version 7**; archived
fixtures cover execution runtimes **117 and 118 / transaction version 7**.
Source revision 119 is not a claim that the public network runs 119.

## Tested configurations

| Consumer | Exact tested configuration | Result and coverage |
| --- | --- | --- |
| Stock subxt, offline | `subxt-core 0.44.3`; validator `0f319fa35fd6a19594a490aad7a27abb337e2874` | PASS: 5 blocks, 25 extrinsics (5 V5 bare, 20 V4 signed), 107 events, 11 expected failed outcomes, 3 transfers; field/byte/extension/phase assertions and historical metadata selection |
| Stock subxt, online | `subxt 0.44.3`; validator `a15e0244f136b9e9650897f899d702439c2c73c4` | PASS: finalized local transfer block through `OnlineClient<SubstrateConfig>`, raw body parity, dynamic fields, signature, extensions and events; no Quip types or metadata overrides |
| Quip Apps | Apps `2046697423e7f5f285caa8bb0d3195b73e85b5d1`, dependency baseline `ca23a8a67ac3b9c16fdc6beac7bdcc5a644d2714`; `api`/`types`/`types-codec` **16.5.6**, `util`/`util-crypto` **14.0.3**, existing signedness patch retained | PASS: same 5 blocks / 25 extrinsics / 107 events / 11 failures / 3 transfers; metadata-only registry, no `chainTypes` or `typesBundle`. This is a decoding regression, not browser UI validation |
| Stock polkadot-js comparison | Unpatched **16.5.6**, same fixtures | NOT CONFORMANT: all 5 V5 bare inherents rejected by the signature-emptiness heuristic. All 20 signed items and 107 events still decode. The explicit stock mode reproduces this failure; its zero exit code is not a conformance pass |
| Polkadot REST API block routes | **v0.3.2**, upstream `283555b8e8e9cf488b11e9dbc1ba5f12344f5319`, plus the [local signature patch](../scripts/metadata-conformance/polkadot-rest-api-0.3.2-signature.patch) committed in validator `147fca4a974c6f7d5f957f2570fe64db401e7119`; upstream uses subxt **0.51.1** | PASS: actual HTTP service, local finalized mixed block with 2 extrinsics and 14 events; count/order/index, fields, raw bytes, complete 1,660-byte envelope and event phases. Only the routes listed below are covered |
| Stock Polkadot REST API | Same **v0.3.2**, without the patch | NOT CONFORMANT: block and per-extrinsic responses drop the first envelope byte, returning 1,659 bytes. Default strict test fails; explicit stock mode proves exact `envelope[1:]` truncation |
| Go / Subscan | `scale.go v1.11.2` evidence | DEFERRED: V16 metadata parses, but stock hybrid-signed extrinsic decoding fails. Scratch decoder fixes are not complete-indexer acceptance; no production deployment, DB/Redis audit or historical repair is claimed |

The Apps patch is
`.yarn/patches/@polkadot-types-npm-16.5.6-6fe2703ed8.patch` in the Apps repository.
It trusts the wire signedness flag rather than whether a composite signature
object appears empty. Keep it until a replacement upstream version passes the
same regression. The REST patch uses the signature metadata shape to preserve
composite bytes while retaining the established standard enum-signature response.
Both patches are consumer fixes, not missing runtime type definitions.

## Account and encoded H4 layout

The transaction account mapping is:

```text
AccountId32 = blake2_256(ASCII("quip-account-v1") || public_key_bytes)
```

Concatenate the literal domain bytes and all **929** public-key bytes, with no
SCALE length prefix. The output is 32 bytes; SS58 represents that account, not
the hybrid public key. The implementation is in
[`transaction-crypto-core`](../crates/transaction-crypto-core/src/lib.rs) and
[`transaction-crypto`](../crates/transaction-crypto/src/lib.rs). Consumer tests
check the account derivation independently against the decoded public key.

| Encoded component | Bytes | Meaning |
| --- | ---: | --- |
| H4 transaction public key | 929 | Full hybrid public key |
| Signature field | 731 | Metadata represents the bytes as `[u8; 512]` followed by `[u8; 219]` |
| `HybridTxSignature` envelope | 1,660 | Public key followed by the encoded signature field; a concrete composite, with no `MultiSignature` discriminant |

These are **encoded field sizes**, not the complete extrinsic length. The full
envelope must not be labeled Ed25519, Sr25519 or ECDSA, or confused with the
731-byte signature field. Fixed-size array/chunk encoding adds no vector length
prefix. The checked-in signing fixture also verifies that its JSON signing
payload encodes exactly to the raw payload; it is a synthetic FRAME fixture,
not an on-chain transaction.

Current mixed blocks contain a **V5 bare** timestamp (`0x05` format byte) and
**V4 signed** transactions (`0x84`). These bytes follow the extrinsic's compact
length prefix. Determine signedness from the wire format, not from signature
object emptiness. This does not establish support for V5 General transactions.

## Evidence and reproduction

The [fixture and command reference](fixtures/metadata-conformance/README.md)
contains capture/verification, offline subxt, online subxt and actual REST
commands. The [runtime tests](../runtime/tests/metadata_v16.rs) have seven passing
tests; the [online example](../runtime/examples/metadata_online.rs) consumes a
finalized transaction produced by the existing Quip signer. The producer is
not a decoder under test. Local fixtures record default build features, prove
`:code` equals the supplied Wasm, and exclude the Benchmark API. A debug build
of this ordinary runtime configuration is not release packaging evidence.

The archived manifests bind each block to **parent-state metadata**, including
upgrade block **330109** (executes under 117, installs 118) and **330110** (executes
under 118), plus transfers **591863** and **591865**. They record exact RPC
parameters, genesis/block/parent hashes, code hashes, file sizes and SHA-256.
The ordinary local fixture is block **107**, built from validator
`e5de2ddcb347565d15b75df823a116b67f5b602d`; its manifest identifies the artifact.
The synthetic changed-call test exercises rejection with the wrong historical
metadata, since the real boundary block bodies happen to decode under both
versions.

The archived RPC was `https://bootnode-2.aglais.quip.network:20049/rpc` at capture
on 2026-10-13. This is provenance, not an availability guarantee. The older
`bootnode-*.testnet.quip.network` names failed DNS during this investigation;
do not substitute them in these reproduction commands.

From the Apps checkout, use `QUIP_METADATA_FIXTURES` to point to this repository's
`docs/fixtures/metadata-conformance`, then run `yarn test:quip-metadata` (or
`node scripts/metadataConformance.mjs`). Apps' README documents the separate
unpatched `--stock` comparison. Do not change its older pinned validator
submodule merely to locate these fixtures. Unknown-extension warnings for
`AuthorizeCall` and `EthSetOrigin` remain visible; the regression independently
proves their zero-byte layout from metadata.

[REST results](fixtures/metadata-conformance/rest-0.3.2-results.json) pin the
upstream commit, patch SHA-256, local runtime/manifest hashes and HTTP response
hashes before/after the patch on the same finalized block **5**. That run was
built from validator `a15e0244f136b9e9650897f899d702439c2c73c4`. Reproduction uses a
fresh disposable node/capture, so generated hashes and block numbers can change.
Both reviewers independently reran the patched HTTP test and the six Rust
extrinsic tests. The patch also passed strict library clippy. The temporary
services were stopped after review; no upstream patch publication or service
deployment was performed.

## REST endpoint limits and remaining work

The REST acceptance covers these **`/v1` routes only**, with block fee processing
enabled by default:

- `GET /blocks/{hash}`
- `GET /blocks/{hash}/extrinsics/{index}` for both bare and signed indices
- `GET /blocks/{hash}/extrinsics-raw`
- `GET /pallets/system/storage/events?at={hash}`

Two known assumptions survive outside this patch and must remain visible to
wallet/exchange integrators:

- **`POST /transaction/parse`** still unconditionally strips the first signature
  byte in `handlers/transaction/parse.rs`, so composite signatures are truncated.
- **`GET /node/transaction-pool` with fees**, including the `rc/node` variant,
  uses `handlers/node/common.rs::extract_tip_from_extrinsic_bytes`, which assumes
  standard 64/65-byte `MultiSignature` payloads. It can fail or misparse hybrid
  transaction extensions. Treat this path as unsupported for Quip pending a
  metadata-driven fix and endpoint regression.

These residual findings were confirmed by source review; those HTTP paths were
not acceptance-tested. The current patch must not be advertised as complete
REST API compatibility. Planner owns the REST follow-up and ticket reporting.

The local online tests use **tip zero** and disabled metadata-hash mode. They do
not establish nonzero-tip handling, merkleized/offline signing, arbitrary event
variants, or decoding extensions absent from REST's JSON schema. Historical
metadata selection is covered by the offline fixtures, not claimed for the
online subxt client's cached metadata or REST upgrade handling. The online
subxt smoke's 32 MiB RPC response cap is solely for its debug `:code` provenance
check; ordinary block decoding does not require that test-specific cap.

Go/Subscan remains separately owned by the Go/explorer workstream, with planner
coordinating its issue linkage and scheduling. The [deferred follow-up plan](metadata-v15-external-decoding-plan.md#deferred-gosubscan-follow-up)
records the nested-signature panic, nondeterministic map-selection workaround,
raw-span experiments and future disposable full-indexer validation. An inactive
indexer does not require an old-database audit for QUI-901.

## Review and ticket handoff

Reviewer and rr approved each implementation gate through validator `147fca4`
and Apps `2046697423`. The final documentation commit needs its own review.
Planner owns the external [QUI-901](https://linear.app/quip-network/issue/QUI-901/metadata-v15-published-type-bundle-so-external-tools-decode-hybrid)
update/closure and follow-up links; this repository does not assert those
external actions are complete.

The ticket acceptance should describe V16 and the tested configurations above,
retain stock failures and REST route limits, record **no bundle required**, and
link the separate Go/Subscan and REST follow-ups. The old runtime-116 rollout,
mandatory package publication and Sidecar `SAS_SUBSTRATE_TYPES_BUNDLE` setup are
not acceptance requirements for this metadata-native REST service. No new
runtime/spec/transaction-version change, push, merge, deployment or package
publication was part of this work.

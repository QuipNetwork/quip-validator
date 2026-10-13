# Metadata V16 External Decoding Plan

Finalized scope: 2026-10-13, after review by **rr** and **reviewer** and the user's consumer/scope decisions.

Related ticket: [QUI-901](https://linear.app/quip-network/issue/QUI-901/metadata-v15-published-type-bundle-so-external-tools-decode-hybrid).
The filename and ticket title retain V15 for historical continuity; this plan targets **V16**.

Implementation results (2026-10-13): [supported consumer matrix, wire layout,
bundle decision and limits](metadata-consumer-compatibility.md). Implementation
gates through validator `147fca4` and Apps `2046697423` are approved by reviewer
and rr. The baseline and sequence below preserve the original plan; final docs
review and planner-owned ticket handoff remain separate completion steps.

## Goal and agreed scope

Demonstrate that Quip's published metadata describes its hybrid extrinsics sufficiently for the required consumers, with reproducible decoding tests and documentation.

Required consumers:

1. **Stock subxt**, without handwritten Quip Rust types.
2. **Quip Apps / polkadot-js**, without Quip-specific type registration, while retaining and explicitly testing the existing signedness patch.
3. **Polkadot REST API**, using metadata-native decoding.

**Go/Subscan fixes are deferred to a separate follow-up.** Known Go decoding failures do not block QUI-901, and closing QUI-901 must not imply that Go/Subscan is fixed. The indexer is not currently running, so deployed-image inspection, existing-database investigation and historical repair are not part of this plan.

A published `@quip/types` package remains conditional: create it only if a required consumer demonstrably needs explicit type registration. A decoder implementation bug is not evidence that a type bundle is needed.

No new network reset, runtime-version bump, signature-format change or account-format change is planned for this compatibility work.

## Verified baseline

Source reviewed: quip-validator `48908450f0b911d79c358b04effbf0f97d3757fb`, runtime **119**, transaction version **7**.

The runtime metadata implementation is already present:

- `runtime/src/apis.rs` publishes V16 through the legacy metadata runtime API used by `state_getMetadata`.
- The versioned API continues to offer V14, V15 and V16.
- `runtime/tests/metadata_v16.rs` contains five conformance tests, including the legacy Wasm API path, extrinsic versions `[4, 5]`, the twelve transaction-extension identifiers and their order, and the non-opaque hybrid signature type graph.
- The metadata describes a 929-byte H4 public key and a signature field represented as `[u8; 512]` followed by `[u8; 219]` (731 encoded bytes). The complete encoded `HybridTxSignature` envelope is **1,660 bytes**. Do not label that full envelope as a bare cryptographic signature.
- Stock `subxt-core 0.44.3` decodes a generated mixed V5-bare/V4-signed set using metadata only. Current assertions cover count, signedness and pallet/call names; stronger field and event assertions remain to be added.
- The Apps patch addresses signedness detection, independently of metadata type discovery. Publishing V16 does not remove that requirement.

rr separately probed the live **aglais runtime 118 / transaction version 7** and local runtime-119 Wasm. The local artifact was subsequently identified as a **runtime-benchmarks build** (it exposes `Benchmark@2`), so it provides layout evidence, not production-artifact validation. Keep those evidence sets distinct; the source revision is not proof of the currently deployed runtime version. The live endpoint used was `https://bootnode-2.aglais.quip.network:20049/rpc`; the previously documented `bootnode-*.testnet.quip.network` hostnames failed DNS during this investigation.

The old tasks to switch legacy metadata from V14 and bump runtime 115 to 116 are complete/obsolete, not new implementation steps. Acceptance below concerns remaining downstream evidence, not repeating that rollout.

## Planned consumer matrix (see results above)

| Consumer | Starting version / implementation | Required result | Scope |
|---|---|---|---|
| Stock subxt | `subxt-core 0.44.3`, matching existing tests; pin the online client too | Decode mixed blocks, signed transfer fields, extensions and events without custom Quip types | Required |
| Quip Apps / polkadot-js | Pin the exact Apps revision and dependency versions during implementation; identify the existing signedness patch | Correct signed/bare classification and calls, accounts, arguments and events without Quip-specific registration | Required |
| Polkadot REST API | `v0.3.2`, the published release checked on 2026-10-13 | Decode and serve block/extrinsic/event data for the same runtime and fixture semantics | Required |
| Go / Subscan | `scale.go v1.11.2` parses V16; stock signed-hybrid decoding still fails | Record known limitation and link the deferred decoder follow-up | Informational; not a QUI-901 gate |
| Other indexers / exchange tools | No additional consumer selected | Do not claim compatibility without versioned evidence | Outside this acceptance matrix |

Record exact versions in results. Test stock polkadot-js separately from patched Apps where practical, so the supported configuration is explicit. A known stock signedness failure does not invalidate patched Apps acceptance, but must remain visible in the matrix and documentation.

## Implementation sequence

Each step should be a separate reviewable commit; use separate repository PRs where Apps or integration infrastructure changes are needed.

### 1. Establish a shared fixture set and provenance

- Reuse the checked-in H4 signing fixture and its generator in `docs/polkadotjs/fixtures/hybrid-signing.json`; do not maintain independently invented signature bytes.
- Capture metadata, a signed balance transfer, a V5 bare inherent, their containing block and relevant events. Include representative transaction extensions.
- Store a manifest recording source commit or runtime artifact hash, genesis hash, block hash/number where applicable, spec version, transaction version, metadata version and expected outputs.
- Keep archived live-spec-118 fixtures separate from locally produced spec-119 fixtures. Record build features, and generate production-build metadata for the target-runtime acceptance tests rather than treating the scratch benchmark build as production. rr's observed range was blocks 591849–591948 and included two signed balance transfers; promote useful scratch artifacts into reproducible fixtures with their full hashes rather than relying on temporary paths.
- Use one fixture source for Rust, Apps and REST API validation. REST API online tests may replay equivalent transactions on a disposable node, with outputs checked against the same expectations.
- Pair each block with the metadata appropriate to its execution. Add an upgrade-boundary case with explicit provenance rather than decoding historical blocks using current-head metadata.

Acceptance: fixtures regenerate or can be independently verified, and consumers agree on expected count, order, extrinsic indices, calls, accounts and values.

### 2. Strengthen stock subxt conformance

Extend `runtime/tests/metadata_v16.rs` to assert:

- Decoded signer/address matches the fixture account.
- Public key, encoded signature field and complete signature envelope match the fixture bytes and lengths: 929, 731 and 1,660 respectively.
- The 512/219 metadata representation is encoding-equivalent to the fixed encoded signature field, not merely a matching type shape.
- Transfer destination and amount are correct.
- Transaction extensions, including those after the signature boundary, decode with the correct values and order.
- Events decode against the appropriate metadata and retain their extrinsic association.
- The V5 bare inherent remains bare and the V4 signed transaction remains signed.

Add a stock online subxt smoke test against a disposable node built from the target revision: observe a finalized block containing the signed transfer and decode it without custom Rust types. Existing signing helpers may construct the transaction; this ticket tests decoding, not a new signer.

Acceptance: field-level and online tests pass using stock subxt, with no Quip-specific decoder types or hidden type registrations.

### 3. Validate Apps and Polkadot REST API

Apps:

- Pin the tested Apps revision, runtime revision and polkadot-js package versions.
- Load V16 without Quip-specific `chainTypes` or `typesBundle` registration.
- Exercise the shared mixed-block cases and verify signedness, calls, accounts, argument values, events and transaction extensions.
- Retain the signedness patch until an equivalent upstream release passes the regression test. Report stock and patched behavior separately.

Polkadot REST API:

- Pin `v0.3.2` initially and connect it to the disposable target node.
- Query its block/extrinsic/event endpoints for the finalized fixture transactions.
- Assert no extrinsics disappear and that decoded fields, indices and event associations match the fixture expectations.
- If the service fails despite successful stock-subxt decoding, isolate the service's serializer or chain assumptions. Do not assume missing type registration or advertise compatibility based solely on subxt library tests.

Acceptance: all three required consumers pass the matrix. No Subscan DB/Redis environment is needed for this work.

### 4. Record the type-bundle decision and refresh documentation

- Document `AccountId = blake2_256("quip-account-v1" || public_key)` and the mixed V5-bare/V4-signed block format.
- Replace stale envelope sizes with the current encoded H4 layout; distinguish public key, signature field and full envelope.
- Publish the exact supported consumer matrix, reproduction commands, known Apps patch and deferred Go limitation.
- Remove the old Sidecar/`SAS_SUBSTRATE_TYPES_BUNDLE` requirement from the proposed ticket acceptance. Polkadot REST API uses metadata-native decoding and does not support those legacy configuration variables.
- If all required consumers work without explicit registration, record **no `@quip/types` package required** and close that decision explicitly.
- If a required consumer needs registration, document the exact missing types first, then implement the smallest versioned bundle and CI that compares it to runtime metadata. Do not duplicate metadata definitions unnecessarily.
- Correct stale RPC hostname references encountered in the relevant compatibility documentation, while labeling archived fixture provenance separately from current endpoint availability.

### 5. Review and close with evidence

- Run the focused runtime tests, the online subxt smoke and the Apps/REST API integration tests affected by the work.
- Have reviewer and rr review the implementation and versioned results.
- Update QUI-901 with the final consumer matrix, fixture provenance, bundle decision and link to the separately owned Go/Subscan follow-up.
- Do not claim universal external-tool compatibility; acceptance is limited to the tested versions and configurations.

Implementation evidence is linked above. Repository changes do not themselves publish a package, modify the Linear ticket or deploy a service.

## Acceptance checklist

Already implemented and reviewed in the source baseline:

- [x] Legacy metadata API publishes V16; versioned metadata remains available.
- [x] The registry describes `HybridTxSignature` as a concrete composite.
- [x] Extrinsic metadata advertises `[4, 5]` and the expected extension layout.
- [x] Stock subxt has a basic mixed V5-bare/V4-signed decoding test.

Required before QUI-901 closure:

- [x] Shared fixtures include complete provenance and expected field values.
- [x] Stock subxt field-level decoding and finalized-block smoke pass.
- [x] Apps regression tests pass without Quip-specific type registration; any signedness patch is documented.
- [x] Polkadot REST API tested block/extrinsic/event routes pass on pinned v0.3.2 plus the local metadata-aware signature patch; stock failure and parse/pool gaps remain explicit.
- [x] Counts, order, indices, decoded values and event associations agree across required consumers.
- [x] Historical/upgrade-boundary metadata selection has explicit test coverage.
- [x] Account derivation, encoded H4 layout and mixed formats are documented.
- [x] The conditional `@quip/types` decision is recorded, with a tested/published package only if required.
- [ ] Planner confirms the external ticket wording/closure and links the separate follow-ups. Repository handoff text replaces stale V15, runtime-116, mandatory bundle and Sidecar acceptance.
- [x] Reviewer and rr have reviewed the implementation evidence.
- [x] The deferred Go/Subscan limitation and follow-up ownership are explicit.
- [ ] Reviewer and rr approve the final documentation commit.

## Deferred Go/Subscan follow-up

This section preserves the research and proposed follow-up without adding it to QUI-901's implementation gates.

Verified release findings:

- Go module `github.com/itering/scale.go` **v1.11.2**, published September 11, 2026, includes the V16 view-function ID correction from 8 to 32 bytes and versioned extension handling. It parses Quip's live-spec-118 and local-spec-119 V16 metadata. The module requires Go 1.24 or later.
- Stock v1.11.2 still panics when treating nested hybrid signature fields as strings. In rr's 100-block sample, 100 bare inherents decoded but all 387 signed extrinsics failed with the stock decoder.
- The Subscan repository's existing tolerant patch avoids the panic but chooses the flat signature value through Go map iteration; that can produce inconsistent values, including the public key.
- rr's separate scratch raw-span patch decoded all 487 sample extrinsics. All 387 signed envelopes were 1,660 bytes, equal to an independently extracted byte span, and stable over five repeated decodes. The local-spec-119 signing fixture matched too. Stock and patched upstream tests passed all nine packages.
- Those scratch results prove the tested decoder behavior, not the untested complete indexer pipeline. No production change was made.

When the Go/Subscan work is scheduled:

1. Upgrade the actual consumer to v1.11.2 or a newer version proven against the fixtures, and replace the map-iteration patch with deterministic, lossless signature-envelope handling. Preserve structured `SignatureRaw` and standard signature behavior; document the downstream field contract.
2. Classify current V4-signed/V5-bare extrinsics from their format, not signature string emptiness. Do not generalize this to authorized V5-general transactions; add format-specific fixtures when supporting them.
3. Propagate per-extrinsic failures and reject incomplete block ingestion. Test counts/order/indices and prove an injected decode failure cannot partially commit a block.
4. Validate the complete consumer using a clean disposable DB/Redis setup. Existing production-database inspection or repair is unnecessary because the indexer is inactive.
5. Retire the consumer's V15 negotiation shim only after the full consumer passes. Retain the runtime's versioned metadata API.
6. Track the separate upstream V14 decoder omission: it leaves the final `RuntimeMetadataV14.ty` compact type ID unread. In the captured fixtures this is 274 for spec 118 and 291 for spec 119, both resolving to the runtime type. Consume or assert that exact field; never allow arbitrary trailing bytes. V15/V16 fully consume the tested metadata.
7. Consider proposing the generic decoder fixes upstream. Track this work under the Go/explorer ownership and link it from QUI-901 without making it a closure dependency.

## Boundaries and remaining technical risks

- No signing/fee-estimation redesign, wire-format change, canonical-signer work or merkleized-metadata/offline-signing activation.
- No new spec-version or transaction-version bump solely for consumer tests or documentation. Reassess only if implementation uncovers a genuine runtime defect requiring a production change.
- The SDK's V16 extension map currently assumes extension version 0. Existing conformance tests pin the current layout; a future extension-version change needs a separate SDK correction.
- `CheckMetadataHash` mode is disabled in the existing signing path. Enabling it requires separate verification of metadata hashing and signing compatibility.
- Metadata self-description does not guarantee every consumer can process arbitrary composite signatures or mixed extrinsic formats. Required consumers need empirical results; unselected tools remain unverified.

## References

- [scale.go v1.10.9 to v1.11.2 changes](https://github.com/subscan-explorer/scale.go/compare/v1.10.9...v1.11.2)
- [scale.go v1.11.2 extrinsic decoder](https://github.com/subscan-explorer/scale.go/blob/v1.11.2/extrinsic.go)
- [Polkadot REST API v0.3.2 migration guide](https://github.com/paritytech/polkadot-rest-api/blob/v0.3.2/docs/guides/MIGRATION.md)
- [Runtime metadata APIs](../runtime/src/apis.rs)
- [Metadata conformance tests](../runtime/tests/metadata_v16.rs)
- [Signing fixture](polkadotjs/fixtures/hybrid-signing.json)

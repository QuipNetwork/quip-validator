# Metadata V16 External Decoding Plan

> Retargeted from V15 to V16 (see Decisions). The file name and
> the QUI-901 ticket title still reference V15 for historical continuity.

Related ticket: [QUI-901](https://linear.app/quip-network/issue/QUI-901/metadata-v15-published-type-bundle-so-external-tools-decode-hybrid)

## Goal

Make Quip's runtime metadata self-describing so stock metadata-driven tools can
decode hybrid extrinsics without hand-written Rust types or Quip-specific
decoder configuration.

The first phase publishes and tests Metadata V16. A separate `@quip/types`
package is introduced only if supported consumers still require explicit type
registration after the V16 rollout.

## Current State

The runtime already advertises Metadata V14, V15, and V16 through the versioned
metadata runtime API, and negotiating clients already receive V16 today (stock
subxt 0.44 tries `Metadata_metadata_at_version` in the order `[16, 15, 14]`). A
local conformance probe established that:

- `HybridTxSignature` is a concrete metadata composite with `public` and
  `signature` fields;
- the H4 public key resolves to `[u8; 929]`;
- the H4 signature resolves to `[u8; 512]` followed by `[u8; 219]`, for a total
  of 731 bytes;
- stock `subxt-core 0.44.3` accepts the metadata with no custom types;
- stock subxt decodes a mixed block containing a V5 bare `Timestamp::set` and
  a V4 signed `Balances::transfer_allow_death`.

The remaining publication gap is the legacy metadata runtime API used by
`state_getMetadata`. The current implementation wraps `Runtime::metadata()`,
which intentionally returns Metadata V14.

The Apps Yarn patch is a separate concern. It fixes polkadot-js inferring
signedness from whether a custom signature value looks empty. Metadata V16 does
not remove the need for that fix.

## Decisions

1. Publish Metadata V16 from Quip's legacy `metadata()` runtime API.
   V16 is chosen over V15 because:
   - V16's extrinsic section lists all supported extrinsic format versions
     (`versions: [4, 5]`), which describes Quip's mixed V5-bare/V4-signed
     blocks. V15 carries a single version, and the SDK conversion emits only
     the minimum (`4`).
   - Both in-tree consumers are V16-native: subxt 0.44 converts V16 directly
     and already negotiates it today; polkadot-js 16.x treats V16 as its
     internal "latest" format.
   - polkadot-js's V15→V16 up-conversion is lossy: it squashes `versions` to
     the single V15 entry and, in `@polkadot/types` 16.5.6, mis-keys
     `implicit` as `implict`, silently dropping the additional-signed type id.
2. Keep this as a Quip runtime override; do not change the SDK-wide
   `Runtime::metadata()` generator.
3. Publish this metadata change as part of the Runtime 116 H2/H4 chain-wipe
   relaunch. Set `transaction_version` to 7 because the H4 signed-extrinsic
   wire format is incompatible with H3.
4. Keep the Apps decoder patch until the equivalent fix ships upstream.
5. Treat `@quip/types` as a conditional second phase, not a prerequisite for
   the Metadata V16 rollout.

## Phase 1: Runtime Metadata V16

### 1. Add conformance tests

Add a focused runtime integration test, for example:

```text
runtime/tests/metadata_v16.rs
```

Add `frame-metadata` and `subxt-core` as runtime dev dependencies through the
workspace dependency table. Pin `frame-metadata` to the version the SDK fork's
`frame-support` already resolves transitively (currently 23.0.1) to avoid a
duplicate in the tree, and pin `subxt-core` to 0.44.3 to match the probe above.

The test must:

- request and decode `Runtime::metadata_at_version(16)`;
- assert the returned metadata is V16;
- assert the extrinsic section lists `versions == [4, 5]` and that the
  transaction-extension map covers the runtime's extensions;
- inspect the extrinsic address, call, signature, and transaction extension
  types;
- assert that `HybridTxSignature` is not represented as `Vec<u8>` or another
  opaque sequence;
- pin the H4 public-key length at 929 bytes;
- pin the H4 signature layout at 512 plus 219 bytes;
- generate a V5 bare timestamp inherent and a V4 signed balance transfer;
- decode both with stock subxt using metadata only;
- assert signedness, pallet name, and call name for both extrinsics.

### 2. Publish V16 from `state_getMetadata`

Change the runtime API implementation in `runtime/src/apis.rs` from the V14
inherent metadata path to:

```rust
Runtime::metadata_at_version(16)
    .expect("Metadata V16 is supported")
```

Do not change
`polkadot-sdk/substrate/frame/support/procedural/src/construct_runtime/expand/metadata.rs`.
Changing the SDK generator would alter the legacy metadata behavior of every
runtime using the fork.

### 3. Version the runtime

- Bump `spec_version` from `115` to `116`.
- Set `transaction_version` to `7` for the incompatible H4 signed-extrinsic
  encoding.
- Add a version comment explaining that Runtime 116 is the H2/H4 chain-wipe
  relaunch and publishes Metadata V16 through the legacy runtime API.

### 4. Regenerate the polkadot-js signing fixture

The checked-in fixture embeds `spec_version`/`transaction_version`
(`docs/polkadotjs/fixtures/hybrid-signing.json`), and
`runtime/tests/signing_fixture.rs` fails when it drifts from the generator.
Regenerate and commit it together with the version bump:

```text
cargo run -p quip-protocol-runtime --example generate_polkadotjs_signing_fixture -- --write
```

### 5. Run a live-node smoke test

Start a temporary dev chain and verify:

- `state_getMetadata` returns a V16 metadata blob;
- `Metadata_metadata_at_version(16)` returns the expected V16 graph;
- a stock online subxt client submits or observes a signed transfer;
- subxt decodes the containing block's V5 bare and V4 signed extrinsics;
- events and transaction extensions decode without custom types.

Note that the `browser-signer-test` CI job already runs a stock polkadot-js
`ApiPromise` (no `chainTypes`) against a live node via
`js/quip-signer/test/local-node.mjs`; that path fetches `state_getMetadata` and
acts as the standing regression gate for this change. Consider extending it to
assert the returned metadata version so CI catches an accidental revert.

## Polkadot SDK Scope

No production SDK change is required for Phase 1. The fork already contains the
large-array `TypeInfo` support needed for the H4 public key and signature.

One known incompleteness in the fork's V16 path: `metadata-ir` converts the
extrinsic IR to V16 with a hardcoded `transaction_extensions_by_version` map
keyed `0` ("assume version 0 for all extensions"). This is correct today —
extension version 0 is the only one, V4 signed extrinsics fall back to the
maximum map key, and V5 bare extrinsics carry no extensions — but the
conformance test pins the map shape so a future extension-version change cannot
silently emit wrong metadata. Track replacing the placeholder with real
per-version extension data as a fork follow-up.

An optional SDK hardening change can add unit tests that pin:

- public-key metadata as `[u8; 929]`;
- signature metadata as the 512/219 split;
- the SCALE signature length at exactly 731 bytes;
- encoding equivalence between the runtime type and its metadata-only shape.

These tests protect the existing workaround but do not need to block the
runtime change.

## Apps Validation

After the runtime change lands:

1. Update the Apps `quip-protocol-rs` submodule.
2. Confirm the Apps deployment runs a V16-capable polkadot-js (16.x line);
   V16 is its native "latest" format, while the V15 up-conversion is lossy
   (see Decisions).
3. Load the V16 metadata without Quip-specific `chainTypes`.
4. Decode a fixture containing a V5 bare inherent and a V4 signed hybrid
   transaction.
5. Confirm calls, accounts, events, and transaction extensions render
   correctly.
6. Keep the existing `@polkadot/types` Yarn patch until its signedness fix is
   available in an upstream release.

Metadata V16 solves type discovery. The Apps patch solves signedness detection;
the two changes are complementary.

## Phase 2: Conditional `@quip/types`

Create a public `quip/types` repository and publish `@quip/types` only if a
supported consumer cannot use the published Metadata V16 directly.

Before starting this phase, test current versions of:

- polkadot-js;
- polkadot-rest-api;
- Subsquid or SubQuery, depending on the selected indexer;
- any exchange integration tooling in scope.

The deprecated Substrate API Sidecar and
`SAS_SUBSTRATE_TYPES_BUNDLE` are not sufficient reasons to create the package.

If required, `@quip/types` should provide:

- polkadot-js `typesBundle` and `chainTypes`;
- hybrid public-key, signature, and envelope definitions;
- signed-extension registration;
- runtime spec-version ranges;
- documentation for
  `AccountId = blake2_256("quip-account-v1" || public_key)`;
- documentation for mixed V5-bare/V4-signed blocks;
- CI that decodes the same fixtures as the Rust conformance test and detects
  drift from runtime metadata.

## Acceptance Criteria

- [ ] `state_getMetadata` publishes Metadata V16.
- [ ] The V16 registry fully describes the hybrid signature envelope.
- [ ] The V16 extrinsic section advertises `versions == [4, 5]`.
- [ ] Stock subxt decodes a signed balance transfer with no custom Rust types.
- [ ] Stock subxt decodes a mixed V5-bare/V4-signed block.
- [ ] Apps decodes and renders the same block without Quip-specific type
      registration.
- [ ] The existing Apps signedness patch remains covered by a regression test.
- [ ] The polkadot-js signing fixture is regenerated and matches the Rust
      generator.
- [ ] The runtime is released with `spec_version = 116` and
      `transaction_version = 7`.
- [ ] A decision on `@quip/types` is recorded after downstream compatibility
      testing.

## Rollout Order

1. Add protocol conformance tests.
2. Switch the legacy runtime metadata response to V16.
3. Bump the runtime spec version, regenerate the signing fixture, and run Rust
   CI.
4. Run the live-node subxt smoke test.
5. Validate Apps and update its protocol submodule.
6. Test other downstream consumers.
7. Introduce `@quip/types` only if the compatibility evidence requires it.

## Risks

- **Legacy metadata compatibility:** consumers that can only parse Metadata V15
  or older will break when `state_getMetadata` starts returning V16. The
  versioned metadata API keeps serving 14, 15, and 16, so only legacy-blob
  consumers are affected. Before release, build an explicit consumer matrix —
  polkadot-js/Apps version, polkadot-rest-api, the selected indexer, exchange
  tooling — and run smoke tests for each supported entry.
- **Metadata/wire drift:** the signature's metadata-only split must remain
  encoding-equivalent to the 731-byte wire signature. Pin both in tests.
- **Extension-version placeholder:** the SDK fork's V16 conversion assumes
  extension version 0 for every transaction extension. Correct today; pinned by
  the conformance test and tracked as a fork follow-up.
- **`CheckMetadataHash` interaction:** the extension ships in `TxExtension`
  with mode disabled. Switching the published metadata to V16 changes the
  hash a mode-1 consumer would compute. No impact while the mode stays off,
  but re-check the merkleized-metadata version interaction before enabling it.
- **Cross-repository ordering:** Apps validation must use the exact protocol
  revision containing the runtime change.
- **Duplicate sources of truth:** if `@quip/types` is introduced, validate it
  against runtime metadata in CI rather than maintaining definitions by hand.

## Out of Scope

- Changing the hybrid signature or account-id wire formats.
- Changing signing, fee estimation, or canonical signer behavior.
- Merkleized metadata and offline-signing flows.
- Removing the Apps Yarn patch before the upstream decoder fix is available.

# Metadata V15 External Decoding Plan

Related ticket: [QUI-901](https://linear.app/quip-network/issue/QUI-901/metadata-v15-published-type-bundle-so-external-tools-decode-hybrid)

## Goal

Make Quip's runtime metadata self-describing so stock metadata-driven tools can
decode hybrid extrinsics without hand-written Rust types or Quip-specific
decoder configuration.

The first phase publishes and tests Metadata V15. A separate `@quip/types`
package is introduced only if supported consumers still require explicit type
registration after the V15 rollout.

## Current State

The runtime already exposes Metadata V15 through the versioned metadata runtime
API. A local conformance probe established that:

- `HybridTxSignature` is a concrete metadata composite with `public` and
  `signature` fields;
- the public key resolves to `[u8; 1344]`;
- the signature resolves to `[u8; 2048]` followed by `[u8; 436]`, for a total
  of 2,484 bytes;
- stock `subxt-core 0.44.3` accepts the metadata with no custom types;
- stock subxt decodes a mixed block containing a V5 bare `Timestamp::set` and
  a V4 signed `Balances::transfer_allow_death`.

The remaining publication gap is the legacy metadata runtime API used by
`state_getMetadata`. The current implementation wraps `Runtime::metadata()`,
which intentionally returns Metadata V14.

The Apps Yarn patch is a separate concern. It fixes polkadot-js inferring
signedness from whether a custom signature value looks empty. Metadata V15 does
not remove the need for that fix.

## Decisions

1. Publish Metadata V15 from Quip's legacy `metadata()` runtime API.
2. Keep this as a Quip runtime override; do not change the SDK-wide
   `Runtime::metadata()` generator.
3. Bump `spec_version` for the runtime upgrade, but keep `transaction_version`
   unchanged because the extrinsic wire format does not change.
4. Keep the Apps decoder patch until the equivalent fix ships upstream.
5. Treat `@quip/types` as a conditional second phase, not a prerequisite for
   the Metadata V15 rollout.

## Phase 1: Runtime Metadata V15

### 1. Add conformance tests

Add a focused runtime integration test, for example:

```text
runtime/tests/metadata_v15.rs
```

Add `frame-metadata` and `subxt-core` as runtime dev dependencies through the
workspace dependency table.

The test must:

- request and decode `Runtime::metadata_at_version(15)`;
- assert the returned metadata is V15;
- inspect the extrinsic address, call, signature, extension, and signed
  extension types;
- assert that `HybridTxSignature` is not represented as `Vec<u8>` or another
  opaque sequence;
- pin the public-key length at 1,344 bytes;
- pin the signature layout at 2,048 plus 436 bytes;
- generate a V5 bare timestamp inherent and a V4 signed balance transfer;
- decode both with stock subxt using metadata only;
- assert signedness, pallet name, and call name for both extrinsics.

### 2. Publish V15 from `state_getMetadata`

Change the runtime API implementation in `runtime/src/apis.rs` from the V14
inherent metadata path to:

```rust
Runtime::metadata_at_version(15)
    .expect("Metadata V15 is supported")
```

Do not change
`polkadot-sdk/substrate/frame/support/procedural/src/construct_runtime/expand/metadata.rs`.
Changing the SDK generator would alter the legacy metadata behavior of every
runtime using the fork.

### 3. Version the runtime

- Bump `spec_version` from `112` to `113`.
- Keep `transaction_version` at `5`.
- Add a version comment explaining that 113 changes metadata publication only;
  consensus and extrinsic encoding are unchanged.

### 4. Run a live-node smoke test

Start a temporary dev chain and verify:

- `state_getMetadata` returns a V15 metadata blob;
- `Metadata_metadata_at_version(15)` returns the expected V15 graph;
- a stock online subxt client submits or observes a signed transfer;
- subxt decodes the containing block's V5 bare and V4 signed extrinsics;
- events and signed extensions decode without custom types.

## Polkadot SDK Scope

No production SDK change is required for Phase 1. The fork already contains the
large-array `TypeInfo` support needed for the H3 public key and signature.

An optional SDK hardening change can add unit tests that pin:

- public-key metadata as `[u8; 1344]`;
- signature metadata as the 2,048/436 split;
- the SCALE signature length at exactly 2,484 bytes;
- encoding equivalence between the runtime type and its metadata-only shape.

These tests protect the existing workaround but do not need to block the
runtime change.

## Apps Validation

After the runtime change lands:

1. Update the Apps `quip-protocol-rs` submodule.
2. Load the V15 metadata without Quip-specific `chainTypes`.
3. Decode a fixture containing a V5 bare inherent and a V4 signed hybrid
   transaction.
4. Confirm calls, accounts, events, and signed extensions render correctly.
5. Keep the existing `@polkadot/types` Yarn patch until its signedness fix is
   available in an upstream release.

Metadata V15 solves type discovery. The Apps patch solves signedness detection;
the two changes are complementary.

## Phase 2: Conditional `@quip/types`

Create a public `quip/types` repository and publish `@quip/types` only if a
supported consumer cannot use the published Metadata V15 directly.

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

- [ ] `state_getMetadata` publishes Metadata V15.
- [ ] The V15 registry fully describes the hybrid signature envelope.
- [ ] Stock subxt decodes a signed balance transfer with no custom Rust types.
- [ ] Stock subxt decodes a mixed V5-bare/V4-signed block.
- [ ] Apps decodes and renders the same block without Quip-specific type
      registration.
- [ ] The existing Apps signedness patch remains covered by a regression test.
- [ ] The runtime is released with `spec_version = 113` and
      `transaction_version = 5`.
- [ ] A decision on `@quip/types` is recorded after downstream compatibility
      testing.

## Rollout Order

1. Add protocol conformance tests.
2. Switch the legacy runtime metadata response to V15.
3. Bump the runtime spec version and run Rust CI.
4. Run the live-node subxt smoke test.
5. Validate Apps and update its protocol submodule.
6. Test other downstream consumers.
7. Introduce `@quip/types` only if the compatibility evidence requires it.

## Risks

- **Legacy metadata compatibility:** old consumers may assume
  `state_getMetadata` always returns V14. Mitigate with Apps and infrastructure
  smoke tests before release.
- **Metadata/wire drift:** the signature's metadata-only split must remain
  encoding-equivalent to the 2,484-byte wire signature. Pin both in tests.
- **Cross-repository ordering:** Apps validation must use the exact protocol
  revision containing the runtime change.
- **Duplicate sources of truth:** if `@quip/types` is introduced, validate it
  against runtime metadata in CI rather than maintaining definitions by hand.

## Out of Scope

- Changing the hybrid signature or account-id wire formats.
- Changing signing, fee estimation, or canonical signer behavior.
- Merkleized metadata and offline-signing flows.
- Removing the Apps Yarn patch before the upstream decoder fix is available.

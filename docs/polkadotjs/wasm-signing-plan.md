# Polkadot.js Extrinsic Signing Plan

## Goal

Enable Quip accounts in the local Polkadot.js Apps checkout (`../apps`) to
estimate fees, sign, submit, and decode ordinary runtime extrinsics, starting
with:

- `system.remark`
- `balances.transferKeepAlive`

Balance transfers for known accounts and WASM-backed extrinsic decoding already
work. This plan completes the missing signing path for Quip's hybrid transaction
signature.

## Scope

The first implementation targets:

- the existing opt-in development signer in `../apps`;
- funded development accounts;
- accounts imported through the Quip mnemonic dialog;
- the existing browser WASM package and injected signer interface;
- automated wire-format and end-to-end tests.

The first implementation does not include production key custody. The
`DevSeedProvider` remains development-only. A production browser extension or
isolated background signer is a follow-up phase.

No runtime change is expected. If the runtime signature type, signed extensions,
or encoded transaction format must change, stop and review runtime versioning
before proceeding.

## Signing Contract

The intended path is:

```text
Apps transaction
  -> tx.signAsync(account, { signer })
  -> Polkadot.js SignerPayload.toRaw()
  -> QuipSigner.signRaw()
  -> sign raw payload, or Blake2-256(payload) when payload length > 256
  -> WASM produces SCALE(HybridTxSignature { public, signature })
  -> Polkadot.js inserts the returned envelope into the extrinsic
  -> runtime derives the account from the embedded public key and verifies it
```

The runtime contract is:

```text
AccountId = blake2b_256("quip-account-v1" || full_h4_public_key_bytes)
HybridTxSignature.public    = [u8; 929]
HybridTxSignature.signature = [u8; 731]
encoded signature envelope  = 1660 bytes
```

The returned signer result must contain the raw SCALE-encoded hybrid envelope.
It must not contain a `MultiSignature` variant byte.

Quip accounts must remain injected accounts with source `quip`. They must not
fall through to Apps' standard local `AccountSigner`, because that signer only
supports conventional Polkadot key types.

Use `Signer.signRaw`, not `Signer.signPayload`. Polkadot.js supplies
`signRaw` with the fully SCALE-encoded `ExtrinsicPayload` through
`SignerPayload.toRaw()`. Reconstructing that payload manually would duplicate
metadata and signed-extension logic.

## Phase 1: Pin Shared Wire Fixtures

Add a generated fixture that Rust and TypeScript tests consume. It should
contain:

- deterministic test seed;
- H4 public key;
- derived account id and SS58 address;
- representative `SignerPayloadJSON`;
- SCALE-encoded raw signing payload;
- actual message signed;
- SCALE-encoded hybrid signature envelope;
- complete signed extrinsic.

Include payload cases of 255, 256, and 257 bytes to lock down Substrate's rule:
payloads longer than 256 bytes are Blake2-256 hashed before signing.

Generate the fixture from Rust rather than maintaining long encoded values by
hand. Rust tests must verify that the fixture agrees with:

- `transaction-crypto-core`;
- `HybridTxSignature::encode()`;
- runtime transaction validation.

TypeScript tests must verify `messageToSign`, envelope bytes, and signed
extrinsic decoding against the same fixture.

## Phase 2: Harden the Browser Signer

Retain the existing architecture in `js/quip-signer`:

- `QuipSigner.signRaw` applies the 256-byte payload rule;
- `DevSeedProvider` resolves the seed by decoded account id, independent of the
  SS58 prefix;
- WASM signs the exact message bytes it receives.

Add or complete:

- strict address and account-id validation;
- a 1660-byte envelope length check;
- WASM `verifyEnvelope` verification before a development signature is
  returned;
- clear errors for unknown accounts, malformed addresses, unavailable WASM
  exports, and invalid envelopes;
- sequence-safe signer result ids;
- TypeScript tests for 255/256/257-byte payloads;
- tests for unknown and mismatched accounts;
- tests proving no `MultiSignature` discriminant is added;
- typechecking using the actual installed Polkadot.js interfaces.

Do not log or persist mnemonic phrases or raw seeds. Development seed storage
must remain in page memory and behind the existing explicit opt-in.

## Phase 3: Complete Apps Routing

In `../apps`:

1. Initialize the Quip injection before Apps calls `web3Enable`.
2. Ensure known and mnemonic-imported accounts are marked as injected with
   source `quip`.
3. Verify the transaction modal resolves `web3FromSource("quip")`.
4. Pass the resulting injected signer to `tx.signAsync`/`signAndSend`.
5. Keep view-only accounts visible, but disable signing when their provider has
   no corresponding secret.
6. Display actionable errors for unavailable, locked, or rejected signing.

Retain fee-estimation support for the custom signature size. Scope and test the
`GenericExtrinsicSignatureV4.signFake` patch so `paymentInfo` constructs a
1660-byte fake `ExtrinsicSignature` for Quip without changing other chains'
signature handling.

Use runtime metadata and Polkadot.js signed extensions. Do not manually assemble
the nonce, era, genesis hash, block hash, transaction version, or signed
extension fields.

## Phase 4: Validate Signed Extrinsics

Exercise the path in this order:

1. `system.remark` from a funded known account;
2. `balances.transferKeepAlive` from Alice to Bob;
3. a transfer in the opposite direction;
4. a funded account imported through the mnemonic dialog;
5. a long payload that triggers Blake2-256 signing.

For each relevant case, assert:

- fee estimation succeeds;
- the Quip injected signer is called;
- the signed extrinsic version and signed bit are correct;
- the signer matches the account derived from the embedded public key;
- the signature field is the 1660-byte hybrid envelope;
- the existing decoder round-trips the signed extrinsic;
- the node includes the transaction;
- expected success events, nonce changes, and balance changes occur.

Negative cases must cover:

- a tampered envelope;
- an envelope signed by a different account;
- an unknown Quip account;
- a stale nonce where practical.

The invalid signature cases should be rejected as `InvalidTransaction::BadProof`
or the equivalent RPC error.

## Phase 5: Automate the Integration

Add a local-node integration test that covers the signing protocol independently
of the UI. Add a focused headless Apps test for account discovery, signer
routing, fee estimation, and one successful submission.

CI should:

- build the browser WASM package deterministically;
- run transaction-crypto Rust tests;
- run signer TypeScript tests and typechecking;
- start a local dev node for the signing integration test;
- prevent production builds from enabling the development seed provider.

Avoid committing generated WASM artifacts unless the repository's existing
release process requires them. Pin the toolchain and `wasm-pack` version used to
produce release artifacts.

## Phase 6: Production Signer Follow-up

After the development path is proven, move production signing into a browser
extension or isolated background context with:

- encrypted key storage;
- explicit unlock and transaction confirmation;
- origin authorization;
- no seed exposure to the Apps page;
- the same `signRaw` and hybrid-envelope contract.

Add metadata registration only if a supported Apps flow demonstrates that it is
required.

## Validation Commands

Protocol:

```bash
cargo test -p quip-transaction-crypto-core \
  -p quip-transaction-crypto \
  -p quip-transaction-crypto-wasm
cargo test -p quip-protocol-runtime hybrid_signed_extrinsic
cargo check -p quip-transaction-crypto-wasm --target wasm32-unknown-unknown
```

Browser signer:

```bash
make wasm-signer
cd js/quip-signer
yarn typecheck
yarn test
```

Apps:

```bash
cd ../apps
QUIP_DEV_SIGNER=1 yarn start
```

Use repository-supported equivalents where a listed test script does not yet
exist; adding the missing focused test script is part of the implementation.

## Acceptance Criteria

The implementation is complete when:

- known and imported Quip accounts route through `QuipSigner.signRaw`;
- `system.remark` and `balances.transferKeepAlive` can be fee-estimated,
  signed, submitted, included, and decoded;
- the raw hybrid signature envelope is inserted without a `MultiSignature`
  prefix;
- 255/256/257-byte payload behavior is pinned by shared fixtures;
- invalid account/signature combinations are rejected;
- automated Rust, TypeScript, and local-node integration tests cover the path;
- production page code does not receive or persist raw seed material.

## Versioning

Client-only signer and Apps changes require no runtime version bump.

Review `spec_version` if runtime metadata, account types, signed extensions, or
signature type metadata change.

Review `transaction_version` if signed extrinsic bytes change, including the
`HybridTxSignature` SCALE shape, signed-extension order, or signed payload
fields.

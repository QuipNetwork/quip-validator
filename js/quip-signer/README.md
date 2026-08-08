# @quip-network/quip-signer

TypeScript wrapper for Quip browser transaction signing.

The package expects a WASM module built from
`crates/transaction-crypto-wasm`. The wrapper keeps the WASM API structural so
the generated module can be imported directly or passed through extension
background/page boundaries.

## Development Usage

```ts
import * as wasm from 'quip-transaction-crypto-wasm';
import { DevSeedProvider, QuipSigner, injectQuip } from '@quip-network/quip-signer';

const { accounts, provider } = await DevSeedProvider.fromSeeds(wasm, [
  {
    name: 'Alice',
    seedHex: '0x0707070707070707070707070707070707070707070707070707070707070707'
  }
]);

injectQuip({
  accounts,
  signer: new QuipSigner(provider)
});
```

`DevSeedProvider` is for fixtures and local smoke tests only. A production
extension should keep private key material in extension-controlled storage and
only expose the injected accounts plus the signer.

## Mnemonic Import

`DevSeedProvider.fromMnemonics` derives master seeds from BIP39 phrases using
the WASM module, matching substrate's `Pair::from_phrase`, so imported accounts
resolve to the same addresses the runtime recognizes:

```ts
const { accounts, provider } = await DevSeedProvider.fromMnemonics(wasm, [
  {
    name: 'Alice',
    mnemonic: 'bottom drive obey lake curtain smoke basket hold race lonely fit walk'
  }
]);
```

Each `mnemonic` may be an English BIP39 phrase, optionally followed by
`///<password>`, or a `0x`-prefixed 64-digit hex seed. Derivation junctions
(`//hard`, `/soft`) are intentionally not supported and are rejected.

The underlying `wasm.seedFromMnemonic(secretUri)` export returns the master seed
hex, which can also be passed to `publicFromSeed` / `signPayloadFromSeed`.

## Signer Contract

`QuipSigner` implements `signRaw` (not `signPayload`): polkadot-js hands
`signRaw` the fully SCALE-encoded `ExtrinsicPayload` bytes via `toRaw()`, so the
signer needs no metadata-aware registry. The signer reproduces substrate's
`SignedPayload::using_encoded` rule (blake2-256 the payload when it exceeds 256
bytes, otherwise sign verbatim), signs with H3, and returns:

```ts
{
  id,
  signature
}
```

`signature` is the SCALE-encoded `HybridTxSignature { public, signature }`
envelope expected by the Quip runtime. It is not a `MultiSignature` variant.

The wrapper validates every seed, account, payload, public key, and returned
envelope at its boundary. If the WASM module does not expose
`verifyEnvelope`, or verification fails, signing stops before the envelope is
returned to polkadot-js.

Quip's fixed-size signature also needs the fee-estimation compatibility patch.
Pass the `GenericExtrinsicSignatureV4` class from the same polkadot-js
installation as the consuming application:

```ts
import { GenericExtrinsicSignatureV4 } from '@polkadot/types';
import { patchExtrinsicSignFake } from '@quip-network/quip-signer';

patchExtrinsicSignFake(GenericExtrinsicSignatureV4);
```

The patch only changes `signFake` when the active registry declares Quip's
3,828-byte `ExtrinsicSignature`; every other registry delegates to polkadot-js's
original implementation.

## Validation

From the protocol repository:

```sh
make wasm-signer
npm ci --prefix js/quip-signer
npm run typecheck --prefix js/quip-signer
npm test --prefix js/quip-signer
npm run build --prefix js/quip-signer

# With a dev node already listening on ws://127.0.0.1:9944
# (build it with: cargo build -p quip-network-node --features dev-chain-id)
npm run test:integration --prefix js/quip-signer
```

The unit suite consumes the Rust-generated fixture and loads the actual WASM
artifact. The local-node integration estimates fees, submits short and
over-256-byte payloads, exercises two accounts and mnemonic import, and confirms
that tampered envelopes, mismatched accounts, stale nonces, and unknown local
accounts are rejected.

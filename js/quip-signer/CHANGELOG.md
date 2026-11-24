# Changelog

## 0.2.0 (unreleased, breaking)

Not published yet. The WASM module it wraps comes from the same quip-validator
build.

### Breaking

- **Phrase-derived keys change.** `seedFromMnemonic`, and therefore
  `DevSeedProvider.fromMnemonics`, now derive the QUIP HD v1 key for the
  transaction suite: the standard BIP39 seed (PBKDF2-HMAC-SHA512 over the NFKD
  phrase, salt `"mnemonic"` + NFKD password, 2048 rounds) at
  `m/0x514E'/0x400'/20049'/0'/0'`. 0.1.0 used Substrate's entropy-based seed.
  The same phrase now gives a different account, the one the node and the Quip
  SDK derive. Re-import such accounts from the phrase, or keep using their
  `0x` seed. `0x` seeds derive exactly as before.
- **Secret URIs follow the node's grammar, without derivation.** Rejected now:
  - a phrase character other than ASCII letters, digits and spaces (tabs,
    no-break spaces, quotes, line breaks);
  - an empty phrase (`""`, `///pw`): there is no development phrase here;
  - an uppercase `0X` prefix;
  - `///` after a `0x` seed, even an empty one;
  - any `/` or `//` junction, as before. The signers take a bare phrase
    (optionally `///password`) or a raw `0x` seed. An account derived with
    junctions (`//…`) can't be imported into the signers in this release; a
    hybrid `key inspect` scheme in the SDK fork is follow-up QUI-2531.
  A `/` inside the password (`phrase///p/w`) is part of the password.
- **Error text changed.** WASM errors describe the problem and never include
  the input. `seedFromMnemonic` errors start with `invalid secret URI:`.

### Security

- `verifyEnvelope` refuses a public key whose sr25519 half is the Ristretto
  identity (GregoAI wp-2 G-002). Such a key's classical signature can be forged
  without a secret.

### Tests

- `npm test` checks the QUIP HD golden parity file and the H4 golden vectors
  through the real WASM build.

## 0.1.0

First version.

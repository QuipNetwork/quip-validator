# Changelog

The `quip-signer` version is the quip-validator node release version, set by
the release commit (see `pyproject.toml`, `dynamic = ["version"]`).

## Unreleased (breaking)

Ships with the first node release that includes it, which must be at least
0.6.0 (a breaking change after 0.5.1).

### Breaking

- **Phrase-derived keys change.** `seed_from_mnemonic` and
  `HybridSigner.from_mnemonic` now derive the QUIP HD v1 key for the
  transaction suite: the standard BIP39 seed (PBKDF2-HMAC-SHA512 over the NFKD
  phrase, salt `"mnemonic"` + NFKD password, 2048 rounds) at
  `m/0x514E'/0x400'/20049'/0'/0'`. Earlier releases used Substrate's
  entropy-based seed. The same phrase now gives a different account, the one
  the node and the Quip SDK derive. `0x` seeds derive exactly as before.
- **Secret URIs follow the node's grammar, without derivation.** Rejected now:
  - a phrase character other than ASCII letters, digits and spaces (tabs,
    no-break spaces, quotes, line breaks);
  - an empty phrase (`""`, `///pw`): there is no development phrase here;
  - an uppercase `0X` prefix;
  - `///` after a `0x` seed, even an empty one;
  - any `/` or `//` junction, as before. The error now says to derive the
    account elsewhere and import its `0x` secret seed.
  A `/` inside the password (`phrase///p/w`) is part of the password.
- **Error text changed.** `QuipSignerError` messages describe the problem and
  never include the input. Before, they were the Rust error's variant name.

### Security

- `verify_envelope` refuses a public key whose sr25519 half is the Ristretto
  identity (GregoAI wp-2 G-002). Such a key's classical signature can be forged
  without a secret.

### Tests

- `tests/test_quip_hd_parity.py` checks the shared QUIP HD golden parity file.

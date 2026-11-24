# QUIP HD v1 and hybrid key hardening (unreleased)

Status: **unreleased.** These changes come from GregoAI audit work package 2,
batch 1 (findings G-001, G-002 and G-003). They change how every phrase-derived
hybrid key is computed. Read "Release blockers" before shipping anything from
here.

## What changes for users

- **Phrase-derived keys change.** A BIP39 phrase now selects its QUIP HD v1 key:
  the standard BIP39 seed (PBKDF2-HMAC-SHA512 over the NFKD phrase, salt
  `"mnemonic"` + NFKD passphrase, 2048 rounds), then hardened derivation along
  `m/0x514E'/<suite>'/20049'/<account>'/<index>'`. The suite levels are
  `0x100` (H1, ed25519 + ML-DSA-44), `0x200` (H2, ed25519 + FN-DSA-512, the
  GRANDPA suite), `0x300` (H3, sr25519 + ML-DSA-44) and `0x400` (H4,
  sr25519 + FN-DSA-512, the BABE and transaction suite). The same phrase, and
  `//Alice` and the other development accounts, now give different keys and
  accounts than before. Raw `0x` seeds without junctions are unchanged.
- **Existing testnet keystores stop matching.** A node keystore re-derives its
  keys from the stored phrase on every start, so a testnet validator upgraded
  to this binary would get `PublicKeyMismatch` and stop authoring. See L5
  below.
- **The browser and Python signers take no junctions.** They take a bare
  phrase (optionally `///password`) or a raw `0x` seed. An account derived with
  junctions (`//…`) can't be imported into the signers in this release. The
  SDK fork's `key inspect` has no hybrid scheme, so its seed for such a path is
  an sr25519 seed for a different account (follow-up QUI-2531). The signers
  also refuse an empty phrase, since they have no development phrase.
- **`0X` is refused everywhere.** Only a lowercase `0x` prefix marks a raw seed.
  Uppercase hex digits after it are fine.

## Node and runtime (quip-validator)

- `insert-hybrid-key`:
  - never prints the SURI it rejects (G-001);
  - prompts for the SURI, without echo, when `--suri` is omitted;
  - strips a UTF-8 byte-order mark from a SURI file;
  - derives with the keystore password, as the keystore does on every start;
  - **refuses a SURI `///password` together with a keystore password.** Pass
    the bare phrase and the node's `--password`, `--password-filename` or
    `--password-interactive` instead.
- Genesis refuses a BABE or GRANDPA session key whose classical half is
  degenerate (G-002): the sr25519 identity, or a small-order ed25519 point. This
  covers the mainnet and relaunch inputs and the live testnet preset. It runs
  the SDK's own check, and the live testnet preset is byte-for-byte unchanged.
- The dev, local, local3 and rehearsal presets carry QUIP HD keys, and their
  genesis hashes change. The live testnet spec (`chain-specs/quip-testnet.json`,
  SHA-256 `0aeeddaf…ad4`) does not change.
- `transaction-crypto-core`:
  - secret URIs follow the SDK fork's grammar without junctions, and a phrase
    gives the QUIP HD H4 key;
  - the envelope `verify` refuses an sr25519 identity public key (G-002);
  - errors carry no input.
- Operator tooling: `scripts/derive-operator-keys.sh` and the
  `derive_genesis_keys` example document QUIP HD. When the keystore has a
  password, derive the genesis material with `"<mnemonic>///<password>"` and
  insert the bare mnemonic with the node's password option. Two caveats:
  - `--password-filename` is read verbatim, including a trailing newline, and
    a SURI password cannot contain a newline, so `"<mnemonic>///<pw>"` never
    matches a password file that ends in one. Write the file with
    `printf '%s' "$pw" > file`, or use `--password` or
    `--password-interactive`.
  - The TX (stash) account comes from the BABE key derived with `///<pw>`.
    To control it from the browser or Python signer, enter
    `"<mnemonic>///<pw>"` there too; the bare mnemonic gives a different
    account.
- `spec_version` stays at 120 on this branch; see the blockers.

## SDK fork (QuipNetwork/polkadot-sdk, `grego/audit-wp2/hybrid-keys`)

- `from_phrase` and `from_string`/`from_string_with_seed` for all four hybrid
  suites use QUIP HD v1. A secret URI's phrase junctions are QUIP HD levels:
  none means account 0, index 0; one sets the account; two set account and
  index. An all-digit junction is its value. A name is
  `blake2_256(SCALE(name))[0..4]` as little-endian, masked to 31 bits.
- **`from_phrase(p).derive(//1)` is not `from_string("p//1")`.** On a pair,
  `derive` applies the HKDF junction to the pair's seed. In a SURI, the
  junction is a QUIP HD level. Upstream treats the two as equal; they are not
  equal here.
- SURI rules:
  - at most two hard junctions;
  - soft junctions are refused;
  - an all-digit junction must be canonical and below 2^31, so `//01`, `//+1`
    and `//2147483648` are refused;
  - `0X` is refused;
  - any `///` after a `0x` seed is refused, even an empty one, and a keystore
    password does not apply to a `0x` seed;
  - an empty phrase still means the development phrase.
- **`0x…//junction` outputs change.** On a raw seed, a hard junction now
  applies pqhybridsign's HKDF junction with the upstream `DeriveJunction`
  chain code, replacing sr25519/ed25519 hierarchical derivation (G-003).
- Weak classical keys are refused on every verification surface (G-002): the
  sr25519 identity for H3/H4, the 14 small-order ed25519 encodings for H1/H2,
  and the H3/H4 VRF paths.
- SURI parse errors carry no part of the input, and the `sc-cli` key commands
  no longer echo a rejected SURI.
- The BABE and GRANDPA `AuthorityPair` types forward `from_string_with_seed`
  to the inner hybrid pair. Otherwise `AuthorityPair::from_string("//Alice")`
  would silently differ from the inner pair.
- pqhybridsign is pinned by git revision until 0.1.1 is published.

## pqhybridsign 0.1.1 (unpublished)

- New `quip_hd` module, behind the `quip-hd` feature: QUIP HD v1 master and
  hardened derivation, BIP39 handling and `hybrid_leaf`, with known-answer
  tests and a golden parity file shared by every layer.
- `hybrid_leaf` is exposed through the Rust ABI (error codes 23
  `HdIndexOutOfRange`, 24 `HdSeedTooShort`), the C header, Python and wasm.
- CI requires wasm-pack 0.13.1 for the checked-in TypeScript types.

## npm `@quip-network/quip-signer` 0.2.0 (unpublished, breaking)

See `js/quip-signer/CHANGELOG.md`. Phrase keys move to QUIP HD; the
secret-URI rules above apply; errors carry no input; `verifyEnvelope` refuses
the sr25519 identity.

## PyPI `quip-signer`, next release (at least 0.6.0, breaking)

See `crates/transaction-crypto-py/CHANGELOG.md`. The changes are the same as
for npm. The version is the node release version.

## Release blockers

- **L5, testnet key rotation.** No testnet node may run this binary before a
  key-rotation plan exists. Upgraded validators would stop authoring, and
  signer users' phrase accounts move. A fresh mainnet genesis does not need the
  plan.
- **The node release must be at least 0.6.0.** main is at 0.5.1, and this is
  a breaking change. The PyPI wheel takes the same version.
- **`spec_version` must be bumped above main's 121 at integration.** It is
  120 here. `js/quip-signer/test/revive-fund-account.mjs` follows it through
  `scripts/regenerate-quip-hd-fixtures.sh`.
- **Publish pqhybridsign 0.1.1, then repin.** The validator workspace, the
  Python signer crate and the SDK fork all pin pqhybridsign by git revision.
  Each moves to `=0.1.1`.
- **The SDK pull request onto #12** (opened by the owner), and the validator's
  SDK pin moved to the merged revision.

## Follow-ups outside this batch

Tracked in the batch plan's "Out of scope" list, items L1 to L4:

- **L1:** quip-miner still derives keys the old way.
- **L2:** xqsa's `quip-signer` version pin is stale.
- **L3:** record the QUIP HD algorithm IDs in ethereum-sdk's registry.
- **L4:** update §9 and §10 of the external hybrid-signature specification.

QUI-2531 tracks a hybrid `key inspect` scheme in the SDK fork, so that an
account derived with junctions can be exported and imported into the signers.

QUI-2530 tracks the four runtime tests that fail under `runtime-benchmarks`.
They fail the same way before this batch.

## Regenerating fixtures

Every checked-in value that pins a QUIP HD-derived key is regenerated by:

```bash
npm ci --prefix js/quip-signer
bash scripts/regenerate-quip-hd-fixtures.sh
```

# Hybrid Crypto Deduplication Plan (Option A)

> **Current state (H2/H4 migration):** Transaction signing and consensus now
> use `pqhybridsign` H4 (`sr25519 + FN-DSA-512`) and H2
> (`ed25519 + FN-DSA-512`). `quip-transaction-crypto-core` calls the library's
> `composite_delta` implementation directly, while the SDK supplies the
> Substrate wrappers. The H3-focused material below records the earlier
> deduplication design and remains relevant only to the legacy H1/H3 follow-up.
>
> `pqhybridsign` is AGPL-3.0-or-later. Its pinned FN-DSA-512 implementation is
> pre-final-FIPS-206; a dependency or wire-format change is consensus-breaking
> and must be paired with runtime/transaction version bumps and regenerated
> vectors.

## Goal

Eliminate the duplicated constants and signing/verification logic between:

- **`quip-transaction-crypto-core`** (this repo, `crates/transaction-crypto-core`) —
  the `no_std`, dependency-light byte implementation used to build the browser
  signer WASM.
- **`quip-crypto-primitives`** (the SDK fork,
  `polkadot-sdk/quip/primitives/crypto`, pulled via the
  `QuipNetwork/polkadot-sdk` `v0.2` git dependency) — the canonical hybrid
  signature scheme the runtime is built from.

Both implement the **H3 suite** (`sr25519 + ML-DSA-44`). They are byte-identical
**by necessity**: the runtime verifies exactly what the browser signs. Today
that parity is maintained by hand in two places, which is fragile — any drift
silently breaks every signed extrinsic.

## What is duplicated (must stay in lockstep)

| Concern | `transaction-crypto-core` | `quip-crypto-primitives` |
|---|---|---|
| Message framing `v ‖ label ‖ len(ctx) ‖ ctx ‖ msg` | `prepare_message` (`lib.rs:417`) | `domain.rs::prepare_message` |
| HKDF seed split (`hybrid-sig` / `classical` / `pq`) | `derive_component_seeds` (`lib.rs:268`) | `seed.rs` |
| Label `hybrid-sr25519-mldsa44-v1\0`, version `0x01` | `H3_LABEL` / `H3_VERSION` (`lib.rs:40-41`) | `suite/sr25519_mldsa44.rs` + `suite/mod.rs::DEFAULT_HYBRID_SIGNATURE_VERSION` |
| Lengths 32/64/64, 1312/2560/2420 | consts (`lib.rs:28-34`) | `classical/sr25519.rs`, `pq/mldsa44.rs` |
| Deterministic sr25519 (`Blake2Rng`, ctx `b"substrate"`) | `sr25519_sign_deterministic` + `Blake2Rng` (`lib.rs:428,500`) | `classical/sr25519.rs` (identical `Blake2Rng`) |
| ML-DSA-44 sign/verify | `ml_dsa_*` (`lib.rs:466-486`) | `pq/mldsa44.rs` |
| Compose / sign / verify flow | `keypair_from_seed` / `sign_h3_deterministic` / `verify_h3` | `fixed.rs` generic engine |

### NOT duplicated — stays in `transaction-crypto-core`

The SDK crate has none of these (no `bip39` dependency, no account model):

- Account-id derivation: `ACCOUNT_ID_DOMAIN = b"quip-account-v1"` +
  `account_id_from_public_bytes` (`lib.rs:54,119`).
- SCALE envelope `HybridTxSignatureBytes` (`lib.rs:77`).
- BIP39 / secret-URI → seed: `master_seed_from_secret_uri`,
  `master_seed_from_mnemonic`, `decode_seed_hex` (`lib.rs:149,176,201`).

## The constraint

`quip-crypto-primitives` depends on `sp-core`, `sp-application-crypto`, and
**`sp-io`**. `sp-io` cannot link in a standalone browser WASM without runtime
host functions, and the bundle must stay small (`-Oz`, ~400 KB). So
`transaction-crypto-core` cannot simply depend on `quip-crypto-primitives`.

### Key enabling finding

In the SDK crate, `sp_*` usage is confined to:

- `substrate/*` — the Substrate `Pair`/`Public`/`Signature` wrappers (expected,
  stays `sp`-bound).
- **`classical/sr25519.rs` only** — uses `sp_core::sr25519::Pair`
  (`from_seed`, `verify`) and `sp_core::hashing::blake2_256`.

Every other module (`domain`, `seed`, `fixed`, `pq/mldsa44`, `suite`, `error`)
is already `sp`-free (`schnorrkel` / `fips204` / `blake2` / `hkdf` / `sha2` /
`rand_core` / `subtle` / `zeroize` only).

`classical/sr25519.rs` can be made `sp`-free with direct `schnorrkel` + `blake2`
calls — and `transaction-crypto-core` **already proves these produce identical
bytes** (the two interoperate in production today):

- `sr25519::Pair::from_seed_slice(seed)` ≡ `MiniSecretKey::from_bytes(seed).expand_to_keypair(Ed25519)`
- `sr25519::Pair::verify(...)` ≡ `PublicKey::verify_simple(b"substrate", ...)`
- `sp_core::hashing::blake2_256(x)` ≡ `Blake2bVar::new(32) … finalize`

So the entire pure crypto stack can move into one `sp`-free crate.

## Target architecture

```
polkadot-sdk/quip/primitives/
├── crypto-core/                 # NEW: quip-crypto-primitives-core (no_std, sp-FREE)
│   └── src/{domain,seed,fixed,error,pq/*,suite/*,classical/*}.rs
│       deps: schnorrkel, ed25519-zebra, fips204, blake2, hkdf, sha2,
│             rand_core, subtle, zeroize (+ codec/scale-info if required)
└── crypto/                      # quip-crypto-primitives (unchanged public API)
    └── src/substrate/*.rs       # sp-bound wrappers; re-exports crypto-core
        deps: quip-crypto-primitives-core + sp-core/sp-io/sp-application-crypto

quip-protocol-rs/crates/transaction-crypto-core/   # SHRINKS
    depends on quip-crypto-primitives-core (sp-free) for the H3 suite;
    keeps ONLY: account-id (quip-account-v1), HybridTxSignatureBytes envelope,
    BIP39/secret-URI seed helpers, and thin sign/verify wrappers.
```

Dependency direction stays correct (SDK is upstream of `quip-protocol-rs`), and
`quip-protocol-rs` consumes the new crate over the **existing**
`QuipNetwork/polkadot-sdk` `v0.2` git dependency.

## Steps

### Phase 1 — SDK fork (`polkadot-sdk`, branch `v0.2`)

1. Create `quip/primitives/crypto-core` crate (`quip-crypto-primitives-core`),
   `no_std`, with the `sp`-free dependency set above.
2. Move `domain.rs`, `seed.rs`, `fixed.rs`, `error.rs`, `pq/`, `suite/`,
   `classical/` into it. Keep the module layout and public API.
3. Rewrite `classical/sr25519.rs` to drop `sp_core`:
   - `from_seed`: `schnorrkel::MiniSecretKey::from_bytes(seed).expand_to_keypair(ExpansionMode::Ed25519)`.
   - `verify`: `schnorrkel::PublicKey::verify_simple(b"substrate", …)`.
   - `blake2_256_seed_counter`: `Blake2bVar` instead of `sp_core::hashing`.
   - Confirm the workspace lints / `no_std` build pass for the new crate.
4. In `quip-crypto-primitives` (`crypto/`): depend on `crypto-core`, delete the
   moved modules, keep `substrate/*`, and `pub use` the core so the existing
   public API (`Sr25519MlDsa44`, `HybridSignatureScheme`, `domain`, `seed`,
   suite consts, `HybridSignatureError`) is unchanged for current consumers.
5. Run the SDK crate's existing suite tests (`suite/sr25519_mldsa44.rs` test
   module) — they must pass against the relocated code unchanged.
6. Land on `v0.2`.

### Phase 2 — `quip-protocol-rs`

7. Add `quip-crypto-primitives-core` to the workspace `Cargo.toml`
   (`git = QuipNetwork/polkadot-sdk, branch = v0.2, default-features = false`),
   mirroring the existing `quip-crypto-primitives` entry.
8. Rewrite `crates/transaction-crypto-core/src/lib.rs`:
   - **Delete** the duplicated constants and logic: lengths, HKDF strings,
     `H3_LABEL`/`H3_VERSION`, `SUBSTRATE_SIGNING_CONTEXT`, `prepare_message`,
     `derive_component_seeds`, `keypair_from_seed`, `sr25519_*`, `ml_dsa_*`,
     `sign_h3_deterministic`, `verify_h3`, `Blake2Rng`, the `*_from_bytes`
     validators.
   - **Keep & re-base on the shared suite**:
     - `HYBRID_PUBLIC_LEN` / `HYBRID_SIGNATURE_LEN` / `HYBRID_SECRET_LEN` ←
       the suite's `HYBRID_PK_LEN` / `HYBRID_SIG_LEN` / `HYBRID_SK_LEN`.
     - `public_key_from_seed`, `sign_payload_from_seed`,
       `sign_payload_from_secret`, `HybridTxSignatureBytes::verify` → thin
       wrappers over `Sr25519MlDsa44::{from_seed_slice, sign_deterministic,
       public, verify}` with `ctx = b""`, `nonce = b""`.
     - `account_id_from_public_bytes` / `ACCOUNT_ID_DOMAIN` — unchanged
       (quip-specific).
     - `HybridTxSignatureBytes` envelope — unchanged (SCALE, quip-tx-specific).
     - `master_seed_from_*` / `decode_seed_hex` — unchanged (BIP39).
   - Map the suite's `HybridSignatureError` into `HybridTxCryptoError` at the
     boundary so the public error type is preserved.
9. Trim `transaction-crypto-core/Cargo.toml`: drop deps now provided by the
   shared crate (`fips204`, `schnorrkel`, `hkdf`, `sha2`, `rand_core` may go if
   no longer referenced directly); keep `bip39`, `substrate-bip39`, `blake2`
   (account-id), `codec`, `zeroize` as needed.
10. Keep the crate `no_std` + `default = ["std"]`; ensure the WASM crate
    (`transaction-crypto-wasm`) still builds.

### Phase 3 — Parity gate (do this regardless of phase order)

11. Add a checked-in golden-vector fixture and a test asserting both
    implementations agree, so they can never silently drift again:
    - Vectors: `seed → public_key`, and `(seed, msg) → signature` for a few
      fixed seeds/messages (incl. a BIP39-derived seed).
    - Generate from `quip-crypto-primitives` (`Sr25519MlDsa44`), assert
      `transaction-crypto-core` reproduces them (and that the runtime verifier
      accepts them). Place under `crates/transaction-crypto-core/tests/`,
      following the existing `quantum-validation` fixture pattern.
12. Capture the current (pre-refactor) signer output as the baseline vectors
    **before** deleting code, so Phase 2 is verified against today's bytes.

## Verification checklist

- [ ] `cargo build -p quip-crypto-primitives-core --no-default-features` (sp-free, `no_std`).
- [ ] SDK `quip-crypto-primitives` tests green; public API unchanged (no consumer edits needed).
- [ ] `cargo test -p quip-transaction-crypto-core` green, incl. new parity fixtures.
- [ ] `make wasm-signer` builds; bundle size not materially larger than ~400 KB.
- [ ] End-to-end: a browser-signed extrinsic still verifies in the runtime
      (the `apps` dev signer with `QUIP_DEV_SIGNER=1`).

## Risks & mitigations

- **Parity-critical, security-sensitive code.** Mitigation: golden vectors
  captured *before* the refactor (step 12); refactor is a pure move + dep-swap,
  not a logic change.
- **Cross-repo / branch coordination.** Phase 1 has landed on `v0.2`, so
  Phase 2 consumes `quip-crypto-primitives-core` over the standard
  `QuipNetwork/polkadot-sdk` `v0.2` git dependency — the same source/branch as
  every other SDK crate. (During development this was a temporary feature-branch
  pin; that has been switched back to `v0.2`.)
- **`classical/sr25519.rs` sp-swap changing bytes.** Mitigation: the swap
  targets are already-proven equivalents (core uses them today); the parity
  fixtures catch any deviation immediately.
- **WASM bloat from accidental `sp-*` pull-in.** Mitigation: the new crate has
  no `sp-*` dependency at all; add a `cargo tree` check on the WASM crate.

## Out of scope

- The `ed25519_mldsa44` suite (consensus keys) — moves with the rest into
  `crypto-core` but no `transaction-crypto-core` consumer change.
- Renaming the public H3 API or the envelope wire format.
- Any change to `quip-account-v1` derivation or the SCALE envelope layout.

# Plan: Integrate H2 and H4 (from pqhybridsign) into quip-validator, replacing H1/H3

## Goal

Replace the existing H1 (ed25519+ML-DSA-44, GRANDPA) and H3 (sr25519+ML-DSA-44, BABE + tx
signing) schemes with **H2 (ed25519+FN-DSA-512)** and **H4 (sr25519+FN-DSA-512)** sourced from
the `pqhybridsign` library (`/home/lazycoder/proj/quip/pqhybridsign`,
gitlab.com/quip.network/pqhybridsign, v0.0.0-rc1).

## Confirmed decisions

- **Scope: full replacement.** H2 takes over GRANDPA, H4 takes over BABE and transaction signing.
- **Dependency: git dependency on pqhybridsign** from the SDK fork's `quip-crypto-primitives-core`
  crate. AGPL-3.0-or-later license consequence is accepted.
- **Follow-up decision (user selects at approval):** what to do with the fork-native H1/H3 code —
  migrate it to pqhybridsign wrappers, delete it, or leave it (see "Research findings" and
  "Phase 6" below).

## Research findings: fork H1/H3 vs pqhybridsign H1/H3 (byte-level comparison)

Verdict: **NOT byte-compatible** — deliberately so, documented in pqhybridsign's DESIGN.md
("Finding 3"). Divergences:

1. **HKDF info strings**: fork `seed.rs:16-18` uses fixed `b"classical"`/`b"pq"`; library
   `pqhybridsign-core/src/derive.rs:14-18,70-77` uses `b"classical/"‖Label\0` / `b"pq/"‖Label\0`.
   Same 32-byte master seed → **different keypairs** in both suites. (Hand-verified: reproduced
   the library's H1 golden vector from scratch in Python; fork-style info reproduces a different,
   non-matching pubkey.)
2. **H3 sr25519 signing context**: fork `classical/sr25519.rs:34` signs under `b"substrate"`;
   the library signs under the suite label → H3 sigs never cross-verify even with shared sk bytes.
3. **H3 deterministic nonce/RNG**: fork BLAKE2b-256 unprefixed + LE-counter Blake2Rng vs library
   domain-separated SHA-512 + BE-counter SeededRng.

Identical on both sides: label bytes (incl. trailing NUL, 26 B), M′ framing
(`0x01‖label‖len(ctx)‖ctx‖msg`), sizes/layout (pk 1344, sk 2624, sig 2484, classical-first),
secret-key encodings, ML-DSA-44 zero-`rnd` deterministic signing with empty fips204 ctx, ed25519
ZIP-215 verify.

**What H1/H3 migration achieves** (quantified):

- Dead code deleted from the fork: `fixed.rs` (427-line generic engine), `seed.rs`, `domain.rs`,
  `pq/{mod,mldsa44}.rs`, `classical/{mod,sr25519,ed25519}.rs`, `FixedHybridSuite` machinery —
  ~1,000+ lines plus their tests. The two suite files shrink to thin wrappers over
  `pqhybridsign::H1`/`H3`, sharing the exact wrapper pattern H2/H4 introduce.
- Dependencies dropped from `quip-crypto-primitives-core`'s manifest: `blake2` and `subtle`
  (leave the dependency graph entirely), `hkdf` and `sha2` (become transitive via
  pqhybridsign-core). `fips204`/`schnorrkel`/`ed25519-zebra` stay in the graph (pqhybridsign
  needs them) but become transitive-only; versions unify (fork pins are semver-compatible with
  the library's).
- **Cost/risk**: the migrated H1/H3 are wire-incompatible with the previously deployed H1/H3
  despite identical label strings (different seed derivation). This is moot for the chain —
  the H2/H4 cutover already invalidates all existing keys/accounts/genesis — but it is a footgun
  for any external tooling that derived H1/H3 keys the fork way; it must be documented, and
  `golden_vectors.txt` would be regenerated against library vectors if H1/H3 wrappers are kept.

**Is it needed?** Not for correctness of the H2/H4 integration — it is a code-hygiene/dedup
follow-up. Its real value: (a) single source of truth for all QUIP hybrid schemes — upstream
fixes propagate, fork shrinks; (b) keeping library-backed H1/H3 available is a cheap hedge if the
pre-FIPS-206 FN-DSA wire format shifts and a fall-back to ML-DSA hybrids is ever needed. If
neither matters, plain deletion achieves most of the dedup with less code to maintain.

## Integration map (what H1/H3 look like today)

- All Substrate deps of quip-validator are **git deps** to
  `github.com/QuipNetwork/polkadot-sdk.git` branch `v0.2`; local checkout at
  `/home/lazycoder/proj/quip/polkadot-sdk` (HEAD `f17113ffe8`, matches `Cargo.lock`).
- H1/H3 live in the fork, two crates:
  - `quip/primitives/crypto-core` (`quip-crypto-primitives-core`): pure `no_std`, sp-free engine.
    Suites in `src/suite/{ed25519,sr25519}_mldsa44.rs` implement `HybridSignatureScheme`
    (`src/lib.rs:65-143`) over the fixed-size engine `src/fixed.rs`. Sizes: pk 1344, sk 2624,
    sig 2484. Labels `b"hybrid-{ed25519,sr25519}-mldsa44-v1\0"`.
  - `quip/primitives/crypto` (`quip-crypto-primitives`): Substrate glue. Generic wrapper in
    `src/substrate/signature.rs` (`SubstrateSignatureScheme` trait + generic
    `Public/Signature/Pair` on `sp_core::crypto::{PublicBytes, SignatureBytes}`, fixed sizes,
    `RuntimePublic` via generic `sp_io::crypto::crypto_*` host fns). H1 module
    `src/substrate/ed25519_mldsa44.rs` (`CRYPTO_ID = *b"h144"`); H3 module
    `src/substrate/sr25519_mldsa44.rs` (`*b"h344"`) plus the hybrid VRF (sr25519 VRF + ML-DSA
    binding sig, `VrfSignature { sr25519, pq_signature: [u8; 2420] }`,
    `VrfOutput = SHA256(pre_output ‖ pq_sig)`, `pub mod babe` transcript helpers).
- Fork touch-points outside `quip/`:
  - `substrate/primitives/consensus/babe/src/lib.rs:39-53` — `app_crypto!(hybrid, BABE)` on the H3
    module; BABE `AuthorityId` **is** the H3 key; VRF types re-exported from it.
  - `substrate/primitives/consensus/grandpa/src/lib.rs:45-49` — `app_crypto!(hybrid, GRANDPA)` on
    H1; `sign_message` uses generic `keystore.sign_with(..., CRYPTO_ID, ...)`.
  - `substrate/client/keystore/src/local.rs:239-330` — `LocalKeystore` dispatches
    `public_keys_with`/`generate_new_with`/`sign_with`/`vrf_sign_with` on `h144`/`h344`.
  - `substrate/client/cli/src/commands/insert_key.rs:31-107` — `InsertKeyScheme` has
    `HybridBabeH344`/`HybridGrandpaH144`.
  - `sp-io`/`sp-keystore` already expose **generic** crypto-id-dispatching host functions and
    keystore trait methods — no changes needed there.
  - `sc-consensus-babe` (`authorship.rs`, `verification.rs`, `lib.rs`) and `pallet-babe`
    (`:369-385`) consume the hybrid VRF via `make_vrf_bytes`.
  - scale-info workaround in `signature.rs:293-357`: `SignatureMetadata2484` struct because
    scale-info has no `TypeInfo` for big `[u8; N]`.
- quip-validator side:
  - `runtime/src/lib.rs:207` — `pub type Signature = HybridTxSignature;` (replaces
    `MultiSignature`); `AccountId = AccountId32 = blake2_256(b"quip-account-v1" ‖ pubkey)`;
    `SessionKeys { babe, grandpa }` (`:57-62`); genesis presets in
    `runtime/src/genesis_config_presets.rs` derive authorities from seeds/hex pubkeys.
  - `crates/transaction-crypto/src/lib.rs` — `HybridTxPublic`/`HybridTxSignature` over the fork's
    H3 wrapper; `crates/transaction-crypto-core` — sp-free mirror (raw `[u8; 1344]`/`[u8; 2484]`,
    BIP39→seed) for the browser signer; `-wasm` and `-py` bindings; `js/quip-signer/` TS signer.
  - `node/src/insert_hybrid_key.rs` — `insert-hybrid-key` subcommand (`BabeH344`/`GrandpaH144`);
    `crates/transaction-crypto/examples/derive_genesis_keys.rs` + `scripts/derive-operator-keys.sh`
    for keygen; `node/src/benchmarking.rs` signs hybrid extrinsics.
  - Tests: `transaction-crypto-core/tests/golden_parity.rs` (+ `golden_vectors.txt`),
    `runtime/tests/signing_fixture.rs` (+ `docs/polkadotjs/fixtures/hybrid-signing.json`),
    extensive unit tests in both transaction-crypto crates.

## pqhybridsign facts that shape the design

- H2/H4 defined in `crates/pqhybridsign/src/suites.rs:24-58` via `hybrid_delta_suite!`
  (`DeltaSuite` trait, `pqhybridsign-core/src/suite.rs:243`).
- Buffer-oriented API, no typed key/sig newtypes, **no codec/scale-info/serde/sp glue**:
  `pqhybridsign_core::composite_delta::{keypair_from_seed, sign, sign_deterministic, verify}`
  (`composite_delta.rs:52,103,130,161`). `sign` returns `Result<usize>` (bytes written).
- Sizes: **pk 929** (32+897), **sk 1409** (64+1345), **sig ≤ 731** (64 ‖ delta(1) ‖ pq);
  `fn-dsa` 0.4.0 always emits 666-byte sigs ⇒ sig is effectively always 731.
  Wire: `ClassicalSig(64) ‖ DeltaLen(1) ‖ PqSig(411+delta)`; real length = `476 + sig[64]`.
- `no_std`-capable, wasm-CI'd, zero Substrate deps — safe for runtime/wasm and for
  `transaction-crypto-core`/browser signer. Feature gotcha: `pqhybridsign` with
  `default-features = false` does **not** enable `pqhybridsign-core/alloc`; without `alloc`,
  ctx+msg > ~484 B fails. Fix: also depend on `pqhybridsign-core` directly with
  `features = ["alloc"]` (runtime has an allocator).
- H2/H4 labels are `"hybrid-{ed25519,sr25519}-falcon512-v1"` (28 bytes with the macro-appended
  trailing NUL — same convention as the fork's H1/H3 labels), and seed derivation is internal to
  `keypair_from_seed(master: &[u8; 32])` (label-bound HKDF info) — i.e. H2/H4 wrap the library's
  pipeline wholesale rather than the fork's `seed.rs`/`domain.rs`/`fixed.rs` engine.
  Golden-vector parity with pqhybridsign (`tests/vectors/h2.json`/`h4.json`) comes for free and
  will be asserted.
- FN-DSA is randomized (`IS_DETERMINISTIC: false`); `sign_deterministic(sk, msg, ctx, nonce)`
  exists for the deterministic path the Substrate glue needs.
- Caveats to record in docs: H2/H4 predate final FIPS 206 (wire format may change upstream — we
  control both sides); AGPL-3.0 dependency.

## Design

New suites `Ed25519FnDsa512` (H2) and `Sr25519FnDsa512` (H4) in the fork's crypto-core implement
the existing `HybridSignatureScheme` trait by delegating to `pqhybridsign_core::composite_delta::*`
— so **all** existing generic Substrate glue (`signature.rs`, sp-io host fns, sp-keystore traits)
is reused unchanged. New CryptoTypeIds `h244` (H2) / `h444` (H4); new substrate wrapper modules;
BABE/GRANDPA/keystore/CLI switched over. The fixed-size-signature assumption is handled by
treating the signature as fixed 731 bytes with zero padding; parsing reads the delta byte at
offset 64 (`real_len = 476 + sig[64]`) and passes the exact slice to `composite_delta::verify`
(current `fn-dsa` always produces 731 anyway). The H4 hybrid VRF mirrors H3's construction with a
731-byte FN-DSA binding signature. The fork's `verify_deterministic` hook (already documented for
Falcon nonce checks) is implemented for H2/H4 if pqhybridsign exposes nonce verification;
otherwise the trait gets a default fallback to `verify` (decided during implementation — the only
consumer is the VRF binding check).

## Work items

### Phase 0 — local dev wiring (no pushes)

- In `quip-validator/Cargo.toml` add a `[patch."https://github.com/QuipNetwork/polkadot-sdk.git"]`
  section (or `.cargo/config.toml` `paths` override) pointing every used SDK crate at
  `../polkadot-sdk`, so fork changes are testable without pushing. Revert before final delivery or
  keep behind an obvious marker, per user preference at that point.
- In `polkadot-sdk/quip/primitives/crypto-core/Cargo.toml`: add
  `pqhybridsign = { git = "https://gitlab.com/quip.network/pqhybridsign.git", tag/rev = <pin>, default-features = false }`
  and `pqhybridsign-core = { same git, default-features = false, features = ["alloc"] }`.
  Pin to the current rc1 commit. For local iteration use a `[patch]`/path override to
  `../pqhybridsign`.
- Same `pqhybridsign`/`pqhybridsign-core` deps for `quip-validator/crates/transaction-crypto-core`
  (sp-free browser path needs the library directly).

### Phase 1 — fork: crypto-core suites (H2/H4)

Files: `polkadot-sdk/quip/primitives/crypto-core/`

- Add `fn-dsa`-backed suites, one file each, mirroring the suite-module pattern of
  `src/suite/ed25519_mldsa44.rs` / `sr25519_mldsa44.rs` but delegating to pqhybridsign instead of
  `fixed.rs`:
  - `src/suite/ed25519_fndsa512.rs` — `Ed25519FnDsa512`, wraps `pqhybridsign::H2`.
    Consts: `HYBRID_PK_LEN = 929`, `HYBRID_SK_LEN = 1409`, `HYBRID_SIG_LEN = 731`.
    `PublicKey`/`Signature` newtypes around fixed arrays (sig zero-padded; `AsRef<[u8]>` returns
    the full padded buffer, plus an accessor for the real length via the delta byte).
    `SecretKey { bytes: [u8; 1409] }` with `ZeroizeOnDrop`.
    `from_seed_slice` → `composite_delta::keypair_from_seed::<H2>`; `sign_deterministic` →
    `composite_delta::sign_deterministic::<H2>`; `sign` → `composite_delta::sign`;
    `verify` → strip padding via delta byte, `composite_delta::verify::<H2>`.
  - `src/suite/sr25519_fndsa512.rs` — same for `pqhybridsign::H4`.
- Register in `src/suite/mod.rs`; re-export in `src/lib.rs`.
- Trait fit check (`HybridSignatureScheme`, `src/lib.rs:65-143`): if `verify_deterministic` can't
  be honored through pqhybridsign's public API, give it a default method falling back to `verify`
  and override only where meaningful (H1/H3 keep current behavior).
- Tests per suite: roundtrip, determinism, tamper rejection, and **byte-exact parity against
  pqhybridsign golden vectors** (`pqhybridsign/tests/vectors/h2.json`, `h4.json` — seed→pk,
  seed+msg→sig).

### Phase 2 — fork: Substrate glue + consensus swap

Files: `polkadot-sdk/quip/primitives/crypto/`, `substrate/{primitives,client}/...`

- `src/substrate/ed25519_fndsa512.rs` (H2): mirror `ed25519_mldsa44.rs` —
  `CRYPTO_ID = CryptoTypeId(*b"h244")`, marker `SubstrateH2` impl `SubstrateSignatureScheme`
  (ed25519-style `derive_seed`), `Public`/`Signature`/`Pair` aliases. No VRF (GRANDPA).
- `src/substrate/sr25519_fndsa512.rs` (H4): mirror `sr25519_mldsa44.rs` —
  `CRYPTO_ID = *b"h444"`, `SubstrateH4`, plus the hybrid VRF block copied from H3 with:
  `pq_signature: [u8; 731]` (padded; real length from delta byte), binding sig via
  `Suite::sign_deterministic` (nonce derivation pinned here — reuse H3's `binding_input`
  approach), `VrfOutput = SHA256(vrf_pre_output ‖ real pq sig bytes)`, `make_bytes`,
  `pub mod babe` transcript helpers. Keep `RANDOMNESS_VRF_CONTEXT = b"BabeVRFInOutContext"`.
- scale-info: add `SignatureMetadata731` (and pubkey-929 equivalent if needed) alongside
  `SignatureMetadata2484` in `signature.rs`, wired into the generic `Signature`/`Public` impls.
- `sp-consensus-babe` (`substrate/primitives/consensus/babe/src/lib.rs:36-80`, `digests.rs`):
  point `app_crypto!(hybrid, BABE)` at `sr25519_fndsa512`; keep the same re-export surface;
  `PUBLIC_KEY_LENGTH` becomes 929 automatically via `ByteArray::LEN`.
- `sp-consensus-grandpa` (`substrate/primitives/consensus/grandpa/src/lib.rs:45-50`): point
  `app_crypto!(hybrid, GRANDPA)` at `ed25519_fndsa512`.
- `sc-keystore` (`substrate/client/keystore/src/local.rs:239-330`): dispatch `h244`→H2 pair,
  `h444`→H4 pair (VRF path for h444), replacing the h144/h344 arms.
- `sc-cli` (`substrate/client/cli/src/commands/insert_key.rs:31-107`): replace variants with
  `HybridBabeH444`/`HybridGrandpaH244` (kebab-case CLI values follow suit).
- `sc-consensus-babe` and `pallet-babe`: no structural changes expected (they go through the
  generic VRF re-exports/`make_vrf_bytes`) — verify by compiling; fix call sites only if the
  VrfSignature layout change forces it.
- Run the fork's quip crate test suites (`cargo test -p quip-crypto-primitives-core
  -p quip-crypto-primitives`) including the BABE transcript-parity tests updated to H4.

### Phase 3 — quip-validator: transaction crypto + signers

- `crates/transaction-crypto-core/src/lib.rs`: switch from `Sr25519MlDsa44` to pqhybridsign `H4`
  via `composite_delta` (stays sp-free/wasm-safe). New sizes: `HybridTxSignatureBytes { public:
  [u8; 929], signature: [u8; 731] }`. Keep `ACCOUNT_ID_DOMAIN = b"quip-account-v1"` (account IDs
  change anyway because pubkey bytes change; bumping the domain is a one-line decision to confirm
  at implementation time — default: keep).
- `crates/transaction-crypto/src/lib.rs`: repoint `HybridPublic`/`HybridSignatureBytes`/
  `HybridPair` aliases (lines ~47) at the fork's `sr25519_fndsa512` wrapper; `HybridTxPublic`/
  `HybridTxSignature` shape unchanged.
- Regenerate `crates/transaction-crypto-core/tests/golden_vectors.txt` (from pqhybridsign vectors /
  the new implementation, cross-checked both ways) and update `golden_parity.rs` size constants.
- `crates/transaction-crypto-wasm`, `crates/transaction-crypto-py`: rebuild against updated core;
  update `test_parity.py` expectations.
- `js/quip-signer/`: update size constants (929/731) and any hard-coded H3 assumptions; run its
  test suite.

### Phase 4 — quip-validator: runtime, node, genesis, docs

- `runtime/src/lib.rs`: `Signature = HybridTxSignature` stays (underlying sizes change);
  bump `transaction_version` **and** `spec_version` (comment at `:74-76` documents the precedent).
- `runtime/src/genesis_config_presets.rs`: update the `*_authority_from_public_hex` helpers and
  length checks (1344→929) and re-derive built-in dev/testnet authorities.
- `node/src/insert_hybrid_key.rs`: `HybridScheme` → `BabeH444`/`GrandpaH244` mapping to the new
  CryptoTypeIds; `node/src/cli.rs`/`command.rs` follow.
- `crates/transaction-crypto/examples/derive_genesis_keys.rs` + `scripts/derive-operator-keys.sh`:
  update schemes/lengths.
- `runtime/tests/signing_fixture.rs` + `docs/polkadotjs/fixtures/hybrid-signing.json`: regenerate
  via `runtime/examples/generate_polkadotjs_signing_fixture.rs`.
- `node/src/benchmarking.rs`: verify benchmark signing still compiles/works with new sizes.
- Docs sweep: `docs/hybrid-crypto-dedup-plan.md`, `docs/testnet-keys.md`,
  `docs/polkadotjs/README.md`, `docs/genesis-quip-testnet.md`, crate READMEs — update scheme
  names, sizes, labels, and add the pqhybridsign provenance + AGPL + pre-FIPS-206 notes.

### Phase 5 — H1/H3 follow-up (executed per the option selected at approval)

**Option A — Migrate H1/H3 to pqhybridsign wrappers (recommended):**

- Rewrite `crypto-core/src/suite/ed25519_mldsa44.rs` / `sr25519_mldsa44.rs` as thin wrappers over
  `pqhybridsign::H1`/`H3` via `pqhybridsign_core::composite::{keypair_from_seed, sign,
  sign_deterministic, verify}` (fixed-size API, no padding logic needed — sig is exactly 2484).
  Same `HybridSignatureScheme` impl pattern the H2/H4 suites establish in Phase 1.
- Delete the now-dead engine: `src/fixed.rs`, `src/seed.rs`, `src/domain.rs`, `src/pq/`,
  `src/classical/`, the `FixedHybridSuite` machinery in `src/suite/mod.rs`, and their tests.
- `crypto-core/Cargo.toml`: drop direct deps `blake2`, `subtle`, `hkdf`, `sha2`, `fips204`,
  `schnorrkel`, `ed25519-zebra` (the latter three remain transitive via pqhybridsign); keep
  `zeroize`, `rand_core`, `thiserror`.
- Substrate wrapper modules (`crypto/src/substrate/{ed25519,sr25519}_mldsa44.rs`, `h144`/`h344`)
  stay as-is — they are generic over the suite and keep working, now library-backed. H3's VRF
  module is untouched except that the binding sig flows through the wrapper (byte-layout
  unchanged: 2420-byte ML-DSA sig).
- Add golden-vector parity tests against `pqhybridsign/tests/vectors/h1.json`/`h3.json`.
- Document prominently (crate docs + `docs/hybrid-crypto-dedup-plan.md`): **library-backed H1/H3
  derive different keys than the legacy fork implementation from the same seed** (DESIGN.md
  Finding 3) — legacy derivations are gone for good.

**Option B — Delete H1/H3 entirely:**

- Everything in Option A except the rewrite: remove the two suite files and the two substrate
  wrapper modules (and their tests) instead of wrapping; retire `h144`/`h344`; drop the same
  engine code and deps. Smallest maintenance surface; no ML-DSA fallback path retained.

**Option C — Leave H1/H3 untouched:**

- Skip Phase 6. The fork keeps its engine, direct deps, and tests alongside pqhybridsign. Zero
  additional risk; zero dedup.

### Phase 6 — verification

- `cargo test` in fork: `quip-crypto-primitives-core`, `quip-crypto-primitives`,
  `sp-consensus-babe`, `sp-consensus-grandpa`, `sc-keystore`.
- `cargo build --release` (or at least `cargo check --workspace`) + `cargo test --workspace` in
  quip-validator, including the runtime wasm build (`SKIP_WASM_BUILD=0`).
- Golden-parity and signing-fixture gates green; js signer tests green; py parity tests green.
- Smoke: build the node, `insert-hybrid-key --scheme hybrid-babe-h444` /
  `hybrid-grandpa-h244` into a fresh keystore, start a 2-node local devnet, observe BABE block
  production and GRANDPA finality; submit one signed transfer via the updated signer.
- `cargo tree` check that `sp-core`/`sp-runtime`/`sp-application-crypto` remain untouched in the
  fork diff.

## Risks / notes

- **AGPL-3.0**: pqhybridsign becomes a transitive dep of the runtime and node — accepted, but
  record it in the dedup-plan doc.
- **Pre-FIPS-206 FN-DSA**: upstream warns H2/H4 wire formats may change; pin the pqhybridsign git
  rev deliberately and treat any bump as a consensus-breaking change requiring a
  `transaction_version` bump.
- **Variable-length sigs**: padded-fixed-731 is safe today (fn-dsa 0.4.0 always emits 666) and
  forward-compatible via the delta byte; the padding/parse rule must be identical in Rust, wasm,
  Python, and TS signers — the golden vectors are the guardrail.
- **VRF determinism**: H4's binding signature uses `sign_deterministic`; nonce derivation must be
  fixed and documented, since validators on different versions must agree on verification (not on
  the nonce itself — only verify-side agreement is consensus-critical).
- Fork changes ultimately need to be pushed to `QuipNetwork/polkadot-sdk` (branch v0.2) and the
  `[patch]` overrides removed — push only with explicit user approval at the end.

## Phase 7 — apps/ alignment, final repin, and override cleanup (added 2026-08-20)

Because quip-validator still carries local `[patch]` path overrides for the
polkadot-sdk fork, the apps/ (polkadot-js fork) integration must be developed
and tested against the **local quip-validator checkout**, not the submodule
pin:

1. **Testing phase (local only):** point apps/ at the local quip-validator
   copy (which itself resolves the SDK via the local `[patch]` overrides) for
   the signer build and all integration tests (`yarn test:quip-signing`
   against a fresh local H2/H4 node). Do not fetch/reset the submodule for
   testing.
2. **After the pushes land** (polkadot-sdk `v0.2` first, then quip-validator):
   - repin the `apps/quip-validator` submodule to the pushed quip-validator
     commit;
   - remove the local path overrides: the `[patch."…QuipNetwork/polkadot-sdk.git"]`
     section in quip-validator and any local-path pointing introduced in apps/
     for testing;
   - refresh lockfiles and re-run the final gate set (fork suites, golden
     parity, signing fixture, apps signer + local-node integration test).
3. Only then is the whole stack (pqhybridsign → polkadot-sdk fork →
   quip-validator → apps) self-consistent from the remotes alone.

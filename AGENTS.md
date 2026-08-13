# AGENTS.md

This file provides guidance to coding assistants (Claude Code, Codex, Cursor, etc.) when working with code in this repository.

## Overview

Quip Network solochain node built on Substrate (Polkadot SDK `polkadot-stable2512-2`). This is a standalone blockchain with Aura consensus (block authoring) and GRANDPA (finality), currently at the template stage with a single custom pallet.

## Chain specs

Omit `--chain` to join the live public Quip Testnet (EIP-155 `20033`).
`--chain=local` is a disposable Alice-and-Bob chain. It is not the public testnet.

| Flag | Preset | Network | EIP-155 |
|------|--------|---------|--------:|
| (omit `--chain`) | `quip-testnet` | live public testnet | 20033 |
| `--chain=quip-testnet` | `quip-testnet` | live public testnet | 20033 |
| `--dev` or `--chain=dev` | Development | Alice-only local | 1337 |
| `--chain=local` | Local Testnet | Alice and Bob local | 1337 |
| `--chain=local3` | Local Testnet (3 Validators) | Alice, Bob, and Charlie local | 1337 |

Aliases for the public testnet: `quip_testnet`, `testnet`. A path argument loads that JSON file and uses the chain ID in its genesis.

## Commands

```bash
# Build (release, compiles both native binary and Wasm runtime)
cargo build --release

# Build (debug, faster compilation)
cargo build

# Run all tests
cargo test

# Test a single pallet
cargo test -p pallet-template

# Run a single test by name
cargo test -p pallet-template it_works_for_default_value

# Check without building (fast feedback)
cargo check

# Clippy (workspace lints defined in root Cargo.toml)
cargo clippy --all-targets

# Build benchmarks
cargo build --release --features runtime-benchmarks

# Join the live public testnet (default: omit --chain, EIP-155 20033)
cargo build --release
./target/release/quip-network-node

# Same network, explicit flag
./target/release/quip-network-node --chain=quip-testnet

# Alice-only local chain (EIP-155 1337). Required for a single validator.
./target/release/quip-network-node --dev

# Purge Alice-only local chain state (EIP-155 1337)
./target/release/quip-network-node purge-chain --dev

# Alice and Bob local chain (EIP-155 1337). Not the public testnet.
./target/release/quip-network-node --chain=local

# Three-validator local chain (EIP-155 1337)
./target/release/quip-network-node --chain=local3

# Generate rust docs
cargo +nightly doc --open
```

CI (`.gitlab-ci.yml`) runs `cargo fmt --check`, `cargo clippy --workspace -D warnings`, `cargo test`, and a runtime release build on every merge request. `browser-signer-test` also checks the signing fixture. Run `cargo clippy --all-targets` and `cargo test` locally before pushing to catch failures early.

## Architecture

Three-crate workspace:

**`node/`** — Native binary (`quip-network-node`). Handles networking (libp2p), consensus orchestration, RPC server, and chain specification. Key files:
- `chain_spec.rs` — Genesis configuration. Default (no `--chain`) is `quip-testnet`.
- `service.rs` — Node service wiring (Aura + GRANDPA consensus, transaction pool, networking)
- `rpc.rs` — Custom RPC endpoint registration

**`runtime/`** — Blockchain state transition function (`quip-protocol-runtime`). Compiles to both native and Wasm. The Wasm blob is embedded in the native binary and can be upgraded on-chain without hard forks.
- `lib.rs` — Runtime type definitions, pallet composition via `#[frame_support::runtime]` macro, block time constants (6s slots)
- `configs/mod.rs` — All pallet `Config` trait implementations (system params, weights, fees)
- `apis.rs` — Runtime API implementations exposed to the node
- `genesis_config_presets.rs` — Genesis state presets for dev/testnet

**`pallets/evm-chain-id/`** — Stores the EIP-155 chain ID set at genesis (`1337` local, `20033` testnet). `pallet-revive` reads it through `Get<u64>`.

**`pallets/template/`** — Custom FRAME pallet (`pallet-template`). Starting point for Quip-specific logic.
- `lib.rs` — Pallet definition (storage, events, errors, dispatchable calls)
- `mock.rs` — Mock runtime for unit tests
- `tests.rs` — Pallet unit tests using `frame_support::assert_ok!` / `assert_noop!`
- `weights.rs` — Benchmark-derived weight constants
- `benchmarking.rs` — Benchmark definitions (behind `runtime-benchmarks` feature)

## Key Patterns

**`no_std` by default**: Runtime and pallet crates use `#![cfg_attr(not(feature = "std"), no_std)]`. All dependencies in these crates must support `no_std`. Use `default-features = false` for Substrate deps and gate std-only code behind `#[cfg(feature = "std")]`.

**Pallet indices are stable**: Pallet indices in `runtime/src/lib.rs` (e.g., `#[runtime::pallet_index(7)]`) must never change after chain launch — they're encoded in storage keys and extrinsics.

**Call indices are stable**: Similarly, `#[pallet::call_index(N)]` values in pallet dispatchables must remain fixed for backward compatibility.

**Feature flags**:
- `runtime-benchmarks` enables benchmark code.
- `try-runtime` enables migration testing.
- `metadata-hash` embeds a runtime metadata hash at compile time (slows builds; used for release artifacts).
- `on-chain-release-build` is the convenience aggregate enabled for production runtime blobs.

These gate code at both pallet and runtime level.

**Workspace lints**: The root `Cargo.toml` defines strict clippy + rustc lints that apply to every crate via `[workspace.lints]`. CI enforces these (`cargo clippy --workspace -D warnings`); run `cargo clippy --all-targets` locally before pushing.

## Implementation Rules

### General

- Prefer small, reviewable changes over broad refactors.
- Keep new abstractions narrowly scoped to the problem being solved.
- Follow existing workspace patterns for crate layout, runtime wiring, tests, and features.

### FRAME / Substrate

- Do not add `#[pallet::getter(...)]` storage getter macros.
- Do not call generated pallet storage getter methods.
- Access pallet storage directly through the storage types by default, for example:
  - `JobOrders::<T>::get(order_id)`
  - `Solvers::<T>::insert(account, info)`
- If the same storage read is needed in multiple places, an explicit helper method may be added on the pallet `impl`.
- Prefer explicit named helpers over generated getters so the access path stays visible in code review.
- When a test needs to read pallet storage, import the storage type directly unless there is an existing explicit helper with real reuse value.
- Keep pallet APIs explicit. Avoid convenience wrappers that merely rename storage access unless they materially improve reuse or readability.

### Runtime

- Keep pallet indices stable once introduced.
- Prefer runtime configuration via `parameter_types!` and explicit `impl pallet_x::Config for Runtime` blocks.
- Avoid adding runtime-only behavior into pure helper crates.
- When you change `spec_version`, `transaction_version`, or the signed-extension set in `runtime/src/lib.rs`, regenerate the Polkadot.js signing fixture before you push. `docs/polkadotjs/fixtures/hybrid-signing.json` embeds those values. `browser-signer-test` runs `cargo test -p quip-protocol-runtime --test signing_fixture` and fails if the fixture is stale:

```bash
cargo run -p quip-protocol-runtime --example generate_polkadotjs_signing_fixture -- --write
cargo test -p quip-protocol-runtime --test signing_fixture
```

### Validation / Pure logic

- Put pure deterministic math and validation into standalone crates where possible.
- Keep Substrate-bound types and dispatch logic in pallets.
- When parity with a reference implementation matters, prefer checked-in fixtures generated from the real reference implementation.

## Environment Setup

Rust toolchain is pinned in `env-setup/rust-toolchain.toml` (stable channel, includes `wasm32-unknown-unknown` target). Alternatively, use Nix: `cd env-setup && nix develop` (requires `clang`, `protobuf`, `rustup`).

## Versioning & release tags

Cross-repo standard — `quip-protocol/docs/VERSIONING.md` is canonical; `docs/release.md` is the release checklist.

| Artifact | Format | Example |
|----------|--------|---------|
| Git tag (pre-release) | SemVer hyphenated `vMAJOR.MINOR.PATCH-rcN` | `v0.2.1-rc18` |
| Git tag (stable) | `vMAJOR.MINOR.PATCH` | `v0.2.1` |
| `Cargo.toml` `version` | bare SemVer (toolchain-native) | `0.2.1` |

Rules:
- Pre-release git tags MUST be hyphenated (`-rcN` / `-alphaN` / `-betaN`). Never the PEP 440 no-hyphen form (`v0.2.1rc18`) for a git tag: `quip-node-manager` orders release candidates by splitting on the hyphen, so a no-hyphen tag collapses every rc to one value and freezes node updates.
- The `MAJOR.MINOR.PATCH` numerics must match `Cargo.toml`'s `version`; only the separator and the rc suffix differ. A Cargo SemVer pre-release, if ever set, is also hyphenated (`0.2.1-rc.18`).
- CI (`.gitlab-ci.yml`): container images publish on release tags ONLY — branch pushes never build or push images. The floating tag follows the branch the tag was cut from (resolved by the `resolve-floating-tag` job via commit ancestry): a tag on `v0.2` publishes `:<tag>` + `:v0.2`, a tag on `main` publishes `:<tag>` + `:latest`, both plus `:sha-<short-sha>`.

## License

Unlicense (public domain).

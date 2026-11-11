# AGENTS.md

This file provides guidance to coding assistants (Claude Code, Codex, Cursor, etc.) when working with code in this repository.

## Overview

Quip Network solochain node built on Substrate (Polkadot SDK
`polkadot-stable2603`). This is a standalone blockchain with hybrid H4 BABE
block authoring and H2 GRANDPA finality.

## Chain specs

Omit `--chain` to join the live public Quip Testnet (EIP-155 `20033`).
`--chain=local` is a disposable Alice-and-Bob chain. It is not the public testnet.

| Flag | Preset | Network | EIP-155 |
|------|--------|---------|--------:|
| (omit `--chain`) | pinned spec | live public testnet | 20033 |
| `--chain=quip-testnet` | pinned spec | live public testnet | 20033 |
| `--dev` or `--chain=dev` | Development | Alice-only local | 1337 |
| `--chain=local` | Local Testnet | Alice and Bob local | 1337 |
| `--chain=local3` | Local Testnet (3 Validators) | Alice, Bob, and Charlie local | 1337 |
| *(none exists)* | `quip-mainnet` — runtime preset, **export-blocked** | mainnet, not launched | 20049 |
| *(none exists)* | `quip-testnet-relaunch` — runtime preset, **export-blocked** | relaunch testnet, not launched | 20033 |

Aliases for the public testnet: `quip_testnet`, `testnet`. A path argument loads that JSON file and uses the chain ID in its genesis.

### Mainnet — not runnable yet

There is **no mainnet network to join**, and no `--chain` value for it. Do not add one.

The two presets in the last two rows exist in both runtime crates and are registered in the GenesisBuilder API, but they **refuse to export** until the owner supplies real inputs: the six mainnet council accounts, five validator stash accounts with BABE/GRANDPA keys, the three relaunch testnet council accounts, and the vesting allocation total, recipients, custody account and tranche terms. `quip_mainnet_config_genesis()` returns `Err` with a named reason rather than emitting placeholders.

To see the refusal rather than take this on faith:

```bash
cargo test -p quip-protocol-runtime --lib wasm_preset_api_lists_new_networks_and_reports_why_export_is_blocked
```

It asserts that the preset list contains both names and that asking for `quip-mainnet` returns the refusal reason.

Neither name is a node alias. `load_spec` maps only `dev`, `local`, `rehearsal`, `local3`, the testnet aliases, and otherwise treats the argument as a filesystem path (`node/src/command.rs:39-54`). The test `unprovisioned_networks_have_no_builtin_chain_alias` (`node/src/command.rs:340`) fails if anyone wires them. So today `--chain=quip-mainnet` produces a file-not-found error, which is the intended behaviour, not a bug.

Why there is no flag rather than a flag that refuses: a chain alias promises a launchable network identity, and this one cannot be kept yet. The preset has no genesis to return while inputs are unset; session keys stay `None` by design, so a node built from it would run, author nothing and never finalize; and a preset resolved at runtime gives two operators different genesis from the same binary, which is a fork at block zero rather than a chain. The identity has to be a vendored raw spec with a pinned genesis hash, and decision D6 in `../TWO-NETWORK-DECISIONS.md` requires the alias, the raw spec and the identity to land in the same change. The negative test keeps that gap deliberate instead of accidental.

What must happen before mainnet can run, in order:

1. Owner supplies the addresses, keys and allocation figures above.
2. The preset exports successfully; generate canonical raw specs through the preset, not by hand.
3. Vendor the raw spec and pin its SHA-256, mirroring what the testnet did in QUI-2080 part 1 (`chain-specs/`, `scripts/check-live-testnet-spec.py`).
4. Wire the alias in `load_spec`, delete or update the negative test above, and re-run the live-spec gate.
5. Insert validator keys on the operators' nodes and bootstrap with a quorum of genesis validators. The decided mainnet identity — protocol id `quip-network`, token symbol `QUIP`, EIP-155 `20049` — lives in `../GENESIS-POSITION.md`; only the chain id is currently in code (`MAINNET_CHAIN_ID = 20_049`), because the chain-spec builder that would set the protocol id does not exist yet. Mainnet must never reuse a testnet or dev key in either direction.

Mainnet reaches genesis by fresh launch. It is never reached by `set_code` from another chain, and no existing database may be pointed at a new genesis.

Runtime 120 is the H2/H4 chain-wipe relaunch, not an in-place upgrade from the
earlier H1/H3 network. Work on this launch must assume fresh genesis/state,
fresh validator databases, re-inserted H4 BABE and H2 GRANDPA keys, and
incompatible old signed extrinsics (`transaction_version = 7`).

## Commands

```bash
# Build (release, compiles both native binary and Wasm runtime)
cargo build --release

# Build (debug, faster compilation)
cargo build

# Run all tests
cargo test

# Test a single pallet
cargo test -p pallet-evm-chain-id

# Run a single test by name
cargo test -p pallet-evm-chain-id genesis_persists_local_chain_id

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

CI (`.gitlab-ci.yml`) gates merge requests with more than a build. `cargo-test` runs `cargo fmt --check`, `cargo clippy --workspace -D warnings`, `cargo test`, a node build, the live-spec startup smoke, and a release runtime build. `browser-signer-test` checks the signing fixture and the Polkadot.js signer. `runtime-parity` runs the parity gate and its negative fixtures. `live-testnet-spec` verifies the vendored `chain-specs/quip-testnet.json` byte-for-byte against its pinned SHA-256 and exercises the alias/metadata identity. Run `cargo clippy --all-targets` and `cargo test` locally before pushing to catch failures early.

Every Rust job runs on the `ci-toolchain` image built from `.gitlab/ci-toolchain.Dockerfile`. The image holds the system build dependencies, the Rust targets, and every pinned helper binary the jobs use. A tool that a job needs belongs in that Dockerfile, not in a job's `before_script`. CI rebuilds the image when a CI Dockerfile changes, and a main pipeline can rebuild it by hand. Each job names the tools it depends on in `IMAGE_TOOLS` and fails before its script if the image lacks one. `make builder-image` builds the image locally for checking a change.

Two further merge-request jobs run automatically and cover benchmarks. `benchmark-preflight` exercises every benchmark against the production runtime constants at low resolution, without touching tracked weights; it fails closed if the runtime stops registering the nine `pallet_vesting` benchmarks, and it currently takes a large fraction of its 90-minute timeout (see QUI-2102). `weights-staleness-check` warns (yellow, never blocking) when a pallet's `lib.rs`/`benchmarking.rs` changed but its `weights.rs`/`benchmark_weights.rs` did not. Regenerating weights is the separate `benchmark-weights` job — **manual by design**, because it runs on the single serialized reference machine and pushes the regenerated files back to the MR branch. Play it when your change can affect weights; it mirrors runtime-level weights into `runtime-testnet`, which the parity gate requires.

Gates that exist as scripts and are worth running before a push, because a gate nobody has watched fail is not a gate:

```bash
bash scripts/check-runtime-parity.sh        # the two runtime trees differ only in approved places
python3 scripts/check-live-testnet-spec.py  # vendored raw spec matches its pinned SHA-256
python3 scripts/test-live-testnet-spec.py   # negative fixtures for that gate
python3 scripts/test-vesting-benchmark-inventory.py  # benchmark inventory guard fixtures
```

## Architecture

Cargo workspace with two runtime crates, one node crate, nine pallets and the transaction-crypto/quantum-validation libraries.

**`node/`** — Native binary (`quip-network-node`). Handles networking (libp2p), consensus orchestration, RPC server, and chain specification. Key files:
- `chain_spec.rs` — Chain-spec construction. The default (no `--chain`) and the `quip-testnet` aliases load the **pinned raw spec** in `chain-specs/quip-testnet.json`, not a runtime preset; see the Mainnet section for why no other network is wired.
- `command.rs` — CLI parsing and `load_spec` (the alias table)
- `service.rs` — Node service wiring (hybrid BABE + GRANDPA consensus, transaction pool, networking)
- `rpc.rs` — Custom RPC endpoint registration

**`runtime/`** — Blockchain state transition function (`quip-protocol-runtime`), `spec_name` `quip`. Compiles to both native and Wasm. The Wasm blob is embedded in the native binary and supports ordinary on-chain upgrades; Runtime 120 (the H2/H4 launch) is explicitly a chain-wipe relaunch instead.

**`runtime-testnet/`** — Second runtime crate (`quip-testnet-runtime`), `spec_name` `quip-testnet`, 600-slot epochs. Mirrored **byte-for-byte** from `runtime/` except the approved network differences; `scripts/check-runtime-parity.sh` enforces that and is a blocking CI job. Two distinct `spec_name` values are load-bearing: `can_set_code` and the `OnSetCode` guard reject a blob from the other network. This crate is **spec-source only**: it remains a compiled, parity-gated workspace member, but is not linked into a shipping binary. Raw chain specs are produced out of band and vendored with hash pins.

Both runtimes share the same layout:
- `lib.rs` — Runtime type definitions, pallet composition via `#[frame_support::runtime]` macro, block time constants (6s slots)
- `configs/mod.rs` — All pallet `Config` trait implementations (system params, weights, fees)
- `apis.rs` — Runtime API implementations exposed to the node
- `genesis_config_presets.rs` — Genesis presets: dev/local/local3/rehearsal, plus the export-blocked `quip-mainnet` and `quip-testnet-relaunch`

**Pallets** (`pallets/`):
- `validator-admission` — Foundation-gated validator admission and governed Root dispatch (`CouncilRootCalls` policy)
- `quantum-pow` — Protocol-level useful-work mining
- `quantum-compute-mempool` — User-submitted Ising job marketplace
- `xqvm` — On-chain XQVM bytecode execution
- `emission-controller` — Fixed annual subnet emissions and budgeted testnet issuance
- `slash-policy` — Bounded council discretionary slashing parameters and dispatch
- `faucet-ops` — Governed testnet faucet with an irreversible disable fuse
- `miner-registry` — Miner descriptors and qblock participation
- `evm-chain-id` — Genesis EIP-155 chain ID (`1337` local, `20033` testnet, `20049` mainnet); `pallet-revive` reads it through `Get<u64>`

Council slash procedures and validator removal semantics: [SLASHING_RUNBOOK.md](SLASHING_RUNBOOK.md).

**Libraries** (`crates/`): `quantum-validation` (deterministic `no_std` Ising validation), `transaction-crypto{,-core,-wasm,-py}` (hybrid H4 signer: `sr25519 + FN-DSA-512`, with Wasm and CPython bindings).

## Key Patterns

**`no_std` by default**: Runtime and pallet crates use `#![cfg_attr(not(feature = "std"), no_std)]`. All dependencies in these crates must support `no_std`. Use `default-features = false` for Substrate deps and gate std-only code behind `#[cfg(feature = "std")]`.

**Pallet indices are stable**: Pallet indices in `runtime/src/lib.rs` (e.g., `#[runtime::pallet_index(8)]`) must never change after chain launch — they're encoded in storage keys and extrinsics. Indices 6 and 7 are retired and must not be reused; index 31 is `slash-policy` and index 32 is the stock SDK `pallet-vesting`. Both runtime crates must agree on every index.

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
- Bump `spec_version` only after the current spec has shipped (a release tag that nodes actually run). If the current spec has not gone live, keep that number and do not invent the next one. Example: 115 has not shipped, so pallet-evm-chain-id stayed on 115 instead of 116.
- When you do change `spec_version`, `transaction_version`, or the signed-extension set in `runtime/src/lib.rs`, regenerate the Polkadot.js signing fixture before you push. `runtime/tests/fixtures/hybrid-signing.json` embeds those values. `browser-signer-test` runs `cargo test -p quip-protocol-runtime --test signing_fixture` and fails if the fixture is stale:

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

Cross-repo standard — `quip-protocol/docs/VERSIONING.md` is the canonical path in the cross-repository workspace.

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

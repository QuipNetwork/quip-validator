# Pallet Revive Integration

## Status

This is the living design, implementation, and decision record for integrating
`pallet-revive` into the Quip runtime. The initial runtime integration is
implemented at runtime `spec_version = 115` / `transaction_version = 6`
(introduced at 113 on-branch, first shipped as 114 in the v0.2.2-rc tags,
bumped to 115 for the benchmark weight regeneration).
Remaining unchecked items are deployment and end-to-end validation work.

Status labels:

- **Decided** — accepted for the initial integration.
- **Candidate** — current working value; still needs an explicit decision.
- **Required** — dictated by the existing runtime or by the Revive API.
- **Open** — no preferred value or implementation has been selected.

## SDK baseline

The workspace currently resolves the Quip Polkadot SDK fork to commit
`f17113ffe89361a26cb286f6afc5a92f443179a8`. At that revision:

- `pallet-revive` is version `0.13.0`.
- `pallet_revive::Config` has 27 associated types.
- The authoritative trait is
  `substrate/frame/revive/src/lib.rs::pallet::Config` in the SDK fork.

Re-check this document against the resolved SDK commit whenever the SDK
dependency is updated.

## Decisions

### EIP-155 chain IDs

| Network/preset | Decimal | Hex | Status |
|---|---:|---:|---|
| Local development (`--dev`) | `1337` | `0x539` | Decided |
| Local multi-validator (`local_testnet`, `local_three_validator`) | `1337` | `0x539` | Decided |
| Public Quip testnet (`quip-testnet`) | `20033` | `0x4e41` | Decided |
| Future production network | TBD | TBD | Open |

`1337` is the conventional EIP-155 identifier for Geth private development
chains. `20033` is a provisional Quip testnet identifier. No current entry for
it was found in the common public chain-ID registry when this decision was
made. Registry availability is not a reservation, so Quip should register the
testnet before advertising a public EVM RPC.

#### Runtime artifact constraint

`pallet_revive::Config::ChainId` is supplied by the compiled runtime, while all
current Quip chain-spec presets use the same embedded runtime Wasm. A single
runtime artifact therefore cannot expose `1337` for the development preset and
`20033` for the public testnet preset using an ordinary `ConstU64`.

Implemented mechanism:

- `20033` is the default release/testnet runtime value.
- The `dev-chain-id` runtime and node feature selects `1337`.
- That feature is required for the `dev`, `local_testnet`, and
  `local_three_validator` presets.

Consequence: a normal testnet/release binary rejects all local presets with an
instruction to rebuild with `dev-chain-id`; only the public `quip-testnet`
preset can run with `20033`. The dedicated development build reports `1337` and
rejects the public testnet preset. This keeps the chain ID a compile-time
runtime constant and avoids untyped storage outside a pallet.

This compatibility check also applies when loading a raw chain-spec JSON by
path. The default/testnet artifact accepts only the canonical `quip_testnet`
chain-spec ID; every other chain-spec ID requires `dev-chain-id`.

Build and run a local runtime with:

```bash
cargo run -p quip-network-node --features dev-chain-id \
  --bin quip-network-node -- --dev
```

Treat a deployed network's chain ID as immutable. Although a runtime upgrade
could technically change it, doing so would invalidate wallet assumptions,
EIP-712 domains, and replay protection.

### Contract deployment policy

The initial Revive integration is permissionless:

- Enable EVM bytecode upload and instantiation.
- Allow any signed account to upload contract code.
- Allow any signed account to instantiate contracts.
- Start without Quip-specific precompiles. Revive's built-in Ethereum
  precompiles remain available.
- Disable Revive debug mode in normal runtime artifacts, including testnet.
  A separate local debug build can be introduced later if execution tracing,
  unlimited contract size, EIP-3607 bypass, or PolkaVM logs are needed.

Transaction fees and storage deposits are the initial economic controls
against abusive deployment. No privileged deployment allowlist is introduced.

## Configuration tracker

### Runtime wiring

| `Config` type | Current candidate | Status | Notes |
|---|---|---|---|
| `Time` | `Timestamp` | Required | Supplies timestamps to contracts. |
| `Balance` | `Balance` (`u128`) | Required | Must match the configured currency. |
| `Currency` | `Balances` | Required | Holds contract balances, deposits, and fees. |
| `OnBurn` | `()` | Decided | Burns native amounts withdrawn for EVM dust/value conversion and rounding. This can be redirected if Quip later adds a treasury. |
| `RuntimeEvent` | `RuntimeEvent` | Required | Aggregated runtime event. |
| `RuntimeCall` | `RuntimeCall` | Required | Aggregated runtime call. |
| `RuntimeOrigin` | `RuntimeOrigin` | Required | Aggregated runtime origin. |
| `RuntimeHoldReason` | `RuntimeHoldReason` | Required | Revive adds hold-reason variants. |
| `WeightInfo` | `pallet_revive::weights::SubstrateWeight<Runtime>` | Required | Use production weights; re-benchmarking remains part of integration validation. |
| `FindAuthor` | `pallet_session::FindAccountFromAuthorIndex<Runtime, Babe>` | Decided | Resolves the BABE authority through session validators and supplies EVM `block.coinbase`. |

### Execution and access

| `Config` type | Current candidate | Status | Notes |
|---|---|---|---|
| `Precompiles` | `()` | Decided | No Quip-specific precompiles initially. Revive's built-in Ethereum precompiles remain available. |
| `AddressMapper` | `pallet_revive::AccountId32Mapper<Runtime>` | Required | Quip's account ID is `AccountId32`; maps it to EVM `H160`. |
| `AllowEVMBytecode` | `ConstBool<true>` | Decided | Enables EVM bytecode upload and instantiation. |
| `UploadOrigin` | `EnsureSigned<AccountId>` | Decided | Allows any signed account to upload code. |
| `InstantiateOrigin` | `EnsureSigned<AccountId>` | Decided | Allows any signed account to instantiate code. Contract-to-contract instantiation is not constrained by this origin. |
| `RuntimeMemory` | `ConstU32<{ 128 * 1024 * 1024 }>` | Decided | Upstream integrity-check baseline; consistent with the SDK executor's default 2,048 additional 64-KiB pages. |
| `PVFMemory` | `ConstU32<{ 512 * 1024 * 1024 }>` | Decided | Conservative Revive integrity-check budget. On this solochain it is not an actual parachain PVF allocation. |
| `DebugEnabled` | `ConstBool<false>` | Decided | Used by normal runtime artifacts, including testnet. A separate local debug build can be added later. |

### Storage economics

Quip has 12 decimal places:

```text
UNIT       = 1_000_000_000_000
MILLI_UNIT = 1_000_000_000
MICRO_UNIT = 1_000_000
```

The candidates below reproduce the SDK's production-style deposit formula at
Quip's denomination.

| `Config` type | Current candidate | Human value | Status |
|---|---:|---:|---|
| `DepositPerByte` | `10 * MICRO_UNIT` | `0.00001` AGLS/byte | Decided |
| `DepositPerItem` | `200 * MILLI_UNIT` | `0.2` AGLS/main-trie item | Decided |
| `DepositPerChildTrieItem` | `2 * MILLI_UNIT` | `0.002` AGLS/contract-storage item | Decided |
| `CodeHashLockupDepositPercent` | `Perbill::from_percent(30)` | 30% | Decided |

At these values, one 32-byte Solidity storage slot has a base deposit of about
`0.00232` AGLS before considering other stored data. A 1 KiB contract-storage
item costs about `0.01224` AGLS, and an `AccountId32` to `H160` address mapping
costs about `0.20052` AGLS.

Revive documents the three deposit rates as safe to change on a live chain
because refunds are calculated pro rata. They are nevertheless economic
parameters. The initial testnet adopts these values together as the SDK's
production-style baseline. Reassess them from observed deployment volume,
state growth, and token economics before production.

The 30% code-hash lockup makes contract instances contribute toward keeping
their referenced code available. It is a held, potentially refundable deposit,
not a transaction fee.

### Ethereum identity, gas, and fees

| `Config` type | Current candidate | Status | Notes |
|---|---|---|---|
| `ChainId` | `1337` for local development; `20033` for testnet | Decided | See the chain-ID decision above. |
| `NativeToEthRatio` | `ConstU32<1_000_000>` | Decided | Derived as `10^(18 - 12)` to map the 12-decimal native token to 18-decimal EVM values. |
| `FeeInfo` | `pallet_revive::evm::fees::Info<Address, Signature, EthExtraImpl>` | Required | Production Ethereum transaction support cannot use the mock `()` implementation. |
| `MaxEthExtrinsicWeight` | `FixedU128::from_rational(9, 10)` | Decided | Caps one Ethereum transaction at 90% of the normal maximum-extrinsic weight. |
| `GasScale` | `ConstU32<1_000>` | Decided | Targets familiar EVM gas magnitudes. With multiplier `1`, this and the native ratio report an initial base gas price of 1 gwei. Must be nonzero. |

`NativeToEthRatio` preserves exact denomination conversion:

```text
1 AGLS = 10^12 native planks = 10^18 EVM wei
```

`GasScale` raises the reported gas price and proportionally lowers gas units;
the total native execution fee remains approximately unchanged apart from
rounding. The 90% weight cap is applied to the normal dispatch class's maximum
extrinsic weight, not directly to the entire block. Verify gas estimates,
rounding, and out-of-gas behavior during end-to-end validation.

#### Transaction-payment coupling

The production `FeeInfo` implementation only supports Revive's
`BlockRatioFee`. The integration replaces the former `IdentityFee` with:

```rust
type WeightToFee =
    pallet_revive::evm::fees::BlockRatioFee<1, 1, Runtime, Balance>;
```

The `1/1` ref-time coefficient preserves the previous `IdentityFee` price for
ref-time-only weights. `LengthToFee` remains `IdentityFee<Balance>` and the fee
multiplier remains fixed at `1`. Revive's generated runtime integrity test
passes. The current `u64::MAX` proof-size block limit matches the SDK's
production runtime baseline, but gives proof size a very small relative fee;
selecting a finite proof-size budget remains an explicit economics follow-up.

### Ethereum JSON-RPC sidecar

The pinned SDK exposes Revive's Ethereum RPC as a sidecar process connected to
the node's WebSocket RPC. Embedded integration remains blocked on
[paritytech/polkadot-sdk#11297](https://github.com/paritytech/polkadot-sdk/pull/11297),
which introduces the `SubstrateClientT` abstraction and embeds the ETH RPC
server into an omni-node. That PR is still open as of 2026-08-07 and is not in
Quip's pinned SDK commit `f17113f`.

At that commit, the sidecar also cannot be added as an ordinary Git dependency:
its build script requires `revive-dev-runtime` to be built in the SDK workspace.
The sidecar image therefore builds `pallet-revive-eth-rpc` from a complete,
exactly pinned checkout of the Quip SDK fork. Runtime deployments connect this
standalone process to the node's WebSocket RPC and must choose an intentional
receipt-retention and persistence policy.

Release-tag pipelines publish `docker/revive-eth-rpc.Dockerfile` as
`$CI_REGISTRY_IMAGE/quip-network-evm-sidecar` for `linux/amd64` and
`linux/arm64`. Its release tag, `sha-<short-sha>`, and branch-derived floating
tags match the node image.

## Pending decisions and validation

This section is the resume point for future planning sessions. Nothing listed
here has been accepted unless another section records it as decided.

### Configuration status

All 27 `pallet_revive::Config` associated types are now either explicitly
decided or mechanically required by the existing runtime. The future
production Chain ID remains open and must be distinct from local development
and testnet IDs.

### Cross-cutting architecture and economics

- **Proof-size budget and price:** decide whether to replace the SDK-style
  `u64::MAX` proof-size block limit with an intentional finite value. This can
  materially change proof-size pricing and block admission, so it is not
  bundled into the initial Revive runtime upgrade.
- **Fee multiplier policy:** the initial integration retains the fixed
  multiplier of `1`. Revisit congestion-based adjustment from observed
  testnet load.
- **Testnet deployment:** the runtime supports upgrading the existing testnet.
  `InitializeReviveAccount` reproduces Revive's genesis-time minimum-balance
  initialization and is idempotent. No native accounts are pre-mapped; users
  can call Revive's permissionless account-mapping extrinsic as needed.
- **Public registration:** register testnet Chain ID `20033` before publishing
  a public EVM RPC. Select and register a separate production ID later.

### Implementation checklist

- [x] Add `pallet-revive` to workspace and runtime dependencies.
- [x] Propagate `std`, `runtime-benchmarks`, and `try-runtime` features.
- [x] Assign stable runtime pallet index `14`; existing indices `0..=13`
      must not move.
- [x] Verify that the runtime macro includes Revive's hold reason in the
      aggregated `RuntimeHoldReason` type.
- [x] Implement the accepted `pallet_revive::Config` values.
- [x] Add `pallet_revive::evm::tx_extension::SetOrigin<Runtime>` after
      transaction payment and before `WeightReclaim`.
- [x] Define `EthExtraImpl` for Ethereum transactions.
- [x] Replace the generic unchecked extrinsic wrapper with Revive's EVM-aware
      wrapper while preserving Quip's hybrid native signature flow.
- [x] Implement Revive runtime APIs.
- [x] Add a pinned SDK Ethereum RPC sidecar Dockerfile.
- [x] Publish the pinned sidecar as a release-tag-only, multi-architecture
      `quip-network-evm-sidecar` image with node-matching tag semantics.
- [ ] Define the production deployment and persistent/archive receipt storage
      before exposing the public testnet Ethereum RPC.
- [x] Leave genesis mapped accounts empty; permissionless explicit mapping is
      available and avoids divergence from an upgraded existing testnet.
- [x] Add initialization required for an existing-testnet
      runtime upgrade.
- [x] Increment runtime `spec_version` to `115` and `transaction_version` to
      `6`.
- [ ] Run runtime benchmarks and replace provisional weights where necessary.

### Pinned-SDK tooling notes

- `cargo check -p quip-network-node --features runtime-benchmarks` succeeds
  with `SKIP_PALLET_REVIVE_FIXTURES=1`, validating Quip's benchmark registry
  and feature wiring. Generating the actual Revive fixtures fails in the pinned
  SDK's fixture builder under Rust 1.95 because custom JSON targets now require
  `-Zjson-target-spec`; the SDK builder does not pass that flag. Do not treat a
  fixture-skipped check as a substitute for running the benchmarks.
- `cargo check -p quip-network-node --features try-runtime` currently fails in
  the pinned SDK's `pallet-staking` migration because its
  `MigrateDisabledValidators` implementation is missing `peek_disabled`.
  Quip does not configure staking; this is an SDK feature-unification failure,
  but it must be resolved before using `try-runtime` for testnet rehearsal.

### Validation checklist

- [x] Runtime integrity tests pass with the selected memory, fee, gas, and
      block-weight parameters.
- [x] Existing native transactions using Quip's hybrid signature continue to
      encode and validate correctly in runtime tests.
- [ ] Ethereum legacy and typed transactions validate the expected Chain ID
      and reject transactions signed for another network.
- [ ] `eth_chainId` and the EVM `CHAINID` opcode return `1337` on local builds
      and `20033` on testnet builds.
- [ ] Contract upload, deployment, calls, events, and termination work.
- [ ] Storage deposits, code-hash lockups, refunds, and address-mapping
      deposits match the accepted economics.
- [ ] Gas estimation, fee conversion, rounding, out-of-gas behavior, and the
      90% maximum-extrinsic cap behave as intended.
- [ ] Built-in precompiles work while no Quip-specific precompiles are
      exposed.
- [ ] Debug-only behavior remains unavailable in normal runtime artifacts.
- [ ] Runtime APIs and Ethereum RPCs work with representative wallet and
      contract-development tooling.
- [x] Formatting, runtime/node clippy with warnings denied, runtime/node tests,
      both Chain-ID runtime test builds, and the runtime release/Wasm build
      pass.
- [ ] Run the complete workspace CI matrix before deployment.
- [ ] If upgrading the existing testnet, migration checks and a testnet smoke
      test demonstrate safe initialization without changing existing pallet or
      call indices.

## Parameters not present in this Revive version

Unlike older `pallet-contracts` configurations, this `pallet-revive::Config`
does not expose `MaxCodeLen`, `MaxStorageKeyLen`, `Schedule`, `CallFilter`,
`ChainExtension`, or `WeightPrice`. Do not copy those parameters from
`pallet-contracts` integration guides.

## Decision log

| Date | Decision |
|---|---|
| 2026-07-16 | Use EIP-155 chain ID `1337` for local development. |
| 2026-07-16 | Use EIP-155 chain ID `1337` for the `local_testnet` and `local_three_validator` presets. |
| 2026-07-16 | Use EIP-155 chain ID `20033` provisionally for the public Quip testnet. |
| 2026-07-16 | Defer selection of a distinct production chain ID until a production network is planned. |
| 2026-07-16 | Make EVM code upload and contract instantiation permissionless for signed accounts. |
| 2026-07-16 | Start with no Quip-specific precompiles and disable Revive debug mode in normal runtime artifacts. |
| 2026-07-16 | Adopt the SDK production-style storage deposit baseline: `10 * MICRO_UNIT` per byte, `200 * MILLI_UNIT` per main-trie item, and `2 * MILLI_UNIT` per contract child-trie item. |
| 2026-07-16 | Set `CodeHashLockupDepositPercent` to 30% for the initial testnet. |
| 2026-07-17 | Set `NativeToEthRatio` to `1_000_000`, preserving exact conversion between 12-decimal AGLS and 18-decimal EVM values. |
| 2026-07-17 | Set `GasScale` to `1_000`, yielding an initial reported base gas price of 1 gwei with fee multiplier `1`. |
| 2026-07-17 | Cap an Ethereum transaction at 90% of the normal maximum-extrinsic weight. |
| 2026-07-17 | Burn minor EVM dust/value-conversion and rounding withdrawals by configuring `OnBurn = ()`. |
| 2026-07-17 | Resolve EVM `block.coinbase` from the BABE authority through `Session::Validators`. |
| 2026-07-17 | Use Revive integrity-check memory baselines of 128 MiB runtime memory and 512 MiB PVF memory. |
| 2026-07-17 | Use default/testnet and `dev-chain-id` runtime artifacts to deliver Chain IDs `20033` and `1337`, respectively. |
| 2026-07-17 | Require the `dev-chain-id` node/runtime artifact for all local presets and reject `quip-testnet` from that artifact; only the default/testnet artifact can run the public testnet with Chain ID `20033`. |
| 2026-07-17 | Replace transaction-payment `IdentityFee` with `BlockRatioFee<1, 1, Runtime, Balance>` while retaining the fixed multiplier and length fee. |
| 2026-07-17 | Add Revive at stable pallet index `14`, with runtime APIs, EVM-aware extrinsics, and idempotent existing-chain account initialization. |
| 2026-07-17 | Use a pinned SDK Ethereum RPC sidecar while embedded integration is blocked on paritytech/polkadot-sdk#11297; defer the contract upload/call smoke test. |
| 2026-07-31 | Publish the pinned SDK sidecar as the release-tag-only, multi-architecture `quip-network-evm-sidecar` image. |
| 2026-08-07 | Remove the in-repository Revive local orchestration and funding command; retain the standalone sidecar image and CI release publishing. |
| 2026-08-10 | Reintroduce local EVM orchestration (`make evm-explorer`): native `--dev` node, prebuilt sidecar image, and Blockscout replacing the removed Subscan stack. No funding command this time. |

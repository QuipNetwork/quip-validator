# Phase 1: faucet containment and scheduled subnet issuance

Baseline: validator onboarding plan rev 3; cryptoeconomics MR !2 at `03ecb99`.
Branch: `feat/validator-onboarding-phase-1`, based on locally recorded
`main` / `origin/main` `c236a6866951ea29a91c281188883daed928bc7b`.
Remote freshness could not be verified: the initial fetch failed at DNS.

## Behavior and accounting

Runtime 119 appends EmissionController at index 19. Existing pallet and call
indices are preserved; transaction version remains 7. Version 118 is already
tagged (`v0.3.2`, rc1, rc2).

The controller mints 40 million QUIP per fixed 365-day accounting year into
subnet pots, using on-chain time and cumulative entitlement. Issuance does not
require mining activity. Whitepaper §7.1 says subnet emissions start at launch;
§7.2 specifies 40M every year. They continue after vesting, rather than beginning
only when vesting ends. Genesis may supply an explicit positive Unix start time;
otherwise the first nonzero consensus timestamp establishes the anchor. No
Unix-epoch backfill occurs. There is no automatic cliff/vesting pallet in this
phase and no maximum-supply cap.

`on_initialize` reads the preceding block's timestamp, so funding lags by one
block. Gaps catch up on the next observation. Each pot receives the difference
between its cumulative rounded-down entitlement and its lifetime issued amount.
Repeated calls and clock regression cannot mint twice. Fractional base units
carry forward; the sum can trail the ideal budget by less than one base unit per
route. Arithmetic and deposits are checked, with transactional rollback on
failure. Invalid allocations (duplicates, zero shares, excessive length, or
shares not totaling 100%) are rejected at genesis.

Existing testnet/dev presets select the whitepaper equal-split fallback: Ising
ID 0 and QVRF ID 1 each receive 50%. QVRF's allocation accumulates in its pot;
there is no QVRF payout consumer yet. Accounts are derived by hashing the domain,
pallet ID, and subnet ID. No account key is provided. Routing is immutable in
Phase 1; adding governance/QCA routing requires a reviewed migration preserving
already-accrued entitlements. The paper does not mandate this testnet split for
a future mainnet genesis.

QPoW requests the existing maximum 1 UNIT reward from the Ising pot. The
controller transfers up to the available balance, retaining the pot's ED. It
returns the actual amount, which QPoW records in miner totals and proof events.
An empty/locked pot or failed transfer yields zero reward without invalidating a
valid proof. Rewards no longer issue currency a second time. The reserved ED is
part of scheduled issuance, not an extra mint. **D1 release decision: the 1-UNIT cap is a testnet-only interim payout policy.**
At six-second blocks Ising accrues about 3.8 QUIP per block but pays at most one;
at least roughly 74% of its allocation and all of QVRF's allocation accumulate.
This does not implement continuous circulation or share-dependent per-work
implicit decay. Cryptoeconomics/Foundation owns the payout/smoothing decision
before an economic/mainnet release. Runtime 119 must not be presented as the
completed mainnet emission model. Do not silently drain accumulated pots as part
of a later change; the release decision must specify treatment of that surplus.

Staking is not present in Phase 1. No validator allocation or permission to mint
staking rewards is invented. Future staking must use the shared reservation or
prefunding accounting from plan §6.4 before a nonzero EraPayout; classic staking
cannot simply mint again against balances already issued to these pots.

## Faucet authority and fuse

The faucet has stored `Enabled` / `PermanentlyDisabled` state, defaulting to
permanently disabled. `mint` keeps call index 0; `disable` is added at index 1.
There is no enable/reset dispatchable. Mint and disable require the configured
origin; bare Root is rejected in the Phase 1 runtime.

Until the Phase 2 Foundation collective exists, an explicit genesis-appointed
signed operational account is the authority. A multisig account can be appointed.
Testnet/dev presets appoint the same account as their genesis sudo key, but the
faucet call is submitted **directly as that signed account**, not through sudo.
No lookup of the mutable sudo key is used: changing sudo does not silently
change faucet authority. Phase 2 substitutes the Foundation origin. There is no
ordinary Phase 1 authority-rotation call; loss of the operational key requires a
reviewed runtime upgrade. This is an interim trusted operator, not completed
Foundation governance.

Faucet minting goes through the controller and a separate lifetime testnet budget
of 1 billion QUIP in the current test presets. Exhaustion fails atomically. This
budget is a finite operational choice for disposable test currency, not part of
the scheduled 40M/year mainnet budget. Disabling the faucet does not refund or
reset counters, and does not disable legitimate scheduled subnet issuance.

## Genesis and upgrade policy

No mainnet spec or allocation is invented here. A fresh mainnet genesis must
set the faucet to `PermanentlyDisabled`, omit its authority, and set its budget
to zero. Default genesis does all three. Supply/endowments, schedule activation,
and subnet allocations require explicit mainnet genesis configuration. Never
reuse faucet-endowed testnet state as mainnet stake.

The legacy faucet had no state. Its v0-to-v1 upgrade writes nonterminal `Paused`,
clears any authority, and is idempotent. A pre-existing explicit
`PermanentlyDisabled` fuse is preserved. `Paused` rejects minting; only a reviewed
activation migration may move it to Enabled. There is no public enable/reset
call. Mainnet/default genesis remains terminal `PermanentlyDisabled`. A new controller on an existing chain
has missing configuration and therefore defaults to no scheduled or faucet
issuance. QPoW proofs continue, with zero payouts while pots are empty. This is
intentional fail-closed migration behavior, **not an automatically activated
live-testnet emission migration**. Choose a fresh testnet genesis for rehearsal;
an in-place activation must be a separately reviewed migration supplying the
intended authority, budget, time anchor, and routes. It must accept only the Paused legacy state, and must never reset an
already blown fuse or existing issuance counters. **D2 release decision:** choose
this nonterminal pause model; do not use runtime 119 as a routine live testnet
upgrade until the activation migration or coordinated fresh-genesis rollout is
specified. A temporary zero-reward interval is not silently approved for miners.

New test presets change their genesis hash. They cannot be substituted for the
published raw chain spec of the existing network. Deployments continue to use
that raw spec until an explicitly coordinated fresh-network rollout. The
existing chain's runtime-upgrade behavior is as described above.

## External faucet service rollout dependency

The sibling `faucet` service still wraps mint in `Sudo::sudo`
(`../faucet/src/calls.rs:14`) and validates `Sudo.Key` at startup
(`../faucet/src/main.rs:236`). It is incompatible with this runtime's direct
signed-authority calls. Before enabling a fresh Phase 1 testnet, update that
service's call builder, authority/fuse/budget checks, configuration descriptions,
and deployment runbook, or keep the service stopped. Its existing
`QUIP_FAUCET_FAUCET_KEY` must identify the genesis faucet authority, not merely
a current sudo key. This Phase 1 runtime MR does not silently change the
separate service repository or existing deployments. The live network continues
to use its old runtime/service pairing until a coordinated rollout.

The separate `faucet` branch `fix/confirm-sudo-topups` fixes the existing silent
failure independently of runtime 119: it waits for finalization and requires
both System success and an inner successful Sudid for the exact extrinsic.
Timeouts and malformed/missing receipts fail closed. This is still the legacy
sudo service; direct-authority compatibility remains a rollout dependency.
The Akash manifest and service README explicitly mark this boundary.

## Events and operational visibility

`EmissionController.FaucetMinted` is the canonical **issuance** event.
`FaucetOps.Minted` is the operation receipt; indexers must not sum both as new
supply. `SubnetFunded` identifies scheduled issuance, while `RewardPaid` is a
transfer, not another mint. `FundingFailed` exposes a failed accrual (retried on
later blocks). `RewardUnavailable` reports zero payout with either a currency
error or no error for empty/self-transfer cases. Events are bounded by the route
limit and one winning proof per block. Reference weight generation must include
these event writes.

The existing test presets keep their known single operational account for
compatibility with local tooling. It can be a multisig account in a rehearsal
chain spec; the Phase 2 collective is the planned governance replacement.

## Issuance exceptions and boundaries

The controller owns the intended faucet and mining issuance paths. It does not
make Root economically powerless. `balances.force_set_balance`, system storage
writes, runtime replacement, and Root dispatch through retained sudo can bypass
these rules. `InitializeReviveAccount` also retains its one-time bounded ED mint.
Genesis endowments are outside scheduled issuance. The Root exception remains
a trust assumption even after Foundation governance replaces sudo.

The fuse is irreversible through ordinary runtime calls, not against a runtime
replacement or arbitrary Root storage rewrite. The public Rust faucet backend
is for origin-checked runtime callers, not an exposed dispatchable. Future
callers must enforce the fuse and authority or explicitly introduce a reviewed
new issuance path.

## Validation and release gates

Tests cover schedule continuity beyond year ten, annual totals and subnet
isolation, duplicate timestamps, clock regression, rounding accumulation, missing
configuration, authority/Root rejection, fuse irreversibility, legacy upgrade,
budget exhaustion, arithmetic rollback, QPoW proof acceptance with no funds,
actual reward recording, and the explicit Root issuance exception. Runtime
preset tests verify controller/faucet wiring and distinct AccountId32 pots.

Benchmark definitions cover funding routes, pot reward transfers, faucet mint,
and fuse disable. QPoW reserves the reward-backend weight in on_initialize for
its on_finalize transfer. The new weight files contain conservative provisional
estimates, **not measured production weights**. Run the repository's manual
reference-machine benchmark job and commit its generated weights before release.

Pallet try-runtime tests exercise the migration checks and state invariants.
A live-snapshot try-runtime rehearsal and a full runtime/Wasm release build are
separate release gates, not implied by pallet unit tests. Exact executed checks
and outstanding environment limitations accompany the Phase 1 review request.
rr's Phase 1 review permits Phase 2 to proceed. rr approved the follow-up runtime changes on 2026-09-29
(message msg_20260928_230129_f8884534). This approval does not waive the
remaining release gates.

### Executed locally

All commands used cached dependencies (`--offline`). Native runtime checks used
`SKIP_WASM_BUILD=1 SKIP_PALLET_REVIVE_FIXTURES=1`; the release build omitted
`SKIP_WASM_BUILD` and produced the runtime Wasm.

| Check | Result |
| --- | --- |
| `cargo test --workspace` | 411 passed, 2 pre-existing sweep-dependent tests ignored |
| Affected pallet tests with `runtime-benchmarks,try-runtime` | 152 passed, same 2 ignored |
| Runtime library tests with `try-runtime` | 31 passed |
| Metadata V16 / regenerated signing fixture | 5 + 4 passed (also in workspace suite) |
| `cargo clippy --workspace --all-targets -- -D warnings` | passed |
| Affected pallets + runtime Clippy, all targets, both features, warnings denied | passed |
| `cargo check --workspace` | passed, including node |
| Affected pallets `--no-default-features --target wasm32v1-none` | passed |
| `cargo build --release -p quip-protocol-runtime` | passed, generated compressed and uncompressed Wasm |
| `cargo fmt --all --check`, staged diff whitespace check | passed |

The runtime multisig integration tests emit pre/post-dispatch weight diagnostics
while passing; this phase does not recalibrate upstream multisig weights.
`SKIP_PALLET_REVIVE_FIXTURES=1` excludes building the SDK's external contract
fixtures. These results do not claim a live-chain snapshot rehearsal, reference
machine weight calibration, or published MR/CI run.

### Review follow-up validation

D1 uses the explicitly testnet-only capped payout with retained surplus described
above; Foundation/cryptoeconomics owns the final distribution policy. D2 uses a
non-terminal legacy `Paused` state while preserving any already blown fuse.
After these fixes, the affected pallet suite with both features passed 153 tests
(2 existing sweep tests ignored). Feature-enabled Clippy, no-std checks and the
release runtime/Wasm build passed again.

The sibling faucet receipt implementation passed six source-level tests (including runtime-version
compatibility checks) and
Clippy in a temporary harness against the available current runtime/SDK. Its
original pinned dependencies could not be built in this sandbox; the full
service also requires an uncached `axum` dependency. Those checks do not establish
compatibility with the service's original pins. A full pinned build and service
integration test remain required before deploying that independent fix.

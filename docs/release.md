# Release Checklist

## Pre-tag verification

- [ ] All target-version commits merged to `main` via MR
- [ ] `cargo check --workspace --all-targets` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace` passes
- [ ] Node image builds locally: `docker build -t quip-network-node:rc .`
- [ ] Sidecar Dockerfile passes its static build check:
      `docker build --check -f docker/revive-eth-rpc.Dockerfile .`
- [ ] `./target/release/quip-network-node --version` reports the target version
- [ ] `./target/release/quip-network-node export-chain-spec --chain quip-testnet --raw > /tmp/quip-testnet.raw.json` succeeds
- [ ] Companion `nodes.quip.network` MR with the matching
      `chain-specs/aglais-network.json` (sha256 from the above raw export) is
      merged

## Tag

```bash
git checkout <release-branch>   # main or the active series branch (e.g. v0.2) —
git pull                        # this choice decides the floating tag (see below)
git tag -a v<MAJOR>.<MINOR>.<PATCH> -m "v<MAJOR>.<MINOR>.<PATCH>: <one-line summary>"
git push origin v<MAJOR>.<MINOR>.<PATCH>
```

The GitLab CI pipeline at `.gitlab-ci.yml` picks up the tag via the
`$CI_COMMIT_TAG` rule and publishes
`registry.gitlab.com/quip.network/quip-validator/quip-network-node:v<MAJOR>.<MINOR>.<PATCH>`
and
`registry.gitlab.com/quip.network/quip-validator/quip-network-evm-sidecar:v<MAJOR>.<MINOR>.<PATCH>`.

### Version-tag format (shared standard)

Pre-release tags use **SemVer hyphenated** pre-releases —
`v<MAJOR>.<MINOR>.<PATCH>-rcN` (e.g. `v0.2.1-rc18`), **never** the PEP 440
no-hyphen form `v0.2.1rc18`. This is the cross-repo standard so `quip-node-manager`
(and any SemVer consumer) can order release candidates correctly; see
`quip-protocol/docs/VERSIONING.md` for the full rationale.

Both container images publish on release tags **only** — branch pushes never
build or push images. Their floating tags follow the branch the tag was cut from,
resolved by the `resolve-floating-tag` CI job via commit ancestry (tag
pipelines carry no branch variable): a tag on `v0.2` publishes `:<tag>` +
`:v0.2`, a tag on `main` publishes `:<tag>` + `:latest`, both plus
`:sha-<short-sha>`. A tag reachable from neither branch gets only `:<tag>` +
`:sha-<short-sha>`.

## Post-tag verification

- [ ] CI pipeline on the tag completes green (`glab ci status --live`)
- [ ] Both images are present:

  ```bash
  docker pull registry.gitlab.com/quip.network/quip-validator/quip-network-node:v<MAJOR>.<MINOR>.<PATCH>
  docker pull registry.gitlab.com/quip.network/quip-validator/quip-network-evm-sidecar:v<MAJOR>.<MINOR>.<PATCH>
  ```

- [ ] Sidecar image starts and reports its CLI help:

  ```bash
  docker run --rm \
    registry.gitlab.com/quip.network/quip-validator/quip-network-evm-sidecar:v<MAJOR>.<MINOR>.<PATCH> \
    --help
  ```

- [ ] Smoke test against the published spec:

  ```bash
  curl -fsSL https://gitlab.com/quip.network/nodes.quip.network/-/raw/main/chain-specs/aglais-network.json \
      -o /tmp/aglais-network.json
  docker run --rm -v /tmp:/spec \
      registry.gitlab.com/quip.network/quip-validator/quip-network-node:v<MAJOR>.<MINOR>.<PATCH> \
      --chain=/spec/aglais-network.json --tmp --name v-smoke --no-mdns
  ```

  Expect peer discovery against at least one of the three canonical bootnodes
  within 60 seconds.

## Runtime 119 mempool changes

The pinned AGLS block 153726 reports runtime spec 119 / transaction version 7.
The earlier claim that 119 had not shipped is obsolete. The mempool changes
described for that runtime are:

- `IsingParams.min_energy_milli` is renamed to `max_energy_milli`. The SCALE
  position and type are unchanged, so positional decoders keep working.
  Clients that encode `propose_job` by metadata field name must use the new
  name.
- `TopNWeighted` splits the reward by rank. With `k` ranked solvers, rank `r`
  (0 is best) has weight `k - r`. Energy sign and magnitude no longer affect
  the split. Any rounding remainder goes to rank 0.
- Orders are finalised in `on_initialize` at their effective expiry block, at
  most 8 per block with overflow carried to the next block. An expiry that falls
  in the current block (zero `block_wait` or zero `deadline_blocks`) is finalised
  in the next block. `OrderExpired` fires there for every order. `ResultReady`
  and `OrderResults` move from `claim_reward` to expiry. `claim_reward` only pays
  and emits `RewardClaimed` and `OrderClosed`.
- Pallet storage version moves 2 to 3. The migration indexes existing open
  orders. Orders already past expiry are finalised from the upgrade block
  onwards, eight per block. The migration runs inside one block and reads every
  open order, so check the `OpenOrders` count on the live chain before scheduling
  the upgrade.

## Runtime 120 version assignment

The !71 stack assigns **spec 120 / transaction version 8 / QuantumPowApi v3 /
QuantumPow storage v7**. The spec bump is required because 119 is deployed.
This is a candidate assignment, not a released or deployed runtime.

Read-only checks on **2026-10-25 at 22:29 UTC** established:

- A fresh `chain_getFinalizedHead` from
  `https://bootnode-2.aglais.quip.network:20049/rpc` returned block **156607**,
  hash `0x0df326c87aee42ebfea81e5b2e8761e90af39c9e5a827008ecef905a2d934a6b`.
  The genesis hash was
  `0x542a9915acf840b8940d35ca6de242f669a0c6e3b40f9a8d8c14b9bf7b8ebecb`.
- `state_getRuntimeVersion` at that same block reported **spec 119 / tx 7**
  and API ID `0x17f0e16273cf5f3b` at **version 2**.
  `state_getStorage` at that block for QuantumPow's storage-version key
  `0x9b2c4dbe49d7a1aed7ce99e4b8c072e84e7b9012096b41c4eb3aaf947f6ea429`
  returned `0x0600` (**storage version 6**).
- `git ls-remote --heads --tags origin` identified `main` and the peeled
  latest release tag `v0.4.0` as
  `bc4552a2cc6913bd057ecfdf9abc99197393b55c`. Its
  [runtime source](https://gitlab.com/quip.network/quip-validator/-/blob/bc4552a2cc6913bd057ecfdf9abc99197393b55c/runtime/src/lib.rs)
  declares **spec 119 / tx 7**. Reading `runtime/src/lib.rs` at every returned
  head/tag object (76 refs, including annotated tags and their peeled entries)
  found no spec above 119; no object was unavailable. The !71 parent remained
  `c722e308e1954456dd6cd1c0c3e9e3716470879a`.

This evidence establishes 120 as unused by the inspected refs and newer than
the pinned live runtime. It is not a reservation across future branches or
releases. Repeat the check before release, reconcile any conflicting assignment,
and regenerate/test the signing fixture whenever the version changes. Historical
spec-119 fixtures remain unchanged; they describe the chain at their pinned blocks.

## Pending !71 API-v3 release gate

Follow the [miner/client upgrade contract](runtime-119-miner-integration.md).
The filename is historical: this branch assigns spec 120 to the API3/tx8/storage7
upgrade. These are release obligations, not evidence that the checks have already
passed.

- [ ] Recheck the runtime-120 assignment above against a fresh finalized block
      and current release refs before release. Record spec/transaction/API/storage
      versions; regenerate and test the signing fixture after version changes.
- [ ] Record a compatible companion coordinator/miner revision and local
      API3/tx8 end-to-end submission/finalization evidence. The audited
      canonical `quip-solver` revision
      `4674a34b6c7155aadc60ac3eb7b69db93241c1af` is still v2-shaped and accepts
      API>=2; it does not satisfy this rollout gate. This pre-existing companion
      dependency does not block creating the validator draft MR.
- [ ] Verify miner/indexer decoder selection from the API version **at the
      queried block**, strict full-consumption decoding where clients own it,
      supported-version refusal, and upgrade/reconnect cache invalidation.
      Test both historical v2 reads and current v3 responses. Generated
      `changed_in` client methods do not route decoders automatically.
- [ ] Verify raw QBlock legacy/current layouts independently of API/spec
      version, including unmeasured-frustration backfill and preserved nonce.
- [ ] Rehearse the full runtime upgrade with pre/post checks against a pinned
      real snapshot and record all history/index checks, weights and limits.
      Partial pallet fixtures and API response replay are not this rehearsal.
- [ ] Complete runtime release/signing, workspace tests/Clippy, benchmark
      preflight, required reference-machine weights and actual MR CI checks.

Deployment and merging require their own release authorization; completing
the stacked refactor or its test suite does not perform those actions.

## What v0.2.0 ships

- First semver tag for the validator image; previously only `:latest` and
  `:sha-<short>` were published.
- Built-in `quip-testnet` chain spec preset with three operator-controlled
  bootnodes and a `ChainType::Live` genesis.
- Helper script (`scripts/derive-operator-keys.sh`) and example
  (`crates/transaction-crypto/examples/derive_genesis_keys.rs`) for
  reproducing operator key generation end-to-end.
- macOS-only `.cargo/config.toml` rpath fix so `cargo build` works on a
  fresh Xcode install without `LIBCLANG_PATH` exports.

Runtime `spec_version` remains at `101`; v0.2.0 is packaging plus the named
testnet identity, not a runtime upgrade.

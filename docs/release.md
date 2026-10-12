# Release Checklist

## 0.4.0 package inventory (MR !100)

The 0.3.2 release line moves to **0.4.0** in all 13 inheriting workspace
packages and the excluded Python binding (14 packages total):

| Package | Version source |
| --- | --- |
| `quip-network-node` | workspace |
| `quip-protocol-runtime` | workspace |
| `quantum-validation` | workspace |
| `quip-transaction-crypto` | workspace |
| `pallet-emission-controller` | workspace |
| `pallet-evm-chain-id` | workspace |
| `pallet-faucet-ops` | workspace |
| `pallet-miner-registry` | workspace |
| `pallet-quantum-compute-mempool` | workspace |
| `pallet-quantum-pow` | workspace |
| `pallet-template` | workspace |
| `pallet-validator-admission` | workspace |
| `pallet-xqvm` | workspace |
| `quip-transaction-crypto-py` / PyPI `quip-signer` | excluded crate's own Cargo manifest |

Both root and Python `Cargo.lock` files track these versions. The excluded
Python lock had a stale 0.2.2 entry; Cargo refreshed it to match 0.4.0. Python's
`pyproject.toml` uses `dynamic = ["version"]`; maturin reads the excluded
crate's version, so no second version field is added. Its release documentation
allows an independent wheel version; this release explicitly aligns it to 0.4.0.

The independently versioned `quip-transaction-crypto-core` and
`quip-transaction-crypto-wasm` crates retain **0.1.0**. They explicitly opt out
of workspace version inheritance, and their path dependencies have no 0.3.2
constraint. The generated `js/quip-transaction-crypto-wasm/package.json` stays at
0.1.0 to match its Rust source through wasm-pack. The separate TypeScript
`@quip-network/quip-signer` package and its package-lock root entries also retain
0.1.0; the browser CI builds/tests it but does not couple its npm release version
to the node tag. No third-party package versions or historical release notes
are rewritten.

Runtime spec **119**, transaction **7**, and mempool storage **4** are unchanged.
Package versioning does not imply deployment. Runtime 119 targets fresh reset
genesis. Before merge/release, regenerate the changed mempool weights on the
reference runner, including the extra lifecycle cases, then check the generated
output and Normal dispatch limits as described in
[pallet benchmarking](pallet-benchmarking-plan.md#permissionless-settlement-reference-coverage-and-release-gate).


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

Runtime 119 has not shipped. The next tag that carries it includes these
`QuantumComputeMempool` changes:

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

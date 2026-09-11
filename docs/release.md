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

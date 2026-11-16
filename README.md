# Quip Network node

Quip Network solochain node (`quip-network-node`).

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

Runtime 117 is the H2/H4 chain-wipe relaunch. It starts from fresh genesis and
is not an in-place upgrade of the earlier H1/H3 network. Operators must use a
fresh chain database, re-insert the H4 BABE and H2 GRANDPA keys, and sync from
the new genesis; old signed extrinsics are incompatible (`transaction_version`
is 7).

## Getting Started

Depending on your operating system and Rust version, there might be additional
packages required to compile this template. Check the
[Install](https://docs.substrate.io/install/) instructions for your platform for
the most common dependencies. Alternatively, you can use one of the [alternative
installation](#alternatives-installations) options.

Fetch solochain template code:

```sh
git clone https://github.com/paritytech/polkadot-sdk-solochain-template.git solochain-template

cd solochain-template
```

### Build

🔨 Use the following command to build the node without launching it:

```sh
cargo build --release
```

### Join the live public testnet

Omit `--chain` to select `quip-testnet` (EIP-155 `20033`).

```sh
# Live public testnet (EIP-155 20033). Default if you omit --chain.
./target/release/quip-network-node
```

### Embedded Docs

After you build the project, you can use the following command to explore its
parameters and subcommands:

```sh
./target/release/solochain-template-node -h
```

You can generate and view the [Rust
Docs](https://doc.rust-lang.org/cargo/commands/cargo-doc.html) for this template
with this command:

```sh
cargo +nightly doc --open
```

### Single-Node Development Chain

The following command starts a single-node development chain that does not
persist state. `--dev` is the Alice-only local preset (EIP-155 `1337`), not
the public testnet.

```sh
# Alice-only local chain (EIP-155 1337)
./target/release/solochain-template-node --dev
```

To purge the development chain's state, run the following command:

```sh
# Alice-only local chain (EIP-155 1337)
./target/release/solochain-template-node purge-chain --dev
```

To start the development chain with detailed logging, run the following command:

```sh
# Alice-only local chain (EIP-155 1337)
RUST_BACKTRACE=1 ./target/release/solochain-template-node -ldebug --dev
```

Development chains (`--dev` / `--chain=dev`):

- Maintain state in a `tmp` folder while the node is running.
- Use the **Alice** account as the sole validator authority.
- Use the **Alice** account as the default `sudo` account.
- Are preconfigured with a genesis state (`/node/src/chain_spec.rs`) that
  includes several pre-funded development accounts.
- Use EIP-155 chain ID `1337`.


To persist chain state between runs, specify a base path by running a command
similar to the following:

```sh
// Create a folder to use as the db base path
$ mkdir my-chain-state

// Alice-only local chain (EIP-155 1337)
$ ./target/release/solochain-template-node --dev --base-path ./my-chain-state/

// Check the folder structure created inside the base path after running the chain
$ ls ./my-chain-state
chains
$ ls ./my-chain-state/chains/
dev
$ ls ./my-chain-state/chains/dev
db keystore network
```

### Connect with Polkadot-JS Apps Front-End

After you start the node template locally, you can interact with it using the
hosted version of the [Polkadot/Substrate
Portal](https://polkadot.js.org/apps/#/explorer?rpc=ws://localhost:9944)
front-end by connecting to the local node endpoint. A hosted version is also
available on [IPFS](https://dotapps.io/). You can
also find the source code and instructions for hosting your own instance in the
[`polkadot-js/apps`](https://github.com/polkadot-js/apps) repository.

Quip uses hybrid BABE and GRANDPA consensus keys. Polkadot.js Apps does not
require custom types for Quip anymore.

### Multi-Node Local Testnet

A scripted three-validator local network is available two ways.
Both use `--chain=local3` (EIP-155 `1337`). This is not the public testnet.

- **Native build:** `scripts/start-local3.sh` builds the debug binary and starts
  three validators (Alice/Bob/Charlie) against the embedded `local3` chain spec.
- **Docker:** `docker compose up --build` starts the same three-validator
  topology in containers. See the [Docker](#docker) section below.

Both paths use the same hardcoded libp2p node-keys and bootnode peer ID, so
they're interchangeable for development.

For background on multi-node consensus, see [Simulate a
network](https://docs.substrate.io/tutorials/build-a-blockchain/simulate-network/).

## Template Structure

A Substrate project such as this consists of a number of components that are
spread across a few directories.

### Node

A blockchain node is an application that allows users to participate in a
blockchain network. Substrate-based blockchain nodes expose a number of
capabilities:

- Networking: Substrate nodes use the [`libp2p`](https://libp2p.io/) networking
  stack to allow the nodes in the network to communicate with one another.
- Consensus: Blockchains must have a way to come to
  [consensus](https://docs.substrate.io/fundamentals/consensus/) on the state of
  the network. Substrate makes it possible to supply custom consensus engines
  and also ships with several consensus mechanisms that have been built on top
  of [Web3 Foundation
  research](https://research.web3.foundation/Polkadot/protocols/NPoS).
- RPC Server: A remote procedure call (RPC) server is used to interact with
  Substrate nodes.

There are several files in the `node` directory. Take special note of the
following:

- [`chain_spec.rs`](./node/src/chain_spec.rs): A [chain
  specification](https://docs.substrate.io/build/chain-spec/) is a source code
  file that defines a Substrate chain's initial (genesis) state. Chain
  specifications are useful for development and testing, and critical when
  architecting the launch of a production chain. Take note of the
  `development_config` and `testnet_genesis` functions. These functions are
  used to define the genesis state for the local development chain
  configuration. These functions identify some [well-known
  accounts](https://docs.substrate.io/reference/command-line-tools/subkey/) and
  use them to configure the blockchain's initial state.
- [`service.rs`](./node/src/service.rs): This file defines the node
  implementation. Take note of the libraries that this file imports and the
  names of the functions it invokes. In particular, there are references to
  consensus-related topics, such as the [block finalization and
  forks](https://docs.substrate.io/fundamentals/consensus/#finalization-and-forks)
  and other [consensus
  mechanisms](https://docs.substrate.io/fundamentals/consensus/#default-consensus-models)
  such as BABE for block authoring and GRANDPA for finality.


### Runtime

In Substrate, the terms "runtime" and "state transition function" are analogous.
Both terms refer to the core logic of the blockchain that is responsible for
validating blocks and executing the state changes they define. The Substrate
project in this repository uses
[FRAME](https://docs.substrate.io/learn/runtime-development/#frame) to construct
a blockchain runtime. FRAME allows runtime developers to declare domain-specific
logic in modules called "pallets". At the heart of FRAME is a helpful [macro
language](https://docs.substrate.io/reference/frame-macros/) that makes it easy
to create pallets and flexibly compose them to create blockchains that can
address [a variety of needs](https://substrate.io/ecosystem/projects/).

Review the [FRAME runtime implementation](./runtime/src/lib.rs) included in this
template and note the following:

- This file configures several pallets to include in the runtime. Each pallet
  configuration is defined by a code block that begins with `impl
  $PALLET_NAME::Config for Runtime`.
- The pallets are composed into a single runtime by way of the
  [#[runtime]](https://paritytech.github.io/polkadot-sdk/master/frame_support/attr.runtime.html)
  macro, which is part of the [core FRAME pallet
  library](https://docs.substrate.io/reference/frame-pallets/#system-pallets).

### Custody vesting

Both runtimes include stock vesting at pallet index 32. Mainnet allocation
amounts, custody identity, recipients and tranche terms remain unresolved and
block mainnet export. Genesis creates no vesting schedules; later signed
multisig custody transfers issue one schedule per tranche.

Vesting constrains the recipient's **total balance**, including held stake:
ordinary transfers preserve `total >= frozen`. Held stake discounts the amount
that must stay free, so a pre-bonded account with enough held stake can transfer
a new grant immediately, even before its vesting start. Both initial `bond`
and `bond_extra` from vested funds are intentionally allowed. Guaranteeing that
the grant itself stays illiquid requires a new owner decision and a hold-based
design; the current pallet does not provide that guarantee.

The minimum vested transfer is `100 * EXISTENTIAL_DEPOSIT` (0.1 QUIP). This raises
the cost of filling an account's 28 schedule slots: at the former ED floor,
28 × ED (0.028 QUIP) could fill them, while council Root force-remove is denied.
The higher floor prices this attack; it does not prevent it. Completed schedules
can be cleaned up with `vest`/`vest_other`; merging requires the recipient.

The immutable pre-vesting evidence is in
[`tests/fixtures/pre-vesting/manifest.json`](tests/fixtures/pre-vesting/manifest.json).
The original `f7da5ec` post-vesting capture is preserved in
[`tests/fixtures/post-vesting-f7da5ec/manifest.json`](tests/fixtures/post-vesting-f7da5ec/manifest.json);
its validation claims predate the review corrections above. The corrected
100×ED capture is recorded separately in
[`tests/fixtures/post-vesting-fix/manifest.json`](tests/fixtures/post-vesting-fix/manifest.json).
SDK weights remain provisional until the reference-machine run and final
metadata/storage/signing review tracked by
[QUI-2101](https://linear.app/quip-network/issue/QUI-2101/vesting-release-gate-reference-machine-weights-and-metadata-re),
owned by `roman@postquant.xyz`.

### Pallets

The runtime in this project is constructed using many FRAME pallets that ship
with [the Substrate
repository](https://github.com/paritytech/polkadot-sdk/tree/master/substrate/frame) and a
template pallet that is [defined in the
`pallets`](./pallets/template/src/lib.rs) directory.

A FRAME pallet is comprised of a number of blockchain primitives, including:

- Storage: FRAME defines a rich set of powerful [storage
  abstractions](https://docs.substrate.io/build/runtime-storage/) that makes it
  easy to use Substrate's efficient key-value database to manage the evolving
  state of a blockchain.
- Dispatchables: FRAME pallets define special types of functions that can be
  invoked (dispatched) from outside of the runtime in order to update its state.
- Events: Substrate uses
  [events](https://docs.substrate.io/build/events-and-errors/) to notify users
  of significant state changes.
- Errors: When a dispatchable fails, it returns an error.

Each pallet has its own `Config` trait which serves as a configuration interface
to generically define the types and parameters it depends on.

### Compute mempool audit behavior

Open orders accept at most 32 distinct solvers; a solver already admitted to an
order can still improve its submission. Expiry removes solution payloads while
retaining ranking, destination and poll-result records for settlement. Solver
registration reserves one existential deposit and deregistration refunds it.
Payouts below that deposit are skipped and their shares stay with the proposer.
The runtime checks that the minimum reward covers the smallest 32-way weighted
share (1,893,939,393 units versus the 1,000,000,000-unit deposit).

Expiry processes at most eight orders and eight buckets per block. The oldest
bucket receives the first four-order share; later due buckets share the remaining
budget. Under overflow, the oldest bucket progresses while later due buckets
can settle before it empties. A large or sustained backlog still delays work;
there is no fixed wall-clock settlement guarantee independent of backlog size.

Closed orders and all their per-order auxiliary records are automatically
removed after at least 28 eras of blocks: 403,200 blocks on the main runtime and
100,800 on the testnet runtime. This is independent of the staking unbonding
period. The cleanup hook processes at most eight records and eight buckets per
block, so backlog can extend retention. Unpaid expired orders retain their
records and escrow until settlement or reclaim. Poll results survive at least
their TTL, and order IDs are never reused. Archive consumers must retain closed
order history off chain before this window ends.

Open-order pagination decodes only the records needed to fill a page, but still
visits O(N) keys and sorts them in O(N log N); stale index entries can cause extra
decodes. This residual RPC cost remains part of QUIP-15/R7.

Before release, run the reference-machine benchmarks for the 32 maximum-size
solution deletions (QUIP-28), ED reserve/refund (QUIP-29), expiry fairness cursor
and look-ahead reads (QUIP-27), and the eight-order closed-record sweep plus
queue insertion (QUIP-30). `finalise_expired_orders` populates all 32 solution
slots at maximum payload size; `prune_closed_orders` also populates every
auxiliary prefix. The `prune_closed_orders(k)` weight-table entry uses the same handwritten
bound at the maximum eight bucket visits; the hook keeps its actual
order/bucket allowance as a componentwise floor. This entry is explicitly
unmeasured and must be replaced by the reference-machine `benchmark-weights`
job before release. The handwritten CPU, database and proof allowances are
conservative and unmeasured, and the automatic weight-staleness warning does
not validate them. Regeneration must reconcile the generated measurements
with these wrappers and mirror runtime weights before the release gate passes.

QUIP-08 — **Accepted Low** (owner decision): a proposer freeze can prevent
reserved-reward settlement. The attacker costs itself the entire reward it
denies: `reclaim_order` is closed once a solution exists, so no funds reach the
attacker and the solver loses its expected payment. `claim_reward` retains
`unreserve` followed by `transfer`.

The evaluated `repatriate_reserved` replacement does not solve this at SDK
`d511a17`: `balances/src/impl_currency.rs` lines 725-733 call
`do_transfer_reserved` with `BestEffort, Polite`, which cannot move frozen
funds; `balances/src/lib.rs` line 1298 also rejects a new beneficiary with
`DeadAccount`. `slash_reserved` plus `resolve_creating` was rejected because
it emits a misleading `Balances::Slashed` event on every settlement
(`impl_currency.rs` lines 703-708). Escrow based on holds was rejected as
disproportionate for a Low finding.

## Alternatives Installations

Instead of installing dependencies and building this source directly, consider
the following alternatives.

### Nix

Install [nix](https://nixos.org/) and
[nix-direnv](https://github.com/nix-community/nix-direnv) for a fully
plug-and-play experience for setting up the development environment. To get all
the correct dependencies, activate direnv `direnv allow`.

### Docker

A multi-stage `Dockerfile` builds the `quip-network-node` binary on top of
`debian:bookworm-slim` (~80 MB runtime image). The image exposes the binary
directly as ENTRYPOINT, so any Substrate CLI flag works at `docker run` time.

#### Build

```sh
docker build -t quip-network-node:local .
```

The first build compiles the full workspace and takes a while. BuildKit cache
mounts (declared in the Dockerfile) keep the cargo registry and target
directory between local rebuilds.

#### Pre-built images

Every push to `main` and every git tag publishes an image to the project's
GitLab Container Registry, so you don't have to build locally:

```sh
docker pull registry.gitlab.com/quip.network/quip-validator/quip-network-node:latest
```

Tag scheme:

- `:latest` — tip of `main`. Floating, advances on every merge.
- `:sha-<short>` — pinned to a specific commit on `main` or to a tagged release.
- `:<git-tag>` — pinned to a release tag (e.g. `:v0.1.0`).

#### Run as a validator

```sh
# Three-validator local chain (EIP-155 1337). Not the public testnet.
docker run --rm -v quip-data:/data -p 9944:9944 -p 30333:30333 \
  quip-network-node:local \
  --chain=local3 --base-path=/data \
  --validator --alice \
  --unsafe-rpc-external --rpc-cors=all
```

`--unsafe-rpc-external` is required because Substrate refuses to combine
`--rpc-external` with `--validator` by default (a safety guard against
exposing a validator's RPC to the public internet). For local development
the unsafe flag is fine; for production validators you almost certainly do
not want any external RPC at all.

#### Run as a full node

Same command, omit `--validator` (and the `--alice/--bob/--charlie` shortcut):

```sh
# Three-validator local chain (EIP-155 1337). Not the public testnet.
docker run --rm -v quip-data:/data -p 9944:9944 -p 30333:30333 \
  quip-network-node:local \
  --chain=local3 --base-path=/data \
  --bootnodes=/dns/<bootnode-host>/tcp/30333/p2p/<peer-id> \
  --rpc-external --rpc-cors=all
```

#### Local 3-node network via docker-compose

`docker-compose.yml` reproduces `scripts/start-local3.sh` in containers.
Each service passes `--chain=local3` (EIP-155 `1337`). This is not the public
testnet.

```sh
docker compose up --build           # start local3 (EIP-155 1337)
docker compose down                 # stop, keep chain state
docker compose down -v              # stop and wipe state
```

Then connect Polkadot.js Apps to `ws://localhost:9944` (node1),
`ws://localhost:9945` (node2), or `ws://localhost:9946` (node3).

## Public testnet

`quip-testnet` is the live public testnet ("AGLS" tokens, 12 decimals,
EIP-155 `20033`). Omitting `--chain`, or using `quip-testnet`, `quip_testnet`
or `testnet`, loads the exact raw artifact embedded from
[`chain-specs/quip-testnet.json`](chain-specs/quip-testnet.json).
Its historical runtime and genesis storage are independent of the runtime
compiled into a new node binary.

The file is a byte-for-byte copy of the public testnet raw spec published as
[`chain-specs/aglais-network.json`](https://gitlab.com/quip.network/nodes.quip.network/-/raw/main/chain-specs/aglais-network.json)
in the `nodes.quip.network` repository. It is vendored here as
`quip-testnet.json` because the local filename identifies the network it
serves; the upstream name refers to the same network under its older AGLS
label. The upstream file's SHA-256 was verified against GitLab's reported
content hash at upstream commit
`d65ab5b7ad62dad2de603c2611c14a72b24c1537`.
Its pinned SHA-256 is
`0aeeddaf5b546a491d96c35fd636614c6ab42d4e5d9724126182b52c8c694ad4`;
its genesis hash is
`0x542a9915acf840b8940d35ca6de242f669a0c6e3b40f9a8d8c14b9bf7b8ebecb`.
The blocking `live-testnet-spec` CI job checks the file's exact bytes,
including formatting. Node tests check all four aliases and the genesis hash.

The registered runtime `quip_testnet` preset remains available through
GenesisBuilder for policy auditing. Exporting that preset with a newly
compiled runtime does **not** reproduce the published live genesis.
`dev`, `local`, `local3` and `rehearsal` continue to use their runtime builders.
The unfinished `quip-mainnet` and `quip-testnet-relaunch` presets have no
built-in node chain aliases.

### Quickstart (built node)

```sh
cargo build --release -p quip-network-node
# Live public testnet (EIP-155 20033). Omit --chain for the same artifact.
./target/release/quip-network-node --chain=quip-testnet --base-path=./quip-data
```

The three canonical bootnodes (`bootnode-{1,2,3}.aglais.quip.network`) are
embedded in the raw file, with the live protocol ID `agls-network` and AGLS
properties unchanged.

### Using the raw file with an existing image

Explicit file loading also works with existing images; those images do not
acquire this new embedded-artifact behavior merely because this repository
changed. Validate the checked-out artifact before using its path:

```sh
python3 scripts/check-live-testnet-spec.py

docker run --rm -v "$PWD/chain-specs:/spec:ro" -v quip-data:/data \
  -p 9944:9944 -p 30333:30333 \
  registry.gitlab.com/quip.network/quip-validator/quip-network-node:v0.3.0 \
  --chain=/spec/quip-testnet.json --base-path=/data
```

The vendored path represents the public network currently selected by the
four aliases. A future approved relaunch must replace the artifact and update
its checksum, genesis-hash tests and provenance in the same reviewed activation
commit. The old artifact remains in Git history and release provenance. A new
genesis requires fresh databases; this change does not activate a relaunch.

CI also starts a non-validator against a temporary copy with **only bootnodes
removed**, checks its genesis over RPC, and restarts the same genesis-only DB.
Isolation relies on empty bootnodes and reserved-peer lists, disabled mDNS,
and disabled ordinary peers. The pinned litep2p backend still runs periodic
Kademlia queries. The smoke test therefore verifies empty bootnodes before
launch, observes at least ten seconds after RPC readiness, requires a Kademlia
query log marker, and rejects outbound dial logs across the full run. Missing
discovery logging fails the smoke test instead of silently shortening coverage.
This smoke test checks startup and DB reopen, not the shipped peer list, live
sync, consensus, or compatibility with an existing production DB.

### Running a validator

Operator validator slots are committed in
[`runtime/src/genesis_config_presets.rs`](runtime/src/genesis_config_presets.rs).
The [`scripts/derive-operator-keys.sh`](scripts/derive-operator-keys.sh) helper
derives operator keys.

### Temporary audit XQVM pin

This is a local integration draft, not an admission-restoration release.
The upstream verifier revision still needs panel review. QUI-2353 must
recalibrate verification weights on the reference machine before program
admission can be restored; sequential loop count matters independently of
nesting depth. Release/runtime-version selection remains a separate step.

This branch pins xquad v0.5.0 plus the audit fixes at commit
`85aefe5f5ff877f83c74ddfa127554f63616e929`. That commit declares `xqvm 0.5.0`,
but differs from the published crates.io `xqvm 0.5.0` at `a99f7d2e`.
The registry release does not contain these audit fixes. Both execution
pallets and both runtime test dependencies inherit the same workspace git
dependency; no member may independently select the registry package with
that version. The lockfile and resolved graph must contain exactly one XQVM
package, from the pinned git source, to prevent QUIP-19/O8 version drift.

This pin is temporary. Replace it only with a published release containing
these fixes, after verifying its source identity. Releasing xquad is a
separate operation.

The dependency fixes QUIP-02/16/17 and O3/O7; its newer instruction and fault
handling also closes the version drift behind QUIP-19/O8. Signed
`store_program` regressions cover the all-register cycle, unreachable reads and
transient stack peaks, including the 8192/8193 boundary. The two-second cycle
watchdog catches nontermination; it is not a benchmark or a weight estimate.
The verifier's maximum program-size/block-count paths need a reference-machine
weight pass before release; dependency-only changes do not trigger the pallet
source staleness warning.

The on-chain xqvm conformance suite resolves the locked Cargo package graph.
A draft git dependency fetches its exact commit; a registry release fetches
its version tag and verifies it against the published crate's
`.cargo_vcs_info.json`. A future published release containing the audit fixes
therefore requires no stale hard-coded commit to be carried forward. The
existing registry `0.5.0` is not a replacement for this pin. The `XQUAD_DIR`
override must match the same resolved source identity.

The on-chain replay uses the pinned `xqasm` library and `xqvm/tests/vectors`.
It compares every output slot in the pallet event format (unset slots become
zero), and checks memory rejection against the same native VM at the chain's
actual allocation limit. Vectors with unset calldata slots are reported as
outside the pallet's integer-only calldata API; they are not counted as
on-chain passes. Native conformance remains responsible for those host-only
inputs. This distinction also avoids treating upstream vectors with a 64 GiB
allocation budget as successful executions under the chain's 16 MiB bound.

### Audit SDK commission guard (QUIP-12)

SDK revision `3e74664a56c75e56ece2b894b989717d2ecd9a46` adds an increase-only commission guard for asynchronous election
providers. Active and ready rounds retain their snapshot commission; equal or
lower commission remains valid. Synchronous, zero-duration providers are
unchanged. During an active round an account with no current `Validators`
entry may only register at zero commission, subject to `MinCommission`.
Nonzero registration is accepted between rounds. This deliberately covers
re-entry after `chill` or `nominate`, including atomic batches, without changing
those exit calls or adding a per-round preference snapshot.

`CommissionChangeDuringElection` is appended at error index 34. The metadata
gate checks this exact addition on both sides of the immutable baseline;
previous errors, calls, storage and signed extensions are unchanged. Runtime
reward regressions cover direct raises and both batched re-entry paths, and
prove that a chilled nominator still receives its snapshotted reward share.

The additional phase read, 3,500 proof bytes and 5,000,000 picoseconds registered
for `validate` are conservative, unmeasured allowances. The SDK retains the
new-validator benchmark and adds `validate_commission_increase`; both require
a reference-machine pass before release. The manually registered allowance
is not detected by generated-weight staleness checks.

### Audit BABE randomness fix (QUIP-31 F1)

SDK revision `8fcc7e2d686806c0f104e768dd5c17f743704754` consumes the actively
filling randomness segment at epoch rotation, including an exactly full
256-output segment. Every segment is removed after consumption, so partial
outputs cannot carry over. All SDK requirements use this revision and the
committed lockfile contains one SDK source.

The fork includes separate test-harness and equivocation-fixture repairs. Five
regressions fail on the original `d511a17` production code and pass with F1:
one output, two outputs, 256 outputs, 257 outputs, and consecutive epochs.
There is no storage-layout, client or genesis change.

Runtime 120 also rejects session `set_keys` and `purge_keys` in the last
64 BABE slots of each epoch: 384 seconds, or 64/2400 (about 2.67%) of a
mainnet/dev epoch and 64/600 (about 10.67%) of a testnet epoch. The window
starts exactly at `epoch_start + epoch_duration - 64` and ends when the
next epoch begins. This reduces the final-primary-author key-selection
residual. Under the independent-slot model, let `α` be the honest-and-online
share of BABE weight and `c_h = 1 - (1 - c)^α` the probability of an honest,
online primary in a slot. The probability of a 64-slot tail with no such
primary is `(1 - c_h)^64 = (1 - c)^(64α)`. At the genesis configuration
`c = 1/4`, the approximately `1e-8` figure assumes **all validators are honest
and online** (`α = 1`):

| Honest-and-online share `α` | 1 | 4/5 | 2/3 | 3/5 | 1/2 |
|---|---:|---:|---:|---:|---:|
| Per-epoch tail probability | 1.0e-8 | 4.0e-7 | 4.7e-6 | 1.6e-5 | 1.0e-4 |

This is a probabilistic bound, not elimination of the residual. It does not
cover a coalition publishing or withholding its own tail primaries, whose
VRF outputs it knows in advance; that grinding opportunity is intrinsic to
BABE. Offline honest validators, network partitions or denial of service
against authors near epoch end reduce the honest-and-online share.
Equivocation adds no randomness. QUI-2352 tracks the separate deterministic
fix, a one-session key commitment delay, outside runtime 120. The residual
risk requires an explicit release decision until that fix lands.

Compile-time assertions pin the **genesis configuration**, not the live
configuration: raw Root can still call `plan_config_change`. Changing `c`
or the slot mode requires reviewing the cutoff. The council dispatcher rejects `Babe::plan_config_change`
at every epoch offset, including requests that repeat the current configuration.
Changing the primary probability or slot mode therefore requires a governed
runtime code upgrade that reviews this bound; the SDK Root-only API itself
remains present.

Operators see `system.CallFiltered` for either key call during the window;
retry after the next epoch boundary. Transaction-pool validation does not
apply `BaseCallFilter`, so a key call included during the window fails with
`CallFiltered` and still pays its transaction fee. Wrapped calls report that inner error
through their usual result/event (for example `utility.BatchInterrupted`
or `proxy.ProxyExecuted`; Revive emits `EthExtrinsicRevert` while its outer
extrinsic succeeds). Genesis session keys are installed normally,
and block 1 at epoch offset zero accepts key changes. No repository
onboarding script submits `set_keys` automatically, so no retry loop changes
are required here. This statement covers this repository only;
`quip-node-manager` has not been verified.

Raw Root dispatch bypasses the base call filter, but validators cannot
obtain Root on their own. The council's two-thirds `dispatch_as_root` path
also checks the window recursively, including utility calls that change
the dispatch origin, and rejects a blocked motion with `RootCallDenied`.
Governance can still replace runtime code through its existing upgrade
powers. The cutoff changes no storage layout, signed extensions, or call
indices; the unshipped runtime remains spec 120 / transaction version 7.

Session call weights add a conservative allowance for the filter's five
storage reads, 8,000 proof bytes and 5,000,000 picoseconds of arithmetic.
The council filter reads the window once per call tree and charges the same
allowance once on every governed Root dispatch; BABE storage reads occur
only if the tree contains a matching key call. These allowances are
**UNMEASURED**, tracked by QUI-2353, and require reference-machine weight
validation before release; no generated weights were replaced.

The pinned SDK is a linear stack: the four F1 commits are replayed on the
reviewed QUIP-12 commission guard at `3e74664a`. Both fixes are present in this
one revision. The validator pin includes the reviewed commission metadata
gates and economic regressions. The original QUIP-12 and isolated F1 branches
remain available at their reviewed heads.

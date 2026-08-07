# Substrate Node Template

A fresh [Substrate](https://substrate.io/) node, ready for hacking :rocket:

A standalone version of this template is available for each release of Polkadot
in the [Substrate Developer Hub Parachain
Template](https://github.com/substrate-developer-hub/substrate-node-template/)
repository. The parachain template is generated directly at each Polkadot
release branch from the [Solochain Template in
Substrate](https://github.com/paritytech/polkadot-sdk/tree/master/templates/solochain)
upstream

It is usually best to use the stand-alone version to start a new project. All
bugs, suggestions, and feature requests should be made upstream in the
[Substrate](https://github.com/paritytech/polkadot-sdk/tree/master/substrate)
repository.

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

The following command starts a single-node development chain that doesn't
persist state:

```sh
./target/release/solochain-template-node --dev
```

To purge the development chain's state, run the following command:

```sh
./target/release/solochain-template-node purge-chain --dev
```

To start the development chain with detailed logging, run the following command:

```sh
RUST_BACKTRACE=1 ./target/release/solochain-template-node -ldebug --dev
```

Development chains:

- Maintain state in a `tmp` folder while the node is running.
- Use the **Alice** and **Bob** accounts as default validator authorities.
- Use the **Alice** account as the default `sudo` account.
- Are preconfigured with a genesis state (`/node/src/chain_spec.rs`) that
  includes several pre-funded development accounts.


To persist chain state between runs, specify a base path by running a command
similar to the following:

```sh
// Create a folder to use as the db base path
$ mkdir my-chain-state

// Use of that folder to store the chain state
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
require custom types for Quip anymore; for usage notes, see
[docs/polkadotjs/README.md](/Users/romanuseinov/projects/quip/quip-protocol-rs/docs/polkadotjs/README.md).

### Multi-Node Local Testnet

A scripted three-validator local network is available two ways:

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
docker pull registry.gitlab.com/quip.network/quip-protocol-rs/quip-network-node:latest
```

Tag scheme:

- `:latest` — tip of `main`. Floating, advances on every merge.
- `:sha-<short>` — pinned to a specific commit on `main` or to a tagged release.
- `:<git-tag>` — pinned to a release tag (e.g. `:v0.1.0`).

#### Run as a validator

```sh
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
docker run --rm -v quip-data:/data -p 9944:9944 -p 30333:30333 \
  quip-network-node:local \
  --chain=local3 --base-path=/data \
  --bootnodes=/dns/<bootnode-host>/tcp/30333/p2p/<peer-id> \
  --rpc-external --rpc-cors=all
```

#### Local 3-node network via docker-compose

`docker-compose.yml` reproduces `scripts/start-local3.sh` in containers:

```sh
docker compose up --build           # start
docker compose down                 # stop, keep chain state
docker compose down -v              # stop and wipe state
```

Then connect Polkadot.js Apps to `ws://localhost:9944` (node1),
`ws://localhost:9945` (node2), or `ws://localhost:9946` (node3).

## Public testnet

`quip-testnet` is the public testnet ("AGLS" tokens, 12 decimals). The
canonical genesis is baked into the `v0.2.0+` binary as the `quip-testnet`
chain spec preset and also published as a raw JSON file at
`nodes.quip.network/chain-specs/quip-testnet.json`.

### Quickstart (Docker)

```sh
# Pull the matching release image
docker pull registry.gitlab.com/quip.network/quip-protocol-rs/quip-network-node:v0.2.0

# Join the testnet as a full node (no validator key required)
docker run --rm -v quip-data:/data -p 9944:9944 -p 30333:30333 \
  registry.gitlab.com/quip.network/quip-protocol-rs/quip-network-node:v0.2.0 \
  --chain=quip-testnet --base-path=/data \
  --name="my-quip-node"
```

The three canonical bootnodes (`bootnode-{1,2,3}.testnet.quip.network`) are
embedded in the chain spec, so peer discovery happens automatically.

### Using the hosted raw chain spec

Alternatively, fetch the published JSON spec from `nodes.quip.network` and
pass its path to `--chain`:

```sh
curl -fsSL https://gitlab.com/quip.network/nodes.quip.network/-/raw/main/chain-specs/quip-testnet.json \
    -o quip-testnet.json

docker run --rm -v "$PWD:/spec" -v quip-data:/data -p 9944:9944 -p 30333:30333 \
  registry.gitlab.com/quip.network/quip-protocol-rs/quip-network-node:v0.2.0 \
  --chain=/spec/quip-testnet.json --base-path=/data
```

### Running a validator

Operator validator slots are committed at genesis (see
[`docs/genesis-quip-testnet.md`](docs/genesis-quip-testnet.md)). To rotate or
add a slot, follow [`docs/testnet-keys.md`](docs/testnet-keys.md) and the
`scripts/derive-operator-keys.sh` helper.

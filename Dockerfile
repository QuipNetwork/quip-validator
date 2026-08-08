# Pin Rust deliberately: the runtime↔host ABI (sp-io ext_* host functions) is
# toolchain-sensitive. Rust 1.96.0 regressed the wasm32v1-none runtime link
# ("undefined symbol: ext_*"). Keep this in lockstep with the CI toolchain
# image (.gitlab/ci-toolchain.Dockerfile); bump both together after verifying.
ARG RUST_VERSION=1.95.0
ARG DEBIAN_VERSION=bookworm

FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS builder
# Substrate build deps; pinning system lib versions across Debian point
# releases is brittle and this is a build-only stage.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang libclang-dev protobuf-compiler pkg-config libssl-dev cmake \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32v1-none \
 && rustup component add rust-src
# Rewrite ssh://git@gitlab.com/ → https://gitlab.com/ so cargo can fetch public
# deps (e.g. quip.network/xq-rs) without an SSH key in the build context.
# Cargo's lock keeps the original ssh:// source identity, so rev pins still
# match. CARGO_NET_GIT_FETCH_WITH_CLI is required for git's url.insteadOf to
# take effect (cargo's libgit2 backend ignores it). The CARGO_HTTP_* settings
# harden crate downloads against the runner VM's flaky network: cargo's HTTP/2
# multiplexing stalls on crates.io from VMs/CI ("[28] Timeout was reached /
# failed to transfer more than 10 bytes in 30s") — force HTTP/1.1 and retry.
RUN git config --global url."https://gitlab.com/".insteadOf "ssh://git@gitlab.com/"
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true \
    CARGO_NET_RETRY=10 \
    CARGO_HTTP_MULTIPLEXING=false
WORKDIR /build
COPY . .

# `substrate-build-script-utils` embeds the commit hash into
# `SUBSTRATE_CLI_IMPL_VERSION` (what `--version` prints). It checks the
# `SUBSTRATE_CLI_GIT_COMMIT_HASH` env var first and only falls back to `git
# rev-parse` if empty. The build context excludes `.git/` (see `.dockerignore`),
# so without the build-arg the binary ends up tagged `0.2.x-unknown`. CI passes
# `$CI_COMMIT_SHORT_SHA`; local `docker build` users can pass
# `--build-arg SUBSTRATE_CLI_GIT_COMMIT_HASH=$(git rev-parse --short=11 HEAD)`.
ARG SUBSTRATE_CLI_GIT_COMMIT_HASH=""
ENV SUBSTRATE_CLI_GIT_COMMIT_HASH=${SUBSTRATE_CLI_GIT_COMMIT_HASH}

# Local Compose stacks set this to `dev-chain-id`; release images leave it
# empty and retain the public-testnet Chain ID. Keep the feature choice in the
# image build because pallet-revive's EIP-155 ChainId is a runtime constant.
ARG NODE_BUILD_FEATURES=""
RUN if [ -n "$NODE_BUILD_FEATURES" ]; then \
        cargo build --release -p quip-network-node --features "$NODE_BUILD_FEATURES"; \
    else \
        cargo build --release -p quip-network-node; \
    fi \
 && cp target/release/quip-network-node /usr/local/bin/quip-network-node

FROM debian:${DEBIAN_VERSION}-slim AS runtime
# runtime deps tracked with the Debian base; pinning point versions here adds
# churn without a security benefit. uid 1000 matches the PUID/PGID default;
# entrypoint.sh remaps the quip user at runtime when PUID/PGID are set.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates libssl3 gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --home-dir /data --uid 1000 --shell /bin/false quip
# db_version self-heal target for entrypoint.sh. Must equal sc-client-db's
# CURRENT_VERSION (substrate/client/db/src/upgrade.rs); bump in lockstep when a
# database schema migration lands in the pinned polkadot-sdk.
ENV QUIP_DB_VERSION=4
COPY --from=builder /usr/local/bin/quip-network-node /usr/local/bin/quip-network-node
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
WORKDIR /data
EXPOSE 30333 9944 9615
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]

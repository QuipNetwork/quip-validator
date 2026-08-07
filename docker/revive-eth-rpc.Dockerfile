# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.95.0
ARG DEBIAN_VERSION=bookworm

FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS builder

# Build from the same SDK revision used by Quip. pallet-revive-eth-rpc's build
# script depends on revive-dev-runtime through the SDK workspace, so building
# from a complete pinned checkout is required until the package is embeddable.
ARG POLKADOT_SDK_REPOSITORY=https://github.com/QuipNetwork/polkadot-sdk.git
ARG POLKADOT_SDK_REV=f17113ffe89361a26cb286f6afc5a92f443179a8

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang cmake git libclang-dev libssl-dev pkg-config protobuf-compiler \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add wasm32v1-none \
    && rustup component add rust-src

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true \
    CARGO_NET_RETRY=10 \
    CARGO_HTTP_MULTIPLEXING=false

WORKDIR /polkadot-sdk
RUN git init . \
 && git remote add origin "$POLKADOT_SDK_REPOSITORY" \
 && git fetch --depth=1 origin "$POLKADOT_SDK_REV" \
 && git checkout --detach FETCH_HEAD \
 && test "$(git rev-parse HEAD)" = "$POLKADOT_SDK_REV"

RUN cargo build --locked --release -p pallet-revive-eth-rpc --bin eth-rpc

FROM debian:${DEBIAN_VERSION}-slim AS runtime

# curl is used only by the container healthcheck; SQLite stores the receipt
# index when archive mode is selected.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl libsqlite3-0 libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1001 --shell /bin/false eth-rpc

COPY --from=builder /polkadot-sdk/target/release/eth-rpc /usr/local/bin/eth-rpc

USER eth-rpc
EXPOSE 8545 9616
ENTRYPOINT ["/usr/local/bin/eth-rpc"]
CMD ["--help"]

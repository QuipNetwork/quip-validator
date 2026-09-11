# syntax=docker/dockerfile:1.7
#
# CI toolchain image: rust:1.95.0-bookworm + the system deps needed to build the
# quip runtime (clang/libclang for bindgen, protobuf-compiler, libssl, cmake)
# plus the wasm32v1-none target and rust-src component required by
# substrate-wasm-builder.
#
# CI builds and publishes this image to the project registry as
# $CI_REGISTRY_IMAGE/ci-toolchain, from the ci-image-toolchain-* jobs in
# `.gitlab-ci.yml`. It rebuilds only when a CI Dockerfile changes, in a stage
# ahead of everything that pulls it. Nothing is pushed by hand.
#
# It must stay multi-arch: the faucet repo's per-arch build-binary jobs pull it
# on both amd64 and arm64 runners. Each arch is built on a native runner and
# combined by the ci-image-toolchain manifest job, so no QEMU emulation is
# involved. The faucet repo needs pull access to this project's registry — a
# CI_JOB_TOKEN allowlist entry or a group deploy token — because these images
# are no longer on a public Docker Hub namespace.
#
# `make builder-image` still builds it locally for testing a change before
# pushing the branch. That target does not publish; CI owns publishing.

# Pin Rust in lockstep with the production Dockerfile. Rust 1.96.0 regressed
# the wasm32v1-none runtime link ("undefined symbol: ext_*"); 1.95.0 is the
# last known-good. Bump both Dockerfiles together after verifying a build.
ARG RUST_VERSION=1.95.0
ARG DEBIAN_VERSION=bookworm

FROM rust:${RUST_VERSION}-${DEBIAN_VERSION}

# jq is not a build dep — it is baked in for the bench-stage jobs that parse
# benchmark JSON (run-quantum-pow-sweeps.sh). Installing it in a job's
# before_script put a Debian-mirror fetch and an unpinned package version
# inside a timing measurement on the serialized reference machine; having it
# in the image removes both. (QUI-948)
#
# python3-venv and python3-dev are baked for the same reason. The base image
# already ships python3 and the venv module, but Debian splits ensurepip out
# into python3-venv, so without it `python3 -m venv` produces an environment
# with no pip; python3-dev supplies the Python.h the PyO3 extension compiles
# against. Both were apt-installed in before_script until now, and naming the
# already-present python3 on those lines made apt upgrade the whole
# interpreter stack to the current point release. Under runner contention that
# upgrade spent 16 minutes unpacking one package and hit the 30m job timeout,
# failing py-signer-test and release:build-pypi on a commit that passed on an
# idle runner in 45 seconds.
#
# nodejs and npm serve browser-signer-test, which builds and runs the
# TypeScript signer against a wasm bundle. These are Debian's 18.20.4 / 9.2.0,
# the exact versions that job's before_script installed, so baking them changes
# no behaviour. package.json declares no engines floor.
#
# ca-certificates and curl are named explicitly even though the base image
# already carries them. Several jobs curl checksum-pinned release binaries
# (solc, resolc, cargo-binstall) and a TLS failure there is a confusing way to
# discover an implicit dependency. Naming them keeps the image honest about
# what it guarantees, and costs nothing when they are already satisfied.
# Pinning system lib versions across Debian point releases is brittle and this
# image is build tooling only.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang libclang-dev protobuf-compiler pkg-config libssl-dev cmake jq \
        python3-venv python3-dev \
        nodejs npm \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32v1-none \
 && rustup component add rust-src clippy rustfmt

# cargo-sweep prunes the host cache volume that .cargo-host-cache mounts at
# /ci-cache (scripts/prune-ci-cache.sh, docs/ci-cache.md). It is baked here for
# the same reason as jq above: the prune runs in an after_script on the heavy
# Rust jobs, and a per-job `cargo install` would put a crates.io fetch and a
# source build inside their timeout. Compiling it once per image build costs
# minutes that the image build already has and the jobs do not.
ARG CARGO_SWEEP_VERSION=0.8.0
RUN cargo install cargo-sweep --locked --version "${CARGO_SWEEP_VERSION}"

# Cargo prefers the git CLI for fetching from GitLab; this matches the
# behavior of the production Dockerfile so cargo can resolve ssh:// deps
# without an in-image SSH key.
RUN git config --global url."https://gitlab.com/".insteadOf "ssh://git@gitlab.com/"
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

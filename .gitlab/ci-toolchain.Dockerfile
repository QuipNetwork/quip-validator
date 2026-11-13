# syntax=docker/dockerfile:1.7
#
# CI toolchain image: rust:1.95.0-bookworm + the system deps needed to build the
# quip runtime (clang/libclang for bindgen, protobuf-compiler, libssl, cmake)
# plus the wasm32v1-none target and rust-src component required by
# substrate-wasm-builder, and every pinned helper the jobs run (cargo-sweep,
# solc, resolc, wasm-pack, cargo-zigbuild, maturin, twine, ziglang).
#
# A tool a job needs belongs here, not in its before_script. Every install
# that ran per job put a network fetch inside a job timeout, on every merge
# request pipeline, and on the reference machine inside a timing measurement.
#
# CI builds and publishes this image to the project registry as
# $CI_REGISTRY_IMAGE/ci-toolchain, from the ci-image-toolchain-* jobs in
# `.gitlab-ci.yml`. It rebuilds when a CI Dockerfile changes, in a stage ahead
# of everything that pulls it, and by hand from a main pipeline when the base
# image or a runner's cached copy needs a refresh. Nothing is pushed from a
# workstation.
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

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

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
# failing py-signer-test and release:build-pypi-linux on a commit that passed
# on an idle runner in 45 seconds.
#
# nodejs and npm serve browser-signer-test, which builds and runs the
# TypeScript signer against a wasm bundle. These are Debian's 18.20.4 / 9.2.0,
# the exact versions that job's before_script installed, so baking them changes
# no behaviour. package.json declares no engines floor.
#
# ca-certificates and curl are named explicitly even though the base image
# already carries them. The solc/resolc and cargo-binstall layers below fetch
# release binaries at image-build time; a TLS failure there is a confusing way
# to discover an implicit dependency. Naming them keeps the image honest about
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

# wasm32v1-none is the runtime target. wasm32-unknown-unknown is the browser
# signer bundle that browser-signer-test builds with wasm-pack, and
# aarch64-unknown-linux-gnu is the cross-built Linux wheel in
# release:build-pypi-linux. The two extra targets were a `rustup target add`
# in each job, which downloaded rust-std on every run.
RUN rustup target add wasm32v1-none wasm32-unknown-unknown aarch64-unknown-linux-gnu \
 && rustup component add rust-src clippy rustfmt

# cargo-sweep prunes the host cache volume that .cargo-host-cache mounts at
# /ci-cache (scripts/prune-ci-cache.sh). It is baked here for
# the same reason as jq above: the prune runs in an after_script on the heavy
# Rust jobs, and a per-job `cargo install` would put a crates.io fetch and a
# source build inside their timeout. Compiling it once per image build costs
# minutes that the image build already has and the jobs do not.
ARG CARGO_SWEEP_VERSION=0.8.0
RUN cargo install cargo-sweep --locked --version "${CARGO_SWEEP_VERSION}"

# wasm-pack (browser-signer-test) and cargo-zigbuild
# (release:build-pypi-linux) come as prebuilt release binaries through
# cargo-binstall, into $CARGO_HOME/bin of the image, which stays on PATH when
# a job points CARGO_HOME at its cache. Both were binstalled in the jobs'
# before_script, which put two GitHub release downloads in every merge request
# pipeline. cargo-binstall itself is a version-pinned release tarball, not the
# `install.sh | bash` bootstrap, so the toolchain that builds a published
# wheel cannot be altered by an upstream `main` change. Bump cargo-zigbuild
# deliberately: it changes the linker the release wheels are built with. The
# Makefile's wasm-signer target checks WASM_PACK_VERSION; keep the two pins
# equal.
ARG CARGO_BINSTALL_VERSION=1.20.1
ARG WASM_PACK_VERSION=0.12.1
ARG CARGO_ZIGBUILD_VERSION=0.22.3
RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) triple=x86_64-unknown-linux-musl ;; \
      arm64) triple=aarch64-unknown-linux-musl ;; \
      *) echo "cargo-binstall: unexpected architecture" >&2; exit 1 ;; \
    esac; \
    curl -Lsf --show-error --retry 5 --retry-all-errors --proto '=https' --tlsv1.2 \
        "https://github.com/cargo-bins/cargo-binstall/releases/download/v${CARGO_BINSTALL_VERSION}/cargo-binstall-${triple}.tgz" \
      | tar -xz -C "${CARGO_HOME}/bin"; \
    cargo binstall --no-confirm --locked \
        "wasm-pack@${WASM_PACK_VERSION}" "cargo-zigbuild@${CARGO_ZIGBUILD_VERSION}"

# maturin, twine and ziglang serve release:build-pypi-linux. They live in a
# virtual environment that the job puts first on PATH, so the `python3` that
# cargo-zigbuild shells out to (`python3 -m ziglang`, how it locates zig) is
# the one with ziglang. The image does not put it on PATH itself:
# py-signer-test builds its own venv from the system interpreter and must keep
# doing so. maturin's manylinux policy tables and zig's bundled glibc stubs
# decide the wheels' glibc floor, and both change between releases. Bump
# deliberately, and re-check the wheel tags.
ARG MATURIN_VERSION=1.16.0
ARG TWINE_VERSION=7.0.0
ARG ZIGLANG_VERSION=0.17.1
RUN python3 -m venv /opt/pyrelease \
 && /opt/pyrelease/bin/pip install --no-cache-dir --quiet \
        "maturin==${MATURIN_VERSION}" "twine==${TWINE_VERSION}" "ziglang==${ZIGLANG_VERSION}"

# solc and resolc serve pallet-revive's runtime benchmarks, which compile
# Solidity fixtures to EVM (solc) and to PolkaVM (resolc). Keep these versions
# aligned with the pinned SDK's get-solc/get-resolc actions. Until QUI-2215 the
# bench jobs curl'd both from GitHub releases in a before_script, so a GitHub
# release outage (504s on 2026-11-09) blocked weight regeneration and cost the
# job its place in the reference machine's queue. Fetching them here moves that
# dependency to image-build time, which only runs when this file changes.
#
# Both binaries are checksum-pinned: RESOLC_SHA256 comes from the release's
# official checksums.txt; solc publishes no checksum, so SOLC_SHA256 was pinned
# manually after verifying that the GitHub release artifact and
# binaries.soliditylang.org serve identical bytes.
#
# Both are x86_64-only release binaries, and every job that uses them runs on
# amd64. The arm64 image (the faucet's per-arch builds) skips them; any other
# architecture fails the build rather than ship an image without them.
ARG SOLC_VERSION=0.8.30
ARG SOLC_SHA256=f3e987dc6ecebd4bd350c48edcbc320b46cf9e3109bd3fc3d88f1acaf4c428f7
ARG RESOLC_VERSION=1.0.0
ARG RESOLC_SHA256=e626ba7a1a0d26828c14713781b0082ececc9a0fbfe984f11f0d93d66bbd7806
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) \
        curl -Lsf --show-error --retry 5 --retry-all-errors \
            -o /usr/local/bin/solc \
            "https://github.com/ethereum/solidity/releases/download/v${SOLC_VERSION}/solc-static-linux"; \
        curl -Lsf --show-error --retry 5 --retry-all-errors \
            -o /usr/local/bin/resolc \
            "https://github.com/paritytech/revive/releases/download/v${RESOLC_VERSION}/resolc-x86_64-unknown-linux-musl"; \
        echo "${SOLC_SHA256}  /usr/local/bin/solc" | sha256sum -c -; \
        echo "${RESOLC_SHA256}  /usr/local/bin/resolc" | sha256sum -c -; \
        chmod 0755 /usr/local/bin/solc /usr/local/bin/resolc; \
        solc --version; \
        resolc --version ;; \
      arm64) \
        echo "solc/resolc: x86_64-only release binaries, not installed on arm64" ;; \
      *) \
        echo "solc/resolc: unexpected architecture '$arch'" >&2; \
        exit 1 ;; \
    esac

# Cargo prefers the git CLI for fetching from GitLab; this matches the
# behavior of the production Dockerfile so cargo can resolve ssh:// deps
# without an in-image SSH key.
RUN git config --global url."https://gitlab.com/".insteadOf "ssh://git@gitlab.com/"
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

# Fail the image build here, not in a CI job. solc and resolc are amd64-only
# (see above).
RUN cargo --version && cargo clippy --version && cargo fmt --version \
 && rustup target list --installed | grep -qx wasm32v1-none \
 && rustup target list --installed | grep -qx wasm32-unknown-unknown \
 && rustup target list --installed | grep -qx aarch64-unknown-linux-gnu \
 && cargo sweep --version && wasm-pack --version && command -v cargo-zigbuild \
 && /opt/pyrelease/bin/maturin --version && /opt/pyrelease/bin/twine --version \
 && /opt/pyrelease/bin/python -m ziglang version \
 && node --version && npm --version && jq --version && protoc --version \
 && clang --version | head -1 \
 && { [ "$(dpkg --print-architecture)" != amd64 ] || { solc --version && resolc --version; }; }

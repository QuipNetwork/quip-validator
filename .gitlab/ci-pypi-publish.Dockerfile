# syntax=docker/dockerfile:1.7
#
# CI image for the quip-signer PyPI publish jobs: python:3.12-slim-bookworm
# plus twine and the TLS tooling the upload needs.
#
# This is deliberately NOT the Substrate TOOLCHAIN_IMAGE. The publish jobs
# compile nothing — they consume the dist/ artefact that release:build-pypi
# already produced and upload those exact bytes — so they want a small image
# with no Rust toolchain and no crate source anywhere on the path.
#
# `.gitlab-ci.yml` pulls this from docker.io/carback1/quip-pypi-publish by
# digest. Rebuild and push manually from a workstation when this Dockerfile
# changes, the same flow as the toolchain image: there are no Docker Hub
# credentials in the project or group CI variables. `make pypi-publish-image`
# wraps it. Only linux/amd64 is built — .pypi-publish pins tags: [docker,
# amd64], and the smoke jobs that do run on arm64 use the stock python image
# because they install a wheel rather than publish one.

ARG PYTHON_VERSION=3.12
ARG DEBIAN_VERSION=bookworm

FROM python:${PYTHON_VERSION}-slim-${DEBIAN_VERSION}

# curl and ca-certificates were installed in before_script until now. The
# upload itself is TLS to PyPI and the OIDC exchange that precedes it, so a
# missing trust store fails the job at the worst possible moment — after a tag
# is cut and the wheels are built. Baking them removes a Debian-mirror fetch
# from the job budget, the same failure class that timed out py-signer-test
# and release:build-pypi on 2026-09-03.
# Pinning system lib versions across Debian point releases is brittle and this
# image is CI tooling only.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# twine is pinned to a floor rather than an exact version to match what the job
# asked for before (`twine>=6.1`). Bump deliberately: this is the client that
# uploads released artefacts.
# hadolint ignore=DL3013
RUN pip install --no-cache-dir --upgrade 'twine>=6.1'

# Fail the build rather than the release if either tool is missing.
RUN twine --version && curl --version

#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build, smoke-test, or publish the `quip-signer` Python distribution. Single
# source of truth for the build/smoke/upload commands so the CI jobs that touch
# them don't drift -- changing the artefact matrix, the smoke check, or the
# upload flow is a one-place edit here.
#
# quip-signer is a single PyO3 abi3 cdylib (mixed maturin layout: the Rust
# extension is dropped into the `quip_signer` package under the crate's
# in-tree `python/` python-source).
# There are no pure-Python peer packages and the crate itself is
# `publish = false`, so -- unlike xquad's multi-package python-dists.sh -- this
# handles exactly one distribution and mints exactly one OIDC token.
#
# The three modes map one-to-one onto separate CI jobs. They are split (rather
# than one build-and-publish job) so the cross-built aarch64 wheel is import-
# tested on a NATIVE arm64 runner before any upload: a wheel only imports on its
# own architecture, so a single amd64 job physically cannot exercise both. The
# build job produces dist/ once; two smoke jobs (amd64 + aarch64) each import
# their arch's wheel; publish uploads the exact bytes that passed both smokes.
#
# Modes:
#   build
#       maturin build (abi3 wheels: linux x86_64 + linux aarch64, both via
#       zig at manylinux2014) + maturin sdist, then the metadata/content guards
#       (manylinux platform tag, packed python surface, `twine check`). No
#       import, no network, no token. dist/ is the CI artefact the smoke and
#       publish jobs consume. Runs on every MR/push (dry-run) and before a
#       tagged publish.
#
#   smoke
#       Install the one wheel matching THIS runner's platform tag into a
#       throwaway venv and import + round-trip sign/verify. Run once per arch
#       (amd64 + aarch64 runners) so every shipped wheel -- including the cross-
#       built aarch64 one -- is exercised on its own architecture before upload.
#       Consumes dist/ from the build job; builds nothing.
#
#   publish <testpypi|pypi>
#       Exchange the GitLab OIDC JWT in PYPI_ID_TOKEN for a short-lived API
#       token at the target registry's mint endpoint and `twine upload` the
#       prebuilt dist/. Requires PYPI_ID_TOKEN in env (set by the job's
#       `id_tokens` block in .gitlab-ci.yml). Gated in CI on both smoke jobs
#       passing. Used by release:publish-testpypi (auto) and release:publish-pypi
#       (manual) on release tags.
#
# Run from the workspace root.

set -euo pipefail

# Absolute path to this script's dir so we can invoke sibling scripts (the smoke
# test) regardless of CWD -- smoke_dists cd's into the venv before running it.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MANIFEST="crates/transaction-crypto-py/Cargo.toml"
DIST="dist"

usage() {
	echo "usage: $0 {build | smoke | publish <testpypi|pypi>}" >&2
	exit 2
}

# --- build -----------------------------------------------------------------
build_dists() {
	# Same matrix as xquad's xqffi cdylib:
	#   1. abi3 wheel, linux x86_64  (zig links against glibc 2.17 stubs)
	#   2. abi3 wheel, linux aarch64 (cross-compiled; zig is the linker)
	#   3. sdist                     (universal source fallback for macOS/Windows)
	# The abi3 tag (cp39-abi3-...) comes from pyo3's `abi3-py39` feature, so one
	# wheel per arch serves CPython >=3.9.
	rm -rf "${DIST}"
	mkdir -p "${DIST}"

	maturin build --release --manifest-path "${MANIFEST}" --out "${DIST}" \
		--target x86_64-unknown-linux-gnu --zig --compatibility manylinux2014
	maturin build --release --manifest-path "${MANIFEST}" --out "${DIST}" \
		--target aarch64-unknown-linux-gnu --zig --compatibility manylinux2014
	maturin sdist --manifest-path "${MANIFEST}" --out "${DIST}"

	# Platform-tag guard. `twine check` validates metadata only -- it does NOT
	# inspect platform tags, so a wheel tagged plain `linux_x86_64` (linked
	# against a glibc newer than any manylinux profile) passes twine check and is
	# then rejected at upload time. Assert every wheel carries a
	# manylinux/musllinux tag here so the dry-run actually fails on a
	# non-portable build instead of discovering it mid-publish.
	for whl in "${DIST}"/*.whl; do
		case "${whl}" in
		*manylinux* | *musllinux*) ;;
		*)
			echo "ERROR: ${whl##*/} has no manylinux/musllinux platform tag; PyPI will reject it." >&2
			echo "       The native build linked against too-new a glibc -- build it via cargo-zigbuild too, or lower the build image's glibc." >&2
			exit 1
			;;
		esac
	done

	# Wheel-content guard (QUI-792). A wheel is a zip; `twine check` validates
	# metadata but NEVER opens the archive, and the manylinux guard above only
	# reads the filename tag. When the python-source lived outside the crate,
	# `maturin build` packed only `_quip_signer.abi3.so` and silently dropped the
	# `quip_signer` package surface -- the wheel passed every check above, then
	# imported as an empty namespace package (no `HybridSigner`, etc.). Open each
	# wheel and assert the tracked pure-Python surface is present so a layout
	# regression fails the build HERE (both dry-run and publish) instead of
	# shipping -- or promoting to production -- an unimportable package.
	local required_members=(
		"quip_signer/__init__.py"  # re-exports the extension's public API
		"quip_signer/__init__.pyi" # type stubs
		"quip_signer/py.typed"     # PEP 561 typing marker
	)
	for whl in "${DIST}"/*.whl; do
		local names
		names="$(python3 -c 'import sys, zipfile; print("\n".join(zipfile.ZipFile(sys.argv[1]).namelist()))' "${whl}")"
		for member in "${required_members[@]}"; do
			if ! grep -qxF "${member}" <<<"${names}"; then
				echo "ERROR: ${whl##*/} is missing ${member}; the wheel would import as an empty namespace package (QUI-792)." >&2
				echo "       Check crates/transaction-crypto-py/pyproject.toml: python-source must be in-tree (\"python\"), not an out-of-tree path maturin drops from the wheel." >&2
				exit 1
			fi
		done
	done

	# Metadata validation. Cheap, arch-independent, no upload -- so it runs on the
	# shared build path and gates the publish jobs too (they consume this dist/).
	twine check "${DIST}"/*
}

# --- smoke -----------------------------------------------------------------
smoke_dists() {
	# Import smoke test (QUI-793). The content guard in `build` proves the surface
	# *files* are packed; this proves the built wheel actually *imports and runs*
	# from a clean install -- catching what a file check can't (a broken
	# extension, an ABI/symbol mismatch, a bad re-export). It installs into a
	# throwaway venv with the repo source NOT on the path, so -- unlike the pytest
	# suite, which runs against `maturin develop` (the editable source tree) -- it
	# exercises the artifact a `pip install quip-signer` user actually gets. That
	# develop-vs-wheel gap is exactly what let QUI-792 ship.
	#
	# `--no-index --only-binary=:all:` installs strictly from our built wheels --
	# never PyPI, never an sdist compile fallback that could mask a broken wheel.
	# pip resolves the one wheel matching THIS runner's platform tag, so this runs
	# once per arch: the amd64 job imports the native wheel, the arm64 job imports
	# the cross-built aarch64 wheel. Every shipped wheel is thus import-tested on
	# its own architecture before publish.
	# smoke_root is module-scoped (no `local`) on purpose: the EXIT trap fires
	# after this function has returned, so a `local` would be out of scope — and
	# under `set -u` even reading it would abort — by the time cleanup runs. Route
	# removal through a named function (not an inline `trap 'rm -rf ...' EXIT`) so
	# a future EXIT handler composes instead of silently clobbering this one; the
	# `:-` guard makes it a no-op if we exit before mktemp assigns it.
	smoke_root=""
	cleanup() {
		if [[ -n "${smoke_root:-}" ]]; then
			rm -rf "${smoke_root}"
		fi
	}
	trap cleanup EXIT

	smoke_root="$(mktemp -d)"
	python3 -m venv "${smoke_root}/venv"
	"${smoke_root}/venv/bin/pip" install --quiet --disable-pip-version-check \
		--no-index --only-binary=:all: --find-links "${DIST}" quip-signer
	# Run from the venv dir so no stray `quip_signer/` in CWD can shadow the
	# install; the smoke script is referenced by absolute path since CWD is the venv.
	(cd "${smoke_root}/venv" &&
		"${smoke_root}/venv/bin/python" "${SCRIPT_DIR}/quip_signer_wheel_smoke.py")
}

# --- publish: OIDC mint + twine upload -------------------------------------
publish_dists() {
	local target="$1"
	# TestPyPI and PyPI are independent Trusted-Publishing realms: different OIDC
	# audience (set job-side via id_tokens), different mint endpoint, different
	# upload URL, and a separate per-project publisher config. The job hands us
	# one JWT in PYPI_ID_TOKEN; we exchange it for that project's short-lived API
	# token and upload the prebuilt dist/ with it. (No multi-package JWT juggling
	# here -- one package, one token -- because PyPI's mint endpoint is only
	# single-use per JWT, which only bites monorepos publishing several projects
	# from one job.)
	local mint_url repo_url
	case "${target}" in
	testpypi)
		mint_url="https://test.pypi.org/_/oidc/mint-token"
		repo_url="https://test.pypi.org/legacy/"
		;;
	pypi)
		mint_url="https://pypi.org/_/oidc/mint-token"
		repo_url="https://upload.pypi.org/legacy/"
		;;
	esac

	: "${PYPI_ID_TOKEN:?PYPI_ID_TOKEN is required for publish mode (OIDC trusted publishing -- set by the GitLab id_tokens block in .gitlab-ci.yml)}"

	# Build the request body with python3 rather than string interpolation so a
	# JWT containing characters significant to the shell or JSON can't break the
	# body (none expected -- JWTs are base64url -- but belt-and-suspenders).
	local body
	body="$(python3 -c 'import json,sys; print(json.dumps({"token": sys.argv[1]}))' "${PYPI_ID_TOKEN}")"

	# --fail-with-body: on a non-2xx, curl still prints the response body (which
	# carries the registry's error message) before exiting non-zero.
	local response
	if ! response="$(curl -sS --fail-with-body -X POST "${mint_url}" \
		-H "Content-Type: application/json" -d "${body}")"; then
		echo "Failed to mint ${target} API token: ${response}" >&2
		exit 1
	fi

	# A 200 can still carry an error payload (e.g. publisher-config mismatch) with
	# no "token" field, so treat a missing token as failure and surface the whole
	# response -- with any token value redacted -- to make the misconfig debuggable.
	local api_token
	api_token="$(printf '%s' "${response}" |
		python3 -c 'import sys,json; print(json.load(sys.stdin).get("token") or "")' 2>/dev/null || true)"

	if [[ -z "${api_token}" ]]; then
		local safe_response
		safe_response="$(printf '%s' "${response}" | sed 's/"token":"[^"]*"/"token":"[REDACTED]"/g')"
		echo "Failed to mint ${target} API token (no token in response): ${safe_response}" >&2
		exit 1
	fi

	# --skip-existing makes re-running a tag pipeline idempotent: re-uploading an
	# already-published version is a no-op rather than a hard error, which matters
	# because the pyproject version only bumps per release (a re-tag at the same
	# version must not fail the pipeline).
	TWINE_USERNAME=__token__ TWINE_PASSWORD="${api_token}" \
		twine upload --non-interactive --skip-existing --repository-url "${repo_url}" "${DIST}"/*
}

mode="${1:-}"
case "${mode}" in
build) build_dists ;;
smoke) smoke_dists ;;
publish)
	target="${2:-}"
	case "${target}" in
	testpypi | pypi) publish_dists "${target}" ;;
	*) usage ;;
	esac
	;;
*) usage ;;
esac

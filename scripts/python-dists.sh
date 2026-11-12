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
# The modes map onto separate CI jobs. They are split (rather than one
# build-and-publish job) so the cross-built aarch64 wheel is import-tested on a
# NATIVE arm64 runner before any upload: a wheel only imports on its own
# architecture, so a single amd64 job physically cannot exercise both. The Linux
# build job produces its dist/ once and two smoke jobs (amd64 + aarch64) each
# import their arch's wheel. The macOS job runs build-macos and then smoke on
# the same runner. Publish uploads the exact bytes that passed every smoke.
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
#   build-macos
#       maturin build (abi3 wheels: macOS arm64 at 11.0 + macOS x86_64 at
#       10.12) on a macOS runner, then the platform-tag and packed-surface
#       guards. Its dist/ joins the Linux one in the publish jobs.
#
#   smoke
#       Install the one wheel matching THIS interpreter's platform tag into a
#       throwaway venv and import + round-trip sign/verify. Run once per
#       platform (Linux amd64 + aarch64 runners, macOS arm64 and x86_64 under
#       Rosetta) so every shipped wheel is exercised on its own architecture
#       before upload. PYTHON overrides the interpreter (default python3).
#       Consumes dist/ from a build job; builds nothing.
#
#   publish <testpypi|pypi>
#       Exchange the GitLab OIDC JWT in PYPI_ID_TOKEN for a short-lived API
#       token at the target registry's mint endpoint and `twine upload` the
#       prebuilt dist/, after asserting it holds exactly the five release files
#       and passes `twine check`. Requires PYPI_ID_TOKEN in env (set by the
#       job's `id_tokens` block in .gitlab-ci.yml). Gated in CI on every smoke
#       passing. Used by release:publish-testpypi and release:publish-pypi on
#       release tags.
#
# Run from the workspace root.

set -euo pipefail

# Absolute path to this script's dir so we can invoke sibling scripts (the smoke
# test) regardless of CWD -- smoke_dists cd's into the venv before running it.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MANIFEST="crates/transaction-crypto-py/Cargo.toml"
DIST="dist"

usage() {
	echo "usage: $0 {build | build-macos | smoke | publish <testpypi|pypi>}" >&2
	exit 2
}

# --- build -----------------------------------------------------------------
build_dists() {
	# Same matrix as xquad's xqffi cdylib:
	#   1. abi3 wheel, linux x86_64  (zig links against glibc 2.17 stubs)
	#   2. abi3 wheel, linux aarch64 (cross-compiled; zig is the linker)
	#   3. sdist                     (source fallback; only Windows builds from it)
	# The macOS wheels come from build-macos on a macOS runner.
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
	# inspect platform tags, so a wheel tagged plain `linux_x86_64` passes it and
	# is then rejected at upload time. A bare manylinux/musllinux match is not
	# enough either: a native build is tagged with whatever glibc floor its symbols
	# happen to need (manylinux_2_34 on the CI image), so the floor drifts with the
	# image and the check still passes. Assert the exact tags instead: both wheels
	# are built with `--zig --compatibility manylinux2014`, and maturin's audit
	# fails the build if the cdylib needs a newer glibc than that. Only Linux
	# wheels are judged; other platforms (macOS, Windows) are skipped, not
	# rejected, so adding their wheels later needs no change here.
	local expected_tags=(
		"manylinux2014_x86_64.manylinux_2_17_x86_64"
		"manylinux2014_aarch64.manylinux_2_17_aarch64"
	)
	for whl in "${DIST}"/*.whl; do
		case "${whl##*/}" in
		*manylinux* | *musllinux* | *linux_*) ;;
		*) continue ;;
		esac
		local matched=""
		for tag in "${expected_tags[@]}"; do
			[[ "${whl}" == *"-${tag}.whl" ]] && matched=1
		done
		if [[ -z "${matched}" ]]; then
			echo "ERROR: ${whl##*/} does not carry one of the expected platform tags: ${expected_tags[*]}." >&2
			echo "       The glibc floor moved or the build fell back to a native link; keep --zig --compatibility manylinux2014 on every wheel build." >&2
			exit 1
		fi
	done
	for tag in "${expected_tags[@]}"; do
		if ! compgen -G "${DIST}/*-${tag}.whl" >/dev/null; then
			echo "ERROR: no wheel with platform tag ${tag} in ${DIST}/; expected: ${expected_tags[*]}." >&2
			exit 1
		fi
	done

	check_wheel_contents

	# Metadata validation. Cheap, arch-independent, no upload -- so it runs on the
	# shared build path and gates the publish jobs too (they consume this dist/).
	twine check "${DIST}"/*
}

# Wheel-content guard (QUI-792). A wheel is a zip; `twine check` validates
# metadata but NEVER opens the archive, and the platform-tag guards only read
# the filename. When the python-source lived outside the crate, `maturin build`
# packed only `_quip_signer.abi3.so` and silently dropped the `quip_signer`
# package surface -- the wheel passed every other check, then imported as an
# empty namespace package (no `HybridSigner`, etc.). Open each wheel and assert
# the tracked pure-Python surface is present so a layout regression fails the
# build HERE (both dry-run and publish) instead of shipping -- or promoting to
# production -- an unimportable package. Shared by the Linux and macOS builds.
check_wheel_contents() {
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
}

# --- build-macos -----------------------------------------------------------
build_macos_dists() {
	# The two macOS abi3 wheels, built natively on an Apple Silicon runner:
	#   1. macosx_11_0_arm64   (the first macOS release on Apple Silicon)
	#   2. macosx_10_12_x86_64 (cross-compiled; Rust's oldest supported floor)
	# MACOSX_DEPLOYMENT_TARGET sets both the linker's minimum OS and the tag
	# maturin writes, so the tag is asserted exactly below. No --zig or
	# --compatibility: those are manylinux settings. No `twine check` either:
	# the macOS runner has no twine, so publish checks these wheels' metadata
	# once the Linux and macOS dist/ artefacts are merged.
	rm -rf "${DIST}"
	mkdir -p "${DIST}"

	# Strip with the linker, not with rustc. The crate's `strip = true` makes
	# rustc post-process the dylib with its bundled rust-objcopy, and the
	# llvm-objcopy in LLVM 22.1.2-22.1.6 (rustc 1.95-1.97) writes the Mach-O
	# symbol string table 4 bytes off 8-byte alignment. dyld on macOS 27 refuses
	# to load such a binary ("mis-aligned LINKEDIT string pool"), so the wheel
	# installs and then fails on import. ld64's -x (drop local symbols) and -S
	# (drop debug info) strip the same symbols as rustc's strip and keep the table
	# aligned, with no objcopy pass at all. Turning stripping off instead would
	# also load, but ships ~250 KB of symbols per wheel and only hides the bug;
	# a separate Rust pin for macOS would split the toolchain that is held at
	# 1.95.0 for the wasm runtime. The Linux wheels are ELF, which objcopy
	# writes through a separate backend, and they pass their own smoke.
	# check_macho_alignment below fails the build if a misaligned binary ever
	# comes back, whatever the runner's macOS: an older dyld may load it, so the
	# smoke alone would pass and ship a wheel that breaks on macOS 27.
	# shortcut: tied to the buggy rust-objcopy; once RUST_VERSION in
	# .gitlab/ci-toolchain.Dockerfile reaches 1.98 (LLVM 22.1.8, which fixes it),
	# drop these two lines and let check_macho_alignment confirm the default strip.
	local -x CARGO_PROFILE_RELEASE_STRIP=false
	local -x RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-x -C link-arg=-Wl,-S"

	MACOSX_DEPLOYMENT_TARGET=11.0 maturin build --release --manifest-path "${MANIFEST}" \
		--out "${DIST}" --target aarch64-apple-darwin
	MACOSX_DEPLOYMENT_TARGET=10.12 maturin build --release --manifest-path "${MANIFEST}" \
		--out "${DIST}" --target x86_64-apple-darwin

	local tag
	for tag in macosx_11_0_arm64 macosx_10_12_x86_64; do
		if ! compgen -G "${DIST}/*-${tag}.whl" >/dev/null; then
			echo "ERROR: no wheel with platform tag ${tag} in ${DIST}/; MACOSX_DEPLOYMENT_TARGET was not applied." >&2
			ls -1 "${DIST}" >&2
			exit 1
		fi
	done

	check_wheel_contents
	check_macho_alignment
}

# Mach-O alignment guard, for the llvm-objcopy bug described in
# build_macos_dists. dyld on macOS 27 rejects a dylib whose LC_SYMTAB string
# table offset is not a multiple of 8; older dyld may not, so this checks the
# file itself rather than trusting the runner's import smoke.
check_macho_alignment() {
	local whl so stroff tmp
	tmp="$(mktemp -d)"
	for whl in "${DIST}"/*.whl; do
		rm -rf "${tmp:?}"/*
		unzip -q "${whl}" 'quip_signer/*.so' -d "${tmp}"
		for so in "${tmp}"/quip_signer/*.so; do
			stroff="$(otool -l "${so}" | awk '/cmd LC_SYMTAB/ {f = 1} f && /stroff/ {print $2; exit}')"
			if [[ -z "${stroff}" ]] || ((stroff % 8)); then
				echo "ERROR: ${whl##*/}: ${so##*/} has its symbol string table at offset ${stroff:-<none>}, not 8-byte aligned; dyld on macOS 27 refuses to load it." >&2
				echo "       A post-link strip through rust-objcopy (LLVM 22.1.2-22.1.6) does this; see build_macos_dists." >&2
				rm -rf "${tmp}"
				exit 1
			fi
		done
	done
	rm -rf "${tmp}"
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
	# PYTHON picks the interpreter, and so the architecture pip resolves for: the
	# macOS job points it at an x86_64 CPython to smoke the Intel wheel under
	# Rosetta on an arm64 host.
	"${PYTHON:-python3}" -m venv "${smoke_root}/venv"
	"${smoke_root}/venv/bin/pip" install --quiet --disable-pip-version-check \
		--no-index --only-binary=:all: --find-links "${DIST}" quip-signer
	# Run from the venv dir so no stray `quip_signer/` in CWD can shadow the
	# install; the smoke script is referenced by absolute path since CWD is the venv.
	(cd "${smoke_root}/venv" &&
		"${smoke_root}/venv/bin/python" "${SCRIPT_DIR}/quip_signer_wheel_smoke.py")
}

# --- publish: OIDC mint + twine upload -------------------------------------
# Release-set guard. Publish is the one place the Linux and macOS dist/
# artefacts sit together, so this is where an incomplete or mixed set is caught
# before anything is uploaded. Each pattern must match exactly one file, nothing
# else may be present, and all five must be the same quip-signer version.
check_release_set() {
	local patterns=(
		"*-cp39-abi3-manylinux2014_x86_64.manylinux_2_17_x86_64.whl"
		"*-cp39-abi3-manylinux2014_aarch64.manylinux_2_17_aarch64.whl"
		"*-cp39-abi3-macosx_11_0_arm64.whl"
		"*-cp39-abi3-macosx_10_12_x86_64.whl"
		"*.tar.gz"
	)
	local files=("${DIST}"/*)
	local pattern matches
	for pattern in "${patterns[@]}"; do
		# compgen exits 1 on no match; under pipefail that would abort silently.
		matches=$({ compgen -G "${DIST}/${pattern}" || true; } | wc -l | tr -d ' ')
		if [[ "${matches}" != 1 ]]; then
			echo "ERROR: expected exactly one ${pattern} in ${DIST}/, found ${matches}." >&2
			ls -1 "${DIST}" >&2
			exit 1
		fi
	done
	if [[ "${#files[@]}" != "${#patterns[@]}" ]]; then
		echo "ERROR: ${DIST}/ holds ${#files[@]} files; the release is exactly ${#patterns[@]}:" >&2
		ls -1 "${DIST}" >&2
		exit 1
	fi
	# The sdist names the release (quip_signer-<version>.tar.gz); every wheel must
	# carry the same name and version, so a stale artefact can't ride along.
	local sdist release f
	sdist="$(compgen -G "${DIST}/*.tar.gz")"
	release="${sdist##*/}"
	release="${release%.tar.gz}"
	for f in "${files[@]}"; do
		if [[ "${release}" != quip_signer-* ]] ||
			[[ "${f##*/}" != "${release}.tar.gz" && "${f##*/}" != "${release}-cp39-abi3-"* ]]; then
			echo "ERROR: ${f##*/} is not part of release ${release}; every file must be the same quip-signer version." >&2
			ls -1 "${DIST}" >&2
			exit 1
		fi
	done
}

publish_dists() {
	local target="$1"

	# Before any network call: the full set, then metadata for every file. The
	# macOS wheels get their `twine check` here, since their build has no twine.
	check_release_set
	twine check "${DIST}"/*
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
build-macos) build_macos_dists ;;
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

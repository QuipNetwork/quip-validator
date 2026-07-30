QUIP_PROTOCOL_ROOT ?= /Users/romanuseinov/projects/quip/quip-protocol
QUIP_PROTOCOL_VENV := $(QUIP_PROTOCOL_ROOT)/.venv
QUIP_PROTOCOL_PYTHON := $(QUIP_PROTOCOL_VENV)/bin/python

WASM_SIGNER_CRATE := crates/transaction-crypto-wasm
WASM_SIGNER_PKG := js/quip-transaction-crypto-wasm
WASM_SIGNER_STAGE := target/wasm-signer-pkg
WASM_SIGNER_NAME := quip_transaction_crypto_wasm
WASM_PACK ?= wasm-pack
WASM_PACK_VERSION := 0.12.1

PY_SIGNER_CRATE := crates/transaction-crypto-py
PY_SIGNER_VENV := target/py-signer-venv
PY_SIGNER_PY := $(PY_SIGNER_VENV)/bin/python
PY_SIGNER_WHEELS := target/wheels
# PyO3 builds abi3 against the limited API; allow building on a CPython newer
# than the pinned pyo3 knows about (e.g. 3.14) via the forward-compat escape.
PY_SIGNER_ENV := PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1

polkadot-sdk:
	git clone --branch polkadot-stable2603 git@github.com:QuipNetwork/polkadot-sdk.git

local-3-node:
	./scripts/start-local3.sh

# Build and run the single-validator development node (Chain ID 1337), the
# pinned pallet-revive Ethereum JSON-RPC sidecar, and Subscan EVM indexer/UI.
# Runs attached so Ctrl-C stops the stack and leaves logs visible during testing.
revive-dev:
	docker compose -f docker-compose.revive.yml up --build

# Remove containers, development chain/indexer volumes, and any orphaned
# services left by an interrupted revive-dev run.
revive-dev-down:
	docker compose -f docker-compose.revive.yml down --volumes --remove-orphans

quantum-validation-venv:
	python3 -m venv $(QUIP_PROTOCOL_VENV)
	$(QUIP_PROTOCOL_PYTHON) -m pip install -e $(QUIP_PROTOCOL_ROOT)

quantum-validation-fixtures:
	QUIP_PROTOCOL_ROOT=$(QUIP_PROTOCOL_ROOT) $(QUIP_PROTOCOL_PYTHON) ./scripts/generate_quantum_validation_fixtures.py

# Build the browser signer WASM (requires `wasm-pack`). Outputs are generated
# and git-ignored; this regenerates them in place. Staged under target/ first so
# the curated package.json in the package dir is preserved.
wasm-signer:
	@test "$$($(WASM_PACK) --version)" = "wasm-pack $(WASM_PACK_VERSION)" || { \
		echo "wasm-pack $(WASM_PACK_VERSION) is required" >&2; exit 1; \
	}
	$(WASM_PACK) build $(WASM_SIGNER_CRATE) --target web --release \
		--out-dir $(abspath $(WASM_SIGNER_STAGE)) --out-name $(WASM_SIGNER_NAME)
	cp $(WASM_SIGNER_STAGE)/$(WASM_SIGNER_NAME).js \
		$(WASM_SIGNER_STAGE)/$(WASM_SIGNER_NAME).d.ts \
		$(WASM_SIGNER_STAGE)/$(WASM_SIGNER_NAME)_bg.wasm \
		$(WASM_SIGNER_STAGE)/$(WASM_SIGNER_NAME)_bg.wasm.d.ts \
		$(WASM_SIGNER_PKG)/
	@echo "Built $(WASM_SIGNER_PKG)/$(WASM_SIGNER_NAME)_bg.wasm ($$(wc -c < $(WASM_SIGNER_PKG)/$(WASM_SIGNER_NAME)_bg.wasm) bytes)"

# Tooling venv for the Python signer (maturin + pytest). Created on demand.
$(PY_SIGNER_PY):
	python3 -m venv $(PY_SIGNER_VENV)
	$(PY_SIGNER_PY) -m pip install --quiet --upgrade pip 'maturin>=1.7,<2' pytest

# Build the release wheel for the Python signer and report the extension size.
# The wheel lands in target/wheels/; downstreams `pip install` it (see
# crates/transaction-crypto-py/README.md). Like wasm-signer, the compiled artifact is small
# because the crate links only the sp-free transaction-crypto-core.
py-signer: $(PY_SIGNER_PY)
	$(PY_SIGNER_ENV) $(PY_SIGNER_VENV)/bin/maturin build --release \
		-m $(PY_SIGNER_CRATE)/Cargo.toml --out $(PY_SIGNER_WHEELS)
	@$(PY_SIGNER_PY) -c "import glob, zipfile; \
w = sorted(glob.glob('$(PY_SIGNER_WHEELS)/quip_signer-*.whl'))[-1]; \
z = zipfile.ZipFile(w); \
ext = max((i for i in z.infolist() if i.filename.endswith(('.so', '.pyd', '.dylib'))), key=lambda i: i.file_size); \
print(f'Built {ext.filename} ({ext.file_size} bytes) in {w}')"

# Build + install the extension into the venv AND drop the loose extension into
# crates/transaction-crypto-py/python/quip_signer/ (the submodule/PYTHONPATH
# staging location).
py-signer-develop: $(PY_SIGNER_PY)
	$(PY_SIGNER_ENV) VIRTUAL_ENV=$(abspath $(PY_SIGNER_VENV)) \
		$(PY_SIGNER_VENV)/bin/maturin develop --release \
		-m $(PY_SIGNER_CRATE)/Cargo.toml

# Build the extension and run the Python test suite (parity + API).
py-signer-test: py-signer-develop
	$(PY_SIGNER_PY) -m pytest $(PY_SIGNER_CRATE)/tests -q

# Build and push the multi-arch CI toolchain image to Docker Hub. No CI job
# does this — there are no Docker Hub credentials in the project or group CI
# variables, so it is a workstation action run under the carback1 namespace
# after `docker login`. Both arches are mandatory: the faucet repo's per-arch
# build-binary jobs pull this image on arm64 runners. The context is .gitlab/
# rather than the repo root because nothing in the Dockerfile COPYs, and the
# root would upload target/ and venv/ for no reason.
BUILDER_IMAGE ?= carback1/rust-substrate-builder:latest
builder-image:
	docker buildx build \
		--platform linux/amd64,linux/arm64 \
		--file .gitlab/ci-toolchain.Dockerfile \
		--tag $(BUILDER_IMAGE) \
		--push \
		.gitlab/

.PHONY: local-3-node revive-dev revive-dev-down quantum-validation-venv \
	quantum-validation-fixtures wasm-signer py-signer py-signer-develop \
	py-signer-test
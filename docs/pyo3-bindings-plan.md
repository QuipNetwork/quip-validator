# PyO3 Python Bindings Plan

## Goal

Expose Quip's hybrid (H3 = `sr25519 + ML-DSA-44`) transaction signer to Python,
reusing the **same sp-free core** the browser WASM signer is built from
(`quip-transaction-crypto-core`). This keeps the native extension small and —
more importantly — guarantees the Python signer, the browser signer, and the
runtime verifier all emit byte-identical signatures.

The Python module mirrors the capability set already proven in
`quip-transaction-crypto-wasm` but presents a **Pythonic, `bytes`-based API**
(not the hex-string JS API).

## Why this stays small

`quip-transaction-crypto-wasm` depends **only** on
`quip-transaction-crypto-core`, which is `sp`-free (no `sp-core`/`sp-io`/FRAME).
The PyO3 crate does the same. The native `.so`/`.pyd` therefore links just:

- the H3 suite via `quip-crypto-primitives-core` (`schnorrkel`, `ed25519-zebra`,
  `fips204`, `blake2`, `hkdf`, `sha2`, `subtle`, `zeroize`)
- `bip39` / `substrate-bip39` (mnemonic → seed)
- `codec` (SCALE envelope)
- `pyo3`

No Substrate runtime, no host functions — the binary is on the order of the
~400 KB WASM bundle, versus the tens of MB a runtime-linked build would be.

## Difference from WASM

`extension-module` PyO3 builds require `std` (they link against the Python
runtime), so the crate enables `quip-transaction-crypto-core/std`. This does
**not** pull in `sp-*`; it only flips `std` on the leaf crypto deps. "sp-free"
is preserved; "no_std" is not (and is not needed for a CPython extension).

## Target architecture

```
quip-validator/
└── crates/
    ├── transaction-crypto-core/  # UNCHANGED — sp-free byte API + golden fixture
    ├── transaction-crypto-wasm/  # UNCHANGED — cdylib over core (hex API, browser)
    └── transaction-crypto-py/    # NEW — cdylib over core (bytes API, CPython)
        ├── Cargo.toml            # pyo3 (extension-module, abi3) + core(std)
        ├── pyproject.toml        # maturin backend; python-source = "python"
        ├── README.md             # usage + submodule/editable-install docs
        ├── src/lib.rs            # #[pymodule] quip_signer
        ├── python/               # in-tree python-source (packed INTO the wheel)
        │   └── quip_signer/
        │       ├── __init__.py
        │       ├── *.so          # built artifact (git-ignored, like the .wasm)
        │       └── py.typed + .pyi
        └── tests/
            ├── test_parity.py    # reuses core's golden_vectors.txt
            └── test_api.py
```

The python-source lives **inside** the crate (`python/`) rather than a sibling
`py/quip-signer/`: maturin only packs a python-source under the manifest dir
into the built wheel, so an out-of-tree path shipped a surface-less wheel
(QUI-792). This dir doubles as the submodule/PYTHONPATH staging location.

The crate depends on `quip-transaction-crypto-core` only — the same dependency
edge the WASM crate uses. The `crates/transaction-crypto-py/python/` staging package mirrors the role
`js/quip-transaction-crypto-wasm/` plays for the browser signer: a fixed
location, importable by a downstream that pins `quip-validator` as a git
submodule, into which `make py-signer` drops the built (git-ignored) extension.

## Public API (Python module `quip_signer`)

`bytes` in, `bytes` out. Invalid input raises `QuipSignerError`.

```python
# free functions (mirror the core byte API)
public_from_seed(seed: bytes) -> bytes              # 32B seed -> 1344B H3 public
account_id_from_public(public: bytes) -> bytes      # 1344B public -> 32B account id
seed_from_mnemonic(secret_uri: str) -> bytes        # BIP39/0x-hex URI -> 32B seed
sign_payload_from_seed(seed: bytes, payload: bytes) -> bytes   # -> SCALE envelope
verify_envelope(payload: bytes, envelope: bytes, account_id: bytes) -> bool

# convenience class
class HybridSigner:
    @classmethod
    def from_seed(cls, seed: bytes) -> "HybridSigner": ...
    @classmethod
    def from_mnemonic(cls, secret_uri: str) -> "HybridSigner": ...
    @property
    def public_key(self) -> bytes: ...      # 1344B
    @property
    def account_id(self) -> bytes: ...      # 32B
    def sign(self, payload: bytes) -> bytes: ...   # SCALE envelope

class QuipSignerError(Exception): ...       # via pyo3::create_exception!
```

`sign_payload_from_seed` / `HybridSigner.sign` sign the payload **verbatim** — no
hashing, no length check. The H3 domain prefix is applied intrinsically by the
scheme (callers must not pre-apply it), and Substrate's `SignedPayload` rule
(`blake2_256(payload)` when the encoded payload exceeds 256 bytes) is an
extrinsic convention the caller must apply before signing. See the "Payload
contract" section in `crates/transaction-crypto-py/README.md`.

Each function is a thin wrapper over the existing core entry points
(`public_key_from_seed`, `account_id_from_public_bytes`,
`master_seed_from_secret_uri`, `sign_payload_from_seed`,
`HybridTxSignatureBytes::{decode_envelope, derived_account_id, verify}`).
`HybridTxCryptoError` is mapped to `QuipSignerError` with its `Debug` message at
the boundary.

PyO3 notes: take `&[u8]` for `bytes` inputs; return `Bound<'_, PyBytes>` (not
`Vec<u8>`, which maps to a `list[int]`). `HybridSigner` holds the 32-byte seed
(zeroized on drop) and re-derives per call, or caches the expanded
secret/public — decide during implementation; caching avoids re-running HKDF +
keygen on every `sign`.

## Steps

### Phase 1 — crate scaffold + minimal build

1. Add `crates/transaction-crypto-py` to the workspace `members`. Package
   `quip-transaction-crypto-py`, `[lib] crate-type = ["cdylib"]`.
2. `Cargo.toml` deps:
   - `pyo3 = { version = "0.23+", features = ["extension-module", "abi3-py39"] }`
   - `quip-transaction-crypto-core = { workspace = true, features = ["std"] }`
3. `pyproject.toml` with `build-backend = "maturin"`, `module-name =
   "quip_signer"`, `requires-python = ">=3.9"`.
4. Implement the free functions + `QuipSignerError`; `#[pymodule] fn
   quip_signer`.
5. `maturin develop` into a venv; smoke-test imports.

### Phase 2 — ergonomics

6. Add the `HybridSigner` class (`from_seed` / `from_mnemonic`, `public_key` /
   `account_id` properties, `sign`).
7. Generate `quip_signer.pyi` type stubs (hand-written or `maturin`-emitted) so
   downstream Python gets typing.

### Phase 3 — parity gate

8. `tests/test_parity.py`: load `crates/transaction-crypto-core/tests/golden_vectors.txt`
   (the same checked-in baseline the Rust + WASM signers are gated against) and
   assert `public_from_seed` and `sign_payload_from_seed` reproduce those exact
   bytes. This extends the golden gate to a third implementation — Rust core,
   WASM, and Python now provably agree.
9. `tests/test_api.py`: round-trip sign→verify, account-id derivation, mnemonic
   import, rejection of derivation junctions and bad lengths.

### Phase 4 — build glue + staging package

10. Makefile `py-signer` target mirroring `wasm-signer`:
    `maturin build --release -m crates/transaction-crypto-py/Cargo.toml`,
    then copy the built extension (`*.so`/`*.pyd`) and type stub into
    `crates/transaction-crypto-py/python/quip_signer/` — exactly as `wasm-signer` copies the
    `.wasm`/`.js`/`.d.ts` into `js/quip-transaction-crypto-wasm/`. Echo the
    extension size like the WASM target does. Add a `$(PY_OUT)` "build only if
    missing" rule too.
11. Git-ignore the built `*.so`/`*.pyd` under `crates/transaction-crypto-py/python/` (it's a
    generated artifact, like the `.wasm`); keep `__init__.py`, `py.typed`,
    `.pyi`, and `pyproject.toml` tracked.
12. A size-tuned build: extension built with `opt-level = "z"`, `lto = true`,
    `codegen-units = 1`, `strip = true`. Keep `panic = "unwind"` (PyO3 converts
    Rust panics to Python exceptions; `abort` would kill the interpreter).
13. README for the crate (`crates/transaction-crypto-py/README.md`); usage example.

## Distribution

### Primary — git submodule (selected)

A downstream Python project pins `quip-validator` as a git submodule and
builds the extension locally, exactly like the `apps` repo consumes the browser
signer today (`apps/Makefile`: init submodule → `make -C quip-validator
wasm-signer` → import from `js/...`). The Python analogue:

```make
# downstream Makefile
QUIP_SUBMODULE := quip-validator
py-signer: quip-submodule
	$(MAKE) -C $(QUIP_SUBMODULE) py-signer
	# then either:
	pip install -e $(QUIP_SUBMODULE)/crates/transaction-crypto-py  # editable install, or
	# add $(QUIP_SUBMODULE)/crates/transaction-crypto-py/python to PYTHONPATH and `import quip_signer`
```

`make py-signer` drops the built (git-ignored) `.so` into
`crates/transaction-crypto-py/python/quip_signer/`, so the downstream imports `quip_signer` straight
from the submodule with no PyPI round-trip — the same on-demand, build-locally
model as the WASM signer. Submodule consumers fetch the pinned commit from
origin (just like `apps` does), so the pin must be pushed to be shareable.

### Alternative — local dev build (in-repo)

`make py-signer-develop` → `maturin develop` into the active venv. For working
inside `quip-validator` itself (running `pytest`, iterating on the API).

### Alternative — published wheels + CI (deferred)

PyPI-installable wheels: `manylinux` (via `maturin-action` / `cibuildwheel`),
macOS, Windows; `abi3` → one wheel per platform across Python versions;
`maturin publish` in CI. Pull this in if/when the package should ship to PyPI
rather than be vendored via submodule.

## Workspace / build wrinkles

- **PyO3 in a Substrate workspace.** An `extension-module` cdylib doesn't link
  `libpython` at build time (the symbols are resolved by the interpreter at
  load). On Linux this is transparent; on **macOS** a bare
  `cargo build --workspace` over the py crate needs
  `-C link-arg=-undefined -C link-arg=dynamic_lookup`. `maturin` injects this
  automatically — so **always build the py crate through `maturin`**, and (if
  desired) add the apple-target link args to `.cargo/config.toml` so an
  incidental `cargo build --workspace` doesn't fail. Confirm `cargo build`
  for the existing crates/runtime is unaffected by the new member.
- **`resolver = "2"`** is already set; the new leaf crate won't perturb feature
  unification for the runtime (it isn't in the runtime's dependency graph).
- The git dep `quip-crypto-primitives-core` is consumed transitively via
  `transaction-crypto-core`; no new SDK dependency entry is required.

## Verification checklist

- [ ] `maturin develop` succeeds in a clean venv (Python 3.9+).
- [ ] `cargo build` of the existing crates + runtime still green with the new
      workspace member present.
- [ ] `pytest` green, incl. golden-vector parity against the shared fixture.
- [ ] Built `.so` is small (same order as the ~400 KB WASM bundle), with no
      `sp-*` linked (`cargo tree -p quip-transaction-crypto-py` is sp-free).
- [ ] `make py-signer` drops the extension into `crates/transaction-crypto-py/python/quip_signer/`
      and `import quip_signer` works from `py/` (the submodule consumption path).
- [ ] A Python-signed envelope verifies under the runtime (reuse the
      cross-impl parity: same bytes as `sign_payload_from_seed` in Rust, which
      `transaction-crypto`'s `runtime_signature_scale_matches_core_envelope`
      already proves the runtime accepts).

## Risks & mitigations

- **Silent drift from WASM/runtime.** Mitigation: the Python parity test reuses
  the *same* `golden_vectors.txt` fixture, so all three implementations are
  gated against one checked-in baseline.
- **`bytes` vs `list[int]` foot-gun.** Returning `Vec<u8>` from PyO3 yields a
  `list[int]`; return `PyBytes` explicitly and assert `isinstance(x, bytes)`
  in tests.
- **macOS link failure on `cargo build --workspace`.** Mitigation: build via
  `maturin`; optionally scope `dynamic_lookup` link args to apple targets in
  `.cargo/config.toml`.
- **Seed material in memory.** `HybridSigner` holds secret bytes; zeroize on
  drop (the core already uses `Zeroizing` internally for derived secrets).

## Out of scope

- Hex-string API (that's the WASM/JS surface; Python uses `bytes`).
- Derivation junctions (`//hard`, `/soft`) — same intentional restriction as
  the WASM signer.
- PyPI publishing / manylinux CI (deferred; submodule is the chosen path).
- Async or streaming APIs.
```

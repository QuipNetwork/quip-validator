# quip-signer (Python)

CPython bindings for Quip's hybrid (H3 = `sr25519 + ML-DSA-44`) transaction
signer. A thin PyO3 wrapper over `quip-transaction-crypto-core` — the same
`sp`-free engine the browser WASM signer is built from — so the Python signer,
the browser signer, and the runtime verifier are byte-identical.

## Install

```bash
pip install quip-signer
```

Prebuilt abi3 wheels are published for **linux x86_64** and **linux aarch64**
(CPython ≥ 3.9). macOS and Windows resolve the sdist and build it locally, which
needs a [Rust toolchain](https://rustup.rs/) on the machine. Releasing is
covered in [`docs/releasing-quip-signer.md`](../../docs/releasing-quip-signer.md).

### Building from source (submodule)

If you can't use the published wheel — pinning to an unreleased commit, or
building on an unsupported platform — vendor `quip-validator` as a git
submodule (the same model the browser signer uses) and build the extension
locally with [maturin](https://www.maturin.rs/). The build drops the
git-ignored extension into `crates/transaction-crypto-py/python/quip_signer/`.

```bash
# from your downstream repo, with quip-validator pinned as a submodule:
make -C quip-validator py-signer        # builds the wheel + extension

# then either install the wheel...
pip install quip-validator/target/wheels/quip_signer-*.whl
# ...or, for an editable/dev install:
pip install -e quip-validator/crates/transaction-crypto-py
# ...or add the staging dir to PYTHONPATH after `make py-signer-develop`:
export PYTHONPATH="quip-validator/crates/transaction-crypto-py/python:$PYTHONPATH"
```

The submodule pin must be pushed to origin to be shareable (same as the WASM
signer flow).

## Usage

```python
import quip_signer

# from a BIP39 phrase (or a 0x-prefixed 64-hex seed)
signer = quip_signer.HybridSigner.from_mnemonic(
    "bottom drive obey lake curtain smoke basket hold race lonely fit walk"
)
public = signer.public_key        # 1344 bytes
account = signer.account_id       # 32 bytes
envelope = signer.sign(b"payload bytes")   # SCALE-encoded HybridTxSignature

assert quip_signer.verify_envelope(b"payload bytes", envelope, account)

# free functions mirror the core byte API:
seed = quip_signer.seed_from_mnemonic("0x" + "07" * 32)
pub = quip_signer.public_from_seed(seed)
acct = quip_signer.account_id_from_public(pub)
env = quip_signer.sign_payload_from_seed(seed, b"hello")
```

Invalid input (bad seed length, malformed envelope, derivation junctions in a
URI, …) raises `quip_signer.QuipSignerError`.

## Payload contract (read before signing extrinsics)

`sign()` / `sign_payload_from_seed()` sign the bytes you hand them **exactly as
given**. There is no internal hashing and no length check, so two obligations
are yours:

1. **Do not apply the H3 domain prefix yourself.** The H3 scheme frames every
   message internally as
   `0x01 || "hybrid-sr25519-mldsa44-v1\0" || len(ctx) || ctx || msg` before
   signing. The browser signer, the Python signer, and the runtime verifier all
   share this core, so they agree byte-for-byte. Pass the unframed payload;
   pre-applying the prefix double-frames the message and the runtime rejects the
   signature.

2. **Apply Substrate's >256-byte rule yourself when signing extrinsics.**
   Substrate signs `SignedPayload::using_encoded`, which substitutes
   `blake2_256(payload)` for the raw bytes whenever the SCALE-encoded payload
   exceeds **256 bytes**, and otherwise signs verbatim. This is an extrinsic
   convention, **not** part of H3, so the binding does not do it for you. If you
   pass a >256-byte extrinsic payload here verbatim, you get a signature the
   runtime **silently rejects** with no useful error.

   ```python
   from hashlib import blake2b  # blake2_256 == blake2b(digest_size=32)

   MAX_UNHASHED_PAYLOAD_LEN = 256

   def message_to_sign(payload: bytes) -> bytes:
       if len(payload) > MAX_UNHASHED_PAYLOAD_LEN:
           return blake2b(payload, digest_size=32).digest()
       return payload

   envelope = signer.sign(message_to_sign(extrinsic_payload))
   ```

   (This mirrors the browser signer's `messageToSign` in `js/quip-signer`.)

## Building / testing in-repo

```bash
make py-signer-develop   # maturin develop into a local venv
make py-signer           # release wheel + size report
```

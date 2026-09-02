"""Quip hybrid (sr25519 + FN-DSA-512) transaction signer.

Thin Python surface over the Rust `_quip_signer` extension, which wraps the
same `sp`-free core the browser WASM signer is built from. All bytes in / bytes
out; invalid input raises :class:`QuipSignerError`.
"""

from ._quip_signer import (
    HybridSigner,
    QuipSignerError,
    account_id_from_public,
    public_from_seed,
    seed_from_mnemonic,
    sign_payload_from_seed,
    verify_envelope,
)

__all__ = [
    "HybridSigner",
    "QuipSignerError",
    "account_id_from_public",
    "public_from_seed",
    "seed_from_mnemonic",
    "sign_payload_from_seed",
    "verify_envelope",
]

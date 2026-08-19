//! CPython bindings for Quip's hybrid (H4 = `sr25519 + FN-DSA-512`) transaction
//! signer.
//!
//! This is a thin PyO3 wrapper over `quip-transaction-crypto-core` — the same
//! `sp`-free byte engine the browser WASM signer is built from. Reusing it
//! keeps the native extension small and guarantees the Python signer, the
//! browser signer, and the runtime verifier emit byte-identical signatures.
//!
//! The public surface is `bytes`-oriented (not the hex-string JS API):
//! invalid input raises [`QuipSignerError`].

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use quip_transaction_crypto_core::{
    account_id_from_public_bytes, master_seed_from_secret_uri, public_key_from_seed,
    sign_payload_from_seed as core_sign_payload_from_seed, HybridTxCryptoError,
    HybridTxSignatureBytes, HYBRID_PUBLIC_LEN,
};
use zeroize::Zeroizing;

create_exception!(
    quip_signer,
    QuipSignerError,
    PyException,
    "Raised when the Quip hybrid signer rejects its input."
);

fn map_err(error: HybridTxCryptoError) -> PyErr {
    QuipSignerError::new_err(format!("{error:?}"))
}

/// Derives serialized H4 public bytes (929B) from a 32-byte master seed.
#[pyfunction]
fn public_from_seed<'py>(py: Python<'py>, seed: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    // FN-DSA-512 key generation is CPU-heavy; copy the borrowed seed into an
    // owned (zeroized-on-drop) buffer and release the GIL so other Python
    // threads can progress.
    let seed = Zeroizing::new(seed.to_vec());
    let public = py
        .allow_threads(|| public_key_from_seed(&seed))
        .map_err(map_err)?;
    Ok(PyBytes::new(py, &public))
}

/// Derives the compact 32-byte Quip account id from serialized H4 public bytes.
#[pyfunction]
fn account_id_from_public<'py>(py: Python<'py>, public: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
    if public.len() != HYBRID_PUBLIC_LEN {
        return Err(QuipSignerError::new_err(format!(
            "expected {HYBRID_PUBLIC_LEN}-byte H4 public key, got {}",
            public.len()
        )));
    }
    Ok(PyBytes::new(py, &account_id_from_public_bytes(public)))
}

/// Derives the 32-byte H4 master seed from a limited secret URI.
///
/// Accepts a `0x`-prefixed 64-digit hex seed, or an English BIP39 phrase
/// optionally followed by `///<password>`. Derivation junctions (`//`, `/`) are
/// rejected.
#[pyfunction]
fn seed_from_mnemonic<'py>(py: Python<'py>, secret_uri: &str) -> PyResult<Bound<'py, PyBytes>> {
    let seed = master_seed_from_secret_uri(secret_uri).map_err(map_err)?;
    Ok(PyBytes::new(py, &seed))
}

/// Signs raw payload bytes with a 32-byte master seed, returning the
/// SCALE-encoded `HybridTxSignature` envelope.
///
/// `payload` is signed **exactly as given**: this binding never hashes and never
/// length-checks. The H4 domain prefix
/// (`0x01 || "hybrid-sr25519-falcon512-v1\0" || ...`) is applied intrinsically by
/// the signing scheme, so do NOT pre-apply it — doing so double-frames the
/// message and the runtime rejects the signature.
///
/// Substrate's `SignedPayload` rule — sign `blake2_256(payload)` instead of the
/// raw bytes when the SCALE-encoded payload exceeds 256 bytes — is an extrinsic
/// convention, not part of H4, and is therefore **the caller's responsibility**.
/// Passing a >256-byte extrinsic payload here verbatim yields a signature the
/// runtime silently rejects; hash it to 32 bytes first, then sign the digest.
#[pyfunction]
fn sign_payload_from_seed<'py>(
    py: Python<'py>,
    seed: &[u8],
    payload: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    // FN-DSA-512 key derivation + signing is CPU-heavy; copy the borrowed inputs
    // into owned buffers (zeroizing the secret seed copy) and release the GIL
    // for the duration.
    let seed = Zeroizing::new(seed.to_vec());
    let payload = payload.to_vec();
    let envelope = py
        .allow_threads(|| core_sign_payload_from_seed(&seed, &payload))
        .map_err(map_err)?;
    Ok(PyBytes::new(py, &envelope.encode_envelope()))
}

/// Verifies a SCALE-encoded envelope for the given payload and compact account
/// id (checks both that the embedded public key derives `account_id` and that
/// the hybrid signature is valid).
#[pyfunction]
fn verify_envelope(payload: &[u8], envelope: &[u8], account_id: &[u8]) -> PyResult<bool> {
    let envelope = HybridTxSignatureBytes::decode_envelope(envelope).map_err(map_err)?;
    Ok(envelope.derived_account_id().as_slice() == account_id && envelope.verify(payload))
}

/// A hybrid signer bound to one master seed.
///
/// Holds the 32-byte seed (zeroized on drop) and the derived public key, so
/// repeated signing avoids re-deriving the public key.
#[pyclass]
struct HybridSigner {
    seed: Zeroizing<[u8; 32]>,
    public: [u8; HYBRID_PUBLIC_LEN],
}

#[pymethods]
impl HybridSigner {
    /// Builds a signer from a 32-byte master seed.
    #[staticmethod]
    fn from_seed(py: Python<'_>, seed: &[u8]) -> PyResult<Self> {
        // Validates length (and key derivation) before we retain the seed.
        // FN-DSA-512 derivation is CPU-heavy, so copy the seed into an owned
        // (zeroized-on-drop) buffer and release the GIL while deriving the
        // public key.
        let owned = Zeroizing::new(seed.to_vec());
        let public = py
            .allow_threads(|| public_key_from_seed(&owned))
            .map_err(map_err)?;
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(seed);
        Ok(Self {
            seed: Zeroizing::new(bytes),
            public,
        })
    }

    /// Builds a signer from a secret URI (BIP39 phrase or `0x`-hex seed).
    #[staticmethod]
    fn from_mnemonic(py: Python<'_>, secret_uri: &str) -> PyResult<Self> {
        let seed = Zeroizing::new(master_seed_from_secret_uri(secret_uri).map_err(map_err)?);
        Self::from_seed(py, &seed[..])
    }

    /// Serialized H4 public bytes (929B).
    #[getter]
    fn public_key<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.public)
    }

    /// Compact 32-byte Quip account id for this signer.
    #[getter]
    fn account_id<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &account_id_from_public_bytes(&self.public))
    }

    /// Signs raw payload bytes, returning the SCALE-encoded envelope.
    ///
    /// `payload` is signed exactly as given: no hashing, no length check. The H4
    /// domain prefix is applied intrinsically by the scheme (do not pre-apply
    /// it). Applying Substrate's >256-byte `blake2_256` `SignedPayload` rule is
    /// the caller's responsibility — see [`sign_payload_from_seed`].
    fn sign<'py>(&self, py: Python<'py>, payload: &[u8]) -> PyResult<Bound<'py, PyBytes>> {
        // FN-DSA-512 signing is CPU-heavy; copy inputs into owned buffers
        // (zeroizing the secret seed copy) and release the GIL so other Python
        // threads aren't serialized behind it.
        let seed = Zeroizing::new(self.seed.to_vec());
        let payload = payload.to_vec();
        let envelope = py
            .allow_threads(|| core_sign_payload_from_seed(&seed, &payload))
            .map_err(map_err)?;
        Ok(PyBytes::new(py, &envelope.encode_envelope()))
    }
}

#[pymodule]
fn _quip_signer(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("QuipSignerError", m.py().get_type::<QuipSignerError>())?;
    m.add_function(wrap_pyfunction!(public_from_seed, m)?)?;
    m.add_function(wrap_pyfunction!(account_id_from_public, m)?)?;
    m.add_function(wrap_pyfunction!(seed_from_mnemonic, m)?)?;
    m.add_function(wrap_pyfunction!(sign_payload_from_seed, m)?)?;
    m.add_function(wrap_pyfunction!(verify_envelope, m)?)?;
    m.add_class::<HybridSigner>()?;
    Ok(())
}

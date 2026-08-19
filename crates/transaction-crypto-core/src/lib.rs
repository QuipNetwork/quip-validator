#![cfg_attr(not(feature = "std"), no_std)]

//! Pure transaction/account identity helpers for Quip's hybrid signer.
//!
//! This crate is intentionally bytes-oriented:
//! - it derives compact 32-byte account ids from hybrid public bytes
//! - it signs raw payload bytes with the H4 suite
//! - it encodes/decodes the runtime signature envelope as raw bytes
//!
//! The H4 suite itself (the `sr25519 + FN-DSA-512` keygen / sign / verify logic,
//! message framing, and seed derivation) is **not** reimplemented here. It uses
//! the shared, `sp`-free [`pqhybridsign_core::composite_delta`] implementation
//! with [`pqhybridsign::H4`], which the runtime wrapper also uses — so browser
//! and runtime signing remain byte-identical by construction.
//!
//! What stays quip-specific and lives here: account-id derivation
//! (`quip-account-v1`), the SCALE transaction-signature envelope, and the
//! BIP39 / secret-URI → master-seed helpers. None of those exist in the shared
//! crate.
//!
//! It does not depend on runtime traits, `sp_core`, or `sp_io`.

extern crate alloc;

use alloc::vec::Vec;

use bip39::{Language, Mnemonic};
use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use codec::{Decode, DecodeWithMemTracking, Encode};
use pqhybridsign::{H4, MIN_FALCON512_SIG_LEN};
use pqhybridsign_core::composite_delta;
use pqhybridsign_core::suite::DeltaSuite;
use zeroize::Zeroize;

const MASTER_SEED_LEN: usize = 32;
const CLASSICAL_SIGNATURE_LEN: usize = 64;
const DELTA_LEN: usize = 1;
const MIN_HYBRID_SIGNATURE_LEN: usize = CLASSICAL_SIGNATURE_LEN + DELTA_LEN + MIN_FALCON512_SIG_LEN;

/// Serialized H4 public-key length in bytes.
pub const HYBRID_PUBLIC_LEN: usize = H4::PUBLIC_KEY_LEN;
/// Maximum serialized H4 signature length in bytes.
pub const HYBRID_SIGNATURE_LEN: usize = H4::MAX_SIGNATURE_LEN;
/// Serialized H4 secret-key length in bytes.
pub const HYBRID_SECRET_LEN: usize = H4::SECRET_KEY_LEN;
/// Fixed length of derived Quip account ids.
pub const ACCOUNT_ID_LEN: usize = 32;

/// Domain separator for account-id derivation from the hybrid public key.
pub const ACCOUNT_ID_DOMAIN: &[u8] = b"quip-account-v1";

/// Error returned by byte-level hybrid transaction crypto helpers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HybridTxCryptoError {
    InvalidLength {
        expected: usize,
        actual: usize,
    },
    InvalidPublicKey,
    InvalidSecretKey,
    InvalidSeed,
    SigningFailed,
    /// The BIP39 phrase was not a valid English mnemonic.
    InvalidMnemonic,
    /// The secret URI contained `/`-prefixed derivation junctions, which the
    /// browser signer intentionally does not support.
    UnsupportedDerivationPath,
}

pub type HybridResult<T> = core::result::Result<T, HybridTxCryptoError>;

fn exact_array<const N: usize>(bytes: &[u8]) -> HybridResult<[u8; N]> {
    bytes
        .try_into()
        .map_err(|_| HybridTxCryptoError::InvalidLength {
            expected: N,
            actual: bytes.len(),
        })
}

fn signature_wire_len(signature: &[u8; HYBRID_SIGNATURE_LEN]) -> usize {
    MIN_HYBRID_SIGNATURE_LEN + usize::from(signature[CLASSICAL_SIGNATURE_LEN])
}

fn validate_signature_padding(signature: &[u8; HYBRID_SIGNATURE_LEN]) -> HybridResult<usize> {
    let wire_len = signature_wire_len(signature);
    if signature[wire_len..].iter().any(|byte| *byte != 0) {
        return Err(HybridTxCryptoError::InvalidLength {
            expected: wire_len,
            actual: HYBRID_SIGNATURE_LEN,
        });
    }
    Ok(wire_len)
}

/// Bytes-level transaction signature envelope.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Encode, Decode, DecodeWithMemTracking)]
pub struct HybridTxSignatureBytes {
    pub public: [u8; HYBRID_PUBLIC_LEN],
    pub signature: [u8; HYBRID_SIGNATURE_LEN],
}

impl HybridTxSignatureBytes {
    /// Creates a bytes-level envelope after validating the input lengths and encodings.
    pub fn new(public: &[u8], signature: &[u8]) -> HybridResult<Self> {
        let public = exact_array(public)?;
        let signature = exact_array(signature)?;
        validate_signature_padding(&signature)?;

        Ok(Self { public, signature })
    }

    /// Returns the derived compact account id for the embedded public key.
    pub fn derived_account_id(&self) -> [u8; ACCOUNT_ID_LEN] {
        account_id_from_public_bytes(&self.public)
    }

    /// Verifies the embedded signature against the provided raw message bytes.
    pub fn verify(&self, message: &[u8]) -> bool {
        let Ok(wire_len) = validate_signature_padding(&self.signature) else {
            return false;
        };
        composite_delta::verify::<H4>(&self.public, message, b"", &self.signature[..wire_len])
    }

    /// SCALE-encodes the bytes-level envelope.
    pub fn encode_envelope(&self) -> Vec<u8> {
        self.encode()
    }

    /// Decodes a SCALE-encoded bytes-level envelope.
    pub fn decode_envelope(bytes: &[u8]) -> HybridResult<Self> {
        let decoded =
            Self::decode(&mut &bytes[..]).map_err(|_| HybridTxCryptoError::InvalidLength {
                expected: HYBRID_PUBLIC_LEN + HYBRID_SIGNATURE_LEN,
                actual: bytes.len(),
            })?;
        Self::new(&decoded.public, &decoded.signature)
    }
}

/// Derives the compact Quip account id from serialized H4 public bytes.
pub fn account_id_from_public_bytes(public: &[u8]) -> [u8; ACCOUNT_ID_LEN] {
    let mut hasher = Blake2bVar::new(ACCOUNT_ID_LEN).expect("32-byte Blake2 output is valid");
    hasher.update(ACCOUNT_ID_DOMAIN);
    hasher.update(public);

    let mut out = [0u8; ACCOUNT_ID_LEN];
    hasher
        .finalize_variable(&mut out)
        .expect("output length matches buffer");
    out
}

/// Derives serialized H4 public bytes from a 32-byte H4 master seed.
pub fn public_key_from_seed(seed: &[u8]) -> HybridResult<[u8; HYBRID_PUBLIC_LEN]> {
    let seed = exact_array::<MASTER_SEED_LEN>(seed)?;
    let mut secret = [0u8; HYBRID_SECRET_LEN];
    let mut public = [0u8; HYBRID_PUBLIC_LEN];
    let result = composite_delta::keypair_from_seed::<H4>(&seed, &mut secret, &mut public)
        .map_err(|_| HybridTxCryptoError::InvalidSeed);
    secret.zeroize();
    result.map(|()| public)
}

/// Derives the 32-byte H4 master seed from a limited secret URI.
///
/// This mirrors the supported subset of substrate's `Pair::from_string`:
///
/// - a `0x`-prefixed 64-digit hex string is used directly as the master seed;
/// - otherwise the input is an English BIP39 phrase, optionally followed by
///   `///<password>`.
///
/// Derivation junctions (`//hard`, `/soft`) and the leading-`/` dev-phrase
/// shortcut are intentionally **not** supported: a `/` outside the `///`
/// password separator is rejected rather than silently ignored, so an imported
/// account can never derive to an address the runtime would not recognize.
pub fn master_seed_from_secret_uri(uri: &str) -> HybridResult<[u8; MASTER_SEED_LEN]> {
    let (phrase, password) = match uri.split_once("///") {
        Some((phrase, password)) => (phrase, Some(password)),
        None => (uri, None),
    };
    let phrase = phrase.trim();

    if let Some(hex) = phrase
        .strip_prefix("0x")
        .or_else(|| phrase.strip_prefix("0X"))
    {
        return decode_seed_hex(hex);
    }

    if phrase.contains('/') {
        return Err(HybridTxCryptoError::UnsupportedDerivationPath);
    }

    master_seed_from_mnemonic(phrase, password)
}

/// Derives the 32-byte H4 master seed from an English BIP39 phrase.
///
/// This matches substrate's `Pair::from_phrase`: the mnemonic entropy is run
/// through `substrate_bip39::seed_from_entropy` (PBKDF2-HMAC-SHA512, salt
/// `"mnemonic" || password`, 2048 rounds) and the first 32 bytes of the 64-byte
/// output are taken as the master seed.
pub fn master_seed_from_mnemonic(
    phrase: &str,
    password: Option<&str>,
) -> HybridResult<[u8; MASTER_SEED_LEN]> {
    // `parse_in_normalized` expects already-NFKD input and, unlike `parse_in`,
    // does not pull in `unicode-normalization`. English BIP39 words are
    // lowercase ASCII (always NFKD) and NFKD never changes case, so this yields
    // the same entropy as substrate's `parse_in` for every valid English
    // mnemonic.
    let mnemonic = Mnemonic::parse_in_normalized(Language::English, phrase)
        .map_err(|_| HybridTxCryptoError::InvalidMnemonic)?;
    let (entropy, entropy_len) = mnemonic.to_entropy_array();

    let mut big_seed =
        substrate_bip39::seed_from_entropy(&entropy[..entropy_len], password.unwrap_or(""))
            .map_err(|_| HybridTxCryptoError::InvalidMnemonic)?;

    let mut seed = [0u8; MASTER_SEED_LEN];
    seed.copy_from_slice(&big_seed[..MASTER_SEED_LEN]);
    big_seed.zeroize();

    Ok(seed)
}

/// Decodes a 64-digit hex string into a 32-byte master seed.
fn decode_seed_hex(hex: &str) -> HybridResult<[u8; MASTER_SEED_LEN]> {
    if hex.len() != MASTER_SEED_LEN * 2 {
        return Err(HybridTxCryptoError::InvalidSeed);
    }

    let mut seed = [0u8; MASTER_SEED_LEN];
    for (index, byte) in seed.iter_mut().enumerate() {
        let hi = hex_digit(hex.as_bytes()[index * 2])?;
        let lo = hex_digit(hex.as_bytes()[index * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }

    Ok(seed)
}

fn hex_digit(ch: u8) -> HybridResult<u8> {
    match ch {
        b'0'..=b'9' => Ok(ch - b'0'),
        b'a'..=b'f' => Ok(ch - b'a' + 10),
        b'A'..=b'F' => Ok(ch - b'A' + 10),
        _ => Err(HybridTxCryptoError::InvalidSeed),
    }
}

/// Signs raw payload bytes with a 32-byte H4 master seed and returns the bytes-level envelope.
///
/// # Payload contract
///
/// `payload` is signed **exactly as given** — this function performs no hashing
/// and no length check. Two caller obligations follow:
///
/// - **H4 domain prefix is intrinsic; do NOT pre-apply it.** The H4 scheme
///   frames every message internally as
///   `0x01 ‖ "hybrid-sr25519-falcon512-v1\0" ‖ len(ctx) ‖ ctx ‖ msg` before
///   hashing/signing. Browser, runtime, and Python all go through the same
///   core, so they agree byte-for-byte. Callers pass the unframed payload;
///   pre-applying the prefix yourself would double-frame and the runtime would
///   reject the signature.
/// - **The Substrate >256-byte rule is the caller's job.** Substrate signs
///   `SignedPayload::using_encoded`, which substitutes `blake2_256(payload)`
///   for the raw bytes whenever the SCALE-encoded payload exceeds 256 bytes.
///   That is an extrinsic convention, not part of H4, so this function does not
///   apply it. A caller that hands a >256-byte extrinsic payload here verbatim
///   gets a signature the runtime silently rejects. Hash first, then sign the
///   32-byte digest.
pub fn sign_payload_from_seed(seed: &[u8], payload: &[u8]) -> HybridResult<HybridTxSignatureBytes> {
    let seed = exact_array::<MASTER_SEED_LEN>(seed)?;
    let mut secret = [0u8; HYBRID_SECRET_LEN];
    let mut public = [0u8; HYBRID_PUBLIC_LEN];
    if composite_delta::keypair_from_seed::<H4>(&seed, &mut secret, &mut public).is_err() {
        secret.zeroize();
        return Err(HybridTxCryptoError::InvalidSeed);
    }
    let result = sign_payload_with_arrays(&secret, &public, payload);
    secret.zeroize();
    result
}

/// Signs raw payload bytes with expanded H4 secret bytes and matching public bytes.
///
/// The payload contract is identical to [`sign_payload_from_seed`]: the bytes
/// are signed verbatim (no hashing, no length check), the H4 domain prefix is
/// applied intrinsically by the scheme, and applying Substrate's >256-byte
/// `blake2_256` rule is the caller's responsibility.
pub fn sign_payload_from_secret(
    secret: &[u8],
    public: &[u8],
    payload: &[u8],
) -> HybridResult<HybridTxSignatureBytes> {
    let mut secret = exact_array::<HYBRID_SECRET_LEN>(secret)?;
    let public = exact_array::<HYBRID_PUBLIC_LEN>(public)?;
    let result = sign_payload_with_arrays(&secret, &public, payload);
    secret.zeroize();
    result
}

fn sign_payload_with_arrays(
    secret: &[u8; HYBRID_SECRET_LEN],
    public: &[u8; HYBRID_PUBLIC_LEN],
    payload: &[u8],
) -> HybridResult<HybridTxSignatureBytes> {
    let mut signature = [0u8; HYBRID_SIGNATURE_LEN];
    let wire_len =
        composite_delta::sign_deterministic::<H4>(secret, payload, b"", b"", &mut signature)
            .map_err(|_| HybridTxCryptoError::SigningFailed)?;
    if wire_len != signature_wire_len(&signature) {
        return Err(HybridTxCryptoError::SigningFailed);
    }
    HybridTxSignatureBytes::new(public, &signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_domain_is_pinned() {
        assert_eq!(ACCOUNT_ID_DOMAIN, b"quip-account-v1");
    }

    #[test]
    fn same_public_key_derives_same_account_id() {
        let public = public_key_from_seed(&[1u8; 32]).unwrap();
        let first = account_id_from_public_bytes(public.as_ref());
        let second = account_id_from_public_bytes(public.as_ref());

        assert_eq!(first, second);
    }

    #[test]
    fn different_public_keys_derive_different_account_ids() {
        let alice = public_key_from_seed(&[1u8; 32]).unwrap();
        let bob = public_key_from_seed(&[2u8; 32]).unwrap();

        assert_ne!(
            account_id_from_public_bytes(alice.as_ref()),
            account_id_from_public_bytes(bob.as_ref())
        );
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let envelope = sign_payload_from_seed(&[7u8; 32], b"quip-message").unwrap();
        assert!(envelope.verify(b"quip-message"));
        assert!(!envelope.verify(b"wrong-message"));
    }

    #[test]
    fn public_key_from_seed_matches_signed_envelope_public() {
        let public = public_key_from_seed(&[7u8; 32]).unwrap();
        let envelope = sign_payload_from_seed(&[7u8; 32], b"quip-message").unwrap();

        assert_eq!(public, envelope.public);
    }

    #[test]
    fn envelope_round_trips_through_scale() {
        let envelope = sign_payload_from_seed(&[9u8; 32], b"quip-message").unwrap();
        let encoded = envelope.encode_envelope();
        let decoded = HybridTxSignatureBytes::decode_envelope(&encoded).unwrap();

        assert_eq!(decoded, envelope);
        assert!(decoded.verify(b"quip-message"));
    }

    #[test]
    fn envelope_decode_rejects_empty_bytes() {
        assert!(HybridTxSignatureBytes::decode_envelope(&[]).is_err());
    }

    #[test]
    fn tampered_signature_rejects() {
        let mut envelope = sign_payload_from_seed(&[11u8; 32], b"quip-message").unwrap();
        envelope.signature[envelope.signature.len() / 2] ^= 0xFF;

        assert!(!envelope.verify(b"quip-message"));
    }

    #[test]
    fn noncanonical_signature_padding_rejects() {
        let envelope = sign_payload_from_seed(&[11u8; 32], b"quip-message").unwrap();
        let mut padded = envelope.signature;
        padded[CLASSICAL_SIGNATURE_LEN] = 0;
        assert_eq!(
            HybridTxSignatureBytes::new(&envelope.public, &padded),
            Err(HybridTxCryptoError::InvalidLength {
                expected: MIN_HYBRID_SIGNATURE_LEN,
                actual: HYBRID_SIGNATURE_LEN,
            })
        );
    }

    #[test]
    fn wrong_seed_length_rejects() {
        assert!(public_key_from_seed(&[1u8; 31]).is_err());
    }

    // Standard BIP39 test vector phrase.
    const TEST_PHRASE: &str =
        "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

    #[test]
    fn mnemonic_seed_is_deterministic() {
        let first = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
        let second = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();

        assert_eq!(first, second);
    }

    #[test]
    fn mnemonic_password_changes_seed() {
        let no_password = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
        let with_password = master_seed_from_mnemonic(TEST_PHRASE, Some("hunter2")).unwrap();

        assert_ne!(no_password, with_password);
    }

    #[test]
    fn secret_uri_password_matches_explicit_password() {
        let split =
            master_seed_from_secret_uri(&alloc::format!("{TEST_PHRASE}///hunter2")).unwrap();
        let explicit = master_seed_from_mnemonic(TEST_PHRASE, Some("hunter2")).unwrap();

        assert_eq!(split, explicit);
    }

    #[test]
    fn secret_uri_accepts_hex_seed() {
        let seed = master_seed_from_secret_uri(
            "0x0707070707070707070707070707070707070707070707070707070707070707",
        )
        .unwrap();

        assert_eq!(seed, [7u8; MASTER_SEED_LEN]);
    }

    #[test]
    fn secret_uri_rejects_derivation_junctions() {
        assert_eq!(
            master_seed_from_secret_uri(&alloc::format!("{TEST_PHRASE}//0")),
            Err(HybridTxCryptoError::UnsupportedDerivationPath)
        );
    }

    #[test]
    fn secret_uri_rejects_invalid_phrase() {
        assert_eq!(
            master_seed_from_secret_uri("not a real mnemonic phrase at all"),
            Err(HybridTxCryptoError::InvalidMnemonic)
        );
    }

    #[test]
    fn mnemonic_seed_signs_and_verifies() {
        let seed = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
        let envelope = sign_payload_from_seed(&seed, b"quip-message").unwrap();

        assert!(envelope.verify(b"quip-message"));
    }
}

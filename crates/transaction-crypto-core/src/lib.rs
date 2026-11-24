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
//! the shared, `sp`-free [`pqhybridsign::composite_delta`] implementation
//! with [`pqhybridsign::SrFn512`], which the runtime wrapper also uses — so browser
//! and runtime signing remain byte-identical by construction.
//!
//! What stays quip-specific and lives here: account-id derivation
//! (`quip-account-v1`), the SCALE transaction-signature envelope, and the
//! signers' secret-URI grammar. A phrase becomes a seed through the shared
//! [`pqhybridsign::quip_hd`] (QUIP HD v1), exactly as in the SDK fork.
//!
//! It does not depend on runtime traits, `sp_core`, or `sp_io`.

extern crate alloc;

use alloc::vec::Vec;

use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use codec::{Decode, DecodeWithMemTracking, Encode};
use pqhybridsign::composite_delta;
use pqhybridsign::quip_hd::{self, HybridSuite};
use pqhybridsign::suite::DeltaSuite;
use pqhybridsign::{SrFn512, MIN_FALCON512_SIG_LEN};
use zeroize::Zeroize;

const MASTER_SEED_LEN: usize = 32;

/// The suite whose QUIP HD leaf the signers derive: H4, the transaction suite.
const SIGNER_SUITE: HybridSuite = HybridSuite::SrFn512;
const CLASSICAL_SIGNATURE_LEN: usize = 64;
/// The sr25519 half at the front of an H4 public key.
const CLASSICAL_PUBLIC_LEN: usize = 32;
const DELTA_LEN: usize = 1;
const MIN_HYBRID_SIGNATURE_LEN: usize = CLASSICAL_SIGNATURE_LEN + DELTA_LEN + MIN_FALCON512_SIG_LEN;

/// Serialized H4 public-key length in bytes.
pub const HYBRID_PUBLIC_LEN: usize = SrFn512::PUBLIC_KEY_LEN;
/// Maximum serialized H4 signature length in bytes.
pub const HYBRID_SIGNATURE_LEN: usize = SrFn512::MAX_SIGNATURE_LEN;
/// Serialized H4 secret-key length in bytes.
pub const HYBRID_SECRET_LEN: usize = SrFn512::SECRET_KEY_LEN;
/// Fixed length of derived Quip account ids.
pub const ACCOUNT_ID_LEN: usize = 32;

/// Domain separator for account-id derivation from the hybrid public key.
pub const ACCOUNT_ID_DOMAIN: &[u8] = b"quip-account-v1";

/// Domain separator used by Substrate's ordinary hybrid `Pair::sign` path.
///
/// This must stay byte-for-byte aligned with the Polkadot SDK wrapper so
/// browser and Python transaction signatures verify in the runtime. It is
/// deliberately distinct from the hybrid VRF binding label.
pub const SUBSTRATE_PAIR_SIGNATURE_CONTEXT: &[u8] = b"quip/substrate-pair-signature/v1";

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
    /// The phrase was not a valid English BIP-39 mnemonic, was empty, or held
    /// a character outside the secret-URI phrase class (ASCII letters, digits
    /// and spaces).
    InvalidMnemonic,
    /// A `///password` followed a `0x` seed, or the password held a line break.
    InvalidPassword,
    /// The secret URI contained `/` or `//` derivation junctions, which the
    /// signers intentionally do not support.
    UnsupportedDerivationPath,
}

/// Messages carry no part of the input, so a rejected secret URI is never
/// echoed into a log or an exception.
impl core::fmt::Display for HybridTxCryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidLength { expected, actual } => {
                write!(f, "expected {expected} bytes, got {actual}")
            }
            Self::InvalidPublicKey => f.write_str("invalid hybrid public key"),
            Self::InvalidSecretKey => f.write_str("invalid hybrid secret key"),
            Self::InvalidSeed => {
                f.write_str("invalid seed: expected 0x followed by 64 hex digits (32 bytes)")
            }
            Self::SigningFailed => f.write_str("signing failed"),
            Self::InvalidMnemonic => f.write_str(
                "invalid secret phrase: expected an English BIP-39 mnemonic of ASCII words \
                 separated by spaces, optionally followed by ///password",
            ),
            Self::InvalidPassword => f.write_str(
                "invalid password: a 0x seed takes no ///password, and a password cannot \
                 contain a line break",
            ),
            Self::UnsupportedDerivationPath => f.write_str(
                "derivation paths (/ or //) are not supported here: derive the account \
                 elsewhere (for example with the node's `key inspect`) and import its 0x \
                 secret seed instead",
            ),
        }
    }
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

/// G-002 (GregoAI wp-2): the sr25519 half of an H4 public key must not be the
/// Ristretto identity.
///
/// For `A = identity`, schnorrkel's check `R == s·B - k·A` becomes `R == s·B`,
/// so `R = B, s = 1` verifies every message without a secret, and the
/// post-quantum half signs a message that depends only on the suite label,
/// context and payload. Such a key is post-quantum only while it claims to be
/// hybrid. Ristretto is a prime-order group and its encoding is canonical, so
/// the identity is the only degenerate point and `[0; 32]` its only encoding.
/// Every other encoding schnorrkel rejects is refused by the suite's verify.
/// This matches the SDK fork's `sr25519_public_is_valid` (quip crypto-core
/// `suite/classical_key.rs`).
fn classical_public_is_valid(public: &[u8; HYBRID_PUBLIC_LEN]) -> bool {
    public[..CLASSICAL_PUBLIC_LEN] != [0u8; CLASSICAL_PUBLIC_LEN]
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
    ///
    /// A public key whose sr25519 half is the Ristretto identity is refused
    /// (G-002), as the SDK fork's `Public::to_suite_public` refuses it.
    pub fn verify(&self, message: &[u8]) -> bool {
        if !classical_public_is_valid(&self.public) {
            return false;
        }
        let Ok(wire_len) = validate_signature_padding(&self.signature) else {
            return false;
        };
        composite_delta::verify::<SrFn512>(
            &self.public,
            message,
            SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
            &self.signature[..wire_len],
        )
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
    let result = composite_delta::keypair_from_seed::<SrFn512>(&seed, &mut secret, &mut public)
        .map_err(|_| HybridTxCryptoError::InvalidSeed);
    secret.zeroize();
    result.map(|()| public)
}

/// Derives the 32-byte H4 master seed from a secret URI, using the signers'
/// junction-free subset of the SDK fork's grammar (WP2 batch 1 design §6).
///
/// The grammar is upstream `AddressUri`'s, character for character: a phrase
/// of ASCII letters, digits and spaces, then an optional `///password` that
/// runs to the end and holds no line break. What it accepts:
///
/// - `0x` + 64 hex digits (either case) is the master seed itself. It takes no
///   `///password`, not even an empty one. `0X` is rejected.
/// - Any other phrase is an English BIP-39 mnemonic. The seed is its QUIP HD
///   v1 leaf for H4 at account 0, index 0 ([`master_seed_from_mnemonic`]); the
///   password is the BIP-39 passphrase. This is the seed the SDK fork's
///   `Pair::from_string` gives for the same URI.
///
/// What it rejects, unlike the SDK fork: any `/` or `//` junction before the
/// `///` ([`HybridTxCryptoError::UnsupportedDerivationPath`]; import the
/// derived account's `0x` seed instead), and an empty phrase, because the
/// signers have no development phrase. A `/` inside the password
/// (`phrase///p/w`) is part of the password.
pub fn master_seed_from_secret_uri(uri: &str) -> HybridResult<[u8; MASTER_SEED_LEN]> {
    let (phrase, password) = split_secret_uri(uri)?;

    if let Some(hex) = phrase.strip_prefix("0x") {
        if password.is_some() {
            return Err(HybridTxCryptoError::InvalidPassword);
        }
        return decode_seed_hex(hex);
    }
    if phrase.starts_with("0X") {
        return Err(HybridTxCryptoError::InvalidSeed);
    }
    if phrase.is_empty() {
        return Err(HybridTxCryptoError::InvalidMnemonic);
    }

    master_seed_from_mnemonic(phrase, password)
}

/// Splits a secret URI into its phrase and `///password` the way upstream
/// `AddressUri::parse` does, refusing junctions.
fn split_secret_uri(uri: &str) -> HybridResult<(&str, Option<&str>)> {
    // Every byte counted here is ASCII, so the split is on a char boundary.
    let phrase_len = uri
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b' ')
        .count();
    let (phrase, rest) = uri.split_at(phrase_len);

    if rest.is_empty() {
        return Ok((phrase, None));
    }
    if let Some(password) = rest.strip_prefix("///") {
        if password.contains('\n') {
            return Err(HybridTxCryptoError::InvalidPassword);
        }
        return Ok((phrase, Some(password)));
    }
    if rest.starts_with('/') {
        return Err(HybridTxCryptoError::UnsupportedDerivationPath);
    }
    // A character outside the phrase class: a tab, a no-break space, a quote,
    // a line break, any non-ASCII letter.
    Err(HybridTxCryptoError::InvalidMnemonic)
}

/// Derives the 32-byte H4 master seed from an English BIP-39 phrase: its
/// QUIP HD v1 leaf `m/0x514E'/0x400'/20049'/0'/0'`
/// ([`pqhybridsign::quip_hd::hybrid_leaf`]).
///
/// The BIP-39 seed is standard: PBKDF2-HMAC-SHA512 over the NFKD phrase with
/// salt `"mnemonic" || NFKD(password)`, 2048 rounds, 64 bytes. This replaces
/// the earlier Substrate derivation (PBKDF2 over the mnemonic's entropy, first
/// 32 bytes), so the same phrase now gives a different key.
///
/// This is the library entry point, so the phrase follows QUIP HD's own
/// normalization: surrounding and repeated whitespace (JavaScript `\s`) is
/// accepted. [`master_seed_from_secret_uri`] is stricter about characters.
pub fn master_seed_from_mnemonic(
    phrase: &str,
    password: Option<&str>,
) -> HybridResult<[u8; MASTER_SEED_LEN]> {
    let leaf = quip_hd::hybrid_leaf(SIGNER_SUITE, phrase, password.unwrap_or(""), 0, 0)
        .map_err(|_| HybridTxCryptoError::InvalidMnemonic)?;
    // `leaf` is wiped on drop; the returned copy is the caller's to wipe.
    Ok(*leaf)
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
/// - **H4 domain separation is intrinsic; do NOT pre-apply it.** The H4 scheme
///   frames every message internally as
///   `0x01 ‖ "hybrid-sr25519-falcon512-v1\0" ‖ len(ctx) ‖ ctx ‖ msg` before
///   hashing/signing, using [`SUBSTRATE_PAIR_SIGNATURE_CONTEXT`] as `ctx`.
///   Browser, runtime, and Python all go through the same core, so they agree
///   byte-for-byte. Callers pass the unframed payload; pre-applying the prefix
///   or context yourself would double-frame and the runtime would reject the
///   signature.
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
    if composite_delta::keypair_from_seed::<SrFn512>(&seed, &mut secret, &mut public).is_err() {
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
/// are signed verbatim (no hashing, no length check), the H4 domain separation
/// is applied intrinsically by the scheme, and applying Substrate's >256-byte
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
    let wire_len = composite_delta::sign_deterministic::<SrFn512>(
        secret,
        payload,
        SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
        b"",
        &mut signature,
    )
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
    fn substrate_pair_signature_context_is_pinned() {
        assert_eq!(
            SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
            b"quip/substrate-pair-signature/v1"
        );
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

    /// The compressed Ristretto basepoint `B`.
    const RISTRETTO_BASEPOINT: &str =
        "e2f2ae0a6abc4e71a884a961c500515f58e30b6aa582dd8db6a65945e08d2d76";

    /// An honest envelope with its sr25519 half replaced by the identity and
    /// the classical signature by the secret-free forgery `R = B, s = 1`
    /// (schnorrkel's marker bit set).
    fn identity_half_forgery(message: &[u8]) -> HybridTxSignatureBytes {
        let mut forged = sign_payload_from_seed(&[7u8; 32], message).unwrap();
        forged.public[..CLASSICAL_PUBLIC_LEN].fill(0);
        for (index, byte) in forged.signature[..32].iter_mut().enumerate() {
            *byte = u8::from_str_radix(&RISTRETTO_BASEPOINT[2 * index..2 * index + 2], 16).unwrap();
        }
        forged.signature[32..CLASSICAL_SIGNATURE_LEN].fill(0);
        forged.signature[32] = 1;
        forged.signature[CLASSICAL_SIGNATURE_LEN - 1] |= 0x80;
        forged
    }

    /// The forgery is real: the suite's own verify accepts it, so the G-002
    /// check in `verify` is what refuses it.
    #[test]
    fn the_identity_half_forgery_passes_the_bare_suite_verify() {
        let forged = identity_half_forgery(b"quip-message");
        let wire_len = validate_signature_padding(&forged.signature).unwrap();
        assert!(composite_delta::verify::<SrFn512>(
            &forged.public,
            b"quip-message",
            SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
            &forged.signature[..wire_len],
        ));
    }

    #[test]
    fn verify_refuses_the_sr25519_identity_half() {
        let forged = identity_half_forgery(b"quip-message");
        assert!(!forged.verify(b"quip-message"));
        // Decoding the envelope does not bypass it.
        let decoded = HybridTxSignatureBytes::decode_envelope(&forged.encode_envelope()).unwrap();
        assert!(!decoded.verify(b"quip-message"));
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

    const HEX_SEED: &str = "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a";

    #[test]
    fn mnemonic_seed_is_the_h4_quip_hd_leaf() {
        let leaf =
            quip_hd::hybrid_leaf(HybridSuite::SrFn512, TEST_PHRASE, "hunter2", 0, 0).unwrap();
        assert_eq!(
            master_seed_from_mnemonic(TEST_PHRASE, Some("hunter2")).unwrap(),
            *leaf
        );
        // An absent password and an empty one are the same BIP-39 passphrase.
        assert_eq!(
            master_seed_from_mnemonic(TEST_PHRASE, None).unwrap(),
            master_seed_from_mnemonic(TEST_PHRASE, Some("")).unwrap()
        );
    }

    #[test]
    fn secret_uri_phrase_is_the_mnemonic_seed() {
        let bare = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
        assert_eq!(master_seed_from_secret_uri(TEST_PHRASE).unwrap(), bare);
        // Spaces are in the phrase class; QUIP HD normalizes them away.
        assert_eq!(
            master_seed_from_secret_uri(&alloc::format!("  {TEST_PHRASE}  ")).unwrap(),
            bare
        );
        // An empty `///` is an empty passphrase.
        assert_eq!(
            master_seed_from_secret_uri(&alloc::format!("{TEST_PHRASE}///")).unwrap(),
            bare
        );
        // A `/` after `///` belongs to the password (design v2.2).
        assert_eq!(
            master_seed_from_secret_uri(&alloc::format!("{TEST_PHRASE}///p/w")).unwrap(),
            master_seed_from_mnemonic(TEST_PHRASE, Some("p/w")).unwrap()
        );
        assert_eq!(
            master_seed_from_secret_uri(&alloc::format!("{TEST_PHRASE}////w")).unwrap(),
            master_seed_from_mnemonic(TEST_PHRASE, Some("/w")).unwrap()
        );
    }

    #[test]
    fn secret_uri_raw_seed_takes_either_digit_case_and_no_password() {
        let upper = alloc::format!("0x{}", HEX_SEED.to_uppercase());
        assert_eq!(master_seed_from_secret_uri(&upper).unwrap(), [0x2a; 32]);
        for suri in [
            alloc::format!("0x{HEX_SEED}///"),
            alloc::format!("0x{HEX_SEED}///pw"),
        ] {
            assert_eq!(
                master_seed_from_secret_uri(&suri),
                Err(HybridTxCryptoError::InvalidPassword)
            );
        }
        for suri in [
            alloc::format!("0X{HEX_SEED}"),
            alloc::format!("0x{}", &HEX_SEED[..62]),
            alloc::format!("0x{HEX_SEED} "),
            alloc::format!("0x{HEX_SEED}2a"),
        ] {
            assert_eq!(
                master_seed_from_secret_uri(&suri),
                Err(HybridTxCryptoError::InvalidSeed),
                "{}",
                suri.len()
            );
        }
    }

    #[test]
    fn secret_uri_rejects_every_junction() {
        for suri in [
            alloc::format!("{TEST_PHRASE}//1"),
            alloc::format!("{TEST_PHRASE}//Alice"),
            alloc::format!("{TEST_PHRASE}/soft"),
            alloc::format!("{TEST_PHRASE}//"),
            alloc::format!("{TEST_PHRASE}//1///pw"),
            alloc::format!("0x{HEX_SEED}//1"),
            alloc::format!("0x{HEX_SEED}/1"),
            "//Alice".into(),
        ] {
            assert_eq!(
                master_seed_from_secret_uri(&suri),
                Err(HybridTxCryptoError::UnsupportedDerivationPath),
                "{suri}"
            );
        }
    }

    #[test]
    fn secret_uri_has_no_dev_phrase() {
        for suri in ["", "///pw", "///"] {
            assert_eq!(
                master_seed_from_secret_uri(suri),
                Err(HybridTxCryptoError::InvalidMnemonic),
                "{suri:?}"
            );
        }
    }

    #[test]
    fn secret_uri_phrase_class_is_ascii_letters_digits_and_spaces() {
        // The library entry point accepts what QUIP HD normalizes ...
        let tabbed = TEST_PHRASE.replacen(' ', "\t", 1);
        assert!(master_seed_from_mnemonic(&tabbed, None).is_ok());
        // ... the secret URI, like the SDK's, does not.
        for suri in [
            tabbed,
            TEST_PHRASE.replacen(' ', "\u{a0}", 1),
            TEST_PHRASE.replacen("fit", "\u{fb01}t", 1),
            alloc::format!("\"{TEST_PHRASE}\""),
            alloc::format!("{TEST_PHRASE}\n"),
        ] {
            assert_eq!(
                master_seed_from_secret_uri(&suri),
                Err(HybridTxCryptoError::InvalidMnemonic),
                "{suri:?}"
            );
        }
        // A password runs to the end of the URI and holds no line break.
        assert_eq!(
            master_seed_from_secret_uri(&alloc::format!("{TEST_PHRASE}///pw\nx")),
            Err(HybridTxCryptoError::InvalidPassword)
        );
    }

    /// The messages carry no part of the input, and the junction refusal
    /// points to the `0x` export (design §6).
    #[test]
    fn errors_name_the_problem_without_the_input() {
        for suri in [
            alloc::format!("{TEST_PHRASE}//1"),
            alloc::format!("\"{TEST_PHRASE}///Tr0ub4dor\""),
            alloc::format!("{TEST_PHRASE}///Tr0ub4dor\nx"),
            alloc::format!("0x{HEX_SEED}///Tr0ub4dor"),
            alloc::format!("0X{HEX_SEED}"),
        ] {
            let error = master_seed_from_secret_uri(&suri).unwrap_err();
            for text in [alloc::format!("{error}"), alloc::format!("{error:?}")] {
                for secret in ["bottom", "walk", "Tr0ub4dor", "2a2a"] {
                    assert!(!text.contains(secret), "{text} shows {secret}");
                }
            }
        }
        let message = alloc::format!("{}", HybridTxCryptoError::UnsupportedDerivationPath);
        assert!(message.contains("0x"), "{message}");
    }

    #[test]
    fn mnemonic_seed_signs_and_verifies() {
        let seed = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
        let envelope = sign_payload_from_seed(&seed, b"quip-message").unwrap();

        assert!(envelope.verify(b"quip-message"));
    }
}

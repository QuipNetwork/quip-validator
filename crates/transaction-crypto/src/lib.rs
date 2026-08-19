#![cfg_attr(not(feature = "std"), no_std)]

//! Transaction/account identity glue for Quip's hybrid runtime signer.
//!
//! This crate is intentionally small and policy-focused:
//! - it fixes the transaction signing scheme to H4 (`sr25519 + FN-DSA-512`)
//! - it derives compact 32-byte account ids from the hybrid public key
//! - it defines the transaction signature envelope that carries both:
//!   - the hybrid public key
//!   - the hybrid signature bytes
//!
//! It does not depend on FRAME or runtime code.
//!
//! # Security model
//!
//! Account ids are derived from the hybrid public key via
//! `blake2_256(ACCOUNT_ID_DOMAIN || hybrid_public_bytes)`. This relies on
//! blake2_256 collision resistance (~2^-128 birthday bound) to ensure two
//! distinct hybrid public keys cannot map to the same 32-byte [`AccountId32`].
//! A collision would credit the wrong owner on a verified signature — a
//! catastrophic silent failure the runtime cannot detect.
//!
//! Two invariants make this load-bearing assumption safe in this code:
//!
//! 1. [`ACCOUNT_ID_DOMAIN`] is a fixed byte string. Pinned by the
//!    `account_id_domain_is_pinned` test so a typo is caught at test time.
//! 2. [`HybridPublic`] has a fixed serialized length. This is what makes the
//!    unprefixed `domain || pubkey` concatenation unambiguous — see the
//!    comment on [`account_id_from_public`].
//!
//! Any refactor that truncates, projects, or otherwise reduces the entropy
//! of the hybrid public key input *will* silently weaken account-id collision
//! resistance. Do not change [`account_id_from_public`] without preserving
//! the property that the full pubkey bytes feed the hash.

use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use quip_crypto_primitives::substrate::sr25519_fndsa512;
use quip_transaction_crypto_core::{account_id_from_public_bytes, HybridTxSignatureBytes};
use scale_info::TypeInfo;
use sp_core::Pair as _;
use sp_runtime::{
    traits::{IdentifyAccount, Lazy, Verify},
    AccountId32,
};

/// Hybrid H4 public key used for transaction signing.
pub type HybridPublic = sr25519_fndsa512::Public;

/// Hybrid H4 signature bytes used for transaction signing.
pub type HybridSignatureBytes = sr25519_fndsa512::Signature;

/// Hybrid H4 pair used for transaction signing.
pub type HybridPair = sr25519_fndsa512::Pair;

/// Compact account id used by the runtime for transaction signers.
pub type DerivedAccountId = AccountId32;

/// Derives the compact runtime account id from the H4 hybrid public key.
///
/// The mapping is: `blake2_256(ACCOUNT_ID_DOMAIN || hybrid_public_bytes)`.
///
/// The domain separator is *not* length-prefixed; this is unambiguous only
/// because [`HybridPublic`] has a fixed serialized length. See the crate-level
/// security note for why this invariant is load-bearing.
///
/// The `Vec` allocation is bounded (one allocation, no reallocation) and is
/// negligible next to the FN-DSA-512 verification that follows on every signed
/// extrinsic. A stack-buffer alternative would require either a const
/// pubkey-length at this layer of the type stack (not currently available
/// from `quip-crypto-primitives`) or a streaming hasher (would force a
/// host-hashing-module change tracked separately).
pub fn account_id_from_public(public: &HybridPublic) -> DerivedAccountId {
    // After upstream commit `d125cbde` (polkadot-sdk v0.2), `Public` now
    // implements both `AsRef<[u8]>` and `AsRef<InnerPublic>`, so the bare
    // `public.as_ref()` is ambiguous; we explicitly select the byte slice.
    let pub_bytes: &[u8] = public.as_ref();
    DerivedAccountId::new(account_id_from_public_bytes(pub_bytes))
}

/// Signer identity wrapper for runtime transaction verification.
///
/// This newtype exists so the runtime can implement `IdentifyAccount` locally
/// while still using the underlying hybrid public key wrapper.
#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
)]
pub struct HybridTxPublic(pub HybridPublic);

impl From<HybridPublic> for HybridTxPublic {
    fn from(public: HybridPublic) -> Self {
        Self(public)
    }
}

impl From<HybridTxPublic> for HybridPublic {
    fn from(public: HybridTxPublic) -> Self {
        public.0
    }
}

impl AsRef<HybridPublic> for HybridTxPublic {
    fn as_ref(&self) -> &HybridPublic {
        &self.0
    }
}

impl IdentifyAccount for HybridTxPublic {
    type AccountId = DerivedAccountId;

    fn into_account(self) -> Self::AccountId {
        account_id_from_public(&self.0)
    }
}

/// Runtime transaction signature envelope.
///
/// Carries the full hybrid public key alongside the hybrid signature so
/// runtime verification can:
/// 1. verify the embedded public key derives the claimed `AccountId`
/// 2. verify the hybrid signature under the embedded public key
///
/// The two fields are *not* validated against each other at construction.
/// [`Verify::verify`] is the only path that establishes the relationship;
/// [`Self::new`] and the [`Decode`] path produce envelopes whose fields may
/// be unrelated until `verify` succeeds.
///
/// `MaxEncodedLen` is intentionally not derived because [`HybridSignatureBytes`]
/// (`sr25519_fndsa512::Signature`) does not yet implement it upstream, even
/// though its size is known at compile time via const generics. Add the
/// derive once the upstream gap is closed.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Encode, Decode, DecodeWithMemTracking, TypeInfo)]
pub struct HybridTxSignature {
    pub public: HybridPublic,
    pub signature: HybridSignatureBytes,
}

impl HybridTxSignature {
    /// Builds the envelope from explicit parts without validating that
    /// `signature` is a real signature under `public`.
    ///
    /// Use [`Self::sign`] to produce an envelope whose parts are guaranteed
    /// to be consistent; reserve `new` for deserialization shims and tests.
    pub fn new(public: HybridPublic, signature: HybridSignatureBytes) -> Self {
        Self { public, signature }
    }

    /// Signs the message with the given hybrid H4 pair and returns the full
    /// transaction signature envelope.
    ///
    /// `message` is signed exactly as given. The H4 domain prefix is applied
    /// intrinsically by the scheme (callers must not pre-apply it), and this
    /// does not hash long messages — applying Substrate's `SignedPayload`
    /// >256-byte `blake2_256` rule is the caller's responsibility.
    #[cfg(feature = "std")]
    pub fn sign(pair: &HybridPair, message: &[u8]) -> Self {
        let envelope =
            HybridTxSignatureBytes::new(pair.public().as_ref(), pair.sign(message).as_ref())
                .expect("pair-produced public/signature bytes are valid");

        Self {
            public: HybridPublic::decode(&mut &envelope.public[..])
                .expect("validated public bytes decode into runtime wrapper"),
            signature: HybridSignatureBytes::decode(&mut &envelope.signature[..])
                .expect("validated signature bytes decode into runtime wrapper"),
        }
    }

    /// Returns the derived compact account id for the embedded public key.
    pub fn derived_account_id(&self) -> DerivedAccountId {
        account_id_from_public(&self.public)
    }
}

impl Verify for HybridTxSignature {
    type Signer = HybridTxPublic;

    fn verify<L: Lazy<[u8]>>(
        &self,
        mut msg: L,
        signer: &<Self::Signer as IdentifyAccount>::AccountId,
    ) -> bool {
        let derived = self.derived_account_id();
        if derived != *signer {
            log::trace!(
                target: "quip::tx-verify",
                "account-id mismatch: claimed={signer:?} derived={derived:?}",
            );
            return false;
        }

        let ok = HybridPair::verify(&self.signature, msg.get(), &self.public);
        if !ok {
            log::trace!(
                target: "quip::tx-verify",
                "crypto verify failed for signer={signer:?}",
            );
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Decode;
    use quip_transaction_crypto_core::{
        master_seed_from_mnemonic, master_seed_from_secret_uri, public_key_from_seed,
        sign_payload_from_seed,
    };

    const TEST_PHRASE: &str =
        "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

    #[test]
    fn same_public_key_derives_same_account_id() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let first = account_id_from_public(&pair.public());
        let second = account_id_from_public(&pair.public());

        assert_eq!(first, second);
    }

    #[test]
    fn different_public_keys_derive_different_account_ids() {
        let alice = HybridPair::from_string("//Alice", None).unwrap();
        let bob = HybridPair::from_string("//Bob", None).unwrap();

        assert_ne!(
            account_id_from_public(&alice.public()),
            account_id_from_public(&bob.public())
        );
    }

    #[test]
    fn hybrid_tx_signature_verifies_for_matching_account() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let signature = HybridTxSignature::sign(&pair, b"quip-message");

        assert!(signature.verify(&b"quip-message"[..], &account_id));
    }

    #[test]
    fn runtime_signature_scale_matches_core_envelope() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let seed = pair.to_raw_vec();
        let message = b"quip-signer-fixture";

        let runtime_signature = HybridTxSignature::sign(&pair, message);
        let core_envelope = sign_payload_from_seed(&seed, message).unwrap();

        assert_eq!(runtime_signature.encode(), core_envelope.encode_envelope());
        assert_eq!(runtime_signature.public.encode(), core_envelope.public);
        assert_eq!(
            runtime_signature.signature.encode(),
            core_envelope.signature
        );
    }

    #[test]
    fn mnemonic_seed_matches_substrate_from_phrase() {
        let core_seed = master_seed_from_secret_uri(TEST_PHRASE).unwrap();
        let (pair, substrate_seed) = HybridPair::from_phrase(TEST_PHRASE, None).unwrap();

        // The browser-derived master seed must equal the node's master seed...
        assert_eq!(core_seed.to_vec(), substrate_seed.to_vec());
        // ...and therefore produce the same public key and account id.
        let public = public_key_from_seed(&core_seed).unwrap();
        assert_eq!(pair.public().encode(), public);
    }

    #[test]
    fn mnemonic_password_seed_matches_substrate() {
        let core_seed = master_seed_from_mnemonic(TEST_PHRASE, Some("hunter2")).unwrap();
        let (_pair, substrate_seed) =
            HybridPair::from_phrase(TEST_PHRASE, Some("hunter2")).unwrap();

        assert_eq!(core_seed.to_vec(), substrate_seed.to_vec());
    }

    #[test]
    fn secret_uri_password_matches_substrate_from_string() {
        let uri = format!("{TEST_PHRASE}///hunter2");
        let core_seed = master_seed_from_secret_uri(&uri).unwrap();
        let (_pair, substrate_seed) = HybridPair::from_string_with_seed(&uri, None).unwrap();

        assert_eq!(
            core_seed.to_vec(),
            substrate_seed
                .expect("phrase without junctions yields a seed")
                .to_vec()
        );
    }

    #[test]
    fn account_id_helper_matches_core_public_derivation() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let seed = pair.to_raw_vec();
        let public = public_key_from_seed(&seed).unwrap();

        assert_eq!(pair.public().encode(), public);
        assert_eq!(
            account_id_from_public(&pair.public()).as_ref(),
            account_id_from_public_bytes(&public)
        );
    }

    #[test]
    fn hybrid_tx_signature_rejects_wrong_account() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let wrong_pair = HybridPair::from_string("//Bob", None).unwrap();
        let wrong_account = account_id_from_public(&wrong_pair.public());
        let signature = HybridTxSignature::sign(&pair, b"quip-message");

        assert!(!signature.verify(&b"quip-message"[..], &wrong_account));
    }

    #[test]
    fn hybrid_tx_signature_rejects_wrong_message() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let signature = HybridTxSignature::sign(&pair, b"quip-message");

        assert!(!signature.verify(&b"wrong-message"[..], &account_id));
    }

    #[test]
    fn account_id_domain_is_pinned() {
        // Changing this string re-keys every account at genesis. If that's
        // really the intent, bump the version segment ("v1" -> "v2") in
        // lockstep with the migration plan.
        assert_eq!(
            quip_transaction_crypto_core::ACCOUNT_ID_DOMAIN,
            b"quip-account-v1"
        );
    }

    #[test]
    fn hybrid_tx_signature_rejects_tampered_signature_bytes() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let mut signature = HybridTxSignature::sign(&pair, b"quip-message");

        // SCALE-out, flip a byte, SCALE back in. The signature field is
        // opaque from this crate's perspective, so we mutate via the wire form.
        let mut encoded = signature.signature.encode();
        let mid = encoded.len() / 2;
        encoded[mid] ^= 0xFF;
        signature.signature =
            HybridSignatureBytes::decode(&mut &encoded[..]).expect("re-decode after byte flip");

        assert!(!signature.verify(&b"quip-message"[..], &account_id));
    }

    #[test]
    fn hybrid_tx_signature_rejects_tampered_public_bytes() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let original_account = account_id_from_public(&pair.public());
        let mut signature = HybridTxSignature::sign(&pair, b"quip-message");

        // Flip a byte in the embedded pubkey. The derived account id no
        // longer matches the original signer, so verify rejects on the
        // account-derivation branch (before crypto verify even runs).
        let mut encoded = signature.public.encode();
        encoded[0] ^= 0xFF;
        signature.public =
            HybridPublic::decode(&mut &encoded[..]).expect("re-decode after byte flip");

        assert!(!signature.verify(&b"quip-message"[..], &original_account));
    }

    #[test]
    fn hybrid_tx_signature_round_trips_through_scale() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let signature = HybridTxSignature::sign(&pair, b"quip-message");

        let encoded = signature.encode();
        let decoded = HybridTxSignature::decode(&mut &encoded[..]).expect("round-trip");

        assert_eq!(decoded, signature);
        assert!(decoded.verify(&b"quip-message"[..], &account_id));
    }

    #[test]
    fn hybrid_tx_signature_decode_rejects_empty_bytes() {
        let bytes: &[u8] = &[];
        assert!(HybridTxSignature::decode(&mut &bytes[..]).is_err());
    }

    #[test]
    fn hybrid_tx_signature_decode_rejects_truncated_bytes() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let signature = HybridTxSignature::sign(&pair, b"quip-message");
        let mut encoded = signature.encode();
        encoded.truncate(encoded.len() / 2);
        assert!(HybridTxSignature::decode(&mut &encoded[..]).is_err());
    }

    #[test]
    fn account_id_helper_matches_identify_account() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let public = pair.public();

        let direct = account_id_from_public(&public);
        let via_identify = HybridTxPublic(public).into_account();

        assert_eq!(direct, via_identify);
    }

    #[test]
    fn hybrid_tx_signature_rejects_same_length_different_content() {
        // The pre-existing "wrong-message" test compares "quip-message"
        // (12 bytes) vs "wrong-message" (13 bytes). Add a same-length but
        // bit-different replay to catch a hypothetical length-only check.
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let signature = HybridTxSignature::sign(&pair, b"AAAA-message");

        assert!(!signature.verify(&b"BBBB-message"[..], &account_id));
    }

    #[test]
    fn hybrid_tx_signature_verify_is_idempotent() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let signature = HybridTxSignature::sign(&pair, b"quip-message");

        assert!(signature.verify(&b"quip-message"[..], &account_id));
        assert!(signature.verify(&b"quip-message"[..], &account_id));
    }

    #[test]
    fn hybrid_tx_signature_handles_empty_message() {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let account_id = account_id_from_public(&pair.public());
        let signature = HybridTxSignature::sign(&pair, b"");

        assert!(signature.verify(&b""[..], &account_id));
    }
}

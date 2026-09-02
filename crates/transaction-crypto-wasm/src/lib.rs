//! Browser-WASM bindings for Quip transaction signing.
//!
//! The exported functions intentionally operate on hex strings. This keeps the
//! JavaScript boundary simple and mirrors the hex payloads used by polkadot-js
//! signers.

use quip_transaction_crypto_core::{
    account_id_from_public_bytes, master_seed_from_secret_uri, public_key_from_seed,
    sign_payload_from_seed as core_sign_payload_from_seed, HybridTxSignatureBytes, ACCOUNT_ID_LEN,
};
use wasm_bindgen::prelude::*;

fn strip_hex_prefix(value: &str) -> &str {
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value)
}

fn decode_hex(name: &str, value: &str) -> Result<Vec<u8>, String> {
    hex::decode(strip_hex_prefix(value)).map_err(|error| format!("{name} must be hex: {error}"))
}

fn encode_hex(bytes: impl AsRef<[u8]>) -> String {
    let mut encoded = String::from("0x");
    encoded.push_str(&hex::encode(bytes));
    encoded
}

fn crypto_error(context: &str, error: impl core::fmt::Debug) -> String {
    format!("{context}: {error:?}")
}

fn public_from_seed_impl(seed_hex: &str) -> Result<String, String> {
    let seed = decode_hex("seed", seed_hex)?;
    let public =
        public_key_from_seed(&seed).map_err(|error| crypto_error("invalid seed", error))?;

    Ok(encode_hex(public))
}

fn account_id_from_public_impl(public_hex: &str) -> Result<String, String> {
    let public = decode_hex("public", public_hex)?;

    Ok(encode_hex(account_id_from_public_bytes(&public)))
}

fn seed_from_mnemonic_impl(secret_uri: &str) -> Result<String, String> {
    let seed = master_seed_from_secret_uri(secret_uri)
        .map_err(|error| crypto_error("invalid mnemonic", error))?;

    Ok(encode_hex(seed))
}

fn sign_payload_from_seed_impl(seed_hex: &str, payload_hex: &str) -> Result<String, String> {
    let seed = decode_hex("seed", seed_hex)?;
    let payload = decode_hex("payload", payload_hex)?;
    let envelope = core_sign_payload_from_seed(&seed, &payload)
        .map_err(|error| crypto_error("signing failed", error))?;

    Ok(encode_hex(envelope.encode_envelope()))
}

fn verify_envelope_impl(
    payload_hex: &str,
    envelope_hex: &str,
    account_id_hex: &str,
) -> Result<bool, String> {
    let payload = decode_hex("payload", payload_hex)?;
    let envelope_bytes = decode_hex("envelope", envelope_hex)?;
    let account_id = decode_hex("accountId", account_id_hex)?;

    if account_id.len() != ACCOUNT_ID_LEN {
        return Err(String::from("accountId must decode to 32 bytes"));
    }

    let envelope = HybridTxSignatureBytes::decode_envelope(&envelope_bytes)
        .map_err(|error| crypto_error("invalid envelope", error))?;

    Ok(
        envelope.derived_account_id().as_slice() == account_id.as_slice()
            && envelope.verify(&payload),
    )
}

/// Derive serialized H4 public key bytes from a 32-byte H4 seed.
#[wasm_bindgen(js_name = publicFromSeed)]
pub fn public_from_seed(seed_hex: &str) -> Result<String, JsValue> {
    public_from_seed_impl(seed_hex).map_err(|error| JsValue::from_str(&error))
}

/// Derive the compact 32-byte Quip account id from serialized H4 public bytes.
#[wasm_bindgen(js_name = accountIdFromPublic)]
pub fn account_id_from_public(public_hex: &str) -> Result<String, JsValue> {
    account_id_from_public_impl(public_hex).map_err(|error| JsValue::from_str(&error))
}

/// Derive the 32-byte H4 master seed from a limited secret URI.
///
/// Accepts a `0x`-prefixed 64-digit hex seed, or an English BIP39 phrase
/// optionally followed by `///<password>`. Derivation junctions (`//`, `/`) are
/// rejected. The returned seed hex can be passed to [`public_from_seed`] and
/// [`sign_payload_from_seed`].
#[wasm_bindgen(js_name = seedFromMnemonic)]
pub fn seed_from_mnemonic(secret_uri: &str) -> Result<String, JsValue> {
    seed_from_mnemonic_impl(secret_uri).map_err(|error| JsValue::from_str(&error))
}

/// Sign SCALE-encoded transaction payload bytes and return a SCALE-encoded
/// `HybridTxSignature` envelope.
///
/// The payload hex is signed **exactly as given**: no hashing, no length check.
/// The H4 domain prefix (`0x01 || "hybrid-sr25519-falcon512-v1\0" || ...`) is
/// applied intrinsically by the scheme, so callers must not pre-apply it.
/// Substrate's `SignedPayload` rule — `blake2_256(payload)` when the encoded
/// payload exceeds 256 bytes — is an extrinsic convention and the caller's
/// responsibility; the higher-level `QuipSigner` (`signRaw`) applies it, so
/// callers using that path get it for free.
#[wasm_bindgen(js_name = signPayloadFromSeed)]
pub fn sign_payload_from_seed(seed_hex: &str, payload_hex: &str) -> Result<String, JsValue> {
    sign_payload_from_seed_impl(seed_hex, payload_hex).map_err(|error| JsValue::from_str(&error))
}

/// Verify a SCALE-encoded `HybridTxSignature` envelope for the given payload
/// and compact account id.
#[wasm_bindgen(js_name = verifyEnvelope)]
pub fn verify_envelope(
    payload_hex: &str,
    envelope_hex: &str,
    account_id_hex: &str,
) -> Result<bool, JsValue> {
    verify_envelope_impl(payload_hex, envelope_hex, account_id_hex)
        .map_err(|error| JsValue::from_str(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED_HEX: &str = "0x0707070707070707070707070707070707070707070707070707070707070707";
    const PAYLOAD_HEX: &str = "0x717569702d7369676e65722d66697874757265";

    #[test]
    fn signs_and_verifies_envelope() {
        let public = public_from_seed_impl(SEED_HEX).unwrap();
        let account = account_id_from_public_impl(&public).unwrap();
        let envelope = sign_payload_from_seed_impl(SEED_HEX, PAYLOAD_HEX).unwrap();

        assert!(verify_envelope_impl(PAYLOAD_HEX, &envelope, &account).unwrap());
        assert!(!verify_envelope_impl("0x77726f6e67", &envelope, &account).unwrap());
    }

    #[test]
    fn account_id_rejects_non_hex_public() {
        assert!(account_id_from_public_impl("not-hex").is_err());
    }

    const TEST_PHRASE: &str =
        "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

    #[test]
    fn mnemonic_derives_signable_seed() {
        let seed = seed_from_mnemonic_impl(TEST_PHRASE).unwrap();
        let public = public_from_seed_impl(&seed).unwrap();
        let account = account_id_from_public_impl(&public).unwrap();
        let envelope = sign_payload_from_seed_impl(&seed, PAYLOAD_HEX).unwrap();

        assert!(verify_envelope_impl(PAYLOAD_HEX, &envelope, &account).unwrap());
    }

    #[test]
    fn mnemonic_rejects_derivation_junctions() {
        assert!(seed_from_mnemonic_impl(&format!("{TEST_PHRASE}//0")).is_err());
    }
}

//! Golden-vector parity gate.
//!
//! `golden_vectors.txt` is generated from the signer's H4 byte output by the
//! `generate_golden_vectors` example. This test pins those exact bytes so the
//! suite that the browser signs with and the suite the runtime verifies with
//! can never silently drift apart.
//!
//! Vectors cover `seed -> public_key` and `(seed, msg) -> signature envelope`
//! for several fixed seeds plus a BIP39-derived seed.

use pqhybridsign::H4;
use pqhybridsign::{composite_delta, suite::DeltaSuite};
use quip_transaction_crypto_core::{
    master_seed_from_mnemonic, public_key_from_seed, sign_payload_from_seed, HYBRID_SIGNATURE_LEN,
    SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
};

const FIXTURE: &str = include_str!("golden_vectors.txt");

const TEST_PHRASE: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

fn lookup<'a>(key: &str) -> &'a str {
    for line in FIXTURE.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, value) = line.split_once('=').expect("fixture line is `name=value`");
        if name == key {
            // Leak-free: FIXTURE is 'static, so the slice is too.
            return value;
        }
    }
    panic!("fixture missing key `{key}`");
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn assert_public(name: &str, seed: &[u8]) {
    // Sanity: the recorded seed matches what we feed in.
    assert_eq!(
        to_hex(seed),
        lookup(&format!("{name}_seed")),
        "{name}: seed bytes drifted from fixture"
    );
    let public = public_key_from_seed(seed).expect("public derivation");
    assert_eq!(
        to_hex(&public),
        lookup(&format!("{name}_public")),
        "{name}: public key drifted from H4 golden bytes"
    );
}

fn assert_envelope(seed_name: &str, seed: &[u8], msg_name: &str, msg: &[u8]) {
    let envelope = sign_payload_from_seed(seed, msg).expect("signing");
    assert_eq!(
        to_hex(&envelope.encode_envelope()),
        lookup(&format!("{seed_name}_{msg_name}_envelope")),
        "{seed_name}/{msg_name}: signature envelope drifted from H4 golden bytes"
    );
}

#[test]
fn public_keys_match_h4_golden_vectors() {
    assert_public("seed_01", &[1u8; 32]);
    assert_public("seed_07", &[7u8; 32]);
    assert_public("seed_09", &[9u8; 32]);
    assert_public("seed_11", &[11u8; 32]);

    let bip39 = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
    assert_public("bip39", &bip39);

    let bip39_pw = master_seed_from_mnemonic(TEST_PHRASE, Some("hunter2")).unwrap();
    assert_public("bip39_pw", &bip39_pw);
}

#[test]
fn signature_envelopes_match_h4_golden_vectors() {
    let messages: [(&str, &[u8]); 3] = [
        ("msg_quip", b"quip-message"),
        ("msg_empty", b""),
        ("msg_fixture", b"quip-signer-fixture"),
    ];

    let bip39 = master_seed_from_mnemonic(TEST_PHRASE, None).unwrap();
    for (mname, msg) in &messages {
        assert_envelope("seed_07", &[7u8; 32], mname, msg);
        assert_envelope("bip39", &bip39, mname, msg);
    }
}

#[test]
fn transaction_core_matches_pqhybridsign_h4() {
    let seed = [42u8; 32];
    let message = b"golden vector";
    let mut direct_secret = vec![0u8; H4::SECRET_KEY_LEN];
    let mut direct_public = vec![0u8; H4::PUBLIC_KEY_LEN];
    composite_delta::keypair_from_seed::<H4>(&seed, &mut direct_secret, &mut direct_public)
        .expect("direct H4 key generation");

    assert_eq!(
        public_key_from_seed(&seed).unwrap().as_slice(),
        direct_public
    );

    let mut direct_signature = [0u8; HYBRID_SIGNATURE_LEN];
    let wire_len = composite_delta::sign_deterministic::<H4>(
        &direct_secret,
        message,
        SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
        b"",
        &mut direct_signature,
    )
    .expect("direct H4 signing");
    assert!(composite_delta::verify::<H4>(
        &direct_public,
        message,
        SUBSTRATE_PAIR_SIGNATURE_CONTEXT,
        &direct_signature[..wire_len],
    ));

    let envelope = sign_payload_from_seed(&seed, message).unwrap();
    assert_eq!(envelope.public.as_slice(), direct_public);
    assert_eq!(envelope.signature, direct_signature);
}

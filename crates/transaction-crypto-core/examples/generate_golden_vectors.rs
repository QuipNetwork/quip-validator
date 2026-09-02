use codec::Encode;
use quip_transaction_crypto_core::{
    master_seed_from_mnemonic, public_key_from_seed, sign_payload_from_seed,
};

const TEST_PHRASE: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn print_public(name: &str, seed: &[u8]) {
    println!("{name}_seed={}", hex(seed));
    println!(
        "{name}_public={}",
        hex(&public_key_from_seed(seed).expect("public derivation"))
    );
}

fn print_envelope(seed_name: &str, seed: &[u8], message_name: &str, message: &[u8]) {
    let envelope = sign_payload_from_seed(seed, message).expect("signing");
    println!(
        "{seed_name}_{message_name}_envelope={}",
        hex(&envelope.encode())
    );
}

fn main() {
    print_public("seed_01", &[1u8; 32]);
    print_public("seed_07", &[7u8; 32]);
    print_public("seed_09", &[9u8; 32]);
    print_public("seed_11", &[11u8; 32]);

    let bip39 = master_seed_from_mnemonic(TEST_PHRASE, None).expect("BIP39 seed");
    print_public("bip39", &bip39);
    let bip39_pw =
        master_seed_from_mnemonic(TEST_PHRASE, Some("hunter2")).expect("BIP39 password seed");
    print_public("bip39_pw", &bip39_pw);

    let messages: [(&str, &[u8]); 3] = [
        ("msg_quip", b"quip-message"),
        ("msg_empty", b""),
        ("msg_fixture", b"quip-signer-fixture"),
    ];
    for (message_name, message) in messages {
        print_envelope("seed_07", &[7u8; 32], message_name, message);
        print_envelope("bip39", &bip39, message_name, message);
    }
}

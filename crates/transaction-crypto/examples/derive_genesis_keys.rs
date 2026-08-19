//! Derive hybrid genesis public material (BABE, GRANDPA, TX account) from a
//! single seed URI. Use this to produce the per-operator public bytes that get
//! committed into the `quip_testnet` genesis preset.
//!
//! Generate a fresh BIP39 mnemonic with:
//!     ./target/release/quip-network-node key generate
//! Then feed the value printed after `Secret phrase:` as the argument here:
//!     cd crates/transaction-crypto
//!     cargo run --example derive_genesis_keys --features std -- "<mnemonic>"
//!
//! The URI you feed in is the secret. Never paste it into commits or chat —
//! only the printed `*_pub` / `tx_account_*` lines are safe to share.

use codec::Encode;
use quip_crypto_primitives::substrate::{ed25519_fndsa512, sr25519_fndsa512};
use quip_transaction_crypto::account_id_from_public;
use sp_core::{crypto::Ss58Codec, Pair as _};

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn main() {
    let uri = match std::env::args().nth(1) {
        Some(s) => s,
        None => {
            eprintln!(
                "usage: cargo run --example derive_genesis_keys --features std -- <SEED-URI>\n\n\
                 generate a fresh BIP39 mnemonic with:\n\
                 \t./target/release/quip-network-node key generate\n\
                 and pass the 'Secret phrase' value as <SEED-URI>."
            );
            std::process::exit(2);
        }
    };

    // Surface `SecretStringError` text to the operator instead of panicking
    // with a backtrace — this binary's only caller is `scripts/derive-operator-keys.sh`,
    // which captures stderr and shows it as the failure reason. A clean
    // message ("invalid checksum") is far more actionable than a panic trace.
    let babe = sr25519_fndsa512::Pair::from_string(&uri, None).unwrap_or_else(|e| {
        eprintln!("invalid SURI for hybrid-babe-h444: {e:?}");
        std::process::exit(2);
    });
    let grandpa = ed25519_fndsa512::Pair::from_string(&uri, None).unwrap_or_else(|e| {
        eprintln!("invalid SURI for hybrid-grandpa-h244: {e:?}");
        std::process::exit(2);
    });

    let babe_pub = babe.public();
    let grandpa_pub = grandpa.public();
    let tx_account = account_id_from_public(&babe_pub);

    println!("# Submit these public values back to the release coordinator.");
    println!("# Keep the URI you fed in offline — never share it.");
    println!();
    println!("babe_pub        = 0x{}", hex(babe_pub.as_ref()));
    println!("grandpa_pub     = 0x{}", hex(grandpa_pub.as_ref()));
    println!("tx_account_ss58 = {}", tx_account.to_ss58check());
    println!("tx_account_hex  = 0x{}", hex(&tx_account.encode()));
}

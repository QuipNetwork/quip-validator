//! Prints the development accounts' H4 transaction seeds and account ids as
//! `name_seed=0x…` / `name_account=0x…` lines.
//!
//! The values come from the SDK fork's `Pair::from_string_with_seed("//Name")`
//! (QUIP HD v1 on the development phrase), the same derivation the runtime's
//! dev presets use. `scripts/regenerate-quip-hd-fixtures.sh` reads them to
//! refresh the JS signer seeds and the pinned Alice account.

use quip_transaction_crypto::{account_id_from_public, HybridPair};
use sp_core::Pair;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() {
    for name in ["Alice", "Bob"] {
        let (pair, seed) = HybridPair::from_string_with_seed(&format!("//{name}"), None)
            .expect("the development SURIs parse");
        let seed = seed.expect("a hybrid pair returns its seed");
        let account = account_id_from_public(&pair.public());
        let key = name.to_lowercase();
        println!("{key}_seed=0x{}", hex(&seed));
        println!(
            "{key}_account=0x{}",
            hex(<_ as AsRef<[u8]>>::as_ref(&account))
        );
    }
}

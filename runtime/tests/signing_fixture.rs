#[path = "support/signing_fixture.rs"]
mod signing_fixture_support;

use codec::Decode;
use quip_protocol_runtime::{Runtime, RuntimeGenesisConfig, System, UncheckedExtrinsic};
use serde_json::Value;
use sp_runtime::transaction_validity::InvalidTransaction;
use sp_runtime::{traits::Checkable, BuildStorage};

fn fixture_extrinsic_bytes() -> Vec<u8> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../docs/polkadotjs/fixtures/hybrid-signing.json"
    ))
    .expect("checked-in fixture is valid JSON");
    let encoded = fixture["signedExtrinsicHex"]
        .as_str()
        .expect("signed extrinsic is a string");

    hex::decode(encoded.trim_start_matches("0x")).expect("extrinsic is hex")
}

fn compact_prefix_len(encoded: &[u8]) -> usize {
    match encoded[0] & 0b11 {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 5 + usize::from(encoded[0] >> 2),
    }
}

fn check_fixture_extrinsic(bytes: &[u8]) -> sp_runtime::transaction_validity::TransactionValidity {
    let extrinsic =
        UncheckedExtrinsic::decode(&mut &bytes[..]).expect("signed extrinsic SCALE-decodes");
    let lookup = frame_system::ChainContext::<Runtime>::default();

    extrinsic.check(&lookup).map(|_| Default::default())
}

#[test]
fn checked_in_fixture_matches_rust_generator() {
    let checked_in: Value = serde_json::from_str(include_str!(
        "../../docs/polkadotjs/fixtures/hybrid-signing.json"
    ))
    .expect("checked-in fixture is valid JSON");

    assert_eq!(checked_in, signing_fixture_support::generate_fixture());
}

#[test]
fn fixture_signed_extrinsic_decodes_and_validates() {
    let bytes = fixture_extrinsic_bytes();

    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    ext.execute_with(|| {
        System::set_block_number(1);

        assert!(check_fixture_extrinsic(&bytes).is_ok());
    });
}

#[test]
fn fixture_rejects_tampered_envelope_as_bad_proof() {
    let mut bytes = fixture_extrinsic_bytes();
    // Envelope layout is `{ public: [u8; 1344], signature: [u8; 2484] }`.
    // Corrupt the *signature* half so the derived account still matches and
    // the failure must come from cryptographic verification, not the
    // account-mismatch guard covered by the test below.
    let envelope_offset = compact_prefix_len(&bytes) + 1 + 1 + 32;
    bytes[envelope_offset + 1344 + 100] ^= 0xff;

    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    ext.execute_with(|| {
        System::set_block_number(1);

        assert_eq!(
            check_fixture_extrinsic(&bytes).unwrap_err(),
            InvalidTransaction::BadProof.into()
        );
    });
}

#[test]
fn fixture_rejects_mismatched_account_as_bad_proof() {
    let mut bytes = fixture_extrinsic_bytes();
    let address_offset = compact_prefix_len(&bytes) + 1 + 1;
    bytes[address_offset..address_offset + 32].fill(0x99);

    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    ext.execute_with(|| {
        System::set_block_number(1);

        assert_eq!(
            check_fixture_extrinsic(&bytes).unwrap_err(),
            InvalidTransaction::BadProof.into()
        );
    });
}

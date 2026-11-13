#[path = "support/signing_fixture.rs"]
mod signing_fixture_support;

use codec::Decode;
use quip_testnet_runtime::{Runtime, RuntimeGenesisConfig, System, UncheckedExtrinsic};
use serde_json::Value;
use sp_runtime::transaction_validity::InvalidTransaction;
use sp_runtime::{traits::Checkable, BuildStorage};

fn fixture_extrinsic_bytes() -> Vec<u8> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/hybrid-signing.json"))
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
    let checked_in: Value = serde_json::from_str(include_str!("fixtures/hybrid-signing.json"))
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
    // Envelope layout is `{ public: [u8; 929], signature: [u8; 731] }`.
    // Corrupt the *signature* half so the derived account still matches and
    // the failure must come from cryptographic verification, not the
    // account-mismatch guard covered by the test below.
    let envelope_offset = compact_prefix_len(&bytes) + 1 + 1 + 32;
    bytes[envelope_offset + 929 + 100] ^= 0xff;

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

#[test]
fn ethereum_wrappers_reject_nonzero_versions_but_preserve_signed_payloads() {
    use codec::{Compact, Encode};
    use frame_support::traits::fungible::Mutate;
    use quip_testnet_runtime::{Balances, Executive};
    use sp_runtime::transaction_validity::TransactionSource;

    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/ethereum-envelopes.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3, "legacy, EIP-2930 and EIP-1559 samples");
    for case in cases {
        let captured = hex::decode(case["extrinsic_hex"].as_str().unwrap()).unwrap();
        let mut body = captured.as_slice();
        let Compact(length) = Compact::<u32>::decode(&mut body).unwrap();
        assert_eq!(length as usize, body.len());
        // Captured V5 general/version 1, followed by the four zero bytes of
        // the old extension tuple. The remainder is the unchanged eth_transact.
        assert_eq!(&body[..6], &[0x45, 1, 0, 0, 0, 0]);

        let version_offset = compact_prefix_len(&captured) + 1;
        for version in [1, 255] {
            let mut bytes = captured.clone();
            bytes[version_offset] = version;
            let error = UncheckedExtrinsic::decode(&mut bytes.as_slice())
                .unwrap_err()
                .to_string();
            assert!(error.contains("Invalid extension version"), "{error}");
        }

        // Rewrap captured bytes only: never generate or re-sign the inner tx.
        let mut bare = vec![0x05];
        bare.extend_from_slice(&body[6..]);
        let mut v0 = body.to_vec();
        v0[1] = 0;
        for wrapper in [bare, v0] {
            let mut bytes = Compact(u32::try_from(wrapper.len()).unwrap()).encode();
            bytes.extend(wrapper);
            let mut input = bytes.as_slice();
            let extrinsic = UncheckedExtrinsic::decode(&mut input).unwrap();
            assert!(input.is_empty());
            assert_eq!(extrinsic.0.function.encode(), body[6..]);

            let mut ext = sp_io::TestExternalities::new(
                RuntimeGenesisConfig::default().build_storage().unwrap(),
            );
            ext.execute_with(|| {
                System::set_block_number(1);
                // Fund the captured fixture's signer without signing anything.
                let account = pallet_revive::evm::Account::from_secret_key([7; 32]);
                Balances::set_balance(
                    &account.substrate_account(),
                    1_000_000_000_000_000_000_000_000,
                );
                extrinsic
                    .clone()
                    .check(&frame_system::ChainContext::<Runtime>::default())
                    .expect("same signed Ethereum payload checks in bare/V0 form");
                Executive::validate_transaction(
                    TransactionSource::External,
                    extrinsic,
                    Default::default(),
                )
                .expect("same signed Ethereum payload validates in bare/V0 form");
            });
        }
    }
}

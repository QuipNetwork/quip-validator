use codec::Encode;
use quip_protocol_runtime::{
    native_tx_extension, Address, RuntimeCall, RuntimeGenesisConfig, SignedPayload, System,
    SystemCall, UncheckedExtrinsic, VERSION,
};
use quip_transaction_crypto::{
    account_id_from_public, DerivedAccountId, HybridPair, HybridTxSignature,
};
use quip_transaction_crypto_core::sign_payload_from_seed;
use serde_json::{json, Value};
use sp_core::{
    blake2_256,
    crypto::{Ss58AddressFormat, Ss58Codec},
    Pair as _,
};
use sp_runtime::{generic, BuildStorage};

pub const FIXTURE_SEED: [u8; 32] = [7u8; 32];
pub const ENVELOPE_LEN: usize = 929 + 731;

fn hex(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for byte in bytes {
        use core::fmt::Write as _;
        write!(out, "{byte:02x}").expect("writing to String cannot fail");
    }
    out
}

fn message_to_sign(payload: &[u8]) -> Vec<u8> {
    if payload.len() > 256 {
        blake2_256(payload).to_vec()
    } else {
        payload.to_vec()
    }
}

fn account_address(account_id: &DerivedAccountId) -> String {
    account_id.to_ss58check_with_version(Ss58AddressFormat::custom(42))
}

pub fn generate_fixture() -> Value {
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

    ext.execute_with(|| {
        System::set_block_number(1);

        let pair = HybridPair::from_seed_slice(&FIXTURE_SEED).expect("fixture seed is valid");
        let public = pair.public();
        let account_id = account_id_from_public(&public);
        let address = account_address(&account_id);
        let call: RuntimeCall = SystemCall::remark {
            remark: b"quip-signer-fixture".to_vec(),
        }
        .into();
        let tx_extension = native_tx_extension(generic::Era::Immortal, 0, 0);
        let payload =
            SignedPayload::new(call.clone(), tx_extension.clone()).expect("payload is valid");
        let raw_payload = payload.encode();
        let actual_message = message_to_sign(&raw_payload);
        let envelope =
            payload.using_encoded(|encoded| HybridTxSignature::sign(&pair, encoded).encode());
        let core_envelope = sign_payload_from_seed(&FIXTURE_SEED, &actual_message)
            .expect("core signing succeeds")
            .encode_envelope();

        assert_eq!(envelope, core_envelope);
        assert_eq!(envelope.len(), ENVELOPE_LEN);

        let signature = payload.using_encoded(|encoded| HybridTxSignature::sign(&pair, encoded));
        let extrinsic: UncheckedExtrinsic = generic::UncheckedExtrinsic::new_signed(
            call.clone(),
            Address::Id(account_id.clone()),
            signature,
            tx_extension,
        )
        .into();

        let payload_cases = [255usize, 256, 257]
            .into_iter()
            .map(|length| {
                let raw = vec![length as u8; length];
                let message = message_to_sign(&raw);
                let case_envelope = sign_payload_from_seed(&FIXTURE_SEED, &message)
                    .expect("fixture signing succeeds")
                    .encode_envelope();

                json!({
                    "length": length,
                    "rawPayloadHex": hex(&raw),
                    "messageToSignHex": hex(&message),
                    "signatureEnvelopeHex": hex(case_envelope),
                })
            })
            .collect::<Vec<_>>();

        json!({
            "schemaVersion": 1,
            "seedHex": hex(FIXTURE_SEED),
            "publicKeyHex": hex(public.encode()),
            "accountIdHex": hex(<DerivedAccountId as AsRef<[u8]>>::as_ref(&account_id)),
            "ss58Address": address,
            "envelopeLength": ENVELOPE_LEN,
            "signerPayload": {
                "address": account_address(&account_id),
                "blockHash": hex([0u8; 32]),
                "blockNumber": "0x00000000",
                "era": "0x00",
                "genesisHash": hex([0u8; 32]),
                "method": hex(call.encode()),
                "nonce": "0x00000000",
                "signedExtensions": [
                    "AuthorizeCall",
                    "CheckNonZeroSender",
                    "CheckSpecVersion",
                    "CheckTxVersion",
                    "CheckGenesis",
                    "CheckMortality",
                    "CheckNonce",
                    "CheckWeight",
                    "ChargeTransactionPayment",
                    "CheckMetadataHash",
                    "EthSetOrigin",
                    "WeightReclaim"
                ],
                "specVersion": format!("0x{:08x}", VERSION.spec_version),
                "tip": "0x00000000000000000000000000000000",
                "transactionVersion": format!("0x{:08x}", VERSION.transaction_version),
                "version": 4
            },
            "rawSigningPayloadHex": hex(raw_payload),
            "actualMessageToSignHex": hex(actual_message),
            "signatureEnvelopeHex": hex(envelope),
            "signedExtrinsicHex": hex(extrinsic.encode()),
            "payloadBoundaryCases": payload_cases
        })
    })
}

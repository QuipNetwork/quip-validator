//! Assertions over shared wire fixtures using stock subxt's dynamic decoder.
//! No Quip runtime account, signature, call, extension or event decoding types.

use codec::{Compact, Decode, Encode};
use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
use serde_json::Value as Json;
use std::{fs, path::PathBuf};
use subxt_core::{
    blocks::Extrinsics,
    config::{
        substrate::{BlakeTwo256, SubstrateConfig, SubstrateHeader},
        Header as _,
    },
    events::{Events, Phase},
    ext::scale_value::{self, At, Value, ValueDef},
    Metadata,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/fixtures/metadata-conformance")
}

fn text(value: &Json) -> &str {
    value.as_str().expect("fixture string")
}

fn bytes(value: &Json) -> Vec<u8> {
    hex::decode(text(value).trim_start_matches("0x")).expect("fixture hex")
}

fn sha256(data: &[u8]) -> String {
    hex::encode(sp_core::hashing::sha2_256(data))
}

fn decode_value(data: &[u8], ty: u32, metadata: &Metadata) -> Value<u32> {
    let mut cursor = data;
    let value = scale_value::scale::decode_as_type(&mut cursor, ty, metadata.types())
        .expect("stock dynamic decoder accepts the complete type");
    assert!(cursor.is_empty(), "type must consume all its bytes");
    assert_eq!(encode_value(&value, metadata), data, "dynamic round trip");
    value
}

fn encode_value(value: &Value<u32>, metadata: &Metadata) -> Vec<u8> {
    let mut encoded = Vec::new();
    scale_value::scale::encode_as_type(value, value.context, metadata.types(), &mut encoded)
        .expect("decoded value re-encodes from metadata");
    encoded
}

/// Flatten only byte arrays and composite/newtype wrappers, never variants.
fn byte_leaves(value: &Value<u32>) -> Vec<u8> {
    match &value.value {
        ValueDef::Composite(fields) => fields.values().flat_map(byte_leaves).collect(),
        ValueDef::Primitive(_) => {
            vec![u8::try_from(value.as_u128().expect("u8 leaf")).expect("byte is in range")]
        }
        other => panic!("expected byte array/newtype, got {other:?}"),
    }
}

fn variant(value: &Value<u32>, name: &str) {
    let ValueDef::Variant(v) = &value.value else {
        panic!("expected variant {name}");
    };
    assert_eq!(v.name, name);
}

fn id_address(value: &Value<u32>) -> Vec<u8> {
    variant(value, "Id");
    byte_leaves(value.at(0).expect("Id account field"))
}

fn unwrap_single(mut value: &Value<u32>) -> &Value<u32> {
    while let ValueDef::Composite(fields) = &value.value {
        if fields.len() != 1 {
            break;
        }
        value = fields.values().next().unwrap();
    }
    value
}

pub fn assert_all_fixtures(expected_extensions: &[&str]) {
    let signing: Json = serde_json::from_str(include_str!(
        "../../../docs/polkadotjs/fixtures/hybrid-signing.json"
    ))
    .unwrap();
    let mut totals = (0, 0, 0, 0); // blocks, bare, signed, transfer events
    for directory in ["aglais-117-118", "local-119"] {
        let directory = root().join(directory);
        let manifest: Json =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        // Consumer tests fail if any shared source blob silently drifts.
        for (file, entry) in manifest["files"].as_object().unwrap() {
            let data = fs::read(directory.join(file)).unwrap();
            assert_eq!(data.len() as u64, entry["bytes"].as_u64().unwrap());
            assert_eq!(sha256(&data), text(&entry["sha256"]));
        }
        for block in manifest["blocks"].as_array().unwrap() {
            totals.0 += 1;
            let number = block["number"].as_u64().unwrap();
            let (execution_spec, post_spec) = match number {
                330109 => (117, 118),
                330110 | 591863 | 591865 => (118, 118),
                107 => (119, 119),
                _ => panic!("new fixture requires explicit version/outcome expectations"),
            };
            assert_eq!(block["executionSpecVersion"], execution_spec);
            assert_eq!(block["postStateSpecVersion"], post_spec);
            assert_eq!(block["transactionVersion"], 7);
            assert_eq!(
                manifest["files"][text(&block["metadataFile"])]["params"],
                serde_json::json!([block["metadataStateHash"]])
            );
            let metadata_bytes = fs::read(directory.join(text(&block["metadataFile"]))).unwrap();
            let metadata = subxt_core::metadata::decode_from(&metadata_bytes).unwrap();
            let prefixed = RuntimeMetadataPrefixed::decode(&mut &metadata_bytes[..]).unwrap();
            let RuntimeMetadata::V16(v16) = prefixed.1 else {
                panic!("fixture metadata must be V16");
            };
            assert_eq!(v16.extrinsic.versions, [4, 5]);
            assert_eq!(
                v16.extrinsic
                    .transaction_extensions_by_version
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                [0]
            );
            let rpc: Json = serde_json::from_slice(
                &fs::read(directory.join(text(&block["blockFile"]))).unwrap(),
            )
            .unwrap();
            let header: SubstrateHeader<u32, BlakeTwo256> =
                serde_json::from_value(rpc["block"]["header"].clone()).unwrap();
            assert_eq!(header.number as u64, number);
            assert_eq!(
                header.hash_with(BlakeTwo256).as_bytes(),
                bytes(&block["hash"])
            );
            assert_eq!(
                rpc["block"]["header"]["parentHash"],
                block["metadataStateHash"]
            );
            let raw: Vec<_> = rpc["block"]["extrinsics"]
                .as_array()
                .unwrap()
                .iter()
                .map(bytes)
                .collect();
            let extrinsics =
                Extrinsics::<SubstrateConfig>::decode_from(raw.clone(), metadata.clone()).unwrap();
            let expected = block["extrinsics"].as_array().unwrap();
            assert_eq!(extrinsics.len(), raw.len());
            assert_eq!(extrinsics.len(), expected.len());
            for (index, (extrinsic, want)) in extrinsics.iter().zip(expected).enumerate() {
                assert_eq!(extrinsic.index() as usize, index);
                assert_eq!(want["index"].as_u64().unwrap() as usize, index);
                assert_eq!(extrinsic.bytes(), raw[index]);
                let mut format_cursor = extrinsic.bytes();
                Compact::<u32>::decode(&mut format_cursor).unwrap();
                assert_eq!(
                    u8::decode(&mut format_cursor).unwrap() as u64,
                    want["version"].as_u64().unwrap()
                );
                assert_eq!(extrinsic.hash().as_bytes(), bytes(&want["hash"]));
                assert_eq!(extrinsic.is_signed(), want["signed"].as_bool().unwrap());
                assert_eq!(
                    [extrinsic.pallet_index(), extrinsic.variant_index()].to_vec(),
                    bytes(&want["callIndexHex"])
                );
                assert_eq!(sha256(extrinsic.call_bytes()), text(&want["callSha256"]));
                decode_value(extrinsic.call_bytes(), v16.extrinsic.call_ty.id, &metadata);
                if let Some(pallet) = want["pallet"].as_str() {
                    assert_eq!(extrinsic.pallet_name().unwrap(), pallet);
                    assert_eq!(extrinsic.variant_name().unwrap(), text(&want["call"]));
                    let fields = extrinsic.field_values().unwrap();
                    if pallet == "Timestamp" {
                        assert_eq!(
                            fields.at("now").unwrap().as_u128().unwrap().to_string(),
                            text(&want["now"])
                        );
                    } else {
                        assert_eq!(
                            id_address(fields.at("dest").unwrap()),
                            bytes(&want["destination"])
                        );
                        assert_eq!(
                            fields.at("value").unwrap().as_u128().unwrap().to_string(),
                            text(&want["amount"])
                        );
                    }
                }
                if !extrinsic.is_signed() {
                    totals.1 += 1;
                    assert!(extrinsic.address_bytes().is_none());
                    assert!(extrinsic.signature_bytes().is_none());
                    assert!(extrinsic.transaction_extensions().is_none());
                    continue;
                }
                totals.2 += 1;
                let address = decode_value(
                    extrinsic.address_bytes().unwrap(),
                    v16.extrinsic.address_ty.id,
                    &metadata,
                );
                assert_eq!(id_address(&address), bytes(&want["accountId"]));
                let signature_bytes = extrinsic.signature_bytes().unwrap();
                assert_eq!(signature_bytes.len(), 1660);
                assert_eq!(sha256(signature_bytes), text(&want["envelopeSha256"]));
                let signature =
                    decode_value(signature_bytes, v16.extrinsic.signature_ty.id, &metadata);
                let public = byte_leaves(signature.at("public").unwrap());
                assert_eq!(public.len(), 929);
                assert_eq!(sha256(&public), text(&want["publicKeySha256"]));
                assert_eq!(public, signature_bytes[..929]);
                let account_material = [b"quip-account-v1".as_slice(), &public].concat();
                assert_eq!(
                    sp_core::blake2_256(&account_material).to_vec(),
                    bytes(&want["accountId"])
                );
                let sig_field = signature.at("signature").unwrap();
                let ValueDef::Composite(parts) = &unwrap_single(sig_field).value else {
                    panic!("signature's metadata shim is a two-array composite");
                };
                let parts: Vec<_> = parts.values().map(byte_leaves).collect();
                assert_eq!(parts.iter().map(Vec::len).collect::<Vec<_>>(), [512, 219]);
                let sig = parts.concat();
                assert_eq!(sig.len(), 731);
                assert_eq!(sig, signature_bytes[929..]);
                assert_eq!(encode_value(sig_field, &metadata), sig);
                assert_eq!(sha256(&sig), text(&want["signatureFieldSha256"]));
                if manifest["kind"] == "local-non-benchmark" {
                    assert_eq!(public, bytes(&signing["publicKeyHex"]));
                    assert_eq!(want["accountId"], signing["accountIdHex"]);
                }
                assert_eq!(
                    extrinsic.transaction_extensions_bytes().unwrap(),
                    bytes(&want["extensionHex"])
                );
                let extensions = extrinsic.transaction_extensions().unwrap();
                assert_eq!(
                    extensions.nonce().unwrap().to_string(),
                    text(&want["nonce"])
                );
                assert_eq!(extensions.tip().unwrap().to_string(), text(&want["tip"]));
                let extensions: Vec<_> = extensions.iter().collect();
                assert_eq!(
                    extensions.iter().map(|e| e.name()).collect::<Vec<_>>(),
                    expected_extensions
                );
                for extension in extensions {
                    let value = extension.value().unwrap();
                    assert_eq!(encode_value(&value, &metadata), extension.bytes());
                    match extension.name() {
                        "CheckMortality" => assert_eq!(extension.bytes(), bytes(&want["eraHex"])),
                        "CheckNonce" | "ChargeTransactionPayment" => {}
                        "CheckMetadataHash" => {
                            assert_eq!(extension.bytes(), [0]);
                            variant(value.at("mode").unwrap(), "Disabled");
                        }
                        _ => assert!(
                            extension.bytes().is_empty(),
                            "{} must be zero-sized",
                            extension.name()
                        ),
                    }
                }
            }
            let event_bytes = fs::read(directory.join(text(&block["eventsFile"]))).unwrap();
            let events =
                Events::<SubstrateConfig>::decode_from(event_bytes.clone(), metadata.clone());
            let events = events.iter().collect::<Result<Vec<_>, _>>().unwrap();
            let count = Compact::<u32>::decode(&mut &event_bytes[..]).unwrap();
            assert_eq!(events.len(), count.0 as usize);
            assert_eq!(
                count.encode().len() + events.iter().map(|e| e.bytes().len()).sum::<usize>(),
                event_bytes.len(),
                "events consume all bytes"
            );
            let mut outcomes = vec![None; expected.len()];
            let mut transfers = vec![0; expected.len()];
            let mut code_updated = false;
            for (event_index, event) in events.iter().enumerate() {
                assert_eq!(event.index() as usize, event_index);
                let fields = event.field_values().unwrap();
                let event_encoding = [
                    &[event.pallet_index(), event.variant_index()],
                    event.field_bytes(),
                ]
                .concat();
                decode_value(
                    &event_encoding,
                    metadata.outer_enums().event_enum_ty(),
                    &metadata,
                );
                let Phase::ApplyExtrinsic(index) = event.phase() else {
                    continue;
                };
                let index = index as usize;
                assert!(index < expected.len());
                let want = &expected[index];
                match (event.pallet_name(), event.variant_name()) {
                    ("System", "ExtrinsicSuccess" | "ExtrinsicFailed") => {
                        let failed = event.variant_name() == "ExtrinsicFailed";
                        assert!(
                            outcomes[index].replace(failed).is_none(),
                            "one outcome per extrinsic"
                        );
                        if failed {
                            let error = fields.at("dispatch_error").unwrap();
                            variant(error, "Module");
                            let module = error.at(0).unwrap();
                            assert_eq!(module.at("index").unwrap().as_u128(), Some(13));
                            assert_eq!(byte_leaves(module.at("error").unwrap()), [19, 0, 0, 0]);
                        }
                    }
                    ("Balances", "Transfer") => {
                        assert_eq!(want["pallet"], "Balances");
                        assert_eq!(
                            byte_leaves(fields.at("from").unwrap()),
                            bytes(&want["accountId"])
                        );
                        assert_eq!(
                            byte_leaves(fields.at("to").unwrap()),
                            bytes(&want["destination"])
                        );
                        assert_eq!(
                            fields.at("amount").unwrap().as_u128().unwrap().to_string(),
                            text(&want["amount"])
                        );
                        transfers[index] += 1;
                        totals.3 += 1;
                    }
                    ("System", "CodeUpdated") => {
                        assert_eq!((number, index), (330109, 1));
                        code_updated = true;
                    }
                    _ => {}
                }
            }
            for (index, want) in expected.iter().enumerate() {
                let failed = match number {
                    330109 => (2..=6).contains(&index),
                    591863 | 591865 => (2..=4).contains(&index),
                    _ => false,
                };
                assert_eq!(
                    outcomes[index],
                    Some(failed),
                    "block {number}, extrinsic {index}"
                );
                assert_eq!(transfers[index], usize::from(want["pallet"] == "Balances"));
            }
            assert_eq!(code_updated, number == 330109);
        }
    }
    assert_eq!(totals, (5, 5, 20, 3));
}

pub fn assert_upgrade_boundary() {
    let directory = root().join("aglais-117-118");
    let old = subxt_core::metadata::decode_from(
        &fs::read(directory.join("metadata-330109.scale")).unwrap(),
    )
    .unwrap();
    let new = subxt_core::metadata::decode_from(
        &fs::read(directory.join("metadata-330110.scale")).unwrap(),
    )
    .unwrap();
    // The actual upgrade block's calls are common to both versions. This
    // synthetic call intentionally exercises a surface added in runtime 118.
    for name in ["remove_program", "evict_program"] {
        let call = subxt_core::dynamic::tx("Xqvm", name, vec![Value::from_bytes([0x42; 32])]);
        let encoded = subxt_core::tx::create_v5_bare::<SubstrateConfig, _>(&call, &new)
            .unwrap()
            .into_encoded();
        let decoded =
            Extrinsics::<SubstrateConfig>::decode_from(vec![encoded.clone()], new.clone()).unwrap();
        let extrinsic = decoded.iter().next().unwrap();
        assert_eq!(extrinsic.pallet_name().unwrap(), "Xqvm");
        assert_eq!(extrinsic.variant_name().unwrap(), name);
        assert_eq!(
            byte_leaves(
                extrinsic
                    .field_values()
                    .unwrap()
                    .at("program_hash")
                    .unwrap()
            ),
            [0x42; 32]
        );
        assert!(
            Extrinsics::<SubstrateConfig>::decode_from(vec![encoded], old.clone()).is_err(),
            "117 must reject {name}, introduced in 118"
        );
    }
}

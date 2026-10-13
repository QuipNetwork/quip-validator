//! Read-only stock-subxt smoke against a finalized transfer on a disposable node.
//! Run after produce-conformance-transfer.mjs; see the fixture README.
//! No generated or handwritten Quip account, signature, call or event types.

use codec::{Compact, Decode, Encode};
use frame_metadata::RuntimeMetadata;
use serde_json::{json, Value as Json};
use std::{env, fs, net::SocketAddr, time::Duration};
use subxt::{
    backend::{legacy::LegacyRpcMethods, rpc::RpcClient},
    config::Header as _,
    events::Phase,
    ext::{
        jsonrpsee::{client_transport::ws::WsTransportClientBuilder, core::client::Client},
        scale_value::{self, At, Value, ValueDef},
    },
    utils::H256,
    OnlineClient, SubstrateConfig,
};

const EXTENSIONS: [&str; 12] = [
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
    "WeightReclaim",
];

fn bytes(value: &Json) -> Vec<u8> {
    hex::decode(value.as_str().unwrap().trim_start_matches("0x")).unwrap()
}

fn byte_leaves(value: &Value<u32>) -> Vec<u8> {
    match &value.value {
        ValueDef::Composite(fields) => fields.values().flat_map(byte_leaves).collect(),
        ValueDef::Primitive(_) => vec![u8::try_from(value.as_u128().unwrap()).unwrap()],
        other => panic!("expected byte array/newtype, got {other:?}"),
    }
}

fn id_address(value: &Value<u32>) -> Vec<u8> {
    let ValueDef::Variant(variant) = &value.value else {
        panic!("MultiAddress variant")
    };
    assert_eq!(variant.name, "Id");
    byte_leaves(value.at(0).unwrap())
}

fn decode(data: &[u8], ty: u32, metadata: &subxt::Metadata) -> Value<u32> {
    let mut cursor = data;
    let value = scale_value::scale::decode_as_type(&mut cursor, ty, metadata.types()).unwrap();
    assert!(cursor.is_empty());
    let mut encoded = Vec::new();
    scale_value::scale::encode_as_type(&value, ty, metadata.types(), &mut encoded).unwrap();
    assert_eq!(encoded, data);
    value
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(60), check()).await??;
    Ok(())
}

async fn check() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        2,
        "usage: metadata_online PRODUCER_JSON NODE_WASM"
    );
    let report: Json = serde_json::from_slice(&fs::read(&args[0])?)?;
    let wasm = fs::read(&args[1])?;
    let signing: Json = serde_json::from_str(include_str!(
        "../../docs/polkadotjs/fixtures/hybrid-signing.json"
    ))?;
    let url = env::var("QUIP_WS_URL").unwrap_or_else(|_| "ws://127.0.0.1:19944".into());
    let address: SocketAddr = url
        .strip_prefix("ws://")
        .expect("ws:// loopback RPC required")
        .parse()?;
    assert!(
        address.ip().is_loopback(),
        "disposable loopback node required"
    );
    // The artifact check reads :code as RPC hex (~15 MB in this debug build),
    // exceeding jsonrpsee's default 10 MiB transport limit. Decoding stays stock.
    let (sender, receiver) = WsTransportClientBuilder::default()
        .max_response_size(32 * 1024 * 1024)
        .build(url.parse()?)
        .await?;
    let client = RpcClient::new(Client::builder().build_with_tokio(sender, receiver));
    let rpc = LegacyRpcMethods::<SubstrateConfig>::new(client.clone());
    assert_eq!(rpc.system_chain().await?, "Development");
    // Exercise the stock OnlineClient initialization and metadata negotiation.
    let api = OnlineClient::<SubstrateConfig>::from_rpc_client(client).await?;
    let hash: H256 = report["hash"].as_str().unwrap().parse()?;
    let block = api.blocks().at(hash).await?;
    assert_eq!(block.number() as u64, report["number"].as_u64().unwrap());
    assert_eq!(block.header().hash_with(api.hasher()), hash);
    let finalized = api.blocks().at_latest().await?;
    assert!(
        block.number() <= finalized.number(),
        "producer block must be finalized"
    );
    assert_eq!(
        rpc.chain_get_block_hash(Some(block.number().into()))
            .await?,
        Some(hash)
    );

    // The high-level blocks API uses cached client metadata. This smoke requires
    // unchanged parent/post-state metadata and code, plus a matching negotiated
    // registry. Historical upgrades use the separate offline paired fixtures.
    let parent = block.header().parent_hash;
    let response = rpc.state_get_metadata(Some(parent)).await?;
    let RuntimeMetadata::V16(v16) = response.to_frame_metadata()?.1 else {
        panic!("state_getMetadata must publish V16");
    };
    assert_eq!(v16.extrinsic.versions, [4, 5]);
    assert!(!v16.apis.iter().any(|a| a.name == "Benchmark"));
    let metadata_bytes = response.into_raw();
    assert!(
        rpc.state_get_metadata(Some(hash)).await?.into_raw() == metadata_bytes,
        "metadata changed across the block"
    );
    let parent_metadata = subxt::Metadata::decode(&mut &metadata_bytes[..])?;
    let metadata = api.metadata();
    assert!(
        metadata.types().encode() == parent_metadata.types().encode(),
        "negotiated registry differs from parent metadata"
    );
    assert_eq!(metadata.hasher().hash(), parent_metadata.hasher().hash());
    let version = rpc.state_get_runtime_version(Some(parent)).await?;
    assert_eq!(
        (version.spec_version, version.transaction_version),
        (119, 7)
    );
    assert_eq!(api.runtime_version().spec_version, version.spec_version);
    assert_eq!(
        api.runtime_version().transaction_version,
        version.transaction_version
    );
    let code = rpc
        .state_get_storage(b":code", Some(parent))
        .await?
        .expect("runtime code");
    assert!(
        code == wasm,
        "node must execute the supplied non-benchmark Wasm artifact"
    );
    assert!(
        rpc.state_get_storage(b":code", Some(hash)).await? == Some(code.clone()),
        "runtime code changed across the block"
    );

    let raw = rpc
        .chain_get_block(Some(hash))
        .await?
        .expect("finalized block");
    let extrinsics = block.extrinsics().await?;
    assert_eq!(extrinsics.len(), raw.block.extrinsics.len());
    assert_eq!(
        extrinsics.len(),
        2,
        "isolated block: timestamp then fixture transfer"
    );
    assert_eq!(report["extrinsicIndex"], 1);
    assert_eq!(report["accountId"], signing["accountIdHex"]);
    let account = bytes(&report["accountId"]);
    let destination = bytes(&report["destination"]);
    let amount: u128 = report["amount"].as_str().unwrap().parse()?;
    assert_eq!(amount, 10_000_000_000_000);
    let nonce: u64 = report["nonce"].as_str().unwrap().parse()?;
    let mut transfer_events = 0;
    let mut outcome_events = 0;
    for (index, extrinsic) in extrinsics.iter().enumerate() {
        assert_eq!(extrinsic.index() as usize, index);
        assert_eq!(extrinsic.bytes(), raw.block.extrinsics[index].0);
        assert_eq!(
            extrinsic.hash().as_bytes(),
            &sp_core::blake2_256(extrinsic.bytes())
        );
        let mut cursor = extrinsic.bytes();
        Compact::<u32>::decode(&mut cursor)?;
        assert_eq!(u8::decode(&mut cursor)?, if index == 0 { 5 } else { 0x84 });
        decode(extrinsic.call_bytes(), v16.extrinsic.call_ty.id, &metadata);
        let fields = extrinsic.field_values()?;
        if index == 0 {
            assert!(!extrinsic.is_signed());
            assert!(extrinsic.address_bytes().is_none());
            assert!(extrinsic.signature_bytes().is_none());
            assert!(extrinsic.transaction_extensions().is_none());
            assert_eq!(
                (extrinsic.pallet_name()?, extrinsic.variant_name()?),
                ("Timestamp", "set")
            );
            assert!(fields.at("now").unwrap().as_u128().unwrap() > 0);
        } else {
            assert!(extrinsic.is_signed());
            assert_eq!(
                (extrinsic.pallet_name()?, extrinsic.variant_name()?),
                ("Balances", "transfer_keep_alive")
            );
            assert_eq!(id_address(fields.at("dest").unwrap()), destination);
            assert_eq!(fields.at("value").unwrap().as_u128(), Some(amount));
            let address = decode(
                extrinsic.address_bytes().unwrap(),
                v16.extrinsic.address_ty.id,
                &metadata,
            );
            assert_eq!(id_address(&address), account);
            let envelope = extrinsic.signature_bytes().unwrap();
            assert_eq!(envelope.len(), 1660);
            let signature = decode(envelope, v16.extrinsic.signature_ty.id, &metadata);
            let public = byte_leaves(signature.at("public").unwrap());
            assert_eq!(public, bytes(&signing["publicKeyHex"]));
            assert_eq!(public.len(), 929);
            assert_eq!(public, envelope[..929]);
            let signature_field = byte_leaves(signature.at("signature").unwrap());
            assert_eq!(signature_field.len(), 731);
            assert_eq!(signature_field, envelope[929..]);
            assert_eq!(
                sp_core::blake2_256(&[b"quip-account-v1".as_slice(), &public].concat()).to_vec(),
                account
            );
            let extensions = extrinsic.transaction_extensions().unwrap();
            assert_eq!(extensions.nonce(), Some(nonce));
            assert_eq!(extensions.tip(), Some(0));
            assert_eq!(
                extensions.iter().map(|e| e.name()).collect::<Vec<_>>(),
                EXTENSIONS
            );
            for extension in extensions.iter() {
                decode(extension.bytes(), extension.type_id(), &metadata);
                match extension.name() {
                    "CheckMortality" | "CheckMetadataHash" => assert_eq!(extension.bytes(), [0]),
                    "CheckNonce" => assert_eq!(extension.bytes(), Compact(nonce).encode()),
                    "ChargeTransactionPayment" => assert_eq!(extension.bytes(), [0]),
                    _ => assert!(extension.bytes().is_empty()),
                }
            }
        }
        // Exercise subxt's high-level per-extrinsic event filtering too.
        let scoped = extrinsic.events().await?;
        assert_eq!(scoped.extrinsic_index() as usize, index);
        assert_eq!(scoped.extrinsic_hash(), extrinsic.hash());
        let mut success = 0;
        for event in scoped.iter() {
            let event = event?;
            assert_eq!(event.phase(), Phase::ApplyExtrinsic(index as u32));
            let fields = event.field_values()?;
            match (event.pallet_name(), event.variant_name()) {
                ("System", "ExtrinsicFailed") => {
                    panic!("local extrinsic {index} failed: {fields:?}")
                }
                ("System", "ExtrinsicSuccess") => {
                    success += 1;
                    outcome_events += 1;
                }
                ("Balances", "Transfer") => {
                    assert_eq!(index, 1);
                    assert_eq!(byte_leaves(fields.at("from").unwrap()), account);
                    assert_eq!(byte_leaves(fields.at("to").unwrap()), destination);
                    assert_eq!(fields.at("amount").unwrap().as_u128(), Some(amount));
                    transfer_events += 1;
                }
                _ => {}
            }
        }
        assert_eq!(success, 1, "one success outcome per extrinsic");
    }
    assert_eq!((outcome_events, transfer_events), (2, 1));
    let events = block.events().await?;
    let all = events.iter().collect::<Result<Vec<_>, _>>()?;
    let key = hex::decode("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7")?;
    let raw_events = rpc
        .state_get_storage(&key, Some(hash))
        .await?
        .expect("System.Events");
    let count = Compact::<u32>::decode(&mut &raw_events[..])?;
    assert_eq!(all.len(), count.0 as usize);
    let mut encoded_events = count.encode();
    for (index, event) in all.iter().enumerate() {
        assert_eq!(event.index() as usize, index);
        event.field_values()?;
        let encoded = [
            &[event.pallet_index(), event.variant_index()],
            event.field_bytes(),
        ]
        .concat();
        decode(&encoded, metadata.outer_enums().event_enum_ty(), &metadata);
        encoded_events.extend_from_slice(event.bytes());
    }
    assert_eq!(encoded_events, raw_events, "all events consumed in order");
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "consumer": "stock subxt 0.44.3 OnlineClient<SubstrateConfig>",
            "rpc": url, "genesisHash": api.genesis_hash(), "blockHash": hash,
            "number": block.number(), "metadataStateHash": parent,
            "specVersion": version.spec_version, "transactionVersion": version.transaction_version,
            "metadataVersion": 16, "metadataSha256": hex::encode(sp_core::hashing::sha2_256(&metadata_bytes)),
            "runtimeCodeSha256": hex::encode(sp_core::hashing::sha2_256(&code)),
            "extrinsics": extrinsics.len(), "bareV5": 1, "signedV4": 1,
            "events": all.len(), "successOutcomes": outcome_events, "matchingTransfers": transfer_events,
            "transfer": report
        }))?
    );
    Ok(())
}

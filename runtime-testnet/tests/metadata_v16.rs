// Metadata V16 conformance tests (QUI-901 Phase 1).
//
// Hermetic: all metadata comes from `Runtime::metadata_at_version(16)` and
// all decoding is done in-process by stock `subxt-core` with metadata only —
// no node, no network.

use codec::{Decode, Encode};
use frame_metadata::{v16::RuntimeMetadataV16, RuntimeMetadata, RuntimeMetadataPrefixed};
use quip_testnet_runtime::{
    native_tx_extension, Address, BalancesCall, Runtime, RuntimeCall, RuntimeGenesisConfig,
    SignedPayload, System, TimestampCall, UncheckedExtrinsic,
};
use quip_transaction_crypto::{account_id_from_public, HybridPair, HybridTxSignature};
use scale_info::{form::PortableForm, PortableRegistry, Type, TypeDef, TypeDefPrimitive};
use sp_core::Pair as _;
use sp_runtime::{generic, BuildStorage};
use subxt_core::{blocks::Extrinsics, config::substrate::SubstrateConfig};

/// Seed of the signing pair used by the checked-in polkadot-js fixture
/// (`runtime/tests/support/signing_fixture.rs`).
const FIXTURE_SEED: [u8; 32] = [7u8; 32];

/// The runtime's ordered transaction extension identifiers (`TxExtension` in
/// `runtime/src/lib.rs`). Pinned so the V16 extrinsic section cannot silently
/// drift from the extension set used by signed transactions.
const EXPECTED_EXTENSION_IDENTIFIERS: [&str; 12] = [
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

fn metadata_v16_bytes() -> Vec<u8> {
    // Metadata generation touches storage-version host functions, so it must
    // run inside externalities (still fully hermetic: default genesis only).
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

    ext.execute_with(|| {
        let opaque = Runtime::metadata_at_version(16).expect("Metadata V16 is supported");
        opaque.to_vec()
    })
}

fn decode_metadata_v16(bytes: &[u8]) -> RuntimeMetadataV16 {
    let mut input = bytes;
    let metadata = RuntimeMetadataPrefixed::decode(&mut input).expect("metadata SCALE-decodes");
    assert!(input.is_empty(), "metadata has trailing bytes");
    assert_eq!(metadata.0, frame_metadata::META_RESERVED);
    match metadata.1 {
        RuntimeMetadata::V16(metadata) => metadata,
        other => panic!("expected Metadata V16, got a different variant: {other:?}"),
    }
}

fn metadata_v16() -> RuntimeMetadataV16 {
    decode_metadata_v16(&metadata_v16_bytes())
}

fn resolve_type(registry: &PortableRegistry, id: u32) -> &Type<PortableForm> {
    registry
        .resolve(id)
        .unwrap_or_else(|| panic!("type id {id} must resolve in the V16 registry"))
}

/// Follows single-unnamed-field composite wrappers (newtype shims such as
/// `Public`/`Signature`) down to the terminal type.
fn unwrap_newtypes<'r>(registry: &'r PortableRegistry, mut id: u32) -> &'r Type<PortableForm> {
    loop {
        let ty = resolve_type(registry, id);
        match &ty.type_def {
            TypeDef::Composite(composite)
                if composite.fields.len() == 1 && composite.fields[0].name.is_none() =>
            {
                id = composite.fields[0].ty.id;
            }
            _ => return ty,
        }
    }
}

/// Asserts that `id` (after unwrapping newtype shims) is a `[u8; len]` array.
fn expect_u8_array(registry: &PortableRegistry, id: u32, len: u32, what: &str) {
    let ty = unwrap_newtypes(registry, id);
    let TypeDef::Array(array) = &ty.type_def else {
        panic!("{what} must resolve to an array, got {:?}", ty.type_def);
    };
    assert_eq!(array.len, len, "{what} array length");
    let element = resolve_type(registry, array.type_param.id);
    assert!(
        matches!(element.type_def, TypeDef::Primitive(TypeDefPrimitive::U8)),
        "{what} array element must be u8, got {:?}",
        element.type_def,
    );
}

/// Builds a V4 signed extrinsic for `call`, signed by the fixture pair with
/// an immortal era, nonce 0, and tip 0 (same parameters as the checked-in
/// polkadot-js signing fixture).
fn fixture_signed_extrinsic(call: RuntimeCall) -> UncheckedExtrinsic {
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

    ext.execute_with(|| {
        System::set_block_number(1);

        let pair = HybridPair::from_seed_slice(&FIXTURE_SEED).expect("fixture seed is valid");
        let account_id = account_id_from_public(&pair.public());
        let tx_extension = native_tx_extension(generic::Era::Immortal, 0, 0);
        let payload =
            SignedPayload::new(call.clone(), tx_extension.clone()).expect("payload is valid");
        let signature = payload.using_encoded(|encoded| HybridTxSignature::sign(&pair, encoded));

        generic::UncheckedExtrinsic::new_signed(
            call,
            Address::Id(account_id),
            signature,
            tx_extension,
        )
        .into()
    })
}

#[test]
fn metadata_at_version_16_returns_v16() {
    let v16 = metadata_v16();
    assert!(
        !v16.pallets.is_empty(),
        "V16 metadata must carry the pallet graph"
    );
}

fn wasm_for_metadata_test(wasm: Option<&[u8]>, in_ci: bool) -> Option<&[u8]> {
    if wasm.is_none() {
        assert!(
            !in_ci,
            "CI requires a runtime Wasm binary; unset SKIP_WASM_BUILD"
        );
        eprintln!("skipping: runtime wasm binary not built (SKIP_WASM_BUILD)");
    }
    wasm
}

#[test]
fn missing_wasm_is_fatal_in_ci_but_skippable_locally() {
    assert!(wasm_for_metadata_test(None, false).is_none());
    assert!(std::panic::catch_unwind(|| wasm_for_metadata_test(None, true)).is_err());
    assert_eq!(
        wasm_for_metadata_test(Some(b"wasm"), true),
        Some(b"wasm".as_slice())
    );
}

fn wasm_metadata_bytes(version: Option<u32>) -> Option<Vec<u8>> {
    // Regression pin: `state_getMetadata` is served by the legacy
    // `sp_api::Metadata::metadata()` runtime API implemented in
    // `runtime/src/apis.rs`, which must publish V16. The std-side trait impl
    // cannot be invoked natively (`impl_runtime_apis!` moves it onto the
    // client-side `RuntimeApiImpl`, which dispatches through `CallApiAt` and
    // an executor), so this test executes the runtime API's actual wasm entry
    // point — the same code path a node takes. Reverting `metadata()` to the
    // old V14 one-liner `OpaqueMetadata::new(Runtime::metadata().into())`
    // makes this test fail.
    let wasm_binary = wasm_for_metadata_test(
        quip_testnet_runtime::WASM_BINARY,
        std::env::var_os("CI").is_some(),
    )?;

    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    let mut ext = ext.ext();
    let executor = sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build();
    let (method, input) = match version {
        Some(version) => ("Metadata_metadata_at_version", version.encode()),
        None => ("Metadata_metadata", Vec::new()),
    };
    let encoded = executor
        .uncached_call(
            sc_executor_common::runtime_blob::RuntimeBlob::uncompress_if_needed(wasm_binary)
                .expect("runtime wasm blob is valid"),
            &mut ext,
            false,
            method,
            &input,
        )
        .unwrap_or_else(|error| panic!("{method} runtime API call failed: {error:?}"));
    let mut output = &encoded[..];
    let opaque = if version.is_some() {
        Option::<sp_core::OpaqueMetadata>::decode(&mut output)
            .expect("versioned API returns Option<OpaqueMetadata>")
            .expect("requested metadata version is supported")
    } else {
        sp_core::OpaqueMetadata::decode(&mut output).expect("legacy API returns OpaqueMetadata")
    };
    assert!(output.is_empty(), "{method} returned trailing bytes");

    Some(opaque.to_vec())
}

#[test]
fn legacy_metadata_runtime_api_publishes_v16() {
    let Some(bytes) = wasm_metadata_bytes(None) else {
        return;
    };
    decode_metadata_v16(&bytes);
}

#[test]
fn native_embedded_and_wasm_core_versions_agree() {
    let Some(wasm_binary) = wasm_for_metadata_test(
        quip_testnet_runtime::WASM_BINARY,
        std::env::var_os("CI").is_some(),
    ) else {
        return;
    };
    let blob = sc_executor_common::runtime_blob::RuntimeBlob::uncompress_if_needed(wasm_binary)
        .expect("runtime wasm blob is valid");
    let embedded = sc_executor::read_embedded_version(&blob)
        .expect("embedded version decodes")
        .expect("runtime wasm must embed a version");
    let executor = sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build();
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    // Execute the actual export: reading the executor's cached version would
    // merely trust the embedded section and miss a disagreement with the code.
    let encoded = executor
        .uncached_call(blob, &mut ext.ext(), false, "Core_version", &[])
        .expect("Wasm Core_version succeeds");
    let mut input = encoded.as_slice();
    let actual = sp_version::RuntimeVersion::decode(&mut input)
        .expect("Core_version returns RuntimeVersion");
    assert!(input.is_empty(), "Core_version returned trailing bytes");

    // The embedded API section may order entries differently. Preserve every
    // entry (including duplicates) and compare all other version fields as-is.
    let normalize = |mut version: sp_version::RuntimeVersion| {
        let mut apis = version.apis.to_vec();
        apis.sort();
        version.apis = apis.into();
        version
    };
    let native = normalize(quip_testnet_runtime::VERSION);
    assert_eq!(native, normalize(embedded), "native vs embedded version");
    assert_eq!(native, normalize(actual), "native vs Wasm Core_version");
}

#[test]
fn v16_extrinsic_section_advertises_v4_and_v5() {
    let v16 = metadata_v16();
    let extrinsic = &v16.extrinsic;

    assert_eq!(
        extrinsic.versions,
        vec![4, 5],
        "V16 extrinsic section must describe Quip's mixed V5-bare/V4-signed blocks",
    );

    // The SDK fork's V16 conversion currently emits a single
    // `transaction_extensions_by_version` entry keyed 0 ("assume version 0 for
    // all extensions"): extension version 0 is the only one, V4 signed
    // extrinsics fall back to the maximum map key, and V5 bare extrinsics
    // carry no extensions. Pin that shape so a future extension-version
    // change cannot silently emit wrong metadata.
    let by_version = &extrinsic.transaction_extensions_by_version;
    let keys: Vec<u8> = by_version.keys().copied().collect();
    assert_eq!(
        keys,
        vec![0],
        "transaction_extensions_by_version must be keyed by extension version 0 only",
    );
    let indexes = &by_version[&0];
    assert_eq!(
        indexes.len(),
        extrinsic.transaction_extensions.len(),
        "every transaction extension must be covered by the version map",
    );
    let resolved: Vec<u32> = indexes.iter().map(|index| index.0).collect();
    assert_eq!(
        resolved,
        (0..extrinsic.transaction_extensions.len() as u32).collect::<Vec<_>>(),
        "version-0 indexes must resolve, in order, into transaction_extensions",
    );

    let identifiers: Vec<&str> = extrinsic
        .transaction_extensions
        .iter()
        .map(|extension| extension.identifier.as_str())
        .collect();
    assert_eq!(identifiers, EXPECTED_EXTENSION_IDENTIFIERS);
    for extension in &extrinsic.transaction_extensions {
        resolve_type(&v16.types, extension.ty.id);
        resolve_type(&v16.types, extension.implicit.id);
    }
}

#[test]
fn v16_registry_fully_describes_hybrid_signature_envelope() {
    let v16 = metadata_v16();
    let registry = &v16.types;
    let extrinsic = &v16.extrinsic;

    // Address and call types must be real enums (MultiAddress / RuntimeCall),
    // resolvable from the registry.
    let address = resolve_type(registry, extrinsic.address_ty.id);
    assert!(
        matches!(address.type_def, TypeDef::Variant(_)),
        "address type must be the MultiAddress enum, got {:?}",
        address.type_def,
    );
    let call = resolve_type(registry, extrinsic.call_ty.id);
    assert!(
        matches!(call.type_def, TypeDef::Variant(_)),
        "call type must be the RuntimeCall enum, got {:?}",
        call.type_def,
    );

    // The signature type is the HybridTxSignature envelope: a composite with
    // named `public` and `signature` fields, not an opaque `Vec<u8>` or any
    // other sequence.
    let envelope = resolve_type(registry, extrinsic.signature_ty.id);
    assert!(
        !matches!(envelope.type_def, TypeDef::Sequence(_)),
        "HybridTxSignature must not be an opaque byte sequence",
    );
    assert_eq!(envelope.path.ident().as_deref(), Some("HybridTxSignature"));
    let TypeDef::Composite(composite) = &envelope.type_def else {
        panic!(
            "HybridTxSignature must be a composite, got {:?}",
            envelope.type_def
        );
    };
    let field = |name: &str| {
        composite
            .fields
            .iter()
            .find(|field| field.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("HybridTxSignature must have a `{name}` field"))
    };

    // H4 hybrid public key: sr25519 (32) + FN-DSA-512 (897).
    expect_u8_array(registry, field("public").ty.id, 929, "public key");

    // Hybrid signature: 731 fixed envelope bytes, exposed to metadata as the
    // fork's two-array shim `[u8; 512]` followed by `[u8; 219]`
    // (512 + 219 = 731, encoding-equivalent to the wire layout).
    let signature = unwrap_newtypes(registry, field("signature").ty.id);
    let TypeDef::Composite(parts) = &signature.type_def else {
        panic!(
            "signature must resolve to the two-array metadata shim, got {:?}",
            signature.type_def
        );
    };
    assert_eq!(
        parts.fields.len(),
        2,
        "signature shim must be a pair of arrays",
    );
    expect_u8_array(registry, parts.fields[0].ty.id, 512, "signature part 1");
    expect_u8_array(registry, parts.fields[1].ty.id, 219, "signature part 2");
}

#[test]
fn stock_subxt_decodes_mixed_v5_bare_and_v4_signed_extrinsics() {
    let metadata_bytes = metadata_v16_bytes();
    let metadata = subxt_core::metadata::decode_from(&metadata_bytes)
        .expect("stock subxt-core accepts the V16 metadata with no custom types");

    // V5 bare `Timestamp::set` inherent.
    let timestamp_call: RuntimeCall = TimestampCall::set {
        now: 1_756_000_000_000,
    }
    .into();
    let bare: UncheckedExtrinsic = generic::UncheckedExtrinsic::new_bare(timestamp_call).into();

    // V4 signed `Balances::transfer_allow_death` from the fixture signer.
    let pair = HybridPair::from_seed_slice(&FIXTURE_SEED).expect("fixture seed is valid");
    let transfer_call: RuntimeCall = BalancesCall::transfer_allow_death {
        dest: Address::Id(account_id_from_public(&pair.public())),
        value: 123_456_789,
    }
    .into();
    let signed = fixture_signed_extrinsic(transfer_call);

    let extrinsics =
        Extrinsics::<SubstrateConfig>::decode_from(vec![bare.encode(), signed.encode()], metadata)
            .expect("stock subxt-core decodes the mixed extrinsic set from metadata only");

    let decoded = extrinsics.iter().collect::<Vec<_>>();
    assert_eq!(decoded.len(), 2);

    let inherent = &decoded[0];
    assert!(!inherent.is_signed(), "timestamp inherent must be bare");
    assert_eq!(inherent.pallet_name().unwrap(), "Timestamp");
    assert_eq!(inherent.variant_name().unwrap(), "set");

    let transfer = &decoded[1];
    assert!(transfer.is_signed(), "balance transfer must be signed");
    assert_eq!(transfer.pallet_name().unwrap(), "Balances");
    assert_eq!(transfer.variant_name().unwrap(), "transfer_allow_death");
}

// Pin the network identity and exercise FRAME's ordinary upgrade guard with a
// higher-version candidate, so rejection cannot be caused by a stale version.
// FRAME intentionally bypasses these version guards in benchmark builds.
#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn runtime_identity_rejects_cross_network_upgrades() {
    let (expected_name, other_name) = match env!("CARGO_PKG_NAME") {
        "quip-protocol-runtime" => ("quip", "quip-testnet"),
        "quip-testnet-runtime" => ("quip-testnet", "quip"),
        other => panic!("unexpected runtime package: {other}"),
    };
    let current = quip_testnet_runtime::VERSION;
    assert_eq!(current.spec_name, expected_name);
    let Some(wasm) = quip_testnet_runtime::WASM_BINARY else {
        eprintln!("skipping upgrade guard: runtime wasm binary not built (SKIP_WASM_BUILD)");
        return;
    };
    let wasm = sc_executor_common::runtime_blob::RuntimeBlob::uncompress_if_needed(wasm)
        .unwrap()
        .serialize();
    let mut candidate = current;
    candidate.spec_version += 1;
    let same_network = sp_version::embed::embed_runtime_version(&wasm, candidate.clone()).unwrap();
    candidate.spec_name = other_name.into();
    let other_network = sp_version::embed::embed_runtime_version(&wasm, candidate).unwrap();
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(
        sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build(),
    ));
    ext.execute_with(|| {
        assert!(System::can_set_code(&same_network, true)
            .into_result()
            .is_ok());
        frame_support::assert_storage_noop!({
            let error =
                System::set_code(quip_testnet_runtime::RuntimeOrigin::root(), other_network)
                    .unwrap_err()
                    .error;
            assert_eq!(
                error,
                frame_system::Error::<Runtime>::InvalidSpecName.into()
            );
            println!("{expected_name} rejects {other_name} at higher spec version: {error:?}");
        });
    });
}

mod unchecked_upgrade_tests {
    use super::*;
    use frame_support::assert_ok;
    #[cfg(not(feature = "runtime-benchmarks"))]
    use frame_support::assert_storage_noop;
    #[cfg(not(feature = "runtime-benchmarks"))]
    use quip_testnet_runtime::configs::RUNTIME_IDENTITY_MISMATCH;
    use quip_testnet_runtime::{RuntimeOrigin, VERSION};
    use sp_core::storage::well_known_keys::CODE;
    #[cfg(not(feature = "runtime-benchmarks"))]
    use sp_runtime::{traits::Hash as _, DispatchError};

    fn ext() -> sp_io::TestExternalities {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(
            sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build(),
        ));
        ext.execute_with(|| {
            System::set_block_number(1);
            sp_io::storage::set(CODE, b"previous code");
        });
        ext
    }

    #[cfg(not(feature = "runtime-benchmarks"))]
    fn foreign_name() -> &'static str {
        match VERSION.spec_name.as_ref() {
            "quip" => "quip-testnet",
            "quip-testnet" => "quip",
            other => panic!("unexpected runtime identity: {other}"),
        }
    }

    // Synthetic future candidate, not a separately built production runtime.
    fn candidate(name: &str, spec_version: u32) -> Vec<u8> {
        let wasm = quip_testnet_runtime::WASM_BINARY
            .expect("upgrade integration tests require a build without SKIP_WASM_BUILD");
        let wasm = sc_executor_common::runtime_blob::RuntimeBlob::uncompress_if_needed(wasm)
            .unwrap()
            .serialize();
        let mut version = VERSION;
        version.spec_name = name.to_owned().into();
        version.spec_version = spec_version;
        sp_version::embed::embed_runtime_version(&wasm, version).unwrap()
    }

    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn unchecked_foreign_upgrade_rejects_without_changing_code() {
        let code = candidate(foreign_name(), VERSION.spec_version + 1);
        ext().execute_with(|| {
            let before = sp_io::storage::get(CODE);
            assert_storage_noop!({
                let error = System::set_code_without_checks(RuntimeOrigin::root(), code)
                    .unwrap_err()
                    .error;
                assert_eq!(error, DispatchError::Other(RUNTIME_IDENTITY_MISMATCH));
                println!("{} unchecked foreign upgrade: {error:?}", VERSION.spec_name);
            });
            assert_eq!(sp_io::storage::get(CODE), before);
        });
    }

    #[test]
    fn unchecked_same_network_upgrade_preserves_version_number_bypass() {
        for spec_version in [VERSION.spec_version + 1, VERSION.spec_version] {
            let code = candidate(VERSION.spec_name.as_ref(), spec_version);
            ext().execute_with(|| {
                assert_ok!(System::set_code_without_checks(
                    RuntimeOrigin::root(),
                    code.clone()
                ));
                assert_eq!(sp_io::storage::get(CODE).unwrap().to_vec(), code);
            });
        }
    }

    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn authorized_foreign_upgrade_rejects_and_retains_authorization() {
        let code = candidate(foreign_name(), VERSION.spec_version + 1);
        let hash = <Runtime as frame_system::Config>::Hashing::hash(&code);
        ext().execute_with(|| {
            assert_ok!(System::authorize_upgrade_without_checks(
                RuntimeOrigin::root(),
                hash
            ));
            // The SDK storage type is private; read its verified key and public value type.
            let authorization_key =
                frame_support::storage::storage_prefix(b"System", b"AuthorizedUpgrade");
            let before_authorization = frame_support::storage::unhashed::get::<
                frame_system::CodeUpgradeAuthorization<Runtime>,
            >(&authorization_key)
            .unwrap();
            assert_eq!(*before_authorization.code_hash(), hash);
            let before_code = sp_io::storage::get(CODE);
            let error = System::apply_authorized_upgrade(RuntimeOrigin::none(), code)
                .unwrap_err()
                .error;
            assert_eq!(error, DispatchError::Other(RUNTIME_IDENTITY_MISMATCH));
            // The dispatchable's storage layer rolls back its earlier authorization kill.
            let retained = frame_support::storage::unhashed::get::<
                frame_system::CodeUpgradeAuthorization<Runtime>,
            >(&authorization_key)
            .expect("rejected apply must retain the authorization");
            assert_eq!(*retained.code_hash(), hash);
            assert_eq!(retained.encode(), before_authorization.encode());
            assert_eq!(sp_io::storage::get(CODE), before_code);
            println!(
                "{} authorized foreign upgrade: {error:?}; authorization retained",
                VERSION.spec_name
            );
        });
    }

    #[test]
    fn unchecked_upgrade_preserves_extraction_failure_delegation() {
        let code = b"not a Wasm runtime".to_vec();
        ext().execute_with(|| {
            assert!(sp_io::misc::runtime_version(&code).is_none());
            assert_ok!(System::set_code_without_checks(
                RuntimeOrigin::root(),
                code.clone()
            ));
            assert_eq!(sp_io::storage::get(CODE).unwrap().to_vec(), code);
        });
    }
}

// Native metadata captured at the stable2609 port. Future changes require a
// deliberate re-anchor, with provenance recorded alongside the SCALE files.
#[cfg(not(feature = "runtime-benchmarks"))]
#[path = "support/metadata_anchor.rs"]
mod metadata_anchor;

#[cfg(not(feature = "runtime-benchmarks"))]
fn native_2609_anchor_path() -> std::path::PathBuf {
    let spec_name = quip_testnet_runtime::VERSION.spec_name;
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/native-2609")
        .join(format!("{spec_name}.scale"))
}

#[cfg(not(feature = "runtime-benchmarks"))]
fn native_2609_storage_path() -> std::path::PathBuf {
    let spec_name = quip_testnet_runtime::VERSION.spec_name;
    native_2609_anchor_path().with_file_name(format!("{spec_name}-storage.json"))
}

#[cfg(not(feature = "runtime-benchmarks"))]
fn default_genesis_storage_bytes() -> Vec<u8> {
    let storage = RuntimeGenesisConfig::default().build_storage().unwrap();
    assert!(
        storage.children_default.is_empty(),
        "default genesis gained child storage; extend the anchor format deliberately"
    );
    let entries: Vec<_> = storage
        .top
        .into_iter()
        .map(|(key, value)| (hex::encode(key), hex::encode(value)))
        .collect();
    let mut bytes = serde_json::to_vec_pretty(&entries).unwrap();
    bytes.push(b'\n');
    bytes
}

#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn default_genesis_storage_matches_native_2609_anchor() {
    let anchor = std::fs::read(native_2609_storage_path())
        .expect("checked-in default-genesis storage anchor");
    assert!(
        default_genesis_storage_bytes() == anchor,
        "default-genesis key/value storage must reproduce its anchor exactly"
    );
}

#[cfg(not(feature = "runtime-benchmarks"))]
fn native_2609_metadata() -> RuntimeMetadataV16 {
    let bytes = std::fs::read(native_2609_anchor_path()).expect("checked-in native-2609 anchor");
    decode_metadata_v16(&bytes)
}

#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn native_2609_manifest_pins_match_anchor_files() {
    let path = native_2609_anchor_path();
    let directory = path.parent().unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(directory.join("manifest.json")).expect("anchor manifest must exist"),
    )
    .expect("anchor manifest must be valid JSON");
    for name in metadata_anchor::ANCHOR_FILES {
        let bytes = std::fs::read(directory.join(name))
            .unwrap_or_else(|error| panic!("cannot read anchor {name}: {error}"));
        metadata_anchor::check_anchor_pin(&manifest, name, &bytes).unwrap();
    }
}

#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn native_metadata_satisfies_anchor_invariants() {
    // Report the first differing path before checking exact SCALE bytes.
    let anchor = serde_json::to_value(native_2609_metadata()).unwrap();
    let candidate = serde_json::to_value(metadata_v16()).unwrap();
    metadata_anchor::check_invariants(&anchor, &candidate).unwrap();
    let anchor_bytes = std::fs::read(native_2609_anchor_path()).unwrap();
    assert_metadata_bytes_match(&anchor_bytes, &metadata_v16_bytes(), "native metadata");
}

// Surface output is diagnostic only: docs and other omitted fields still fail
// the exact byte comparison even when the rendered diff is empty.
#[cfg(not(feature = "runtime-benchmarks"))]
fn assert_metadata_bytes_match(anchor: &[u8], bytes: &[u8], source: &str) {
    assert!(
        bytes == anchor,
        "{source} must reproduce the anchor byte for byte; surface diff (may omit the differing field):\n{}",
        metadata_surface::diff(
            &metadata_surface::surface(&decode_metadata_v16(anchor), true),
            &metadata_surface::surface(&decode_metadata_v16(bytes), true),
        ).join("\n")
    );
}

// The anchor must describe what the chain actually reports, not only the
// native build. Local SKIP_WASM_BUILD iterations may skip this; CI must have Wasm.
#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
fn wasm_metadata_matches_native_2609_anchor() {
    let anchor = std::fs::read(native_2609_anchor_path()).unwrap();
    for (version, api) in [
        (None, "Metadata_metadata"),
        (Some(16), "Metadata_metadata_at_version(16)"),
    ] {
        let Some(bytes) = wasm_metadata_bytes(version) else {
            return;
        };
        metadata_anchor::check_invariants(
            &serde_json::to_value(decode_metadata_v16(&anchor)).unwrap(),
            &serde_json::to_value(decode_metadata_v16(&bytes)).unwrap(),
        )
        .unwrap_or_else(|error| panic!("{api}: {error}"));
        assert_metadata_bytes_match(&anchor, &bytes, api);
        println!("{api}: {} bytes match the checked-in anchor", bytes.len());
    }
}

// Explicit write path; never run by the normal suite or during comparison.
#[cfg(not(feature = "runtime-benchmarks"))]
#[test]
#[ignore = "capture only"]
fn capture_native_2609_anchor() {
    let path = native_2609_anchor_path();
    let bytes = metadata_v16_bytes();
    let metadata = serde_json::to_value(decode_metadata_v16(&bytes)).unwrap();
    metadata_anchor::check_invariants(&metadata, &metadata)
        .expect("capture must satisfy independent invariants");
    let directory = path.parent().unwrap();
    let manifest_path = directory.join("manifest.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&manifest_path).expect("capture provenance manifest must exist"),
    )
    .unwrap();
    let storage_path = native_2609_storage_path();
    for (file, contents) in [
        (&path, bytes),
        (&storage_path, default_genesis_storage_bytes()),
    ] {
        std::fs::write(file, &contents).unwrap();
        let name = file.file_name().unwrap().to_str().unwrap();
        manifest["files"][name] = serde_json::json!({
            "sha256": hex::encode(sp_io::hashing::sha2_256(&contents)),
            "bytes": contents.len(),
        });
        println!(
            "captured {} bytes at {}; updated manifest pin",
            contents.len(),
            file.display()
        );
    }
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    manifest_bytes.push(b'\n');
    std::fs::write(manifest_path, manifest_bytes).unwrap();
}

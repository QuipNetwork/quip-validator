// Metadata V16 conformance tests (QUI-901 Phase 1).
//
// Hermetic: all metadata comes from `Runtime::metadata_at_version(16)` and
// all decoding is done in-process by stock `subxt-core` with metadata only —
// no node, no network.

use codec::{Decode, Encode};
use frame_metadata::{v16::RuntimeMetadataV16, RuntimeMetadata, RuntimeMetadataPrefixed};
use quip_protocol_runtime::{
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

fn metadata_v16() -> RuntimeMetadataV16 {
    let bytes = metadata_v16_bytes();
    let prefixed =
        RuntimeMetadataPrefixed::decode(&mut &bytes[..]).expect("metadata blob SCALE-decodes");
    match prefixed.1 {
        RuntimeMetadata::V16(v16) => v16,
        other => panic!("expected Metadata V16, got a different variant: {other:?}"),
    }
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

#[test]
fn legacy_metadata_runtime_api_publishes_v16() {
    // Regression pin: `state_getMetadata` is served by the legacy
    // `sp_api::Metadata::metadata()` runtime API implemented in
    // `runtime/src/apis.rs`, which must publish V16. The std-side trait impl
    // cannot be invoked natively (`impl_runtime_apis!` moves it onto the
    // client-side `RuntimeApiImpl`, which dispatches through `CallApiAt` and
    // an executor), so this test executes the runtime API's actual wasm entry
    // point — the same code path a node takes. Reverting `metadata()` to the
    // old V14 one-liner `OpaqueMetadata::new(Runtime::metadata().into())`
    // makes this test fail.
    let Some(wasm_binary) = quip_protocol_runtime::WASM_BINARY else {
        // The wasm blob is only embedded when the runtime was built without
        // SKIP_WASM_BUILD; CI builds it, local quick iterations may not.
        eprintln!("skipping: runtime wasm binary not built (SKIP_WASM_BUILD)");
        return;
    };

    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    let mut ext = ext.ext();
    let executor = sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build();
    let encoded = executor
        .uncached_call(
            sc_executor_common::runtime_blob::RuntimeBlob::uncompress_if_needed(wasm_binary)
                .expect("runtime wasm blob is valid"),
            &mut ext,
            false,
            "Metadata_metadata",
            &[],
        )
        .expect("Metadata_metadata runtime API call succeeds");
    let opaque =
        sp_core::OpaqueMetadata::decode(&mut &encoded[..]).expect("runtime API returns metadata");

    let bytes: &[u8] = &opaque;
    let prefixed = RuntimeMetadataPrefixed::decode(&mut &bytes[..])
        .expect("legacy metadata blob SCALE-decodes");
    assert!(
        matches!(prefixed.1, RuntimeMetadata::V16(_)),
        "legacy metadata() runtime API must publish Metadata V16",
    );
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
    let current = quip_protocol_runtime::VERSION;
    assert_eq!(current.spec_name, expected_name);
    let Some(wasm) = quip_protocol_runtime::WASM_BINARY else {
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
                System::set_code(quip_protocol_runtime::RuntimeOrigin::root(), other_network)
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
    use quip_protocol_runtime::configs::RUNTIME_IDENTITY_MISMATCH;
    use quip_protocol_runtime::{RuntimeOrigin, VERSION};
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
        let wasm = quip_protocol_runtime::WASM_BINARY
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

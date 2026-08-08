use quip_protocol_runtime::WASM_BINARY;
use sc_service::ChainType;

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec;

const LOCAL_CHAIN_ID_FEATURE_ERROR: &str = "local chain presets require a node built with the `dev-chain-id` feature (EIP-155 chain ID 1337); rebuild with `cargo build -p quip-network-node --features dev-chain-id`";
const TESTNET_CHAIN_ID_FEATURE_ERROR: &str = "the `quip-testnet` chain preset requires the default/testnet runtime artifact (EIP-155 chain ID 20033) and cannot run from a `dev-chain-id` build";

pub(crate) fn ensure_local_chain_id_feature() -> Result<(), String> {
    if cfg!(feature = "dev-chain-id") {
        Ok(())
    } else {
        Err(LOCAL_CHAIN_ID_FEATURE_ERROR.to_string())
    }
}

fn ensure_testnet_chain_id_feature() -> Result<(), String> {
    if cfg!(feature = "dev-chain-id") {
        Err(TESTNET_CHAIN_ID_FEATURE_ERROR.to_string())
    } else {
        Ok(())
    }
}

pub(crate) fn ensure_chain_spec_id_compatibility(id: &str) -> Result<(), String> {
    if id == "quip_testnet" {
        ensure_testnet_chain_id_feature()
    } else {
        ensure_local_chain_id_feature()
    }
}

pub fn development_chain_spec() -> Result<ChainSpec, String> {
    ensure_local_chain_id_feature()?;

    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        None,
    )
    .with_name("Development")
    .with_id("dev")
    .with_chain_type(ChainType::Development)
    .with_genesis_config_preset_name(sp_genesis_builder::DEV_RUNTIME_PRESET)
    .build())
}

pub fn local_chain_spec() -> Result<ChainSpec, String> {
    ensure_local_chain_id_feature()?;

    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        None,
    )
    .with_name("Local Testnet")
    .with_id("local_testnet")
    .with_chain_type(ChainType::Local)
    .with_genesis_config_preset_name(sp_genesis_builder::LOCAL_TESTNET_RUNTIME_PRESET)
    .build())
}

pub fn local_three_validator_chain_spec() -> Result<ChainSpec, String> {
    ensure_local_chain_id_feature()?;

    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Development wasm not available".to_string())?,
        None,
    )
    .with_name("Local Testnet (3 Validators)")
    .with_id("local_three_validator")
    .with_chain_type(ChainType::Local)
    .with_genesis_config_preset_name(
        quip_protocol_runtime::genesis_config_presets::LOCAL_THREE_VALIDATOR_RUNTIME_PRESET,
    )
    .build())
}

/// Canonical public quip-testnet chain spec.
///
/// The raw export of this builder (`quip-network-node export-chain-spec
/// --chain quip-testnet --raw`) is what `nodes.quip.network` publishes as
/// `chain-specs/quip-testnet.json`. The hosted JSON is the long-term source
/// of truth; this in-binary preset exists so the genesis is auditable and
/// can be re-derived from source.
pub fn quip_testnet_chain_spec() -> Result<ChainSpec, String> {
    ensure_testnet_chain_id_feature()?;

    let properties = {
        let mut p = sc_service::Properties::new();
        p.insert("tokenSymbol".into(), "AGLS".into());
        p.insert("tokenDecimals".into(), 12.into());
        p.insert("ss58Format".into(), 42.into());
        p
    };

    let boot_nodes = vec![
        "/dns4/bootnode-1.testnet.quip.network/tcp/30333/p2p/12D3KooWBdhB4xGX6hfFsNufqQsG99kekiH9kJhLSiui3RgatnpE"
            .parse()
            .expect("operator-1 testnet bootnode multiaddr is well-formed"),
        "/dns4/bootnode-2.testnet.quip.network/tcp/30333/p2p/12D3KooWPJAHo45AA94u3fYS3tXvyKouZnWihQnXWPHAzikXLfPW"
            .parse()
            .expect("operator-2 testnet bootnode multiaddr is well-formed"),
        "/dns4/bootnode-3.testnet.quip.network/tcp/30333/p2p/12D3KooWM6n7wYvett975UnLYXrvnBGqLk2DLJoCRoFxgXTkptWe"
            .parse()
            .expect("operator-3 testnet bootnode multiaddr is well-formed"),
    ];

    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Testnet wasm not available".to_string())?,
        None,
    )
    .with_name("Quip Testnet")
    .with_id("quip_testnet")
    .with_chain_type(ChainType::Live)
    .with_protocol_id("quip-testnet")
    .with_genesis_config_preset_name(
        quip_protocol_runtime::genesis_config_presets::QUIP_TESTNET_RUNTIME_PRESET,
    )
    .with_properties(properties)
    .with_boot_nodes(boot_nodes)
    .build())
}

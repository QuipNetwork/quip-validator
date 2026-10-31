use quip_protocol_runtime::WASM_BINARY;
use sc_service::ChainType;

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec;

pub fn development_chain_spec() -> Result<ChainSpec, String> {
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

pub fn validator_rehearsal_chain_spec() -> Result<ChainSpec, String> {
    Ok(ChainSpec::builder(
        WASM_BINARY.ok_or_else(|| "Rehearsal wasm not available".to_string())?,
        None,
    )
    .with_name("Validator Rehearsal (4 Validators)")
    .with_id("validator_rehearsal")
    .with_chain_type(ChainType::Local)
    .with_genesis_config_preset_name(
        quip_protocol_runtime::genesis_config_presets::REHEARSAL_RUNTIME_PRESET,
    )
    .build())
}

/// The byte-pinned public testnet artifact, including its historical runtime.
///
/// All live aliases load this file. Rebuilding a runtime preset with the current
/// WASM_BINARY cannot reproduce the published live genesis. The runtime's
/// `quip_testnet` preset remains registered for policy auditing only.
pub fn quip_testnet_chain_spec() -> Result<ChainSpec, String> {
    ChainSpec::from_json_bytes(include_bytes!("../../chain-specs/aglais-network.json").as_slice())
}

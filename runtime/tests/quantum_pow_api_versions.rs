//! Exercise the generated v3 client's `changed_in(3)` methods against frozen
//! responses served by AGLS API v2. This replays wire responses, not old Wasm
//! execution or a whole-state upgrade. No network is needed to run the tests.

use std::sync::Mutex;

use codec::{DecodeAll, Encode};
use frame_support::BoundedVec;
use pallet_quantum_pow::{types, Config, QuantumPowApi};
use quip_protocol_runtime::{apis::RuntimeApi, AccountId, Balance, Block, BlockNumber, Runtime};
use serde_json::Value;
use sp_api::{ApiError, ApiExt, CallApiAt, CallApiAtParams, ConstructRuntimeApi, RuntimeApiInfo};
use sp_core::{hashing::sha2_256, traits::CallContext, H256, U256};
use sp_runtime::traits::BlakeTwo256;
use sp_version::RuntimeVersion;

type Nodes = BoundedVec<u32, <Runtime as Config>::MaxNodes>;
type Edges = BoundedVec<(u32, u32), <Runtime as Config>::MaxEdges>;
type Values = BoundedVec<i32, <Runtime as Config>::MaxAllowedValues>;
type PowApi = dyn QuantumPowApi<Block, BlockNumber, AccountId, Balance, Nodes, Edges, Values>;

const FIXTURE: &[u8] = include_bytes!("fixtures/agls-api-v2-153726.json");
const SNAPSHOT: &[u8] = include_bytes!("fixtures/agls-mining-snapshot-api-v2-153726.scale");

fn bytes(value: &Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap().strip_prefix("0x").unwrap()).unwrap()
}

struct FrozenV2 {
    fixture: Value,
    at: H256,
    version: RuntimeVersion,
    calls: Mutex<Vec<(&'static str, Vec<u8>)>>,
}

impl FrozenV2 {
    fn new() -> Self {
        assert_eq!(
            hex::encode(sha2_256(FIXTURE)),
            "a75214537e26a5c59ab3fbc44ff7471360344fb14e84bb1475737dc77ba22dec"
        );
        let fixture: Value = serde_json::from_slice(FIXTURE).unwrap();
        let core = &fixture["calls"][0];
        assert_eq!(core["method"], "Core_version");
        let version = RuntimeVersion::decode_all(&mut &bytes(&core["output"])[..]).unwrap();
        assert_eq!(
            (version.spec_version, version.transaction_version),
            (119, 7)
        );
        assert_eq!(version.api_version(&<PowApi>::ID), Some(2));
        assert_eq!(<PowApi>::VERSION, 3);
        assert_eq!(hex::encode(<PowApi>::ID), "17f0e16273cf5f3b");
        for entry in fixture["calls"].as_array().unwrap() {
            let raw = if entry["output_file"].is_string() {
                assert_eq!(
                    entry["output_file"],
                    "agls-mining-snapshot-api-v2-153726.scale"
                );
                SNAPSHOT.to_vec()
            } else {
                bytes(&entry["output"])
            };
            assert_eq!(raw.len() as u64, entry["len"].as_u64().unwrap());
            assert_eq!(
                hex::encode(sha2_256(&raw)),
                entry["sha256"].as_str().unwrap()
            );
        }
        Self {
            at: H256::from_slice(&bytes(&fixture["at_block_hash"])),
            fixture,
            version,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn response(&self, method: &str, input: &[u8]) -> Vec<u8> {
        let entry = self.fixture["calls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| {
                entry["method"].as_str() == Some(method) && bytes(&entry["input"]) == input
            })
            .unwrap_or_else(|| panic!("uncaptured API call: {method}({})", hex::encode(input)));
        if entry["output_file"].is_string() {
            SNAPSHOT.to_vec()
        } else {
            bytes(&entry["output"])
        }
    }

    fn assert_wire<T: Encode>(&self, method: &'static str, input: Vec<u8>, value: &T) {
        assert_eq!(
            self.calls.lock().unwrap().last(),
            Some(&(method, input.clone()))
        );
        // The generated SDK client uses Decode, not DecodeAll. Exact re-encoding
        // additionally detects partial consumption of a mismatched wire shape.
        assert_eq!(value.encode(), self.response(method, &input));
    }
}

impl CallApiAt<Block> for FrozenV2 {
    // These are the pinned SDK's macro support exports. The adapter provides
    // no state backend: it only verifies requests and returns captured bytes.
    type StateBackend = sp_api::__private::InMemoryBackend<BlakeTwo256>;

    fn call_api_at(&self, params: CallApiAtParams<Block>) -> Result<Vec<u8>, ApiError> {
        assert_eq!(params.at, self.at);
        assert!(!params.function.contains("before_version"));
        let result = self.response(params.function, &params.arguments);
        self.calls
            .lock()
            .unwrap()
            .push((params.function, params.arguments));
        Ok(result)
    }

    fn runtime_version_at(&self, at: H256, _: CallContext) -> Result<RuntimeVersion, ApiError> {
        assert_eq!(at, self.at);
        Ok(self.version.clone())
    }

    fn state_at(&self, _: H256) -> Result<Self::StateBackend, ApiError> {
        panic!("frozen API response replay has no storage backend")
    }

    fn initialize_extensions(
        &self,
        at: H256,
        _: &mut sp_api::__private::Extensions,
    ) -> Result<(), ApiError> {
        assert_eq!(at, self.at);
        Ok(())
    }
}

#[test]
#[allow(deprecated)] // Deliberately exercise the generated compatibility methods.
fn changed_in_methods_route_and_decode_the_pinned_v2_responses() {
    let client = FrozenV2::new();
    let api = RuntimeApi::construct_runtime_api(&client);
    let at = client.at;
    let hash = H256::from_slice(&bytes(&client.fixture["topology_hash"]));
    assert_eq!(api.api_version::<PowApi>(at).unwrap(), Some(2));
    let difficulty = types::v2::DifficultyConfig {
        min_solutions: 1,
        max_energy_milli: -14_754_648,
        min_diversity_milli: 0,
    };
    let current = api.current_difficulty_before_version_3(at).unwrap();
    assert_eq!(current, difficulty);
    client.assert_wire("QuantumPowApi_current_difficulty", vec![], &current);
    let hardness = api.current_hardness_before_version_3(at).unwrap();
    assert_eq!(hardness, difficulty);
    client.assert_wire("QuantumPowApi_current_hardness", vec![], &hardness);
    for topology in [hash, H256::zero()] {
        let value = api.difficulty_for_before_version_3(at, topology).unwrap();
        assert_eq!(value, (topology == hash).then_some(difficulty));
        client.assert_wire("QuantumPowApi_difficulty_for", topology.encode(), &value);
    }

    let history: Value =
        serde_json::from_slice(include_bytes!("fixtures/agls-qblocks-v2-153726.json")).unwrap();
    let old_record = bytes(&history["entries"][0]["raw_value"]);
    assert_eq!(history["entries"][0]["qblock_block_number"], 1006);
    let check_record = |value: &Option<
        types::v2::QBlockWithNonce<AccountId, Balance, BlockNumber>,
    >| {
        let value = value.as_ref().unwrap();
        assert_eq!(value.solution.encode(), old_record);
        // Frozen state_call nonce, little-endian SCALE U256 (not re-derived by
        // the same production helper used by the runtime API).
        assert_eq!(
            value.nonce,
            U256::from_little_endian(
                &hex::decode("ba93abbccb98495140a61bced2adee78d37d0fa8c0b138aad17625fca00edd7e")
                    .unwrap()
            )
        );
    };
    let winning = api.winning_solution_before_version_3(at, 1006).unwrap();
    check_record(&winning);
    client.assert_wire("QuantumPowApi_winning_solution", 1006u32.encode(), &winning);
    let by_block = api.qblock_by_block_before_version_3(at, 1006).unwrap();
    check_record(&by_block);
    client.assert_wire("QuantumPowApi_qblock_by_block", 1006u32.encode(), &by_block);
    assert_eq!(client.fixture["sample_qblock_id"], 1);
    let by_id = api.qblock_by_id_before_version_3(at, 1).unwrap();
    check_record(&by_id);
    client.assert_wire("QuantumPowApi_qblock_by_id", 1u64.encode(), &by_id);
    let missing = api.winning_solution_before_version_3(at, 1005).unwrap();
    assert!(missing.is_none());
    client.assert_wire("QuantumPowApi_winning_solution", 1005u32.encode(), &missing);

    let graph = include_bytes!("fixtures/agls-topology-153726.scale");
    for topology in [None, Some(hash)] {
        let value = api.mining_snapshot_before_version_3(at, topology).unwrap();
        client.assert_wire("QuantumPowApi_mining_snapshot", topology.encode(), &value);
        let snapshot = value.unwrap();
        assert_eq!(snapshot.difficulty, difficulty);
        assert_eq!(snapshot.topology_hash, hash);
        assert_eq!(
            snapshot.last_proof_block_hash,
            H256::from_slice(
                &hex::decode("aadd43b422fbf15ec2593ac2e7c9e68119945d567d62e3829493fb7a5d542943")
                    .unwrap()
            )
        );
        assert_eq!((snapshot.nodes.len(), snapshot.edges.len()), (4577, 41514));
        // Cross-check the complete graph/spec fields against the separately
        // captured storage value, excluding its trailing registered_at u32.
        assert_eq!(
            (
                snapshot.nodes,
                snapshot.edges,
                snapshot.allowed_h_values,
                snapshot.allowed_j_values,
                snapshot.allowed_spin_values
            )
                .encode(),
            &graph[..graph.len() - 4]
        );
    }
    assert_eq!(client.calls.lock().unwrap().len(), 10);
}

#[test]
fn callers_must_select_the_versioned_method_before_decoding() {
    let client = FrozenV2::new();
    let api = RuntimeApi::construct_runtime_api(&client);
    assert_eq!(api.api_version::<PowApi>(client.at).unwrap(), Some(2));
    // changed_in provides alternate client methods; it is not an automatic
    // version router. An unversioned call still reaches the old export and its
    // permissive Decode silently consumes the first 8 of 16 bytes as an i64.
    let wrong = api.current_difficulty(client.at).unwrap();
    let raw = client.response("QuantumPowApi_current_difficulty", &[]);
    assert_eq!(
        wrong.max_energy_milli,
        i64::from_le_bytes(raw[..8].try_into().unwrap())
    );
    assert_ne!(wrong.max_energy_milli, -14_754_648);
    assert!(types::DifficultyConfig::decode_all(&mut &raw[..]).is_err());
}

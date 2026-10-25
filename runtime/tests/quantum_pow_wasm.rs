//! Execute the existing runtime APIs in native and Wasm engines from identical
//! state. These tests require an embedded Wasm binary: do not set SKIP_WASM_BUILD.
//! When using an external CARGO_TARGET_DIR, set WASM_BUILD_WORKSPACE_HINT to the
//! workspace root so the Wasm builder uses the same Cargo.lock as the native build.

use codec::{DecodeAll, Encode};
use frame_support::{assert_ok, BoundedVec};
use pallet_quantum_pow::{types, Config, RegisteredTopologies};
use quantum_validation::AllowedValueSpec;
use quip_protocol_runtime::{
    AccountId, BuildStorage, QuantumPow, Runtime, RuntimeGenesisConfig, RuntimeOrigin, System,
    Timestamp,
};
use sp_core::{
    traits::{CallContext, CodeExecutor, RuntimeCode, WrappedRuntimeCode},
    H256,
};

type Executor = sc_executor::WasmExecutor<sp_io::SubstrateHostFunctions>;

fn wasm_call(
    executor: &Executor,
    ext: &mut sp_io::TestExternalities,
    method: &str,
    data: &[u8],
) -> Vec<u8> {
    let wasm = quip_protocol_runtime::WASM_BINARY
        .expect("Wasm parity requires rebuilding without SKIP_WASM_BUILD");
    let code = WrappedRuntimeCode(wasm.into());
    let runtime = RuntimeCode {
        code_fetcher: &code,
        heap_pages: None,
        hash: sp_core::hashing::blake2_256(wasm).to_vec(),
    };
    let (result, used_native) =
        executor.call(&mut ext.ext(), &runtime, method, data, CallContext::Onchain);
    assert!(!used_native, "parity must execute actual Wasm");
    result.unwrap_or_else(|error| panic!("{method}: {error}"))
}

fn native_call(ext: &mut sp_io::TestExternalities, method: &str, data: &[u8]) -> Vec<u8> {
    ext.execute_with(|| quip_protocol_runtime::apis::api::dispatch(method, data).unwrap())
}

fn fixture() -> (sp_io::TestExternalities, H256) {
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    let hash = ext.execute_with(|| {
        System::set_block_number(199);
        System::initialize(&200, &H256::repeat_byte(0x45), &Default::default());
        let nodes: BoundedVec<u32, <Runtime as Config>::MaxNodes> =
            (0..16).collect::<Vec<_>>().try_into().unwrap();
        let edges: BoundedVec<(u32, u32), <Runtime as Config>::MaxEdges> = (0..16)
            .flat_map(|u| ((u + 1)..16).map(move |v| (u, v)))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let h = AllowedValueSpec::Set(vec![0].try_into().unwrap());
        let j = AllowedValueSpec::Set(vec![-100_000_000, 100_000_000].try_into().unwrap());
        let spins = AllowedValueSpec::Set(vec![-1000, 1000].try_into().unwrap());
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes,
            edges,
            h,
            j,
            spins,
            types::TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                expected_frustration_milli: 500,
            },
        ));
        let hash = RegisteredTopologies::<Runtime>::iter_keys().next().unwrap();
        pallet_quantum_pow::DefaultTopology::<Runtime>::put(hash);
        pallet_quantum_pow::MineableTopologies::<Runtime>::insert(hash, ());
        pallet_quantum_pow::LastProofBlock::<Runtime>::put(100);
        pallet_quantum_pow::LastProofBlockHash::<Runtime>::put(H256::repeat_byte(0x45));
        hash
    });
    (ext, hash)
}

#[test]
fn difficulty_api_matches_wasm_at_decay_gate_and_crossover() {
    let executor = Executor::builder().build();
    // K16: n=16, m=120, zero field, |J|=100M. Independent calibration:
    // round(-c * 100M * sqrt(15) * 16), c=0.75/0.725/0.7, gives
    // min=-4647580015, knee=-4492660682, max=-4337741348.
    // Start 40000 below the easy cap. Rate 79/1600, floor 1000 and epoch 100
    // cross to linear easing after 1344.050793764... overdue blocks.
    for (elapsed, room, expected) in [
        (0, 40_000, -4_337_781_348),
        (99, 40_000, -4_337_781_348),
        (100, 40_000, -4_337_781_348),
        (101, 40_000, -4_337_781_328),
        (200, 40_000, -4_337_779_373),
        (1_443, 40_000, -4_337_761_612),
        (1_444, 40_000, -4_337_761_602),
        (1_445, 40_000, -4_337_761_592),
        (1_446, 40_000, -4_337_761_582),
        (1_545, 40_000, -4_337_760_592),
        (10_100, 40_000, -4_337_741_348),
        // Rational one-epoch easing is 160800*79/1600 = 7939.5.
        // Pin the actual floating-point/libm path rounding it to 7940.
        (200, 160_800, -4_337_894_208),
    ] {
        let (mut native, hash) = fixture();
        let (mut wasm, wasm_hash) = fixture();
        assert_eq!(hash, wasm_hash);
        for ext in [&mut native, &mut wasm] {
            ext.execute_with(|| {
                System::set_block_number(100 + elapsed);
                pallet_quantum_pow::Difficulties::<Runtime>::insert(
                    hash,
                    types::DifficultyConfig {
                        max_energy_milli: -4_337_741_348 - room,
                    },
                );
            });
        }
        let method = "QuantumPowApi_difficulty_for";
        let native_bytes = native_call(&mut native, method, &hash.encode());
        let wasm_bytes = wasm_call(&executor, &mut wasm, method, &hash.encode());
        assert_eq!(native_bytes, wasm_bytes, "elapsed {elapsed}");
        let actual = Option::<types::DifficultyConfig>::decode_all(&mut &native_bytes[..])
            .unwrap()
            .unwrap();
        assert_eq!(actual.max_energy_milli, expected, "elapsed {elapsed}");
    }
}

#[test]
fn finalized_controller_state_and_header_match_wasm() {
    let executor = Executor::builder().build();
    // Independent SCALE/Blake3 oracle: inner seed is 32 parent bytes (0x45),
    // AccountId32 [1;32], then block 200 as LE32. Length 68 compact-encodes as
    // 0x1101; append elapsed LE64, interpret digest prefix BE64, reduce into
    // the inclusive timing band. Depth 1M makes hardening exactly rate*1000.
    for (previous, expected_rate) in [
        (0, 67),
        (200, 67),
        (201, 67),
        (199, 464),
        (141, 156),
        (140, 416),
        (139, 302),
        (101, 74),
        (100, 67),
        (99, 10),
    ] {
        let (mut native, hash) = fixture();
        let (mut wasm, wasm_hash) = fixture();
        assert_eq!(hash, wasm_hash);
        for ext in [&mut native, &mut wasm] {
            ext.execute_with(|| {
                // The timestamp inherent must be present before finalizing.
                // Slot0 and moment1 agree; no block-authoring claim is made.
                assert_ok!(Timestamp::set(RuntimeOrigin::none(), 1));
                pallet_quantum_pow::LastProofBlock::<Runtime>::put(previous);
                pallet_quantum_pow::BlockBestProof::<Runtime>::put(types::ProofRecord {
                    miner: AccountId::from([1; 32]),
                    submitted_at: 200,
                    energy_milli: -4_399_000_000,
                    anchor_milli: -12_000_000_000,
                    difficulty: types::DifficultyConfig {
                        max_energy_milli: -4_400_000_000,
                    },
                    salt: [0; 32],
                    topology_hash: hash,
                    device_access_time_us: 0,
                    frustration_milli: 500,
                    instance_bar_milli: -4_398_000_000,
                });
            });
        }
        let method = "BlockBuilder_finalize_block";
        let native_header = native_call(&mut native, method, &[]);
        let wasm_header = wasm_call(&executor, &mut wasm, method, &[]);
        // Includes the state root, not only the controller's numeric output.
        assert_eq!(native_header, wasm_header, "previous block {previous}");
        let read = || {
            (
                pallet_quantum_pow::Difficulties::<Runtime>::get(hash).unwrap(),
                pallet_quantum_pow::LastProofBlock::<Runtime>::get(),
                pallet_quantum_pow::BlockBestProof::<Runtime>::get(),
                QuantumPow::qblock_at(200).unwrap(),
            )
        };
        let native_state = native.execute_with(read);
        let wasm_state = wasm.execute_with(read);
        assert_eq!(native_state, wasm_state);
        assert_eq!(native_state.1, 200);
        assert!(native_state.2.is_none());
        assert_eq!(native_state.3.instance_bar_milli, -4_398_000_000);
        assert_eq!(native_state.3.energy_milli, -4_399_000_000);
        assert_eq!(native_state.3.difficulty.max_energy_milli, -4_400_000_000);
        assert_eq!(
            native_state.0.max_energy_milli,
            -4_400_000_000 - expected_rate * 1_000
        );
    }
}

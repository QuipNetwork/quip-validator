//! Execute the existing runtime APIs in native and Wasm engines from identical
//! state. These tests require an embedded Wasm binary: do not set SKIP_WASM_BUILD.
//! When using an external CARGO_TARGET_DIR, set WASM_BUILD_WORKSPACE_HINT to the
//! workspace root so the Wasm builder uses the same Cargo.lock as the native build.

use codec::{DecodeAll, Encode};
use frame_support::{assert_ok, BoundedVec};
use pallet_quantum_pow::{types, Config, RegisteredTopologies};
use quantum_validation::AllowedValueSpec;
use quip_protocol_runtime::{
    native_tx_extension, AccountId, Address, Balances, BuildStorage, EmissionController,
    QuantumPow, Runtime, RuntimeCall, RuntimeGenesisConfig, RuntimeOrigin, SignedPayload, System,
    Timestamp, UncheckedExtrinsic, EXISTENTIAL_DEPOSIT, UNIT,
};
use quip_transaction_crypto::{account_id_from_public, HybridPair, HybridTxSignature};
use sp_core::{
    traits::{CallContext, CodeExecutor, RuntimeCode, WrappedRuntimeCode},
    Pair as _, H256,
};
use sp_runtime::{generic, ApplyExtrinsicResult, DispatchError, StateVersion};

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

fn admission_fixture(miner: &AccountId) -> (sp_io::TestExternalities, H256) {
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    let hash = ext.execute_with(|| {
        System::set_block_number(199);
        System::initialize(&200, &H256::repeat_byte(0x45), &Default::default());
        assert_ok!(Balances::force_set_balance(
            RuntimeOrigin::root(),
            Address::Id(miner.clone()),
            1_000_000_000_000_000_000,
        ));
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(
            miner.clone()
        )));
        // K8 plus eight isolated nodes meets the production minimum of 16.
        // The deliberately overstated root width claim admits this small test
        // fixture as Heuristic; it is not evidence of computational hardness.
        let nodes = (0..16).collect::<Vec<_>>().try_into().unwrap();
        let edges = (0..8)
            .flat_map(|u| ((u + 1)..8).map(move |v| (u, v)))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes,
            edges,
            AllowedValueSpec::Set(vec![0].try_into().unwrap()),
            AllowedValueSpec::Set(vec![-1000, 1000].try_into().unwrap()),
            AllowedValueSpec::Set(vec![-1000, 1000].try_into().unwrap()),
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
        // Exactly 100 elapsed blocks: no decay. Both admitted sigma signs have
        // room for the 180-milli handicap around this synthetic baseline.
        pallet_quantum_pow::Difficulties::<Runtime>::insert(
            hash,
            types::DifficultyConfig {
                max_energy_milli: -10_000,
            },
        );
        hash
    });
    (ext, hash)
}

fn signed_call(
    ext: &mut sp_io::TestExternalities,
    pair: &HybridPair,
    call: RuntimeCall,
) -> UncheckedExtrinsic {
    ext.execute_with(|| {
        let account = account_id_from_public(&pair.public());
        let extension =
            native_tx_extension(generic::Era::Immortal, System::account_nonce(&account), 0);
        let payload = SignedPayload::new(call.clone(), extension.clone()).unwrap();
        let signature = payload.using_encoded(|bytes| HybridTxSignature::sign(pair, bytes));
        generic::UncheckedExtrinsic::new_signed(call, Address::Id(account), signature, extension)
            .into()
    })
}

#[test]
fn instance_admission_and_signed_proof_dispatch_match_wasm() {
    let executor = Executor::builder().build();
    let pair = HybridPair::from_seed_slice(&[7; 32]).unwrap();
    let miner = account_id_from_public(&pair.public());
    assert_eq!(
        hex::encode(miner.as_ref() as &[u8]),
        "808af44598815693be4133f83fee7662b14e4ade7ca81fb91b3932d4845e1a42"
    );
    // Frozen salt search with the production AccountId32 digest and 16-node
    // sampling. Independent star-triangle parity gives counts 3/18/1/20/0
    // over K=28-8+1=21. Round count*1000/21 to get F; sigma_micro=109108,
    // then truncate (500-F)*1000/109108. Exhaustive 8-spin enumeration supplied
    // the masks/energies; the isolated nodes contribute zero.
    for (salt_word, frustrated, phi, sigma, mask, energy, threshold) in [
        (802u32, 3, 143, 3, 89u8, -22_000i64, -10_180),
        (3716, 18, 857, -3, 2, -10_000, -9_820),
        (169764, 1, 48, 4, 88, -26_000, 0),
        (22772, 20, 952, -4, 21, -6_000, 0),
        (1669538, 0, 0, 4, 126, -28_000, 0),
    ] {
        // For each draw, a valid-length solution and a nonempty malformed one
        // distinguish instance rejection from later packed-solution rejection.
        for malformed in [false, true] {
            let (mut native, hash) = admission_fixture(&miner);
            let (mut wasm, wasm_hash) = admission_fixture(&miner);
            assert_eq!(hash, wasm_hash);
            let mut salt = [0; 32];
            salt[..4].copy_from_slice(&salt_word.to_le_bytes());
            let nonce = quantum_validation::derive_nonce(
                &[0x45; 32],
                &QuantumPow::account_to_bytes(&miner),
                &salt,
            );
            let topology =
                native.execute_with(|| RegisteredTopologies::<Runtime>::get(hash).unwrap());
            let (h, j) = quantum_validation::generate_ising_model(
                nonce,
                &topology.nodes,
                &topology.edges,
                &topology.allowed_h_values.as_slice(),
                &topology.allowed_j_values.as_slice(),
            )
            .unwrap();
            assert_eq!(h, vec![0; 16]);
            let count = topology
                .edges
                .iter()
                .enumerate()
                .skip(7)
                .filter(|(k, (u, v))| {
                    (j[(*u - 1) as usize] > 0) ^ (j[(*v - 1) as usize] > 0) ^ (j[*k] > 0)
                })
                .count();
            assert_eq!(count, frustrated);
            assert_eq!((count * 1000 + 10) / 21, phi as usize);
            let independent_energy: i64 = topology
                .edges
                .iter()
                .zip(&j)
                .map(|(&(u, v), &w)| {
                    i64::from(w)
                        * if ((mask >> u) ^ (mask >> v)) & 1 == 0 {
                            1
                        } else {
                            -1
                        }
                })
                .sum();
            assert_eq!(independent_energy, energy);

            let instance_error: Option<DispatchError> = if frustrated == 0 {
                Some(pallet_quantum_pow::Error::<Runtime>::InstanceIsGaugeTrivial.into())
            } else if sigma == 4 || sigma == -4 {
                Some(pallet_quantum_pow::Error::<Runtime>::InstanceOutsideFrustrationBand.into())
            } else {
                None
            };
            let api_input = (hash, miner.clone(), salt).encode();
            let native_check = native_call(&mut native, "QuantumPowApi_instance_check", &api_input);
            let wasm_check = wasm_call(
                &executor,
                &mut wasm,
                "QuantumPowApi_instance_check",
                &api_input,
            );
            assert_eq!(native_check, wasm_check, "instance salt {salt_word}");
            let check =
                Result::<types::InstanceCheck, DispatchError>::decode_all(&mut &native_check[..])
                    .unwrap();
            if let Some(error) = &instance_error {
                assert_eq!(check, Err(*error));
            } else {
                let check = check.unwrap();
                assert_eq!(check.nonce, nonce);
                assert_eq!(
                    (
                        check.frustration_milli,
                        check.frustration_cycles,
                        check.sigmas
                    ),
                    (phi, 21, sigma)
                );
                assert_eq!(
                    (
                        check.anchor_milli,
                        check.live_bar_milli,
                        check.instance_bar_milli
                    ),
                    (-28_000, -10_000, threshold)
                );
            }

            // Binary Set [-1000,1000] packs node i in bit i, low bit first.
            let proof = types::QuantumProof {
                topology_hash: hash,
                nonce,
                salt,
                solutions: if malformed { vec![mask] } else { vec![mask, 0] }
                    .try_into()
                    .unwrap(),
                device_access_time_us: 123,
            };
            let call = RuntimeCall::QuantumPow(pallet_quantum_pow::Call::submit_proof { proof });
            let encoded = signed_call(&mut native, &pair, call).encode();
            let native_result = native_call(&mut native, "BlockBuilder_apply_extrinsic", &encoded);
            let wasm_result = wasm_call(
                &executor,
                &mut wasm,
                "BlockBuilder_apply_extrinsic",
                &encoded,
            );
            assert_eq!(
                native_result, wasm_result,
                "dispatch salt {salt_word}, malformed {malformed}"
            );
            let expected_error = instance_error.or_else(|| {
                malformed.then(|| {
                    pallet_quantum_pow::Error::<Runtime>::PackedSolutionLengthMismatch.into()
                })
            });
            let result = ApplyExtrinsicResult::decode_all(&mut &native_result[..]).unwrap();
            assert_eq!(result, Ok(expected_error.map_or(Ok(()), Err)));
            let root = || sp_io::storage::root(StateVersion::V1);
            assert_eq!(native.execute_with(root), wasm.execute_with(root));
            native.execute_with(|| {
                let accepted = expected_error.is_none();
                assert_eq!(
                    pallet_quantum_pow::BlockProofCount::<Runtime>::get(),
                    u32::from(accepted)
                );
                assert_eq!(System::account_nonce(&miner), 1);
                let best = pallet_quantum_pow::BlockBestProof::<Runtime>::get();
                if accepted {
                    let best = best.unwrap();
                    assert_eq!(best.miner, miner);
                    assert_eq!(
                        (best.energy_milli, best.instance_bar_milli),
                        (energy, threshold)
                    );
                    assert_eq!(best.frustration_milli, phi);
                } else {
                    assert!(best.is_none());
                }
            });
        }
    }
}

fn apply_proof_in_both_engines(
    executor: &Executor,
    native: &mut sp_io::TestExternalities,
    wasm: &mut sp_io::TestExternalities,
    pair: &HybridPair,
    proof: types::QuantumProof<BoundedVec<u8, <Runtime as Config>::MaxNodes>>,
) {
    let call = RuntimeCall::QuantumPow(pallet_quantum_pow::Call::submit_proof { proof });
    let encoded = signed_call(native, pair, call).encode();
    let native_result = native_call(native, "BlockBuilder_apply_extrinsic", &encoded);
    let wasm_result = wasm_call(executor, wasm, "BlockBuilder_apply_extrinsic", &encoded);
    assert_eq!(native_result, wasm_result);
    assert_eq!(
        ApplyExtrinsicResult::decode_all(&mut &native_result[..]).unwrap(),
        Ok(Ok(()))
    );
    let root = || sp_io::storage::root(StateVersion::V1);
    assert_eq!(native.execute_with(root), wasm.execute_with(root));
}

fn ranking_proof(
    hash: H256,
    miner: &AccountId,
    strong: bool,
    device_access_time_us: u64,
) -> types::QuantumProof<BoundedVec<u8, <Runtime as Config>::MaxNodes>> {
    // Reuse the independently checked admission vectors above. Quality is
    // floor(depth * 10^15 / room): 11820/17820 for the stronger proof, versus
    // 180/18180 for the weaker one. Both pass admission.
    let (salt_word, mask) = if strong { (802u32, 89) } else { (3716, 2) };
    let mut salt = [0; 32];
    salt[..4].copy_from_slice(&salt_word.to_le_bytes());
    types::QuantumProof {
        topology_hash: hash,
        nonce: quantum_validation::derive_nonce(
            &[0x45; 32],
            &QuantumPow::account_to_bytes(miner),
            &salt,
        ),
        salt,
        solutions: vec![mask, 0].try_into().unwrap(),
        device_access_time_us,
    }
}

#[test]
fn signed_proof_competition_matches_wasm_in_both_orders_and_on_complete_ties() {
    let executor = Executor::builder().build();
    let pair = HybridPair::from_seed_slice(&[7; 32]).unwrap();
    let miner = account_id_from_public(&pair.public());
    for strong_first in [false, true] {
        let (mut native, hash) = admission_fixture(&miner);
        let (mut wasm, wasm_hash) = admission_fixture(&miner);
        assert_eq!(hash, wasm_hash);
        let mut first_strong = None;
        // The third proof ties the stronger proof completely in quality and
        // energy. Its distinct device time makes incumbent retention visible.
        for (index, strong) in [strong_first, !strong_first, true].into_iter().enumerate() {
            let marker = 100 + index as u64;
            let proof = ranking_proof(hash, &miner, strong, marker);
            apply_proof_in_both_engines(&executor, &mut native, &mut wasm, &pair, proof);
            if strong && first_strong.is_none() {
                first_strong = Some(marker);
            }
            let winner_strong = first_strong.is_some();
            let winner = ranking_proof(hash, &miner, winner_strong, first_strong.unwrap_or(100));
            native.execute_with(|| {
                assert_eq!(System::account_nonce(&miner), index as u32 + 1);
                assert_eq!(
                    pallet_quantum_pow::BlockProofCount::<Runtime>::get(),
                    index as u32 + 1
                );
                assert_eq!(
                    pallet_quantum_pow::Miners::<Runtime>::get(&miner)
                        .unwrap()
                        .proofs_submitted,
                    index as u32 + 1
                );
                assert_eq!(
                    pallet_quantum_pow::BlockBestProof::<Runtime>::get().unwrap(),
                    types::ProofRecord {
                        miner: miner.clone(),
                        submitted_at: 200,
                        energy_milli: if winner_strong { -22_000 } else { -10_000 },
                        anchor_milli: -28_000,
                        difficulty: types::DifficultyConfig {
                            max_energy_milli: -10_000
                        },
                        salt: winner.salt,
                        topology_hash: hash,
                        device_access_time_us: winner.device_access_time_us,
                        frustration_milli: if winner_strong { 143 } else { 857 },
                        instance_bar_milli: if winner_strong { -10_180 } else { -9_820 },
                    }
                );
            });
        }
    }
}

#[test]
fn signed_candidate_ranking_matches_wasm_against_synthetic_incumbents() {
    let executor = Executor::builder().build();
    let pair = HybridPair::from_seed_slice(&[7; 32]).unwrap();
    let miner = account_id_from_public(&pair.public());
    // The real candidate has E=-22000 and quality floor(11820*10^15/17820).
    // First isolate quality from energy: a lower-energy incumbent with quality
    // 1/6000 loses, while a higher-energy one with quality 11819/11820 wins.
    // Then translate all three energies by +/-1 to keep depth and room fixed:
    // exact quality ties must prefer the lower numerical energy in either order.
    for (energy, bar, anchor, candidate_wins) in [
        (-22_001, -22_000, -28_000, true),
        (-21_999, -10_180, -22_000, false),
        (-22_001, -10_181, -28_001, false),
        (-21_999, -10_179, -27_999, true),
    ] {
        let (mut native, hash) = admission_fixture(&miner);
        let (mut wasm, wasm_hash) = admission_fixture(&miner);
        assert_eq!(hash, wasm_hash);
        let original = ranking_proof(hash, &miner, true, 123);
        apply_proof_in_both_engines(&executor, &mut native, &mut wasm, &pair, original);
        let admitted =
            native.execute_with(|| pallet_quantum_pow::BlockBestProof::<Runtime>::get().unwrap());
        // Synthetic record, not a second realizable Ising solution. Only the
        // candidate is re-admitted through the signed production dispatch.
        let mut incumbent = admitted.clone();
        incumbent.energy_milli = energy;
        incumbent.anchor_milli = anchor;
        incumbent.instance_bar_milli = bar;
        for ext in [&mut native, &mut wasm] {
            ext.execute_with(|| pallet_quantum_pow::BlockBestProof::<Runtime>::put(&incumbent));
        }
        let candidate = ranking_proof(hash, &miner, true, 456);
        apply_proof_in_both_engines(&executor, &mut native, &mut wasm, &pair, candidate);
        let mut expected = if candidate_wins { admitted } else { incumbent };
        if candidate_wins {
            expected.device_access_time_us = 456;
        }
        native.execute_with(|| {
            assert_eq!(
                pallet_quantum_pow::BlockBestProof::<Runtime>::get().unwrap(),
                expected
            );
            assert_eq!(pallet_quantum_pow::BlockProofCount::<Runtime>::get(), 2);
            assert_eq!(System::account_nonce(&miner), 2);
        });
    }
}

#[test]
fn signed_winner_finalization_and_payout_match_wasm() {
    let executor = Executor::builder().build();
    let pair = HybridPair::from_seed_slice(&[7; 32]).unwrap();
    let miner = account_id_from_public(&pair.public());
    let pot = EmissionController::pot(pallet_emission_controller::ISING_SUBNET);
    // Available pot funds exclude the existential deposit. Cover zero,
    // partial and full payment without asserting scheduled emission accrual.
    for (available, reward) in [(0, 0), (UNIT / 2, UNIT / 2), (2 * UNIT, UNIT)] {
        for strong in [false, true] {
            let (mut native, hash) = admission_fixture(&miner);
            let (mut wasm, wasm_hash) = admission_fixture(&miner);
            assert_eq!(hash, wasm_hash);
            for ext in [&mut native, &mut wasm] {
                ext.execute_with(|| {
                    assert_ok!(Balances::force_set_balance(
                        RuntimeOrigin::root(),
                        Address::Id(pot.clone()),
                        EXISTENTIAL_DEPOSIT + available,
                    ));
                    // Same synthetic initialized-block setup as the controller
                    // test: moment 1 agrees with slot 0, not full block authoring.
                    assert_ok!(Timestamp::set(RuntimeOrigin::none(), 1));
                });
            }
            let proof = ranking_proof(hash, &miner, strong, 123);
            let salt = proof.salt;
            apply_proof_in_both_engines(&executor, &mut native, &mut wasm, &pair, proof);
            // Take the balance baseline after all signed-extension fees. The
            // finalization delta must be a transfer from the pot, not issuance.
            let (balance_before, issuance_before) = native
                .execute_with(|| (Balances::free_balance(&miner), Balances::total_issuance()));
            let native_header = native_call(&mut native, "BlockBuilder_finalize_block", &[]);
            let wasm_header = wasm_call(&executor, &mut wasm, "BlockBuilder_finalize_block", &[]);
            assert_eq!(
                native_header, wasm_header,
                "available {available}, strong {strong}"
            );
            let root = || sp_io::storage::root(StateVersion::V1);
            assert_eq!(native.execute_with(root), wasm.execute_with(root));
            native.execute_with(|| {
                assert_eq!(Balances::free_balance(&miner), balance_before + reward);
                assert_eq!(
                    Balances::free_balance(&pot),
                    EXISTENTIAL_DEPOSIT + available - reward
                );
                assert_eq!(Balances::total_issuance(), issuance_before);
                let info = pallet_quantum_pow::Miners::<Runtime>::get(&miner).unwrap();
                assert_eq!(info.proofs_submitted, 1);
                assert_eq!(info.proofs_won, 1);
                assert_eq!(info.rewards_earned, reward);
                assert!(pallet_quantum_pow::BlockBestProof::<Runtime>::get().is_none());
                assert_eq!(pallet_quantum_pow::LastProofBlock::<Runtime>::get(), 200);
                assert_eq!(pallet_quantum_pow::QBlockCount::<Runtime>::get(), 1);
                assert_eq!(
                    pallet_quantum_pow::QBlockBlockById::<Runtime>::get(1),
                    Some(200)
                );
                assert_eq!(
                    pallet_quantum_pow::QBlockIdByBlock::<Runtime>::get(200),
                    Some(1)
                );
                assert_eq!(
                    QuantumPow::qblock_at(200).unwrap(),
                    types::QBlock {
                        miner: miner.clone(),
                        salt,
                        energy_milli: if strong { -22_000 } else { -10_000 },
                        reward,
                        submitted_at: 200,
                        difficulty: types::DifficultyConfig {
                            max_energy_milli: -10_000
                        },
                        last_proof_block_hash: H256::repeat_byte(0x45),
                        topology_hash: hash,
                        device_access_time_us: 123,
                        frustration_milli: if strong { 143 } else { 857 },
                        instance_bar_milli: if strong { -10_180 } else { -9_820 },
                    }
                );
                // For n=16,m=28, |J|=1000,h=0, the calibration endpoints are
                // round(-c*16000*sqrt(3.5)) = -22450/-20953 (c=.75/.70).
                // Their span 1497 gives a 1000-milli hardening cap and floor.
                // Strong depth 11820 therefore hardens by 1000; weak depth 180
                // limits the step to 180. Using raw weak E=-10000 instead of
                // its translated depth would incorrectly produce no change.
                assert_eq!(
                    pallet_quantum_pow::Difficulties::<Runtime>::get(hash)
                        .unwrap()
                        .max_energy_milli,
                    if strong { -11_000 } else { -10_180 }
                );
            });
        }
    }
}

#[test]
fn current_api_exports_match_wasm_and_preserve_historical_response_meaning() {
    let executor = Executor::builder().build();
    let captured: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/agls-api-v2-153726.json")).unwrap();
    let history: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/agls-qblocks-v2-153726.json")).unwrap();
    let decode_hex = |value: &serde_json::Value| {
        hex::decode(value.as_str().unwrap().strip_prefix("0x").unwrap()).unwrap()
    };
    let hash = H256::from_slice(&decode_hex(&captured["topology_hash"]));
    let graph = include_bytes!("fixtures/agls-topology-153726.scale");
    let old_snapshot = include_bytes!("fixtures/agls-mining-snapshot-api-v2-153726.scale");
    let old_record = decode_hex(&history["entries"][0]["raw_value"]);
    assert_eq!(history["entries"][0]["qblock_block_number"], 1006);
    assert_eq!(old_record.len(), 180);
    let record_key = pallet_quantum_pow::QBlocks::<Runtime>::hashed_key_for(1006);
    let mut native = sp_io::TestExternalities::default();
    let mut wasm = sp_io::TestExternalities::default();
    for ext in [&mut native, &mut wasm] {
        ext.execute_with(|| {
            // Partial post-upgrade-shaped state, not a migration execution or
            // full historical snapshot. The legacy QBlock remains raw v6.
            System::set_block_number(153_726);
            sp_io::storage::set(
                &RegisteredTopologies::<Runtime>::hashed_key_for(hash),
                graph,
            );
            pallet_quantum_pow::DefaultTopology::<Runtime>::put(hash);
            pallet_quantum_pow::Difficulties::<Runtime>::insert(
                hash,
                types::DifficultyConfig {
                    max_energy_milli: -14_754_648,
                },
            );
            pallet_quantum_pow::LastProofBlock::<Runtime>::put(153_634);
            pallet_quantum_pow::LastProofBlockHash::<Runtime>::put(H256::from_slice(
                &old_snapshot[1..33],
            ));
            sp_io::storage::set(&record_key, &old_record);
            // QBlock1006 really has id1; only this sampled index is installed.
            pallet_quantum_pow::QBlockBlockById::<Runtime>::insert(1, 1006);
            pallet_quantum_pow::QBlockIdByBlock::<Runtime>::insert(1006, 1);
        });
    }
    let root = || sp_io::storage::root(StateVersion::V1);
    let initial_root = native.execute_with(root);
    assert_eq!(initial_root, wasm.execute_with(root));
    let mut query = |method: &str, input: &[u8], expected: &[u8]| {
        let before = native.execute_with(root);
        let result = native_call(&mut native, method, input);
        assert_eq!(
            result,
            wasm_call(&executor, &mut wasm, method, input),
            "{method}"
        );
        assert_eq!(result, expected, "{method}");
        assert_eq!(
            native.execute_with(root),
            before,
            "{method} wrote native state"
        );
        assert_eq!(wasm.execute_with(root), before, "{method} wrote Wasm state");
    };

    query(
        "Core_version",
        &[],
        &quip_protocol_runtime::VERSION.encode(),
    );
    assert_eq!(
        quip_protocol_runtime::VERSION
            .api_version(&[0x17, 0xf0, 0xe1, 0x62, 0x73, 0xcf, 0x5f, 0x3b]),
        Some(3)
    );
    let mut expected_qblock = Vec::new();
    let mut queries = 0;
    for entry in captured["calls"].as_array().unwrap() {
        let method = entry["method"].as_str().unwrap();
        if method == "Core_version" {
            continue;
        }
        let input = decode_hex(&entry["input"]);
        let old = if entry["output_file"].is_string() {
            old_snapshot.to_vec()
        } else {
            decode_hex(&entry["output"])
        };
        assert_eq!(
            hex::encode(sp_core::hashing::sha2_256(&old)),
            entry["sha256"].as_str().unwrap()
        );
        // Independent SCALE-layout oracle: remove the legacy u32 fields from
        // difficulty; for QBlocks also append unmeasured frustration=0 and D.
        // Keep the original nonce bytes. Do not use From<v2> or StoredQBlock.
        let expected = match method {
            "QuantumPowApi_current_difficulty" | "QuantumPowApi_current_hardness" => {
                old[4..12].to_vec()
            }
            "QuantumPowApi_difficulty_for" if old == [0] => old,
            "QuantumPowApi_difficulty_for" => [&old[..1], &old[5..13]].concat(),
            "QuantumPowApi_mining_snapshot" => [&old[..33], &old[37..45], &old[49..]].concat(),
            "QuantumPowApi_winning_solution"
            | "QuantumPowApi_qblock_by_block"
            | "QuantumPowApi_qblock_by_id" => {
                if old == [0] {
                    old
                } else {
                    assert_eq!(old.len(), 213);
                    let q = &old[1..181];
                    assert_eq!(q, old_record);
                    let expected = [
                        &old[..1],
                        &q[..92],
                        &q[96..104],
                        &q[108..],
                        &[0u8; 4],
                        &q[96..104],
                        &old[181..],
                    ]
                    .concat();
                    assert_eq!(expected.len(), 217);
                    let value = Option::<types::QBlockWithNonce<AccountId, u128, u32>>::decode_all(
                        &mut &expected[..],
                    )
                    .unwrap()
                    .unwrap();
                    assert_eq!(value.solution.frustration_milli, 0);
                    assert_eq!(value.solution.instance_bar_milli, -14_700_000);
                    assert_eq!(value.solution.difficulty.max_energy_milli, -14_700_000);
                    expected_qblock = expected.clone();
                    expected
                }
            }
            _ => panic!("unexpected captured method {method}"),
        };
        query(method, &input, &expected);
        queries += 1;
    }
    assert_eq!(queries, 10);
    for ext in [&mut native, &mut wasm] {
        ext.execute_with(|| {
            assert_eq!(sp_io::storage::root(StateVersion::V1), initial_root);
            assert_eq!(
                sp_io::storage::get(&record_key).unwrap().as_ref(),
                old_record
            );
        });
    }

    // Nonzero measured fields distinguish the current-layout branch from the
    // historical backfill above. These bytes are a test record, not chain data.
    expected_qblock[173..177].copy_from_slice(&501u32.to_le_bytes());
    expected_qblock[177..185].copy_from_slice(&(-14_700_180i64).to_le_bytes());
    for ext in [&mut native, &mut wasm] {
        ext.execute_with(|| sp_io::storage::set(&record_key, &expected_qblock[1..185]));
    }
    let current_root = native.execute_with(root);
    for (method, input) in [
        ("QuantumPowApi_winning_solution", 1006u32.encode()),
        ("QuantumPowApi_qblock_by_block", 1006u32.encode()),
        ("QuantumPowApi_qblock_by_id", 1u64.encode()),
    ] {
        assert_eq!(native_call(&mut native, method, &input), expected_qblock);
        assert_eq!(
            wasm_call(&executor, &mut wasm, method, &input),
            expected_qblock
        );
        assert_eq!(native.execute_with(root), current_root);
        assert_eq!(wasm.execute_with(root), current_root);
    }
}

//! Opt-in whole-runtime rehearsal using a complete, root-checked RPC snapshot.
//! See docs/release.md for capture/run commands. Never contacts a live chain.
#![cfg(feature = "try-runtime")]

use std::collections::BTreeMap;

use codec::{DecodeAll, Encode};
use frame_support::{
    traits::{GetStorageVersion, StorageVersion},
    weights::Weight,
};
use pallet_quantum_pow::{
    types, Difficulties, QBlockBlockById, QBlockCount, QBlockIdByBlock, QBlocks,
    RegisteredTopologies, TopologyDims, TopologyHardnessOf,
};
use quip_protocol_runtime::{AccountId, Balance, BlockNumber, Header, QuantumPow, Runtime, System};
use serde::Deserialize;
use sp_core::{
    storage::{ChildInfo, Storage, StorageChild},
    traits::{CallContext, CodeExecutor, RuntimeCode, WrappedRuntimeCode},
    H256,
};
use sp_runtime::{traits::Header as _, StateVersion};

type RawState = BTreeMap<Vec<u8>, Vec<u8>>;

#[derive(Deserialize)]
struct ChildSnapshot {
    key: String,
    entries: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct Snapshot {
    block_hash: H256,
    header: Header,
    genesis_hash: H256,
    runtime_version: serde_json::Value,
    top: Vec<(String, String)>,
    children: Vec<ChildSnapshot>,
}

fn bytes(value: &str) -> Vec<u8> {
    hex::decode(value.strip_prefix("0x").expect("hex prefix")).expect("valid hex")
}

fn raw_state(pairs: &[(String, String)]) -> RawState {
    let mut result = RawState::new();
    for (key, value) in pairs {
        assert!(
            result.insert(bytes(key), bytes(value)).is_none(),
            "duplicate key"
        );
    }
    result
}

impl Snapshot {
    fn externalities(&self) -> sp_io::TestExternalities {
        let mut storage = Storage {
            top: raw_state(&self.top),
            ..Default::default()
        };
        let child_prefix = b":child_storage:default:";
        for child in &self.children {
            let key = bytes(&child.key);
            let id = key.strip_prefix(child_prefix).expect("default child trie");
            // TestExternalities derives each child root from its entries. Keep
            // no supplied root in top storage that could conceal missing data.
            assert!(storage.top.remove(&key).is_some(), "missing child root key");
            assert!(storage
                .children_default
                .insert(
                    id.to_vec(),
                    StorageChild {
                        data: raw_state(&child.entries),
                        child_info: ChildInfo::new_default(id),
                    }
                )
                .is_none());
        }
        assert!(storage
            .top
            .keys()
            .all(|k| !k.starts_with(b":child_storage:")));
        let code = storage
            .top
            .get(b":code".as_slice())
            .expect("on-chain code")
            .clone();
        // The constructor overwrites :code, so explicitly preserve the captured
        // bytes until the full pre-upgrade root has been verified.
        let mut ext =
            sp_io::TestExternalities::new_with_code_and_state(&code, storage, StateVersion::V1);
        let root = ext.execute_with(|| sp_io::storage::root(StateVersion::V1));
        assert_eq!(
            root,
            self.header.state_root().as_bytes(),
            "snapshot state root"
        );
        println!("verified snapshot root: 0x{}", hex::encode(root));
        ext.execute_with(|| {
            assert_eq!(System::block_number(), *self.header.number());
            assert_eq!(System::block_hash(0), self.genesis_hash);
            assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
        });
        ext
    }
}

fn top_state(ext: &mut sp_io::TestExternalities) -> RawState {
    ext.execute_with(|| {
        let mut entries = RawState::new();
        // Include the empty key if present; next_key is exclusive.
        if let Some(value) = sp_io::storage::get(&[]) {
            entries.insert(Vec::new(), value.to_vec());
        }
        let mut previous = Vec::new();
        while let Some(key) = sp_io::storage::next_key(&previous) {
            entries.insert(key.clone(), sp_io::storage::get(&key).unwrap().to_vec());
            previous = key;
        }
        entries
    })
}

fn report_storage_versions(ext: &mut sp_io::TestExternalities) {
    ext.execute_with(|| {
        macro_rules! report {
            ($($pallet:ident),* $(,)?) => {$({
                type P = quip_protocol_runtime::$pallet;
                eprintln!("{}: on-chain={:?}; in-code={:?}", stringify!($pallet),
                    P::on_chain_storage_version(), P::in_code_storage_version());
            })*};
        }
        report!(
            System,
            Timestamp,
            Babe,
            Grandpa,
            Balances,
            TransactionPayment,
            Sudo,
            Template,
            Xqvm,
            QuantumComputeMempool,
            QuantumPow,
            FaucetOps,
            Session,
            MinerRegistry,
            Revive,
            EvmChainId,
            Multisig,
            Utility,
            Proxy,
            EmissionController,
            Foundation,
            FoundationMembership,
            ValidatorAdmission,
            Staking,
            Historical
        );
    });
}

fn verify_pow_state(ext: &mut sp_io::TestExternalities, before: &RawState) -> usize {
    ext.execute_with(|| {
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(7));
        let blocks: Vec<_> = QBlocks::<Runtime>::iter_keys().collect();
        assert!(!blocks.is_empty());
        assert_eq!(QBlockCount::<Runtime>::get(), blocks.len() as u64);
        assert_eq!(QBlockBlockById::<Runtime>::iter().count(), blocks.len());
        assert_eq!(QBlockIdByBlock::<Runtime>::iter().count(), blocks.len());
        for block in &blocks {
            let key = QBlocks::<Runtime>::hashed_key_for(block);
            let raw = before.get(&key).expect("history existed before upgrade");
            assert_eq!(raw.len(), 180, "captured storage-v6 history");
            assert_eq!(sp_io::storage::get(&key).unwrap().as_ref(), raw.as_slice());
            let old =
                types::v2::QBlock::<AccountId, Balance, BlockNumber>::decode_all(&mut &raw[..])
                    .unwrap();
            let current = QuantumPow::qblock_at(*block).unwrap();
            assert_eq!(current.miner, old.miner);
            assert_eq!(current.salt, old.salt);
            assert_eq!(current.last_proof_block_hash, old.last_proof_block_hash);
            assert_eq!(current.energy_milli, old.energy_milli);
            assert_eq!(current.reward, old.reward);
            assert_eq!(current.submitted_at, old.submitted_at);
            assert_eq!(current.topology_hash, old.topology_hash);
            assert_eq!(current.device_access_time_us, old.device_access_time_us);
            assert_eq!(
                current.difficulty.max_energy_milli,
                old.difficulty.max_energy_milli
            );
            assert_eq!(current.instance_bar_milli, old.difficulty.max_energy_milli);
            assert_eq!(current.frustration_milli, 0);
            let id = QBlockIdByBlock::<Runtime>::get(block).unwrap();
            assert_eq!(QBlockBlockById::<Runtime>::get(id), Some(*block));
            for index_key in [
                QBlockIdByBlock::<Runtime>::hashed_key_for(block),
                QBlockBlockById::<Runtime>::hashed_key_for(id),
            ] {
                assert_eq!(
                    sp_io::storage::get(&index_key).unwrap().as_ref(),
                    before[&index_key]
                );
            }
        }
        for id in 1..=QBlockCount::<Runtime>::get() {
            assert!(QBlockBlockById::<Runtime>::contains_key(id));
        }
        let topologies: Vec<_> = RegisteredTopologies::<Runtime>::iter().collect();
        assert_eq!(topologies.len(), 1, "pinned AGLS topology inventory");
        for (hash, meta) in topologies {
            // The pinned graph is already canonical; every byte must survive.
            let key = RegisteredTopologies::<Runtime>::hashed_key_for(hash);
            assert_eq!(sp_io::storage::get(&key).unwrap().as_ref(), before[&key]);
            assert_eq!((meta.nodes.len(), meta.edges.len()), (4577, 41514));
            let dims = TopologyDims::<Runtime>::get(hash).unwrap();
            assert_eq!((dims.nodes, dims.edges), (4577, 41514));
            let hardness = TopologyHardnessOf::<Runtime>::get(hash).unwrap();
            assert_eq!(hardness.residual_difficulty, u32::MAX);
            assert_eq!(hardness.core_width, u32::MAX);
            assert_eq!(hardness.expected_frustration_milli, 500);
            let key = Difficulties::<Runtime>::hashed_key_for(hash);
            let old = before.get(&key).unwrap();
            assert_eq!(old.len(), 16);
            assert_eq!(sp_io::storage::get(&key).unwrap().as_ref(), &old[4..12]);
        }
        blocks.len()
    })
}

#[test]
#[ignore = "requires QUIP_UPGRADE_SNAPSHOT from capture_runtime_upgrade_state.py"]
fn whole_runtime_upgrade_matches_wasm_on_pinned_state() {
    let path = std::env::var("QUIP_UPGRADE_SNAPSHOT").expect("QUIP_UPGRADE_SNAPSHOT is required");
    let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(snapshot.header.hash(), snapshot.block_hash);
    assert_eq!(*snapshot.header.number(), 156607);
    assert_eq!(
        snapshot.block_hash,
        H256::from_slice(&bytes(
            "0x0df326c87aee42ebfea81e5b2e8761e90af39c9e5a827008ecef905a2d934a6b",
        ))
    );
    assert_eq!(snapshot.runtime_version["specVersion"], 119);
    assert_eq!(snapshot.runtime_version["transactionVersion"], 7);
    assert_eq!(snapshot.runtime_version["stateVersion"], 1);
    // Reuse only trie nodes reconstructed and verified above, not externally
    // supplied raw trie nodes. This avoids hashing a million keys twice.
    let (nodes, root) = snapshot.externalities().into_raw_snapshot();
    let mut native =
        sp_io::TestExternalities::from_raw_snapshot(nodes.clone(), root, StateVersion::V1);
    let mut wasm = sp_io::TestExternalities::from_raw_snapshot(nodes, root, StateVersion::V1);
    let before = top_state(&mut native);
    let executor = sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build();
    let old_code = before.get(b":code".as_slice()).unwrap();
    let old_wrapped = WrappedRuntimeCode(old_code.as_slice().into());
    let old_runtime_code = RuntimeCode {
        code_fetcher: &old_wrapped,
        heap_pages: None,
        hash: sp_core::hashing::blake2_256(old_code).to_vec(),
    };
    let (old_result, used_native) = executor.call(
        &mut wasm.ext(),
        &old_runtime_code,
        "Core_version",
        &[],
        CallContext::Onchain,
    );
    assert!(!used_native);
    let old_output = old_result.expect("captured on-chain Wasm Core_version");
    let old_version = sp_version::RuntimeVersion::decode_all(&mut &old_output[..]).unwrap();
    assert_eq!(
        (old_version.spec_version, old_version.transaction_version),
        (119, 7)
    );
    assert_eq!(
        old_version
            .apis
            .iter()
            .find(|(id, _)| *id == [0x17, 0xf0, 0xe1, 0x62, 0x73, 0xcf, 0x5f, 0x3b])
            .unwrap()
            .1,
        2
    );
    assert_eq!(
        top_state(&mut wasm),
        before,
        "old Core_version is read-only"
    );
    println!("captured on-chain Wasm verified: spec119 / tx7 / QuantumPowApi v2");
    let code = quip_protocol_runtime::WASM_BINARY.expect("rebuild without SKIP_WASM_BUILD");
    for ext in [&mut native, &mut wasm] {
        ext.execute_with(|| sp_io::storage::set(b":code", code));
    }
    let wrapped = WrappedRuntimeCode(code.into());
    let runtime_code = RuntimeCode {
        code_fetcher: &wrapped,
        heap_pages: None,
        hash: sp_core::hashing::blake2_256(code).to_vec(),
    };
    let method = "TryRuntime_on_runtime_upgrade";
    let input = frame_try_runtime::UpgradeCheckSelect::All.encode();
    let mut upgraded_root = None;
    for pass in 1..=2 {
        println!("pass={pass}: native upgrade with all checks");
        let native_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            native.execute_with(|| {
                quip_protocol_runtime::apis::api::dispatch(method, &input).unwrap()
            })
        }));
        println!("pass={pass}: Wasm upgrade with all checks");
        let (wasm_result, used_native) = executor.call(
            &mut wasm.ext(),
            &runtime_code,
            method,
            &input,
            CallContext::Onchain,
        );
        assert!(!used_native);
        if native_result.is_err() || wasm_result.is_err() {
            eprintln!(
                "native failed={}; Wasm error={:?}",
                native_result.is_err(),
                wasm_result.as_ref().err()
            );
            eprintln!("native storage versions after attempted upgrade:");
            report_storage_versions(&mut native);
            eprintln!("Wasm storage versions after attempted upgrade:");
            report_storage_versions(&mut wasm);
        }
        let native_output = native_result.expect("whole-runtime native upgrade with all checks");
        let wasm_output = wasm_result.expect("whole-runtime Wasm upgrade with all checks");
        assert_eq!(native_output, wasm_output, "reported migration weight");
        let (weight, limit) = <(Weight, Weight)>::decode_all(&mut &native_output[..]).unwrap();
        assert!(
            weight.all_lte(limit),
            "migration exceeds block limit: {weight:?} > {limit:?}"
        );
        let history_count = verify_pow_state(&mut native, &before);
        assert_eq!(verify_pow_state(&mut wasm, &before), history_count);
        let native_root = native.execute_with(|| sp_io::storage::root(StateVersion::V1));
        let wasm_root = wasm.execute_with(|| sp_io::storage::root(StateVersion::V1));
        assert_eq!(native_root, wasm_root, "whole-state post-upgrade root");
        if let Some(previous) = &upgraded_root {
            assert_eq!(
                &native_root, previous,
                "second upgrade must not mutate state"
            );
        }
        upgraded_root = Some(native_root.clone());
        println!(
            "pass={pass}; history={history_count}; weight={weight:?}; limit={limit:?}; root=0x{}",
            hex::encode(native_root)
        );
    }
    let after = top_state(&mut native);
    for (key, value) in &after {
        if before.get(key) != Some(value) {
            println!("changed key=0x{}; bytes={}", hex::encode(key), value.len());
        }
    }
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        println!("removed key=0x{}", hex::encode(key));
    }
    println!(
        "snapshot verified: top={} children={}",
        snapshot.top.len(),
        snapshot.children.len()
    );
}

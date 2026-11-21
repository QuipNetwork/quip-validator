use crate::{
    mock::*, CpuInfo, Error, Event, GpuInfo, LatestParticipation, LogLevel, MinerKind, MinerSpec,
    NodeDescriptorInput, NodeDescriptorV1Input, NodeDescriptorV2Input, NodeDescriptors, OsInfo,
    RuntimeInfo, SystemInfo,
};
use codec::{Decode, Encode};
use frame_support::{assert_noop, assert_ok, BoundedVec};

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: frame_support::traits::Get<u32>,
{
    items.try_into().ok().unwrap()
}

fn bytes<const N: u32>(value: &[u8]) -> BoundedVec<u8, frame_support::traits::ConstU32<N>> {
    bounded(value.to_vec())
}

fn miner_spec() -> crate::MinerSpecOf<Test> {
    MinerSpec {
        kind: MinerKind::Cpu,
        label: Some(bytes::<64>(b"cpu-0")),
        backend: Some(bytes::<32>(b"local")),
        device_id: Some(bytes::<64>(b"0")),
    }
}

fn v1_input() -> crate::NodeDescriptorV1InputOf<Test> {
    NodeDescriptorV1Input {
        node_id: bytes::<64>(b"node-1"),
        node_name: bytes::<64>(b"Node One"),
        public_host: Some(bytes::<253>(b"miner.example.com")),
        public_port: Some(20050),
        rpc_endpoints: bounded(vec![bytes::<256>(b"https://miner.example.com/rpc")]),
        auto_mine: true,
        log_level: LogLevel::Info,
        miners: bounded(vec![miner_spec()]),
    }
}

fn descriptor() -> crate::NodeDescriptorInputOf<Test> {
    NodeDescriptorInput::V1(v1_input())
}

fn system_info() -> crate::SystemInfoOf<Test> {
    SystemInfo {
        os: OsInfo {
            system: bytes::<64>(b"Linux"),
            release: bytes::<64>(b"6.1.0-rpi"),
            machine: bytes::<64>(b"x86_64"),
        },
        cpu: CpuInfo {
            logical_cores: Some(16),
            physical_cores: Some(8),
            brand: bytes::<96>(b"AMD EPYC 7763"),
            arch: bytes::<16>(b"x86_64"),
        },
        memory_mb: Some(64_000),
        gpus: bounded(vec![GpuInfo {
            index: 0,
            vendor: bytes::<16>(b"NVIDIA"),
            name: bytes::<96>(b"NVIDIA H100 80GB HBM3"),
            memory_mb: Some(81_920),
            utilization_pct: Some(50),
        }]),
    }
}

fn runtime_info() -> crate::RuntimeInfoOf<Test> {
    RuntimeInfo {
        python: bytes::<48>(b"3.13.1"),
        quip_version: bytes::<48>(b"0.2.1"),
        protocol_version: 2,
        in_docker: true,
        docker_image: Some(bytes::<256>(b"quip-node:rc")),
    }
}

fn v2_input() -> crate::NodeDescriptorV2InputOf<Test> {
    let v1 = v1_input();
    NodeDescriptorV2Input {
        node_id: v1.node_id,
        node_name: v1.node_name,
        public_host: v1.public_host,
        public_port: v1.public_port,
        rpc_endpoints: v1.rpc_endpoints,
        auto_mine: v1.auto_mine,
        log_level: v1.log_level,
        miners: v1.miners,
        system_info: Some(system_info()),
        runtime: Some(runtime_info()),
    }
}

#[test]
fn set_descriptor_stores_typed_descriptor_and_reserves_deposit() {
    new_test_ext().execute_with(|| {
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        assert_eq!(stored.schema_version, crate::NODE_DESCRIPTOR_SCHEMA_V1);
        assert_eq!(stored.node_id.as_slice(), b"node-1");
        assert_eq!(stored.node_name.as_slice(), b"Node One");
        assert_eq!(stored.public_port, Some(20050));
        assert_eq!(stored.miners.len(), 1);
        assert_ne!(stored.payload_hash, sp_core::H256::zero());
        assert!(stored.system_info.is_none());
        assert_eq!(Balances::reserved_balance(1), stored.deposit);

        System::assert_last_event(
            Event::DescriptorUpdated {
                who: 1,
                payload_hash: stored.payload_hash,
                payload_len: 71,
            }
            .into(),
        );
    });
}

#[test]
fn set_descriptor_rejects_empty_required_fields() {
    new_test_ext().execute_with(|| {
        let mut input = v1_input();
        input.node_id = bytes::<64>(b"");

        assert_noop!(
            MinerRegistry::set_descriptor(RuntimeOrigin::signed(1), NodeDescriptorInput::V1(input)),
            Error::<Test>::EmptyNodeId
        );
    });
}

#[test]
fn set_descriptor_rejects_empty_optional_strings_when_present() {
    new_test_ext().execute_with(|| {
        let mut input = v1_input();
        input.rpc_endpoints = bounded(vec![bytes::<256>(b"")]);

        assert_noop!(
            MinerRegistry::set_descriptor(RuntimeOrigin::signed(1), NodeDescriptorInput::V1(input)),
            Error::<Test>::EmptyRpcEndpoint
        );
    });
}

#[test]
fn set_descriptor_rejects_descriptor_without_miners() {
    new_test_ext().execute_with(|| {
        let mut input = v1_input();
        input.miners = bounded(vec![]);

        assert_noop!(
            MinerRegistry::set_descriptor(RuntimeOrigin::signed(1), NodeDescriptorInput::V1(input)),
            Error::<Test>::NoMiners
        );
    });
}

#[test]
fn set_descriptor_rejects_zero_public_port() {
    new_test_ext().execute_with(|| {
        let mut input = v1_input();
        input.public_port = Some(0);

        assert_noop!(
            MinerRegistry::set_descriptor(RuntimeOrigin::signed(1), NodeDescriptorInput::V1(input)),
            Error::<Test>::InvalidPort
        );
    });
}

#[test]
fn updating_descriptor_adjusts_reserved_deposit() {
    new_test_ext().execute_with(|| {
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        let first_reserved = Balances::reserved_balance(1);

        let mut input = v1_input();
        input.public_host = None;
        input.rpc_endpoints = bounded(vec![]);
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V1(input)
        ));

        let second_reserved = Balances::reserved_balance(1);
        assert!(second_reserved < first_reserved);
        assert_eq!(
            NodeDescriptors::<Test>::get(1)
                .expect("descriptor stored")
                .deposit,
            second_reserved
        );
    });
}

#[test]
fn clear_descriptor_removes_storage_and_unreserves_deposit() {
    new_test_ext().execute_with(|| {
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        assert!(Balances::reserved_balance(1) > 0);

        assert_ok!(MinerRegistry::clear_descriptor(RuntimeOrigin::signed(1)));

        assert!(NodeDescriptors::<Test>::get(1).is_none());
        assert_eq!(Balances::reserved_balance(1), 0);
        System::assert_last_event(Event::DescriptorCleared { who: 1 }.into());
    });
}

#[test]
fn clear_descriptor_drops_participation_record() {
    new_test_ext().execute_with(|| {
        set_latest_qblock_id(Some(4));
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        assert_ok!(MinerRegistry::participate(
            RuntimeOrigin::signed(1),
            5,
            MinerKind::Cpu,
            None,
        ));
        assert!(LatestParticipation::<Test>::get(1).is_some());

        assert_ok!(MinerRegistry::clear_descriptor(RuntimeOrigin::signed(1)));

        assert!(LatestParticipation::<Test>::get(1).is_none());
    });
}

#[test]
fn participate_requires_descriptor() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            MinerRegistry::participate(RuntimeOrigin::signed(1), 1, MinerKind::Cpu, None),
            Error::<Test>::DescriptorRequired
        );
    });
}

#[test]
fn participate_records_current_candidate_qblock() {
    new_test_ext().execute_with(|| {
        set_latest_qblock_id(Some(4));
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));

        assert_ok!(MinerRegistry::participate(
            RuntimeOrigin::signed(1),
            5,
            MinerKind::QpuDwave,
            Some(30),
        ));

        let stored = LatestParticipation::<Test>::get(1).expect("participation stored");
        assert_eq!(stored.qblock_id, 5);
        assert_eq!(stored.kind, MinerKind::QpuDwave);
        assert_eq!(stored.budget_seconds, Some(30));
        System::assert_last_event(
            Event::MinerParticipated {
                qblock_id: 5,
                who: 1,
                kind: MinerKind::QpuDwave,
                budget_seconds: Some(30),
            }
            .into(),
        );
    });
}

#[test]
fn participate_rejects_stale_or_future_qblock_id() {
    new_test_ext().execute_with(|| {
        set_latest_qblock_id(Some(4));
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));

        assert_noop!(
            MinerRegistry::participate(RuntimeOrigin::signed(1), 4, MinerKind::Cpu, None),
            Error::<Test>::InvalidQBlockId
        );
        assert_noop!(
            MinerRegistry::participate(RuntimeOrigin::signed(1), 6, MinerKind::Cpu, None),
            Error::<Test>::InvalidQBlockId
        );
    });
}

#[test]
fn participate_rejects_duplicate_for_same_qblock() {
    new_test_ext().execute_with(|| {
        set_latest_qblock_id(None);
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        assert_ok!(MinerRegistry::participate(
            RuntimeOrigin::signed(1),
            1,
            MinerKind::Cpu,
            None
        ));

        assert_noop!(
            MinerRegistry::participate(RuntimeOrigin::signed(1), 1, MinerKind::Cpu, None),
            Error::<Test>::DuplicateParticipation
        );

        set_latest_qblock_id(Some(1));
        assert_ok!(MinerRegistry::participate(
            RuntimeOrigin::signed(1),
            2,
            MinerKind::Cpu,
            None
        ));
    });
}

#[test]
fn participants_by_qblock_lists_all_sorted_with_pagination() {
    new_test_ext().execute_with(|| {
        set_latest_qblock_id(Some(4)); // candidate qblock = 5

        // Register and participate out of account order to prove sorting.
        for who in [2u64, 1u64] {
            assert_ok!(MinerRegistry::set_descriptor(
                RuntimeOrigin::signed(who),
                descriptor()
            ));
            assert_ok!(MinerRegistry::participate(
                RuntimeOrigin::signed(who),
                5,
                MinerKind::Cpu,
                None
            ));
        }

        assert_eq!(MinerRegistry::participant_count_by_qblock(5), 2);
        assert_eq!(MinerRegistry::participant_count_by_qblock(4), 0);

        let all = MinerRegistry::participants_by_qblock(5, None, 100);
        assert_eq!(
            all.iter().map(|(who, _)| *who).collect::<Vec<_>>(),
            vec![1, 2]
        );

        // limit caps the page...
        let first = MinerRegistry::participants_by_qblock(5, None, 1);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, 1);

        // ...and start_after resumes strictly after the cursor.
        let next = MinerRegistry::participants_by_qblock(5, Some(1), 100);
        assert_eq!(
            next.iter().map(|(who, _)| *who).collect::<Vec<_>>(),
            vec![2]
        );

        // limit 0 returns nothing.
        assert!(MinerRegistry::participants_by_qblock(5, None, 0).is_empty());
    });
}

#[test]
fn participants_are_indexed_per_qblock() {
    new_test_ext().execute_with(|| {
        set_latest_qblock_id(Some(4)); // candidate qblock = 5
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        assert_ok!(MinerRegistry::participate(
            RuntimeOrigin::signed(1),
            5,
            MinerKind::Cpu,
            None
        ));

        set_latest_qblock_id(Some(5)); // candidate qblock = 6
        assert_ok!(MinerRegistry::participate(
            RuntimeOrigin::signed(1),
            6,
            MinerKind::Gpu,
            Some(10)
        ));

        // The reverse index keeps history across qblocks, unlike
        // LatestParticipation which only holds the most recent record.
        assert_eq!(MinerRegistry::participant_count_by_qblock(5), 1);
        assert_eq!(MinerRegistry::participant_count_by_qblock(6), 1);

        let p5 = MinerRegistry::participants_by_qblock(5, None, 100);
        assert_eq!(p5[0].1.kind, MinerKind::Cpu);

        let p6 = MinerRegistry::participants_by_qblock(6, None, 100);
        assert_eq!(p6[0].1.kind, MinerKind::Gpu);
        assert_eq!(p6[0].1.budget_seconds, Some(10));
    });
}

#[test]
fn set_descriptor_v2_stores_and_returns_system_info() {
    new_test_ext().execute_with(|| {
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V2(v2_input())
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        assert_eq!(stored.schema_version, crate::NODE_DESCRIPTOR_SCHEMA_V2);
        let si = stored.system_info.expect("system_info stored");
        assert_eq!(si.os.system.as_slice(), b"Linux");
        assert_eq!(si.os.machine.as_slice(), b"x86_64");
        assert_eq!(si.cpu.brand.as_slice(), b"AMD EPYC 7763");
        assert_eq!(si.cpu.logical_cores, Some(16));
        assert_eq!(si.memory_mb, Some(64_000));
        assert_eq!(si.gpus.len(), 1);
        assert_eq!(si.gpus[0].name.as_slice(), b"NVIDIA H100 80GB HBM3");
        assert_eq!(si.gpus[0].utilization_pct, Some(50));
    });
}

#[test]
fn set_descriptor_v2_without_system_info_stores_none() {
    new_test_ext().execute_with(|| {
        let mut input = v2_input();
        input.system_info = None;

        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V2(input)
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        assert_eq!(stored.schema_version, crate::NODE_DESCRIPTOR_SCHEMA_V2);
        assert!(stored.system_info.is_none());
    });
}

#[test]
fn set_descriptor_v2_rejects_empty_required_system_info_fields() {
    new_test_ext().execute_with(|| {
        let cases: [(fn(&mut crate::SystemInfoOf<Test>), Error<Test>); 5] = [
            (
                |si| si.os.system = bytes::<64>(b""),
                Error::<Test>::EmptyOsSystem,
            ),
            (
                |si| si.cpu.brand = bytes::<96>(b""),
                Error::<Test>::EmptyCpuBrand,
            ),
            (
                |si| si.cpu.arch = bytes::<16>(b""),
                Error::<Test>::EmptyCpuArch,
            ),
            (
                |si| si.gpus[0].vendor = bytes::<16>(b""),
                Error::<Test>::EmptyGpuVendor,
            ),
            (
                |si| si.gpus[0].name = bytes::<96>(b""),
                Error::<Test>::EmptyGpuName,
            ),
        ];

        for (mutate, expected) in cases {
            let mut si = system_info();
            mutate(&mut si);
            let mut input = v2_input();
            input.system_info = Some(si);
            assert_noop!(
                MinerRegistry::set_descriptor(
                    RuntimeOrigin::signed(1),
                    NodeDescriptorInput::V2(input)
                ),
                expected
            );
        }
    });
}

#[test]
fn set_descriptor_v2_rejects_gpu_utilization_over_100() {
    new_test_ext().execute_with(|| {
        let mut si = system_info();
        si.gpus[0].utilization_pct = Some(101);
        let mut input = v2_input();
        input.system_info = Some(si);

        assert_noop!(
            MinerRegistry::set_descriptor(RuntimeOrigin::signed(1), NodeDescriptorInput::V2(input)),
            Error::<Test>::InvalidGpuUtilization
        );
    });
}

#[test]
fn system_info_bounds_reject_over_length_input() {
    // A GPU name longer than MaxGpuNameBytes (96) cannot even be constructed.
    let too_long_name: Result<crate::GpuNameOf<Test>, _> = vec![b'x'; 97].try_into();
    assert!(too_long_name.is_err());

    // Neither can more GPUs than MaxGpus (16).
    let gpu = GpuInfo {
        index: 0,
        vendor: bytes::<16>(b"NVIDIA"),
        name: bytes::<96>(b"GPU"),
        memory_mb: None,
        utilization_pct: None,
    };
    let too_many: Result<crate::GpusOf<Test>, _> = vec![gpu.clone(); 17].try_into();
    assert!(too_many.is_err());

    // The decode boundary an attacker actually hits: a `Vec<u8>`/`Vec<T>`
    // encodes with the same compact-length prefix as a `BoundedVec`, so feeding
    // over-length raw bytes through `Decode` must be rejected by the bound.
    let raw_name = vec![b'x'; 97].encode();
    assert!(crate::GpuNameOf::<Test>::decode(&mut &raw_name[..]).is_err());
    let raw_gpus = vec![gpu; 17].encode();
    assert!(crate::GpusOf::<Test>::decode(&mut &raw_gpus[..]).is_err());

    // Runtime strings are capped at MaxRuntimeVersionBytes (48) and the docker
    // image at MaxDockerImageBytes (256) — enforced at the decode boundary,
    // since validate_runtime only checks emptiness.
    let raw_version = vec![b'x'; 49].encode();
    assert!(crate::RuntimeVersionOf::<Test>::decode(&mut &raw_version[..]).is_err());
    let raw_image = vec![b'x'; 257].encode();
    assert!(crate::DockerImageOf::<Test>::decode(&mut &raw_image[..]).is_err());
}

#[test]
fn node_descriptor_input_pins_variant_indices_and_round_trips() {
    // V1 must encode to byte 0x00 and V2 to 0x01 so in-flight V1 signed
    // extrinsics keep decoding identically regardless of variant source order.
    let v1 = descriptor();
    assert_eq!(v1.encode()[0], 0);
    let v2: crate::NodeDescriptorInputOf<Test> = NodeDescriptorInput::V2(v2_input());
    assert_eq!(v2.encode()[0], 1);

    // Full SCALE round-trip across both trailing Option fields. Exercise every
    // combination of system_info / runtime being Some or None so the trailing
    // 0x00 discriminant + end-of-buffer decode path is covered for each.
    for (with_system_info, with_runtime) in
        [(true, true), (true, false), (false, true), (false, false)]
    {
        let mut input = v2_input();
        if !with_system_info {
            input.system_info = None;
        }
        if !with_runtime {
            input.runtime = None;
        }
        let v2: crate::NodeDescriptorInputOf<Test> = NodeDescriptorInput::V2(input);
        let decoded = crate::NodeDescriptorInputOf::<Test>::decode(&mut &v2.encode()[..])
            .expect("v2 decodes");
        assert_eq!(decoded, v2);
    }
}

#[test]
fn set_descriptor_v2_stores_empty_gpu_list() {
    new_test_ext().execute_with(|| {
        let mut si = system_info();
        si.gpus = bounded(vec![]);
        let mut input = v2_input();
        input.system_info = Some(si);

        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V2(input)
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        let si = stored.system_info.expect("system_info stored");
        assert!(si.gpus.is_empty());
    });
}

#[test]
fn set_descriptor_v2_allows_empty_os_release_and_machine() {
    // os.release and os.machine are intentionally optional (not required
    // non-empty), unlike os.system / cpu.brand / cpu.arch. Pin that contract.
    new_test_ext().execute_with(|| {
        let mut si = system_info();
        si.os.release = bytes::<64>(b"");
        si.os.machine = bytes::<64>(b"");
        let mut input = v2_input();
        input.system_info = Some(si);

        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V2(input)
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        let si = stored.system_info.expect("system_info stored");
        assert!(si.os.release.is_empty());
        assert!(si.os.machine.is_empty());
    });
}

#[test]
fn v2_deposit_exceeds_v1_by_system_info_and_runtime_bytes() {
    new_test_ext().execute_with(|| {
        // V1 baseline (account 1) and V2 with the same base + system_info +
        // runtime (account 2) so the deposit difference is exactly the survey
        // and runtime bytes.
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        let v1_deposit = NodeDescriptors::<Test>::get(1).expect("v1 stored").deposit;

        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(2),
            NodeDescriptorInput::V2(v2_input())
        ));
        let v2_stored = NodeDescriptors::<Test>::get(2).expect("v2 stored");
        let si = v2_stored.system_info.clone().expect("system_info stored");
        let rt = v2_stored.runtime.clone().expect("runtime stored");

        let si_bytes: usize = si.os.system.len()
            + si.os.release.len()
            + si.os.machine.len()
            + si.cpu.brand.len()
            + si.cpu.arch.len()
            + si.gpus
                .iter()
                .map(|g| g.vendor.len() + g.name.len())
                .sum::<usize>();
        let rt_bytes: usize = rt.python.len()
            + rt.quip_version.len()
            + rt.docker_image.as_ref().map(|i| i.len()).unwrap_or(0);

        // mock DescriptorDepositPerByte = 2.
        assert_eq!(
            v2_stored.deposit - v1_deposit,
            (si_bytes + rt_bytes) as u128 * 2
        );
    });
}

#[test]
fn set_descriptor_v2_stores_and_returns_runtime() {
    new_test_ext().execute_with(|| {
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V2(v2_input())
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        let rt = stored.runtime.expect("runtime stored");
        assert_eq!(rt.python.as_slice(), b"3.13.1");
        assert_eq!(rt.quip_version.as_slice(), b"0.2.1");
        assert_eq!(rt.protocol_version, 2);
        assert!(rt.in_docker);
        assert_eq!(rt.docker_image.expect("image").as_slice(), b"quip-node:rc");
    });
}

#[test]
fn set_descriptor_v2_without_runtime_stores_none() {
    new_test_ext().execute_with(|| {
        let mut input = v2_input();
        input.runtime = None;

        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            NodeDescriptorInput::V2(input)
        ));

        let stored = NodeDescriptors::<Test>::get(1).expect("descriptor stored");
        assert!(stored.runtime.is_none());
    });
}

#[test]
fn set_descriptor_v2_stores_runtime_without_docker_image() {
    // Bare-metal node: runtime present but no container. Exercises the None arm
    // of validate_runtime and the skip-add path of runtime_payload_len.
    new_test_ext().execute_with(|| {
        // V1 baseline (account 1) to isolate the runtime deposit contribution.
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(1),
            descriptor()
        ));
        let v1_deposit = NodeDescriptors::<Test>::get(1).expect("v1 stored").deposit;

        let mut input = v2_input();
        input.system_info = None;
        input.runtime = Some(RuntimeInfo {
            python: bytes::<48>(b"3.13.1"),
            quip_version: bytes::<48>(b"0.2.1"),
            protocol_version: 2,
            in_docker: false,
            docker_image: None,
        });
        assert_ok!(MinerRegistry::set_descriptor(
            RuntimeOrigin::signed(2),
            NodeDescriptorInput::V2(input)
        ));

        let stored = NodeDescriptors::<Test>::get(2).expect("descriptor stored");
        let rt = stored.runtime.expect("runtime stored");
        assert!(!rt.in_docker);
        assert!(rt.docker_image.is_none());

        // Deposit reflects only the version-string bytes (no image, no
        // system_info); mock DescriptorDepositPerByte = 2.
        let rt_bytes = (rt.python.len() + rt.quip_version.len()) as u128;
        assert_eq!(stored.deposit - v1_deposit, rt_bytes * 2);
    });
}

#[test]
fn set_descriptor_v2_rejects_empty_runtime_fields() {
    new_test_ext().execute_with(|| {
        let cases: [(fn(&mut crate::RuntimeInfoOf<Test>), Error<Test>); 3] = [
            (
                |rt| rt.python = bytes::<48>(b""),
                Error::<Test>::EmptyPythonVersion,
            ),
            (
                |rt| rt.quip_version = bytes::<48>(b""),
                Error::<Test>::EmptyQuipVersion,
            ),
            (
                |rt| rt.docker_image = Some(bytes::<256>(b"")),
                Error::<Test>::EmptyDockerImage,
            ),
        ];

        for (mutate, expected) in cases {
            let mut rt = runtime_info();
            mutate(&mut rt);
            let mut input = v2_input();
            input.runtime = Some(rt);
            assert_noop!(
                MinerRegistry::set_descriptor(
                    RuntimeOrigin::signed(1),
                    NodeDescriptorInput::V2(input)
                ),
                expected
            );
        }
    });
}

// --- W1-009: bounded paging, retention pruning, storage-key pin ---
mod w1_009 {
    use super::*;
    use crate::{ParticipantCountByQBlock, ParticipantsByQBlock, ParticipationRecord, PruneCursor};
    use frame_support::{
        traits::{Get, Hooks},
        weights::Weight,
    };

    fn record(qblock_id: u64) -> crate::ParticipationRecordOf<Test> {
        ParticipationRecord {
            qblock_id,
            kind: MinerKind::Cpu,
            budget_seconds: None,
            updated_at: 1,
        }
    }

    /// Seed `n` participants (accounts `base..base+n`) into `qblock_id`.
    fn seed(qblock_id: u64, base: u64, n: u64) {
        for who in base..base + n {
            ParticipantsByQBlock::<Test>::insert(qblock_id, who, record(qblock_id));
        }
        ParticipantCountByQBlock::<Test>::insert(qblock_id, u32::try_from(n).unwrap());
    }

    fn entries(qblock_id: u64) -> usize {
        ParticipantsByQBlock::<Test>::iter_prefix(qblock_id).count()
    }

    fn idle() -> Weight {
        MinerRegistry::on_idle(1, Weight::MAX)
    }

    #[test]
    fn prunes_only_qblocks_outside_the_retention_window() {
        new_test_ext().execute_with(|| {
            for q in 1..=5 {
                seed(q, 100, 1);
            }
            // candidate 6, retention 3 -> qblocks < 3 are prunable.
            set_latest_qblock_id(Some(5));
            for _ in 0..10 {
                let _ = idle();
            }
            for q in 1..=2 {
                assert_eq!(entries(q), 0, "qblock {q} should be pruned");
                assert!(!ParticipantCountByQBlock::<Test>::contains_key(q));
            }
            for q in 3..=5 {
                assert_eq!(entries(q), 1, "qblock {q} is inside the window");
                assert_eq!(ParticipantCountByQBlock::<Test>::get(q), 1);
            }
            assert_eq!(PruneCursor::<Test>::get(), 3);
        });
    }

    #[test]
    fn a_large_qblock_clears_over_several_blocks() {
        // `clear_prefix`'s limit applies to keys committed to the backend (keys
        // still in the overlay are all removed), so commit between blocks as a
        // real chain does; `on_idle` makes exactly one call per block.
        let mut ext = new_test_ext();
        ext.execute_with(|| {
            seed(1, 100, 5); // limit is 2 per block
            set_latest_qblock_id(Some(10));
        });
        ext.commit_all().unwrap();
        let block = |ext: &mut sp_io::TestExternalities| {
            let state = ext.execute_with(|| {
                let _ = idle();
                (
                    entries(1),
                    PruneCursor::<Test>::get(),
                    ParticipantCountByQBlock::<Test>::get(1),
                )
            });
            ext.commit_all().unwrap();
            state
        };
        assert_eq!(block(&mut ext), (5, 1, 5)); // qblock 0 is empty: cursor 0 -> 1
        assert_eq!(block(&mut ext), (3, 1, 5)); // count kept until fully cleared
        assert_eq!(block(&mut ext), (1, 1, 5));
        assert_eq!(block(&mut ext), (0, 2, 0)); // cleared: count removed, cursor advanced
        assert_eq!(block(&mut ext), (0, 3, 0));
    }

    #[test]
    fn pruning_is_idempotent_once_caught_up() {
        new_test_ext().execute_with(|| {
            seed(1, 100, 1);
            seed(4, 100, 1);
            set_latest_qblock_id(Some(5)); // prune_before = 3
            for _ in 0..10 {
                let _ = idle();
            }
            let root = sp_io::storage::root(sp_runtime::StateVersion::V1);
            let cursor = PruneCursor::<Test>::get();
            for _ in 0..5 {
                let w = idle();
                assert_eq!(
                    w,
                    <<Test as frame_system::Config>::DbWeight as Get<
                        frame_support::weights::RuntimeDbWeight,
                    >>::get()
                    .reads(2)
                );
            }
            assert_eq!(PruneCursor::<Test>::get(), cursor);
            assert_eq!(sp_io::storage::root(sp_runtime::StateVersion::V1), root);
            assert_eq!(entries(4), 1);
        });
    }

    #[test]
    fn pruning_catches_up_with_a_backlog_and_follows_the_candidate() {
        new_test_ext().execute_with(|| {
            for q in 1..=40 {
                seed(q, 100, 3);
            }
            set_latest_qblock_id(Some(20)); // prune_before = 18
            for _ in 0..200 {
                let _ = idle();
            }
            assert!((1..18).all(|q| entries(q) == 0));
            assert!((18..=40).all(|q| entries(q) == 3));
            assert_eq!(PruneCursor::<Test>::get(), 18);
            // The candidate advances: pruning resumes from where it stopped.
            set_latest_qblock_id(Some(30)); // prune_before = 28
            for _ in 0..200 {
                let _ = idle();
            }
            assert!((1..28).all(|q| entries(q) == 0));
            assert!((28..=40).all(|q| entries(q) == 3));
            assert_eq!(PruneCursor::<Test>::get(), 28);
        });
    }

    #[test]
    fn on_idle_does_nothing_without_weight_for_a_full_pass() {
        new_test_ext().execute_with(|| {
            seed(1, 100, 2);
            set_latest_qblock_id(Some(10));
            let worst = <() as crate::WeightInfo>::prune_participation(2);
            let short = worst.saturating_sub(Weight::from_parts(1, 0));
            assert_eq!(MinerRegistry::on_idle(1, short), Weight::zero());
            assert_eq!(PruneCursor::<Test>::get(), 0);
            assert_eq!(entries(1), 2);
        });
    }

    /// `on_idle` must reserve the two gating reads (candidate id, cursor) on
    /// top of the benchmarked clear step: with one unit less than both, in
    /// either weight dimension, it does nothing; with both, it prunes and
    /// returns no more than it was given.
    #[test]
    fn on_idle_reserves_its_gating_reads_on_top_of_the_clear_step() {
        new_test_ext().execute_with(|| {
            seed(1, 100, 2);
            set_latest_qblock_id(Some(10));
            let db = <Test as frame_system::Config>::DbWeight::get();
            assert!(db.reads(2).ref_time() > 0, "the mock must price reads");
            let full =
                <() as crate::WeightInfo>::prune_participation(2).saturating_add(db.reads(2));
            for short in [
                full.saturating_sub(Weight::from_parts(1, 0)),
                full.saturating_sub(Weight::from_parts(0, 1)),
            ] {
                assert_eq!(MinerRegistry::on_idle(1, short), Weight::zero());
                assert_eq!(entries(1), 2, "nothing pruned with {short:?}");
                assert_eq!(PruneCursor::<Test>::get(), 0);
            }
            // One pass clears one qblock: qblock 0 (empty) first.
            let used = MinerRegistry::on_idle(1, full);
            assert!(used.all_lte(full), "returned {used:?} > available {full:?}");
            assert_eq!(PruneCursor::<Test>::get(), 1, "the pass ran");
        });
    }

    #[test]
    fn nothing_is_pruned_before_the_window_opens() {
        new_test_ext().execute_with(|| {
            seed(1, 100, 1);
            set_latest_qblock_id(Some(2)); // candidate 3 - 3 = 0: nothing prunable
            for _ in 0..5 {
                let _ = idle();
            }
            assert_eq!(entries(1), 1);
            assert_eq!(PruneCursor::<Test>::get(), 0);
        });
    }

    #[test]
    fn paging_is_complete_and_ordered_in_storage_order() {
        new_test_ext().execute_with(|| {
            // Account ids inserted in a scrambled order; storage order is the
            // Blake2_128Concat order, not numeric order.
            for who in [9_u64, 2, 77, 5, 31, 1, 64, 13, 40, 8, 3] {
                ParticipantsByQBlock::<Test>::insert(7, who, record(7));
            }
            let full: Vec<u64> = ParticipantsByQBlock::<Test>::iter_prefix(7)
                .map(|(who, _)| who)
                .collect();
            let mut paged = Vec::new();
            let mut cursor = None;
            loop {
                let page = MinerRegistry::participants_by_qblock(7, cursor, 3);
                if page.is_empty() {
                    break;
                }
                assert!(page.len() <= 3);
                cursor = page.last().map(|(who, _)| *who);
                paged.extend(page.into_iter().map(|(who, _)| who));
            }
            assert_eq!(
                paged, full,
                "pages must concatenate to the full storage order"
            );
            let mut sorted = full.clone();
            sorted.sort_unstable();
            assert_ne!(
                full, sorted,
                "fixture should exercise non-monotonic storage order"
            );
        });
    }

    #[test]
    fn paging_cursor_survives_removal_of_the_cursor_account() {
        new_test_ext().execute_with(|| {
            for who in 1_u64..=10 {
                ParticipantsByQBlock::<Test>::insert(7, who, record(7));
            }
            let full: Vec<u64> = ParticipantsByQBlock::<Test>::iter_prefix(7)
                .map(|(who, _)| who)
                .collect();
            let cursor = full[4];
            ParticipantsByQBlock::<Test>::remove(7, cursor);
            let rest: Vec<u64> = MinerRegistry::participants_by_qblock(7, Some(cursor), 100)
                .into_iter()
                .map(|(who, _)| who)
                .collect();
            assert_eq!(rest, full[5..].to_vec());
        });
    }

    #[test]
    fn paging_reads_are_bounded_by_limit_not_participants() {
        // Prove the page read is O(limit): the storage proof for a 10-entry page
        // out of 10,000 participants stays small. The pre-W1-009 full-prefix
        // scan would have to touch all 10,000 records (well over 64 KiB).
        let mut ext = new_test_ext();
        ext.execute_with(|| seed(7, 1_000, 10_000));
        ext.commit_all().unwrap();
        let (page, proof) =
            ext.execute_and_prove(|| MinerRegistry::participants_by_qblock(7, None, 10));
        assert_eq!(page.len(), 10);
        let size = proof.encoded_size();
        assert!(
            size < 64 * 1024,
            "page proof is {size} bytes; read is not O(limit)"
        );
    }

    #[test]
    fn participants_by_qblock_storage_key_layout_is_pinned() {
        // The key layout must not change (no migration): twox128(pallet) ++
        // twox128(item) ++ blake2_128_concat(qblock u64) ++
        // blake2_128_concat(account). Built here from primitives and from a
        // literal, so a hasher or prefix change is caught.
        use sp_core::hashing::{blake2_128, twox_128};
        new_test_ext().execute_with(|| {
            let (qblock, who) = (7_u64, 42_u64);
            let mut expected = Vec::new();
            expected.extend(twox_128(b"MinerRegistry"));
            expected.extend(twox_128(b"ParticipantsByQBlock"));
            expected.extend(blake2_128(&qblock.encode()));
            expected.extend(qblock.encode());
            expected.extend(blake2_128(&who.encode()));
            expected.extend(who.encode());
            assert_eq!(
                ParticipantsByQBlock::<Test>::hashed_key_for(qblock, who),
                expected
            );
            let hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, PINNED_KEY_HEX);
        });
    }

    const PINNED_KEY_HEX: &str = "491850926eb92ce9861fdcc5504d045eb956fe48070697cb96311d8e817d446ca5e47192376d46472707aad6430c235b07000000000000003c182333ed1800191f83a7d04a0ef58b2a00000000000000";
}

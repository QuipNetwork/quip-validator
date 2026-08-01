//! Benchmarking setup for pallet-miner-registry.

use super::*;

#[allow(unused)]
use crate::Pallet as MinerRegistry;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
    traits::{Currency, Get, ReservableCurrency},
    BoundedVec,
};
use frame_system::RawOrigin;
use sp_runtime::traits::{SaturatedConversion, Saturating, Zero};

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: Get<u32>,
{
    items
        .try_into()
        .ok()
        .expect("benchmark input fits within bounds")
}

fn filled_bytes<S: Get<u32>>(byte: u8) -> BoundedVec<u8, S> {
    bounded(vec![byte; S::get() as usize])
}

fn maximum_payload_len<T: Config>() -> u32 {
    T::MaxNodeIdBytes::get()
        .saturating_add(T::MaxNodeNameBytes::get())
        .saturating_add(T::MaxPublicHostBytes::get())
        .saturating_add(T::MaxRpcEndpoints::get().saturating_mul(T::MaxRpcEndpointBytes::get()))
        .saturating_add(
            T::MaxMinerSpecs::get().saturating_mul(
                T::MaxMinerLabelBytes::get()
                    .saturating_add(T::MaxMinerBackendBytes::get())
                    .saturating_add(T::MaxMinerDeviceIdBytes::get()),
            ),
        )
        .saturating_add(T::MaxOsStringBytes::get().saturating_mul(3))
        .saturating_add(T::MaxCpuBrandBytes::get())
        .saturating_add(T::MaxArchBytes::get())
        .saturating_add(
            T::MaxGpus::get().saturating_mul(
                T::MaxGpuVendorBytes::get().saturating_add(T::MaxGpuNameBytes::get()),
            ),
        )
        .saturating_add(T::MaxRuntimeVersionBytes::get().saturating_mul(2))
        .saturating_add(T::MaxDockerImageBytes::get())
}

fn benchmark_funding<T: Config>() -> BalanceOf<T> {
    let max_payload: BalanceOf<T> = maximum_payload_len::<T>().saturated_into();
    T::DescriptorDepositBase::get()
        .saturating_add(T::DescriptorDepositPerByte::get().saturating_mul(max_payload))
        .saturating_mul(3u32.saturated_into())
}

fn fund_account<T: Config>(who: &T::AccountId) {
    let _ = T::Currency::make_free_balance_be(who, benchmark_funding::<T>());
}

fn maximum_v2_descriptor<T: Config>() -> NodeDescriptorInputOf<T> {
    let rpc_endpoint = filled_bytes::<T::MaxRpcEndpointBytes>(b'r');
    let rpc_endpoints = bounded::<_, T::MaxRpcEndpoints>(
        (0..T::MaxRpcEndpoints::get())
            .map(|_| rpc_endpoint.clone())
            .collect(),
    );

    let miner_label = filled_bytes::<T::MaxMinerLabelBytes>(b'l');
    let miner_backend = filled_bytes::<T::MaxMinerBackendBytes>(b'b');
    let miner_device = filled_bytes::<T::MaxMinerDeviceIdBytes>(b'd');
    let miners = bounded::<_, T::MaxMinerSpecs>(
        (0..T::MaxMinerSpecs::get())
            .map(|_| MinerSpec {
                kind: MinerKind::Metal,
                label: Some(miner_label.clone()),
                backend: Some(miner_backend.clone()),
                device_id: Some(miner_device.clone()),
            })
            .collect(),
    );

    let gpu_vendor = filled_bytes::<T::MaxGpuVendorBytes>(b'v');
    let gpu_name = filled_bytes::<T::MaxGpuNameBytes>(b'g');
    let gpus = bounded::<_, T::MaxGpus>(
        (0..T::MaxGpus::get())
            .map(|index| GpuInfo {
                index: index.min(u8::MAX.into()) as u8,
                vendor: gpu_vendor.clone(),
                name: gpu_name.clone(),
                memory_mb: Some(u32::MAX),
                utilization_pct: Some(100),
            })
            .collect(),
    );

    NodeDescriptorInput::V2(NodeDescriptorV2Input {
        node_id: filled_bytes::<T::MaxNodeIdBytes>(b'i'),
        node_name: filled_bytes::<T::MaxNodeNameBytes>(b'n'),
        public_host: Some(filled_bytes::<T::MaxPublicHostBytes>(b'h')),
        public_port: Some(u16::MAX),
        rpc_endpoints,
        auto_mine: true,
        log_level: LogLevel::Debug,
        miners,
        system_info: Some(SystemInfo {
            os: OsInfo {
                system: filled_bytes::<T::MaxOsStringBytes>(b's'),
                release: filled_bytes::<T::MaxOsStringBytes>(b'r'),
                machine: filled_bytes::<T::MaxOsStringBytes>(b'm'),
            },
            cpu: CpuInfo {
                logical_cores: Some(u32::MAX),
                physical_cores: Some(u32::MAX),
                brand: filled_bytes::<T::MaxCpuBrandBytes>(b'c'),
                arch: filled_bytes::<T::MaxArchBytes>(b'a'),
            },
            memory_mb: Some(u32::MAX),
            gpus,
        }),
        runtime: Some(RuntimeInfo {
            python: filled_bytes::<T::MaxRuntimeVersionBytes>(b'p'),
            quip_version: filled_bytes::<T::MaxRuntimeVersionBytes>(b'q'),
            protocol_version: u32::MAX,
            in_docker: true,
            docker_image: Some(filled_bytes::<T::MaxDockerImageBytes>(b'd')),
        }),
    })
}

fn near_maximum_v2_descriptor<T: Config>() -> NodeDescriptorInputOf<T> {
    let mut descriptor = maximum_v2_descriptor::<T>();
    let NodeDescriptorInput::V2(input) = &mut descriptor else {
        unreachable!("maximum descriptor always uses schema V2")
    };
    let docker_image = input
        .runtime
        .as_mut()
        .and_then(|runtime| runtime.docker_image.as_mut())
        .expect("maximum V2 descriptor carries a docker image");
    assert!(
        docker_image.pop().is_some(),
        "docker image bound must permit at least one byte"
    );
    descriptor
}

fn store_maximum_descriptor<T: Config>(who: &T::AccountId) {
    fund_account::<T>(who);
    assert!(MinerRegistry::<T>::set_descriptor(
        RawOrigin::Signed(who.clone()).into(),
        maximum_v2_descriptor::<T>(),
    )
    .is_ok());
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn set_descriptor() {
        // Use a normal deterministic account rather than
        // `whitelisted_caller`: reserve/unreserve must remain visible to the
        // benchmark storage tracker so balance-account DB work is charged.
        let caller: T::AccountId = account("caller", 0, 0);
        fund_account::<T>(&caller);
        assert!(MinerRegistry::<T>::set_descriptor(
            RawOrigin::Signed(caller.clone()).into(),
            near_maximum_v2_descriptor::<T>(),
        )
        .is_ok());
        let previous_deposit = NodeDescriptors::<T>::get(&caller)
            .expect("near-maximum V2 descriptor was stored")
            .deposit;
        let descriptor = maximum_v2_descriptor::<T>();

        #[extrinsic_call]
        MinerRegistry::set_descriptor(RawOrigin::Signed(caller.clone()), descriptor);

        let stored = NodeDescriptors::<T>::get(&caller).expect("maximum V2 descriptor was stored");
        assert_eq!(stored.schema_version, NODE_DESCRIPTOR_SCHEMA_V2);
        assert!(stored.deposit > previous_deposit);
        assert_eq!(stored.rpc_endpoints.len() as u32, T::MaxRpcEndpoints::get());
        assert_eq!(stored.miners.len() as u32, T::MaxMinerSpecs::get());
        assert_eq!(
            stored
                .system_info
                .as_ref()
                .expect("system info is present")
                .gpus
                .len() as u32,
            T::MaxGpus::get()
        );
        assert_eq!(T::Currency::reserved_balance(&caller), stored.deposit);
    }

    #[benchmark]
    fn clear_descriptor() {
        let caller: T::AccountId = account("caller", 0, 0);
        store_maximum_descriptor::<T>(&caller);
        let qblock_id = T::QBlockIds::candidate_qblock_id();
        assert!(MinerRegistry::<T>::participate(
            RawOrigin::Signed(caller.clone()).into(),
            qblock_id,
            MinerKind::Metal,
            Some(u32::MAX),
        )
        .is_ok());
        assert!(LatestParticipation::<T>::contains_key(&caller));
        assert!(!T::Currency::reserved_balance(&caller).is_zero());

        #[extrinsic_call]
        MinerRegistry::clear_descriptor(RawOrigin::Signed(caller.clone()));

        assert!(!NodeDescriptors::<T>::contains_key(&caller));
        assert!(!LatestParticipation::<T>::contains_key(&caller));
        assert!(T::Currency::reserved_balance(&caller).is_zero());
    }

    #[benchmark]
    fn participate() {
        let caller: T::AccountId = account("caller", 0, 0);
        store_maximum_descriptor::<T>(&caller);
        let qblock_id = T::QBlockIds::candidate_qblock_id();

        #[extrinsic_call]
        MinerRegistry::participate(
            RawOrigin::Signed(caller.clone()),
            qblock_id,
            MinerKind::Metal,
            Some(u32::MAX),
        );

        let record =
            LatestParticipation::<T>::get(&caller).expect("latest participation was stored");
        assert_eq!(record.qblock_id, qblock_id);
        assert_eq!(record.kind, MinerKind::Metal);
        assert_eq!(
            ParticipantsByQBlock::<T>::get(qblock_id, &caller),
            Some(record)
        );
        assert_eq!(ParticipantCountByQBlock::<T>::get(qblock_id), 1);
    }

    impl_benchmark_test_suite!(
        MinerRegistry,
        crate::mock::new_test_ext(),
        crate::mock::Test
    );
}

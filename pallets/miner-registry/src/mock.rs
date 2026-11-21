use crate as pallet_miner_registry;
use frame_support::{
    derive_impl,
    traits::{ConstU128, ConstU32, ConstU64},
};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;
type Balance = u128;

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Test;

    #[runtime::pallet_index(0)]
    pub type System = frame_system::Pallet<Test>;

    #[runtime::pallet_index(1)]
    pub type Balances = pallet_balances::Pallet<Test>;

    #[runtime::pallet_index(2)]
    pub type MinerRegistry = pallet_miner_registry::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<Balance>;
    // Non-zero DB weights, so a test can tell whether `on_idle` reserves the
    // reads it takes outside the benchmarked body (the default is zero).
    type DbWeight = frame_support::weights::constants::RocksDbWeight;
}

impl pallet_balances::Config for Test {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ConstU32<8>;
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU128<1>;
    type AccountStore = System;
    type WeightInfo = ();
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = ConstU32<0>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

pub struct MockQBlockIds;

impl pallet_miner_registry::QBlockIdProvider for MockQBlockIds {
    fn latest_qblock_id() -> Option<u64> {
        LATEST_QBLOCK_ID.with(|value| *value.borrow())
    }
}

thread_local! {
    static LATEST_QBLOCK_ID: core::cell::RefCell<Option<u64>> = const {
        core::cell::RefCell::new(None)
    };
}

pub fn set_latest_qblock_id(value: Option<u64>) {
    LATEST_QBLOCK_ID.with(|stored| {
        *stored.borrow_mut() = value;
    });
}

impl pallet_miner_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type QBlockIds = MockQBlockIds;
    type MaxNodeIdBytes = ConstU32<64>;
    type MaxNodeNameBytes = ConstU32<64>;
    type MaxPublicHostBytes = ConstU32<253>;
    type MaxRpcEndpointBytes = ConstU32<256>;
    type MaxRpcEndpoints = ConstU32<8>;
    type MaxMinerSpecs = ConstU32<8>;
    type MaxMinerLabelBytes = ConstU32<64>;
    type MaxMinerBackendBytes = ConstU32<32>;
    type MaxMinerDeviceIdBytes = ConstU32<64>;
    type MaxOsStringBytes = ConstU32<64>;
    type MaxCpuBrandBytes = ConstU32<96>;
    type MaxArchBytes = ConstU32<16>;
    type MaxGpuVendorBytes = ConstU32<16>;
    type MaxGpuNameBytes = ConstU32<96>;
    type MaxGpus = ConstU32<16>;
    type MaxRuntimeVersionBytes = ConstU32<48>;
    type MaxDockerImageBytes = ConstU32<256>;
    type DescriptorDepositBase = ConstU128<10>;
    type DescriptorDepositPerByte = ConstU128<2>;
    // Small values so pruning tests can push qblocks out of the window and
    // observe a many-participant qblock clearing over several blocks.
    type ParticipationRetention = ConstU64<3>;
    type MaxParticipationPrunePerBlock = ConstU32<2>;
    type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    set_latest_qblock_id(None);
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(1, 1_000), (2, 1_000)],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    let mut ext: sp_io::TestExternalities = storage.into();
    ext.execute_with(|| {
        System::set_block_number(1);
    });
    ext
}

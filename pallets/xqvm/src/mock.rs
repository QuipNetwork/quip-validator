use crate as pallet_xqvm;
use frame_support::weights::Weight;
use frame_support::{derive_impl, parameter_types};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

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
    pub type Xqvm = pallet_xqvm::Pallet<Test>;

    #[runtime::pallet_index(2)]
    pub type Balances = pallet_balances::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<u64>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type AccountStore = System;
    type Balance = u64;
    type ExistentialDeposit = ExistentialDeposit;
}

parameter_types! {
    pub const MaxProgramSize: u32 = 65_536;
    pub const MaxCallDataLen: u32 = 32;
    pub const MaxOutputSlots: u32 = 32;
    pub const MaxStepLimit: u64 = 100_000;
    pub const MaxVmMemory: u64 = 1_024;
    pub const TestWeightPerStep: Weight = Weight::from_parts(1_000, 0);
    pub const ExistentialDeposit: u64 = 1;
    // Small, round, and easy to assert against: a 16-byte minimal program
    // costs 100 + 16 = 116.
    pub const DepositBase: u64 = 100;
    pub const DepositPerByte: u64 = 1;
}

impl pallet_xqvm::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    type DepositBase = DepositBase;
    type DepositPerByte = DepositPerByte;
    type MaxProgramSize = MaxProgramSize;
    type MaxCallDataLen = MaxCallDataLen;
    type MaxOutputSlots = MaxOutputSlots;
    type MaxStepLimit = MaxStepLimit;
    type MaxVmMemory = MaxVmMemory;
    type WeightPerStep = TestWeightPerStep;
    type WeightInfo = ();
}

/// Balance every test account starts with. Comfortably above the largest
/// deposit any test program costs, so a test that runs out of funds is
/// testing something it meant to.
pub const INITIAL_BALANCE: u64 = 1_000_000;

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(1, INITIAL_BALANCE), (2, INITIAL_BALANCE)],
        ..Default::default()
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    storage.into()
}

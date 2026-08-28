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
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

parameter_types! {
    pub const MaxProgramSize: u32 = 65_536;
    pub const MaxCallDataLen: u32 = 32;
    pub const MaxOutputSlots: u32 = 32;
    pub const MaxStepLimit: u64 = 100_000;
    pub const MaxVmMemory: u64 = 1_024;
    pub const TestWeightPerStep: Weight = Weight::from_parts(1_000, 0);
}

impl pallet_xqvm::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type MaxProgramSize = MaxProgramSize;
    type MaxCallDataLen = MaxCallDataLen;
    type MaxOutputSlots = MaxOutputSlots;
    type MaxStepLimit = MaxStepLimit;
    type MaxVmMemory = MaxVmMemory;
    type WeightPerStep = TestWeightPerStep;
    type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap()
        .into()
}

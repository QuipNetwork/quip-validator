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
    pub const MaxProgramBlocks: u32 = 2_048;
    pub const MaxLoopDepth: u32 = 32;
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
    type MaxProgramBlocks = MaxProgramBlocks;
    type MaxLoopDepth = MaxLoopDepth;
    type MaxCallDataLen = MaxCallDataLen;
    type MaxOutputSlots = MaxOutputSlots;
    type MaxStepLimit = MaxStepLimit;
    type MaxVmMemory = MaxVmMemory;
    type WeightPerStep = TestWeightPerStep;
    type WeightInfo = TestXqvmWeights;
}

/// Test weights with a deliberately non-zero output-slot slope on `execute`,
/// so the `execute` plumbing test (W1-008 part 2) can observe that the
/// declared `output_slots` reaches both the pre-charged and the actual weight.
///
/// Every other component, and `execute` at `o == 0`, delegates unchanged to
/// the substrate reference weights (`()`), so tests that use zero output slots
/// (including the refund test, which pins actual weight to `()`) are
/// unaffected. The reference weights ship a zero `o` coefficient until the
/// owner regenerates them; this mock supplies a non-zero one so the plumbing
/// is testable now rather than only after regeneration.
pub struct TestXqvmWeights;
impl crate::WeightInfo for TestXqvmWeights {
    fn store_program(s: u32, b: u32) -> Weight {
        <() as crate::WeightInfo>::store_program(s, b)
    }
    fn execute(s: u32, o: u32) -> Weight {
        <() as crate::WeightInfo>::execute(s, o)
            .saturating_add(Weight::from_parts(1_000_000, 8).saturating_mul(o.into()))
    }
    fn execute_step(t: u32) -> Weight {
        <() as crate::WeightInfo>::execute_step(t)
    }
    fn remove_program(s: u32) -> Weight {
        <() as crate::WeightInfo>::remove_program(s)
    }
    fn evict_program(s: u32) -> Weight {
        <() as crate::WeightInfo>::evict_program(s)
    }
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

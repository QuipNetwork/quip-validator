use crate as pallet_quantum_pow;
use frame_support::{
    derive_impl, parameter_types,
    traits::{ConstU128, ConstU32},
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
    pub type QuantumPow = pallet_quantum_pow::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<Balance>;
    // TestDefaultConfig sets `DbWeight = ()`, making every read/write cost
    // zero — weight assertions would compare `0 == 0` and pass regardless
    // of the migration's accounting. Real per-op costs make them meaningful.
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

parameter_types! {
    pub const MaxNodes: u32 = 16;
    pub const MaxEdges: u32 = 32;
    pub const MaxSolutions: u32 = 8;
    pub const MinNodes: u32 = 2;
    pub const EpochLength: u64 = 20;
    pub const MinerDeposit: Balance = 100;
    pub const BlockReward: Balance = 50;
    pub const MaxProofsPerBlock: u32 = 8;
    pub const MaxAllowedValues: u32 = 32;
}

impl pallet_quantum_pow::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MaxNodes = MaxNodes;
    type MaxEdges = MaxEdges;
    type MaxSolutions = MaxSolutions;
    type MinNodes = MinNodes;
    type EpochLength = EpochLength;
    type MinerDeposit = MinerDeposit;
    type BlockReward = BlockReward;
    type MaxProofsPerBlock = MaxProofsPerBlock;
    type MaxAllowedValues = MaxAllowedValues;
    type CurveCEasyMilli = ConstU32<700>;
    type CurveCKneeMilli = ConstU32<725>;
    type CurveCHardMilli = ConstU32<750>;
    type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        // Account 3 is funded so tests can use a miner whose puzzle ground
        // state sits below the energy curve (accounts 1 and 2 happen to map to
        // puzzles easier than the curve's easy cap).
        balances: vec![(1, 1_000_000), (2, 1_000_000), (3, 1_000_000)],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    let mut ext: sp_io::TestExternalities = storage.into();
    ext.execute_with(|| {
        System::set_block_number(1);
        // Mirror production: on_initialize at block 1 captures
        // parent_hash() (== block_hash(0)) into LastProofBlockHash so
        // nonce derivation has a stable seed before any proof has won.
        // Without this, tests would see LastProofBlockHash == zero while
        // production sees block_hash(0).
        use frame_support::traits::Hooks;
        QuantumPow::on_initialize(1);
    });
    ext
}

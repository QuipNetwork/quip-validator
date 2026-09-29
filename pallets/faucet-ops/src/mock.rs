use crate as pallet_faucet_ops;
use frame_support::{
    derive_impl,
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
    pub type FaucetOps = pallet_faucet_ops::Pallet<Test>;
    #[runtime::pallet_index(3)]
    pub type EmissionController = pallet_emission_controller::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<Balance>;
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

frame_support::parameter_types! {
    pub const PotId: frame_support::PalletId = frame_support::PalletId(*b"qp/emiss");
}
pub struct Clock;
impl frame_support::traits::UnixTime for Clock {
    fn now() -> core::time::Duration {
        core::time::Duration::from_secs(System::block_number())
    }
}
impl pallet_emission_controller::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type Clock = Clock;
    type Unit = ConstU128<1_000_000_000_000>;
    type PotId = PotId;
    type MaxSubnets = ConstU32<32>;
    type WeightInfo = ();
}
impl pallet_faucet_ops::Config for Test {
    type MintOrigin = crate::EnsureFaucetAuthority<Test>;
    type AuthorityOrigin = frame_system::EnsureRoot<u64>;
    type Emissions = EmissionController;
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(1, 1_000)],
        dev_accounts: None,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    crate::GenesisConfig::<Test> {
        state: crate::FaucetState::Enabled,
        authority: Some(1),
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    pallet_emission_controller::GenesisConfig::<Test> {
        faucet_budget: 10_000,
        ..Default::default()
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    let mut ext: sp_io::TestExternalities = storage.into();
    ext.execute_with(|| {
        System::set_block_number(1);
    });
    ext
}

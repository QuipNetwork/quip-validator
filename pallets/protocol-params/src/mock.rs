use crate as pallet_protocol_params;
use frame_support::{
    derive_impl,
    traits::{ConstU32, UnixTime},
};
use sp_runtime::BuildStorage;

frame_support::construct_runtime!(pub enum Test {
    System: frame_system,
    Balances: pallet_balances,
    Staking: pallet_staking,
    ProtocolParams: pallet_protocol_params,
});
#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = frame_system::mocking::MockBlock<Test>;
    type AccountData = pallet_balances::AccountData<u128>;
}
#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type Balance = u128;
    type AccountStore = System;
}
pub struct Time;
impl UnixTime for Time {
    fn now() -> core::time::Duration {
        core::time::Duration::from_secs(1)
    }
}
#[derive_impl(pallet_staking::config_preludes::TestDefaultConfig)]
impl pallet_staking::Config for Test {
    type OldCurrency = Balances;
    type Currency = Balances;
    type UnixTime = Time;
    type ElectionProvider = frame_election_provider_support::NoElection<(
        u64,
        u64,
        Staking,
        ConstU32<100>,
        ConstU32<64>,
    )>;
    type GenesisElectionProvider = Self::ElectionProvider;
    type AdminOrigin = frame_system::EnsureRoot<u64>;
    type EraPayout = ();
    type VoterList = pallet_staking::UseNominatorsAndValidatorsMap<Test>;
    type TargetList = pallet_staking::UseValidatorsMap<Test>;
    type BondingDuration = ConstU32<28>;
    type SlashDeferDuration = ConstU32<27>;
}
impl crate::Config for Test {
    type CouncilOrigin = frame_system::EnsureRoot<u64>;
    type MaxBudgetWindow = ConstU32<84>;
    type SlashWeightInfo = ();
}
pub fn ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    pallet_balances::GenesisConfig::<Test> {
        balances: vec![(1, 10_000)],
        ..Default::default()
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| {
        System::set_block_number(1);
        frame_support::assert_ok!(Staking::bond(
            RuntimeOrigin::signed(1),
            10_000,
            pallet_staking::RewardDestination::Stash
        ));
        set_era(1);
    });
    ext
}
pub fn set_era(era: u32) {
    pallet_staking::ActiveEra::<Test>::put(pallet_staking::ActiveEraInfo {
        index: era,
        start: Some(0),
    });
    pallet_staking::CurrentEra::<Test>::put(era);
    pallet_staking::ErasStartSessionIndex::<Test>::insert(era, era * 6);
    pallet_staking::BondedEras::<Test>::put(
        (era.saturating_sub(28)..=era)
            .map(|e| (e, e * 6))
            .collect::<Vec<_>>(),
    );
    pallet_staking::EraInfo::<Test>::set_exposure(
        era,
        &1,
        pallet_staking::Exposure {
            total: 10_000u128,
            own: 10_000u128,
            others: vec![],
        },
    );
}

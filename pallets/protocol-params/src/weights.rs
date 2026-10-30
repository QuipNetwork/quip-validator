//! Conservative wrapper overhead, in addition to SDK manual_slash weight.
//! Regenerate on the serialized reference runner before release.
use core::marker::PhantomData;
use frame_support::{
    traits::Get,
    weights::{constants::RocksDbWeight, Weight},
};

pub trait WeightInfo {
    fn discretionary_slash() -> Weight;
    fn set_limits() -> Weight;
}
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: crate::Config> WeightInfo for SubstrateWeight<T> {
    fn discretionary_slash() -> Weight {
        Weight::from_parts(50_000_000, 10_000)
            .saturating_add(Weight::from_parts(1_000_000, 8).saturating_mul(T::MaxBudgetWindow::get().into()))
            .saturating_add(T::DbWeight::get().reads_writes(8, 1))
            .saturating_add(<<T as pallet_staking::Config>::WeightInfo as pallet_staking::WeightInfo>::manual_slash())
    }
    fn set_limits() -> Weight {
        Weight::from_parts(20_000_000, 0).saturating_add(T::DbWeight::get().writes(3))
    }
}
impl WeightInfo for () {
    fn discretionary_slash() -> Weight {
        Weight::from_parts(50_000_000, 10_000)
            .saturating_add(Weight::from_parts(1_000_000, 8).saturating_mul(84))
            .saturating_add(RocksDbWeight::get().reads_writes(8, 1))
            .saturating_add(<() as pallet_staking::WeightInfo>::manual_slash())
    }
    fn set_limits() -> Weight {
        Weight::from_parts(20_000_000, 0).saturating_add(RocksDbWeight::get().writes(3))
    }
}

//! Provisional upper estimates including controller and origin storage access.
use core::marker::PhantomData;
use frame_support::{traits::Get, weights::Weight};
pub trait WeightInfo {
    fn mint() -> Weight;
    fn disable() -> Weight;
    fn set_authority() -> Weight;
}
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn set_authority() -> Weight {
        Self::disable()
    }
    fn mint() -> Weight {
        Weight::from_parts(150_000_000, 16_000)
            .saturating_add(T::DbWeight::get().reads_writes(7, 4))
    }
    fn disable() -> Weight {
        Weight::from_parts(30_000_000, 6_000).saturating_add(T::DbWeight::get().reads_writes(2, 1))
    }
}
impl WeightInfo for () {
    fn set_authority() -> Weight {
        Self::disable()
    }
    fn mint() -> Weight {
        Weight::from_parts(150_000_000, 16_000)
    }
    fn disable() -> Weight {
        Weight::from_parts(30_000_000, 6_000)
    }
}

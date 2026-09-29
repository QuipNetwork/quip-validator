//! Provisional weights. Replace with reference-machine benchmark output before release.
use frame_support::{traits::Get, weights::Weight};
pub trait WeightInfo {
    fn approve() -> Weight;
    fn remove() -> Weight;
    fn advance() -> Weight;
    fn dispatch_as_root() -> Weight;
}
pub struct SubstrateWeight<T>(core::marker::PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn approve() -> Weight {
        Weight::from_parts(100_000_000, 16_384)
            .saturating_add(T::DbWeight::get().reads_writes(2, 1))
    }
    fn remove() -> Weight {
        Self::approve()
    }
    fn advance() -> Weight {
        Self::approve()
    }
    fn dispatch_as_root() -> Weight {
        Self::approve()
    }
}
impl WeightInfo for () {
    fn approve() -> Weight {
        Weight::from_parts(100_000_000, 16_384)
    }
    fn remove() -> Weight {
        Self::approve()
    }
    fn advance() -> Weight {
        Self::approve()
    }
    fn dispatch_as_root() -> Weight {
        Self::approve()
    }
}

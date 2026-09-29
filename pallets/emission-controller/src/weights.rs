//! Conservative provisional weights; regenerate on the reference benchmark host.
use frame_support::{traits::Get, weights::Weight};
pub trait WeightInfo {
    fn accrue(r: u32) -> Weight;
    fn reward() -> Weight;
}
pub struct SubstrateWeight<T>(core::marker::PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn accrue(r: u32) -> Weight {
        Weight::from_parts(40_000_000, 6_000)
            .saturating_add(Weight::from_parts(100_000_000, 8_000).saturating_mul(r.into()))
            .saturating_add(
                T::DbWeight::get().reads_writes(4 + 5 * u64::from(r), 1 + 4 * u64::from(r)),
            )
    }
    fn reward() -> Weight {
        Weight::from_parts(100_000_000, 12_000)
            .saturating_add(T::DbWeight::get().reads_writes(4, 3))
    }
}
impl WeightInfo for () {
    fn accrue(_: u32) -> Weight {
        Weight::from_parts(1_000_000_000, 100_000)
    }
    fn reward() -> Weight {
        Weight::from_parts(100_000_000, 12_000)
    }
}

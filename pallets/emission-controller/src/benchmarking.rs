use super::*;
use alloc::vec;
use frame_benchmarking::v2::*;
use frame_support::{traits::Currency, BoundedVec};
use sp_runtime::Perbill;

#[benchmarks]
mod benchmarks {
    use super::*;
    #[benchmark]
    fn accrue(r: Linear<1, 32>) {
        let count = r.min(T::MaxSubnets::get());
        let mut routes = vec![];
        let share = 1_000_000_000 / count;
        for id in 0..count {
            let parts = if id == count - 1 {
                1_000_000_000 - share * id
            } else {
                share
            };
            routes.push((id, Perbill::from_parts(parts)));
        }
        Routes::<T>::put(BoundedVec::<_, T::MaxSubnets>::try_from(routes).unwrap());
        Enabled::<T>::put(true);
        StartAt::<T>::put(100);
        #[block]
        {
            Pallet::<T>::accrue(100 + YEAR_SECONDS);
        }
        assert!(ScheduledIssued::<T>::get() > 0);
    }
    #[benchmark]
    fn reward() {
        let who: T::AccountId = whitelisted_caller();
        let amount = T::Unit::get();
        let _ = T::Currency::deposit_creating(&Pallet::<T>::pot(0), amount * 2);
        #[block]
        {
            assert_eq!(SubnetRewards::<T, 0>::pay(&who, amount), amount);
        }
    }
    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}

use super::*;
use frame_benchmarking::v2::*;
use frame_support::traits::{Currency, EnsureOrigin};

#[benchmarks(where T: pallet_emission_controller::Config)]
mod benchmarks {
    use super::*;
    #[benchmark]
    fn mint() {
        let origin = T::MintOrigin::try_successful_origin().expect("benchmark authority");
        let who: T::AccountId = whitelisted_caller();
        State::<T>::put(FaucetState::Enabled);
        pallet_emission_controller::FaucetBudget::<T>::put(u128::MAX);
        let amount = <T as Config>::Currency::minimum_balance();
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, who, amount);
    }
    #[benchmark]
    fn disable() {
        let origin = T::MintOrigin::try_successful_origin().expect("benchmark authority");
        State::<T>::put(FaucetState::Enabled);
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin);
        assert_eq!(State::<T>::get(), FaucetState::PermanentlyDisabled);
    }
    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}

use super::*;
use frame_benchmarking::v2::*;

#[benchmarks(where T::GovernanceCall: From<frame_system::Call<T>>)]
mod benchmarks {
    use super::*;
    use alloc::boxed::Box;
    use frame_support::traits::EnsureOrigin;
    fn fill<T: Config>(count: u32) {
        let mut approved = BoundedBTreeSet::<T::AccountId, T::MaxAuthorities>::new();
        for i in 0..count {
            approved.try_insert(account("approved", i, 0)).unwrap();
        }
        Approved::<T>::put(approved);
        Mode::<T>::put(AdmissionMode::FoundationOnly);
    }
    #[benchmark]
    fn approve() {
        fill::<T>(T::MaxAuthorities::get() - 1);
        let origin = T::FoundationOrigin::try_successful_origin().unwrap();
        let who: T::AccountId = whitelisted_caller();
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, who.clone());
        assert!(Approved::<T>::get().contains(&who));
    }
    #[benchmark]
    fn remove() {
        fill::<T>(T::MaxAuthorities::get());
        let origin = T::FoundationOrigin::try_successful_origin().unwrap();
        let who: T::AccountId = account("approved", 0, 0);
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, who.clone());
        assert!(!Approved::<T>::get().contains(&who));
    }
    #[benchmark]
    fn advance() {
        Mode::<T>::put(AdmissionMode::FixedTestnet);
        let origin = T::FoundationOrigin::try_successful_origin().unwrap();
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, AdmissionMode::FoundationOnly);
        assert_eq!(Mode::<T>::get(), AdmissionMode::FoundationOnly);
    }
    #[benchmark]
    fn dispatch_as_root() {
        let origin = T::FoundationOrigin::try_successful_origin().unwrap();
        let call = Box::new(
            frame_system::Call::<T>::remark {
                remark: alloc::vec![],
            }
            .into(),
        );
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, call);
    }
    impl_benchmark_test_suite!(Pallet, crate::tests::ext(), crate::tests::Test);
}

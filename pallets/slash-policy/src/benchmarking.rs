use super::*;
use frame_benchmarking::v2::*;
use frame_election_provider_support::ElectionProvider;
use frame_support::{
    traits::{EnsureOrigin, Get},
    BoundedVec,
};
use sp_runtime::{Perbill, Saturating};

#[benchmarks]
mod benchmarks {
    use super::*;
    #[benchmark]
    fn discretionary_slash() -> Result<(), BenchmarkError> {
        let origin = T::CouncilOrigin::try_successful_origin()
            .map_err(|_| BenchmarkError::Stop("council origin"))?;
        let (stash, _) = pallet_staking::testing_utils::create_stash_controller::<T>(
            0,
            1_000,
            pallet_staking::RewardDestination::Stash,
        )?;
        let own = pallet_staking::Pallet::<T>::slashable_balance_of(&stash);
        let mut total = own;
        let mut others = alloc::vec::Vec::new();
        for i in 0..<T::ElectionProvider as ElectionProvider>::MaxBackersPerWinnerFinal::get() {
            let (who, _) = pallet_staking::testing_utils::create_stash_controller::<T>(
                i.saturating_add(1),
                1_000,
                pallet_staking::RewardDestination::Stash,
            )?;
            let value = pallet_staking::Pallet::<T>::slashable_balance_of(&who);
            total = total.saturating_add(value);
            others.push(pallet_staking::IndividualExposure { who, value });
        }
        let era = T::MaxBudgetWindow::get();
        pallet_staking::ActiveEra::<T>::put(pallet_staking::ActiveEraInfo {
            index: era,
            start: None,
        });
        pallet_staking::CurrentEra::<T>::put(era);
        pallet_staking::ErasStartSessionIndex::<T>::insert(era, era);
        pallet_staking::EraInfo::<T>::set_exposure(
            era,
            &stash,
            pallet_staking::Exposure { total, own, others },
        );
        let history: BoundedVec<_, T::MaxBudgetWindow> = (0..era)
            .map(|e| (e, Perbill::from_parts(1)))
            .collect::<alloc::vec::Vec<_>>()
            .try_into()
            .unwrap();
        SlashHistory::<T>::insert(&stash, history);
        let fraction = DEFAULT_PER_MOTION;
        let apply_era = era
            .saturating_add(T::SlashDeferDuration::get())
            .saturating_add(1);
        let before = pallet_staking::UnappliedSlashes::<T>::decode_len(apply_era).unwrap_or(0);
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, stash.clone(), era, fraction);
        assert_eq!(
            pallet_staking::UnappliedSlashes::<T>::decode_len(apply_era),
            Some(before.saturating_add(1))
        );
        assert_eq!(
            SlashHistory::<T>::get(&stash).len(),
            T::MaxBudgetWindow::get() as usize
        );
        Ok(())
    }

    #[benchmark]
    fn set_limits() -> Result<(), BenchmarkError> {
        let origin = T::CouncilOrigin::try_successful_origin()
            .map_err(|_| BenchmarkError::Stop("council origin"))?;
        let per_motion = MAX_PER_MOTION;
        let cumulative = MAX_CUMULATIVE;
        let window = T::MaxBudgetWindow::get();
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, per_motion, cumulative, window);
        assert_eq!(MaxDiscretionarySlashPerMotion::<T>::get(), per_motion);
        assert_eq!(MaxDiscretionarySlashCumulative::<T>::get(), cumulative);
        assert_eq!(BudgetWindow::<T>::get(), window);
        Ok(())
    }
    impl_benchmark_test_suite!(Pallet, crate::mock::ext(), crate::mock::Test);
}

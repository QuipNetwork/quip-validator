//! Council slashes use staking's exposure accounting and a conservative rolling fraction budget.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
pub use pallet::*;
pub mod weights;
use sp_runtime::Perbill;
use weights::WeightInfo;

/// Immutable policy rails. Raising these requires a runtime upgrade.
pub const MIN_PER_MOTION: Perbill = Perbill::from_parts(1_000_000); // 0.1%
pub const MAX_PER_MOTION: Perbill = Perbill::from_percent(5);
pub const MIN_CUMULATIVE: Perbill = Perbill::from_percent(1);
pub const MAX_CUMULATIVE: Perbill = Perbill::from_percent(10);
pub const DEFAULT_PER_MOTION: Perbill = Perbill::from_percent(1);
pub const DEFAULT_CUMULATIVE: Perbill = Perbill::from_percent(4);
const _: () = assert!(DEFAULT_CUMULATIVE.deconstruct() >= DEFAULT_PER_MOTION.deconstruct());
#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::{pallet_prelude::*, transactional};
    use frame_system::pallet_prelude::*;
    use sp_runtime::{traits::Zero, Perbill, Saturating};

    const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

    #[pallet::config]
    pub trait Config:
        frame_system::Config<RuntimeEvent: From<Event<Self>>> + pallet_staking::Config
    {
        type CouncilOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        /// History is retained for this many eras even when the selected window is shorter.
        #[pallet::constant]
        type MaxBudgetWindow: Get<u32>;
        type SlashWeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::type_value]
    pub fn DefaultPerMotion() -> Perbill {
        DEFAULT_PER_MOTION
    }
    #[pallet::type_value]
    pub fn DefaultCumulative() -> Perbill {
        DEFAULT_CUMULATIVE
    }
    #[pallet::storage]
    pub type MaxDiscretionarySlashPerMotion<T> =
        StorageValue<_, Perbill, ValueQuery, DefaultPerMotion>;
    #[pallet::storage]
    pub type MaxDiscretionarySlashCumulative<T> =
        StorageValue<_, Perbill, ValueQuery, DefaultCumulative>;
    #[pallet::storage]
    pub type BudgetWindow<T: Config> = StorageValue<_, u32, ValueQuery, T::BondingDuration>;
    /// Requested fractions, aggregated by motion era. Cancellation does not restore this budget.
    #[pallet::storage]
    pub type SlashHistory<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        T::AccountId,
        BoundedVec<(u32, Perbill), T::MaxBudgetWindow>,
        ValueQuery,
    >;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A new staking queue entry was created, not an assertion that funds were already burned.
        SlashScheduled {
            stash: T::AccountId,
            offence_era: u32,
            apply_era: u32,
            fraction: Perbill,
            remaining_budget: Perbill,
        },
        /// Council-selected limits, bounded by the immutable ceiling and bonding duration.
        LimitsSet {
            per_motion: Perbill,
            cumulative: Perbill,
            window: u32,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoActiveEra,
        InvalidEra,
        NoExposure,
        InvalidFraction,
        InvalidWindow,
        BudgetExceeded,
        HistoryFull,
        NoSlashScheduled,
        ArithmeticOverflow,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn integrity_test() {
            assert!(T::BondingDuration::get() > 0);
            assert!(T::MaxBudgetWindow::get() >= T::BondingDuration::get());
            assert!(T::SlashDeferDuration::get() > 0);
            assert!(DEFAULT_PER_MOTION >= MIN_PER_MOTION && DEFAULT_PER_MOTION <= MAX_PER_MOTION);
            assert!(DEFAULT_CUMULATIVE >= MIN_CUMULATIVE && DEFAULT_CUMULATIVE <= MAX_CUMULATIVE);
            assert!(DEFAULT_CUMULATIVE >= DEFAULT_PER_MOTION);
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Schedule an exposure-based slash within the per-stash rolling council budget.
        /// Automatic consensus slashes are separate and may exceed the council ceiling.
        /// Governance policy forbids raising hard limits "without a spec-versioned runtime upgrade".
        /// The separately allowed unchecked code-upgrade paths do not enforce that version bump.
        #[pallet::call_index(0)]
        #[pallet::weight(T::SlashWeightInfo::discretionary_slash())]
        #[transactional]
        pub fn discretionary_slash(
            origin: OriginFor<T>,
            stash: T::AccountId,
            era: u32,
            fraction: Perbill,
        ) -> DispatchResult {
            T::CouncilOrigin::ensure_origin(origin)?;
            let active = pallet_staking::ActiveEra::<T>::get()
                .ok_or(Error::<T>::NoActiveEra)?
                .index;
            ensure!(
                era <= active && era >= active.saturating_sub(T::SlashDeferDuration::get()),
                Error::<T>::InvalidEra
            );
            let exposure = pallet_staking::ErasStakersOverview::<T>::get(era, &stash)
                .ok_or(Error::<T>::NoExposure)?;
            ensure!(!exposure.total.is_zero(), Error::<T>::NoExposure);
            let per_motion = MaxDiscretionarySlashPerMotion::<T>::get();
            let cumulative = MaxDiscretionarySlashCumulative::<T>::get();
            Self::check_fractions(per_motion, cumulative)?;
            ensure!(
                !fraction.is_zero() && fraction <= per_motion,
                Error::<T>::InvalidFraction
            );
            let window = BudgetWindow::<T>::get();
            ensure!(
                window >= T::BondingDuration::get() && window <= T::MaxBudgetWindow::get(),
                Error::<T>::InvalidWindow
            );
            let mut history = SlashHistory::<T>::get(&stash);
            // Keep the maximum supported horizon so increasing the window cannot forgive old motions.
            let retained_from = active.saturating_sub(T::MaxBudgetWindow::get().saturating_sub(1));
            history.retain(|(when, _)| *when >= retained_from);
            let budget_from = active.saturating_sub(window.saturating_sub(1));
            let charged = history
                .iter()
                .filter(|(when, _)| *when >= budget_from)
                .try_fold(fraction.deconstruct(), |sum, (_, used)| {
                    sum.checked_add(used.deconstruct())
                })
                .ok_or(Error::<T>::BudgetExceeded)?;
            ensure!(
                charged <= cumulative.deconstruct(),
                Error::<T>::BudgetExceeded
            );
            if let Some((_, used)) = history.iter_mut().find(|(when, _)| *when == active) {
                // The checked cumulative budget above proves this sum is within one Perbill.
                *used = used.saturating_add(fraction);
            } else {
                history
                    .try_push((active, fraction))
                    .map_err(|_| Error::<T>::HistoryFull)?;
            }
            let apply_era = era
                .checked_add(T::SlashDeferDuration::get())
                .and_then(|e| e.checked_add(1))
                .ok_or(Error::<T>::ArithmeticOverflow)?;
            let before = pallet_staking::UnappliedSlashes::<T>::decode_len(apply_era).unwrap_or(0);
            pallet_staking::Pallet::<T>::manual_slash(
                frame_system::RawOrigin::Root.into(),
                stash.clone(),
                era,
                fraction,
            )?;
            let queued = pallet_staking::UnappliedSlashes::<T>::get(apply_era);
            ensure!(
                queued.get(before).is_some_and(|slash| {
                    !slash.own.is_zero() || slash.others.iter().any(|(_, amount)| !amount.is_zero())
                }),
                Error::<T>::NoSlashScheduled
            );
            SlashHistory::<T>::insert(&stash, history);
            Self::deposit_event(Event::SlashScheduled {
                stash,
                offence_era: era,
                apply_era,
                fraction,
                remaining_budget: Perbill::from_parts(
                    cumulative.deconstruct().saturating_sub(charged),
                ),
            });
            Ok(())
        }

        /// Tighten/relax policy within immutable safety bounds; existing motions remain charged.
        #[pallet::call_index(1)]
        #[pallet::weight(T::SlashWeightInfo::set_limits())]
        pub fn set_limits(
            origin: OriginFor<T>,
            per_motion: Perbill,
            cumulative: Perbill,
            window: u32,
        ) -> DispatchResult {
            T::CouncilOrigin::ensure_origin(origin)?;
            Self::check_fractions(per_motion, cumulative)?;
            ensure!(
                window >= T::BondingDuration::get() && window <= T::MaxBudgetWindow::get(),
                Error::<T>::InvalidWindow
            );
            MaxDiscretionarySlashPerMotion::<T>::put(per_motion);
            MaxDiscretionarySlashCumulative::<T>::put(cumulative);
            BudgetWindow::<T>::put(window);
            Self::deposit_event(Event::LimitsSet {
                per_motion,
                cumulative,
                window,
            });
            Ok(())
        }
    }
}

impl<T: Config> Pallet<T> {
    fn check_fractions(per_motion: Perbill, cumulative: Perbill) -> sp_runtime::DispatchResult {
        frame_support::ensure!(
            (MIN_PER_MOTION..=MAX_PER_MOTION).contains(&per_motion)
                && (MIN_CUMULATIVE..=MAX_CUMULATIVE).contains(&cumulative)
                && cumulative >= per_motion,
            Error::<T>::InvalidFraction
        );
        Ok(())
    }
}

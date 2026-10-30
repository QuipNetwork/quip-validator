//! Fixed, time-accrued issuance into subnet pots; rewards spend those pots.
//! No public mint call or mutable schedule. Genesis and missing-state defaults
//! are closed. Faucet issuance is an explicitly separate, finite testnet budget.
#![cfg_attr(not(feature = "std"), no_std)]
extern crate alloc;
pub use pallet::*;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod weights;

use frame_support::{
    traits::{Currency, ExistenceRequirement, Get, Imbalance},
    PalletId,
};
use sp_runtime::{
    traits::{Hash, TrailingZeroInput},
    DispatchError, DispatchResult,
};

/// The annual envelope is the protocol contract; the 75/25 split is V0 policy.
/// Staking mints its share through pallet-staking claims, NOT a controller route.
pub const ANNUAL_EMISSION_QUIP: u128 = 40_000_000;
pub const CONTROLLER_SHARE_PERCENT: u128 = 75;
pub const STAKING_SHARE_PERCENT: u128 = 25;
pub const CONTROLLER_ANNUAL_EMISSION_QUIP: u128 = ANNUAL_EMISSION_QUIP
    .saturating_mul(CONTROLLER_SHARE_PERCENT)
    .saturating_div(100);
pub const STAKING_ANNUAL_EMISSION_QUIP: u128 = ANNUAL_EMISSION_QUIP
    .saturating_mul(STAKING_SHARE_PERCENT)
    .saturating_div(100);
/// Both budgets use the protocol's existing fixed 365-day accounting year.
pub const YEAR_SECONDS: u64 = 365 * 24 * 60 * 60;
pub const MILLISECONDS_PER_YEAR: u64 = YEAR_SECONDS * 1_000;
const _: () = {
    assert!(CONTROLLER_SHARE_PERCENT + STAKING_SHARE_PERCENT == 100);
    assert!(CONTROLLER_ANNUAL_EMISSION_QUIP + STAKING_ANNUAL_EMISSION_QUIP == ANNUAL_EMISSION_QUIP);
    assert!(STAKING_ANNUAL_EMISSION_QUIP * 4 == ANNUAL_EMISSION_QUIP);
    assert!(CONTROLLER_ANNUAL_EMISSION_QUIP == STAKING_ANNUAL_EMISSION_QUIP * 3);
};
/// Ising and QVRF are distinct allocation buckets, not topology identifiers.
pub const ISING_SUBNET: u32 = 0;
/// Reserved QVRF bucket. No payout consumer exists in Phase 1.
pub const QVRF_SUBNET: u32 = 1;

/// Privileged faucet backend. Caller must enforce the origin and one-way fuse.
pub trait FaucetMint<AccountId, Balance> {
    fn mint(who: &AccountId, amount: Balance) -> DispatchResult;
}
/// Mining reward backend, returning the amount actually paid, possibly zero.
pub trait MiningReward<AccountId, Balance> {
    fn pay(who: &AccountId, requested: Balance) -> Balance;
    fn weight() -> frame_support::weights::Weight;
}

/// Compile-time subnet binding prevents a proof caller from selecting another pot.
pub struct SubnetRewards<T, const SUBNET: u32>(core::marker::PhantomData<T>);
impl<T: Config, const SUBNET: u32> MiningReward<T::AccountId, u128> for SubnetRewards<T, SUBNET> {
    fn pay(who: &T::AccountId, requested: u128) -> u128 {
        let pot = Pallet::<T>::pot(SUBNET);
        // Keep the pot alive; its ED remains part of issuance, not a second mint.
        let available =
            T::Currency::free_balance(&pot).saturating_sub(T::Currency::minimum_balance());
        let amount = requested.min(available);
        if amount == 0 || &pot == who {
            Pallet::<T>::deposit_event(Event::RewardUnavailable {
                subnet: SUBNET,
                who: who.clone(),
                requested,
                error: None,
            });
            return 0;
        }
        if let Err(error) =
            T::Currency::transfer(&pot, who, amount, ExistenceRequirement::KeepAlive)
        {
            Pallet::<T>::deposit_event(Event::RewardUnavailable {
                subnet: SUBNET,
                who: who.clone(),
                requested,
                error: Some(error),
            });
            return 0;
        }
        Pallet::<T>::deposit_event(Event::RewardPaid {
            subnet: SUBNET,
            who: who.clone(),
            amount,
        });
        amount
    }
    fn weight() -> frame_support::weights::Weight {
        <T::WeightInfo as weights::WeightInfo>::reward()
    }
}
impl<T: Config> FaucetMint<T::AccountId, u128> for Pallet<T> {
    fn mint(who: &T::AccountId, amount: u128) -> DispatchResult {
        Self::mint_faucet(who, amount)
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use crate::weights::WeightInfo;
    use alloc::vec::Vec;
    use frame_support::{pallet_prelude::*, traits::UnixTime, transactional};
    use frame_system::pallet_prelude::*;
    use sp_runtime::{DispatchResult, Perbill};

    const VERSION: StorageVersion = StorageVersion::new(2);
    #[pallet::pallet]
    #[pallet::storage_version(VERSION)]
    pub struct Pallet<T>(_);
    #[pallet::config]
    pub trait Config: frame_system::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: Currency<Self::AccountId, Balance = u128>;
        type Clock: UnixTime;
        /// Base units per QUIP, fixed by the runtime (10^12).
        type Unit: Get<u128>;
        /// Controller entitlement in base units per protocol accounting year.
        /// Excludes staking, which mints its own share on reward claims.
        #[pallet::constant]
        type AnnualEmission: Get<u128>;
        type PotId: Get<PalletId>;
        #[pallet::constant]
        type MaxSubnets: Get<u32>;
        type WeightInfo: WeightInfo;
    }
    #[pallet::storage]
    pub type Enabled<T> = StorageValue<_, bool, ValueQuery>;
    /// None means anchor at the first nonzero on-chain timestamp, never Unix epoch.
    #[pallet::storage]
    pub type StartAt<T> = StorageValue<_, u64, OptionQuery>;
    #[pallet::storage]
    pub type Routes<T: Config> =
        StorageValue<_, BoundedVec<(u32, Perbill), T::MaxSubnets>, ValueQuery>;
    #[pallet::storage]
    pub type SubnetIssued<T> = StorageMap<_, Twox64Concat, u32, u128, ValueQuery>;
    #[pallet::storage]
    pub type ScheduledIssued<T> = StorageValue<_, u128, ValueQuery>;
    #[pallet::storage]
    pub type FaucetBudget<T> = StorageValue<_, u128, ValueQuery>;
    #[pallet::storage]
    pub type FaucetIssued<T> = StorageValue<_, u128, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub enabled: bool,
        pub start_at: Option<u64>,
        pub routes: Vec<(u32, Perbill)>,
        pub faucet_budget: u128,
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                enabled: false,
                start_at: None,
                routes: Vec::new(),
                faucet_budget: 0,
                _config: Default::default(),
            }
        }
    }
    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            assert!(
                Pallet::<T>::valid_routes(&self.routes, self.enabled),
                "invalid emission routes"
            );
            assert!(
                T::AnnualEmission::get() > 0,
                "annual emission must be nonzero"
            );
            assert_ne!(
                self.start_at,
                Some(0),
                "emission start must not be Unix epoch"
            );
            let routes: BoundedVec<_, T::MaxSubnets> =
                self.routes.clone().try_into().expect("bounded routes");
            Enabled::<T>::put(self.enabled);
            StartAt::<T>::set(self.start_at);
            Routes::<T>::put(routes);
            FaucetBudget::<T>::put(self.faucet_budget);
        }
    }
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        SubnetFunded {
            subnet: u32,
            amount: u128,
        },
        RewardPaid {
            subnet: u32,
            who: T::AccountId,
            amount: u128,
        },
        FaucetMinted {
            who: T::AccountId,
            amount: u128,
        },
        /// Accrual will retry this entitlement; no issuance was recorded.
        FundingFailed {
            subnet: u32,
            amount: u128,
            error: DispatchError,
        },
        /// None indicates an empty pot or self-transfer, Some a currency error.
        RewardUnavailable {
            subnet: u32,
            who: T::AccountId,
            requested: u128,
            error: Option<DispatchError>,
        },
    }
    #[pallet::error]
    pub enum Error<T> {
        ZeroAmount,
        BudgetExceeded,
        Overflow,
        DepositFailed,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_initialize(_: BlockNumberFor<T>) -> Weight {
            // Timestamp is the previous block's consensus timestamp here. The
            // one-block lag avoids wall-clock input and is independent of proofs.
            Self::accrue(T::Clock::now().as_secs());
            T::WeightInfo::accrue(T::MaxSubnets::get())
        }
        fn on_runtime_upgrade() -> Weight {
            if Pallet::<T>::on_chain_storage_version() >= VERSION {
                return T::DbWeight::get().reads(1);
            }
            // Preserve all issued counters and balances. Moving the anchor
            // avoids a silent stall when the annual rate drops from 40M to 30M.
            // Unconfigured/disabled chains remain closed; unanchored genesis
            // still waits for the first consensus timestamp.
            let Ok(start) = Self::rebased_start() else {
                sp_runtime::print(
                    "EmissionController: invalid rebase; storage version not advanced",
                );
                return T::DbWeight::get().reads(5);
            };
            StartAt::<T>::set(start);
            VERSION.put::<Pallet<T>>();
            T::DbWeight::get().reads_writes(5, 2)
        }
        fn integrity_test() {
            assert!(
                T::AnnualEmission::get() > 0,
                "annual emission must be nonzero"
            );
        }
        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<Vec<u8>, sp_runtime::TryRuntimeError> {
            Self::try_state(<frame_system::Pallet<T> as sp_runtime::traits::BlockNumberProvider>::current_block_number())?;
            let start = if Pallet::<T>::on_chain_storage_version() < VERSION {
                Self::rebased_start()?
            } else {
                StartAt::<T>::get()
            };
            let counters: Vec<_> = Routes::<T>::get()
                .into_iter()
                .map(|(subnet, _)| (subnet, SubnetIssued::<T>::get(subnet)))
                .collect();
            Ok((
                start,
                ScheduledIssued::<T>::get(),
                T::Currency::total_issuance(),
                counters,
            )
                .encode())
        }
        #[cfg(feature = "try-runtime")]
        fn post_upgrade(state: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let (start, issued, issuance, counters): (Option<u64>, u128, u128, Vec<(u32, u128)>) =
                Decode::decode(&mut &state[..]).map_err(|_| "invalid rebase pre-state")?;
            ensure!(
                Pallet::<T>::on_chain_storage_version() == VERSION,
                "rebase did not complete"
            );
            ensure!(StartAt::<T>::get() == start, "incorrect rebase anchor");
            ensure!(
                ScheduledIssued::<T>::get() == issued,
                "rebase changed issued counter"
            );
            ensure!(
                T::Currency::total_issuance() == issuance,
                "rebase changed issuance"
            );
            for (subnet, amount) in counters {
                ensure!(
                    SubnetIssued::<T>::get(subnet) == amount,
                    "rebase changed subnet counter"
                );
            }
            Self::try_state(<frame_system::Pallet<T> as sp_runtime::traits::BlockNumberProvider>::current_block_number())
        }
        #[cfg(feature = "try-runtime")]
        fn try_state(_: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            ensure!(
                Self::valid_routes(&Routes::<T>::get(), Enabled::<T>::get()),
                "invalid emission routes"
            );
            ensure!(
                FaucetIssued::<T>::get() <= FaucetBudget::<T>::get(),
                "faucet budget exceeded"
            );
            let mut total = 0u128;
            for (subnet, _) in Routes::<T>::get() {
                total = total
                    .checked_add(SubnetIssued::<T>::get(subnet))
                    .ok_or("issuance overflow")?;
            }
            ensure!(
                total == ScheduledIssued::<T>::get(),
                "scheduled issuance mismatch"
            );
            Ok(())
        }
    }
    impl<T: Config> Pallet<T> {
        pub fn pot(subnet: u32) -> T::AccountId {
            // Hash before decoding so even small test AccountIds retain the
            // subnet domain (PalletId truncation alone would alias u64 pots).
            let hash = T::Hashing::hash_of(&(b"quip/emission/pot", T::PotId::get().0, subnet));
            T::AccountId::decode(&mut TrailingZeroInput::new(hash.as_ref()))
                .expect("hash decodes account id")
        }
        pub fn valid_routes(routes: &[(u32, Perbill)], enabled: bool) -> bool {
            if routes.len() > T::MaxSubnets::get() as usize {
                return false;
            }
            if routes.is_empty() {
                return !enabled;
            }
            let mut sum = 0u64;
            for (i, (id, share)) in routes.iter().enumerate() {
                if share.deconstruct() == 0 || routes[..i].iter().any(|(other, _)| other == id) {
                    return false;
                }
                sum += u64::from(share.deconstruct());
            }
            sum == 1_000_000_000
        }
        /// Rebase only an enabled, anchored schedule with previously paid emissions.
        /// ScheduledIssued is the sum of SubnetIssued, checked by try_state.
        fn rebased_start() -> Result<Option<u64>, &'static str> {
            let start = StartAt::<T>::get();
            let issued = ScheduledIssued::<T>::get();
            if !Enabled::<T>::get() || start.is_none() || issued == 0 {
                return Ok(start);
            }
            let elapsed = sp_runtime::helpers_128bit::multiply_by_rational_with_rounding(
                issued,
                u128::from(YEAR_SECONDS),
                T::AnnualEmission::get(),
                sp_runtime::Rounding::Down,
            )
            .ok_or("rebase duration overflow or zero annual emission")?;
            let elapsed = u64::try_from(elapsed).map_err(|_| "rebase duration exceeds u64")?;
            let start = T::Clock::now()
                .as_secs()
                .checked_sub(elapsed)
                .filter(|start| *start != 0)
                .ok_or("rebase precedes Unix epoch")?;
            // Flooring can leave less than one second of entitlement below the
            // counters. It never remints; the next timestamp second resumes accrual.
            Ok(Some(start))
        }
        /// Cumulative entitlement avoids per-block rounding loss and handles gaps.
        pub fn entitlement(elapsed: u64) -> Option<u128> {
            let annual = T::AnnualEmission::get();
            let years = annual.checked_mul(u128::from(elapsed / YEAR_SECONDS))?;
            let fraction =
                annual.checked_mul(u128::from(elapsed % YEAR_SECONDS))? / u128::from(YEAR_SECONDS);
            years.checked_add(fraction)
        }
        pub fn accrue(now: u64) {
            if !Enabled::<T>::get() || now == 0 {
                return;
            }
            let Some(start) = StartAt::<T>::get() else {
                StartAt::<T>::put(now);
                return;
            };
            let Some(total) = Self::entitlement(now.saturating_sub(start)) else {
                return;
            };
            for (subnet, share) in Routes::<T>::get() {
                let target = share.mul_floor(total);
                let amount = target.saturating_sub(SubnetIssued::<T>::get(subnet));
                if amount != 0 {
                    if let Err(error) = Self::fund_subnet(subnet, amount) {
                        Self::deposit_event(Event::FundingFailed {
                            subnet,
                            amount,
                            error,
                        });
                    }
                }
            }
        }
        #[transactional]
        fn fund_subnet(subnet: u32, amount: u128) -> DispatchResult {
            let next = SubnetIssued::<T>::get(subnet)
                .checked_add(amount)
                .ok_or(Error::<T>::Overflow)?;
            let total = ScheduledIssued::<T>::get()
                .checked_add(amount)
                .ok_or(Error::<T>::Overflow)?;
            Self::deposit_exact(&Self::pot(subnet), amount)?;
            SubnetIssued::<T>::insert(subnet, next);
            ScheduledIssued::<T>::put(total);
            Self::deposit_event(Event::SubnetFunded { subnet, amount });
            Ok(())
        }
        #[transactional]
        pub fn mint_faucet(who: &T::AccountId, amount: u128) -> DispatchResult {
            ensure!(amount > 0, Error::<T>::ZeroAmount);
            let next = FaucetIssued::<T>::get()
                .checked_add(amount)
                .ok_or(Error::<T>::Overflow)?;
            ensure!(next <= FaucetBudget::<T>::get(), Error::<T>::BudgetExceeded);
            Self::deposit_exact(who, amount)?;
            FaucetIssued::<T>::put(next);
            Self::deposit_event(Event::FaucetMinted {
                who: who.clone(),
                amount,
            });
            Ok(())
        }
        fn deposit_exact(who: &T::AccountId, amount: u128) -> DispatchResult {
            ensure!(
                T::Currency::total_issuance().checked_add(amount).is_some(),
                Error::<T>::Overflow
            );
            let imbalance = T::Currency::deposit_creating(who, amount);
            ensure!(imbalance.peek() == amount, Error::<T>::DepositFailed);
            drop(imbalance);
            Ok(())
        }
    }
}

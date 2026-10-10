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

/// Seconds in the protocol's fixed 365-day accounting year (not a calendar year).
pub const YEAR_SECONDS: u64 = 365 * 24 * 60 * 60;
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

    const VERSION: StorageVersion = StorageVersion::new(1);
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
                T::Unit::get().checked_mul(40_000_000).is_some(),
                "annual emission overflow"
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
            // No genesis execution on upgrade: old chains deliberately remain
            // disabled until a separately reviewed activation migration.
            if Pallet::<T>::on_chain_storage_version() < VERSION {
                VERSION.put::<Pallet<T>>();
                T::DbWeight::get().reads_writes(1, 1)
            } else {
                T::DbWeight::get().reads(1)
            }
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
        /// Cumulative entitlement avoids per-block rounding loss and handles gaps.
        pub fn entitlement(elapsed: u64) -> Option<u128> {
            let annual = T::Unit::get().checked_mul(40_000_000)?;
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

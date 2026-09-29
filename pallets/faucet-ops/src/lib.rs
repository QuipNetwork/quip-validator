//! Budgeted testnet faucet with an irreversible fuse and explicit authority.
//! Missing state and default genesis permanently disable the faucet. Phase 1
//! uses a genesis-designated signed account (which may be a multisig account);
//! Phase 2 can substitute the Foundation origin without weakening the fuse.
#![cfg_attr(not(feature = "std"), no_std)]
pub use pallet::*;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod weights;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::traits::EnsureOrigin;
use scale_info::TypeInfo;
pub use weights::*;

#[derive(
    Clone,
    Copy,
    Default,
    Debug,
    PartialEq,
    Eq,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    TypeInfo,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum FaucetState {
    Enabled,
    #[default]
    PermanentlyDisabled,
    /// Closed legacy state; only a reviewed activation migration may enable it.
    Paused,
}
/// Signed, genesis-appointed operational authority. Root deliberately fails.
pub struct EnsureFaucetAuthority<T>(core::marker::PhantomData<T>);
impl<T: Config> EnsureOrigin<T::RuntimeOrigin> for EnsureFaucetAuthority<T> {
    type Success = T::AccountId;
    fn try_origin(origin: T::RuntimeOrigin) -> Result<Self::Success, T::RuntimeOrigin> {
        if let Ok(who) = frame_system::ensure_signed(origin.clone()) {
            if Authority::<T>::get().as_ref() == Some(&who) {
                return Ok(who);
            }
        }
        Err(origin)
    }
    #[cfg(feature = "runtime-benchmarks")]
    fn try_successful_origin() -> Result<T::RuntimeOrigin, ()> {
        let who: T::AccountId = frame_benchmarking::whitelisted_caller();
        Authority::<T>::put(&who);
        Ok(frame_system::RawOrigin::Signed(who).into())
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::{pallet_prelude::*, traits::Currency};
    use frame_system::pallet_prelude::*;
    use pallet_emission_controller::FaucetMint;
    use sp_runtime::traits::Zero;
    type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
    const VERSION: StorageVersion = StorageVersion::new(1);
    #[pallet::pallet]
    #[pallet::storage_version(VERSION)]
    pub struct Pallet<T>(_);
    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_balances::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: Currency<Self::AccountId>;
        type MintOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type Emissions: FaucetMint<Self::AccountId, BalanceOf<Self>>;
        type WeightInfo: WeightInfo;
    }
    #[pallet::storage]
    pub type State<T> = StorageValue<_, FaucetState, ValueQuery>;
    #[pallet::storage]
    pub type Authority<T: Config> = StorageValue<_, T::AccountId, OptionQuery>;
    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub state: FaucetState,
        pub authority: Option<T::AccountId>,
    }
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                state: FaucetState::PermanentlyDisabled,
                authority: None,
            }
        }
    }
    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            assert!(
                self.state != FaucetState::Enabled || self.authority.is_some(),
                "enabled faucet needs an authority"
            );
            State::<T>::put(self.state);
            Authority::<T>::set(self.authority.clone());
        }
    }
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        Minted {
            who: T::AccountId,
            amount: BalanceOf<T>,
        },
        PermanentlyDisabled,
    }
    #[pallet::error]
    pub enum Error<T> {
        ZeroAmount,
        // Keep the historical error index reserved for metadata compatibility.
        DepositFailed,
        Disabled,
    }
    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn on_runtime_upgrade() -> Weight {
            if Pallet::<T>::on_chain_storage_version() < VERSION {
                // Legacy pallet had no storage. Never infer authorization from sudo.
                // Preserve any explicitly blown fuse, even if version metadata
                // was absent. Missing legacy state is closed but nonterminal.
                if State::<T>::try_get().ok() != Some(FaucetState::PermanentlyDisabled) {
                    State::<T>::put(FaucetState::Paused);
                }
                Authority::<T>::kill();
                VERSION.put::<Pallet<T>>();
                T::DbWeight::get().reads_writes(2, 3)
            } else {
                T::DbWeight::get().reads(1)
            }
        }
        #[cfg(feature = "try-runtime")]
        fn pre_upgrade() -> Result<alloc::vec::Vec<u8>, sp_runtime::TryRuntimeError> {
            Ok((
                Pallet::<T>::on_chain_storage_version() < VERSION,
                State::<T>::try_get().ok() == Some(FaucetState::PermanentlyDisabled),
            )
                .encode())
        }
        #[cfg(feature = "try-runtime")]
        fn post_upgrade(before: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
            let (legacy, was_disabled) = <(bool, bool)>::decode(&mut &before[..])
                .map_err(|_| "bad faucet migration state")?;
            if legacy {
                ensure!(
                    State::<T>::get()
                        == if was_disabled {
                            FaucetState::PermanentlyDisabled
                        } else {
                            FaucetState::Paused
                        }
                        && !Authority::<T>::exists(),
                    "legacy faucet must fail closed"
                );
            }
            Ok(())
        }
    }
    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Mint from the finite testnet budget using the configured authority.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::mint())]
        pub fn mint(
            origin: OriginFor<T>,
            who: T::AccountId,
            amount: BalanceOf<T>,
        ) -> DispatchResult {
            T::MintOrigin::ensure_origin(origin)?;
            ensure!(
                State::<T>::get() == FaucetState::Enabled,
                Error::<T>::Disabled
            );
            ensure!(!amount.is_zero(), Error::<T>::ZeroAmount);
            T::Emissions::mint(&who, amount)?;
            Self::deposit_event(Event::Minted { who, amount });
            Ok(())
        }
        /// Blow the fuse. There is intentionally no enable or reset call.
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::disable())]
        pub fn disable(origin: OriginFor<T>) -> DispatchResult {
            T::MintOrigin::ensure_origin(origin)?;
            ensure!(
                State::<T>::get() == FaucetState::Enabled,
                Error::<T>::Disabled
            );
            State::<T>::put(FaucetState::PermanentlyDisabled);
            Self::deposit_event(Event::PermanentlyDisabled);
            Ok(())
        }
    }
}
extern crate alloc;

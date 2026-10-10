#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{dispatch::GetDispatchInfo, pallet_prelude::*, traits::UnfilteredDispatchable};
pub use pallet::*;
pub mod weights;
use scale_info::TypeInfo;
pub use weights::WeightInfo;

/// Permissionless admission is reserved until objective eligibility is specified.
/// No governance vote can substitute for those checks.
pub trait Graduation {
    fn ready() -> bool;
}
impl Graduation for () {
    fn ready() -> bool {
        false
    }
}

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
pub enum AdmissionMode {
    #[default]
    FixedTestnet,
    FoundationOnly,
    PermissionlessEligibility,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use alloc::{boxed::Box, vec::Vec};
    use frame_system::pallet_prelude::*;
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

    #[pallet::config]
    pub trait Config: frame_system::Config<RuntimeEvent: From<Event<Self>>> {
        type FoundationOrigin: EnsureOrigin<Self::RuntimeOrigin>;
        type GovernanceCall: Parameter
            + GetDispatchInfo
            + UnfilteredDispatchable<RuntimeOrigin = Self::RuntimeOrigin>;
        #[pallet::constant]
        type MaxAuthorities: Get<u32>;
        type Graduation: Graduation;
        type WeightInfo: WeightInfo;
    }
    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);
    #[pallet::storage]
    pub type Mode<T> = StorageValue<_, AdmissionMode, ValueQuery>;
    #[pallet::storage]
    pub type Approved<T: Config> =
        StorageValue<_, BoundedBTreeSet<T::AccountId, T::MaxAuthorities>, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub mode: AdmissionMode,
        pub approved: Vec<T::AccountId>,
    }
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                mode: AdmissionMode::FixedTestnet,
                approved: Vec::new(),
            }
        }
    }
    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            assert!(
                self.mode != AdmissionMode::PermissionlessEligibility,
                "permissionless genesis requires a future eligibility implementation"
            );
            let mut approved = BoundedBTreeSet::<T::AccountId, T::MaxAuthorities>::new();
            for who in &self.approved {
                assert!(
                    approved
                        .try_insert(who.clone())
                        .expect("too many approved validators"),
                    "duplicate approved validator"
                );
            }
            Mode::<T>::put(self.mode);
            Approved::<T>::put(approved);
        }
    }
    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        fn integrity_test() {
            assert!(T::MaxAuthorities::get() > 0 && T::MaxAuthorities::get() <= 32);
        }
        #[cfg(feature = "try-runtime")]
        fn try_state(_: BlockNumberFor<T>) -> Result<(), sp_runtime::TryRuntimeError> {
            ensure!(
                Approved::<T>::get().len() <= T::MaxAuthorities::get() as usize,
                "admission bound exceeded"
            );
            ensure!(
                Mode::<T>::get() != AdmissionMode::PermissionlessEligibility
                    || T::Graduation::ready(),
                "permissionless mode without objective graduation"
            );
            Ok(())
        }
    }
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        Approved { who: T::AccountId },
        Removed { who: T::AccountId },
        ModeAdvanced { mode: AdmissionMode },
        RootDispatched { result: DispatchResult },
    }
    #[pallet::error]
    pub enum Error<T> {
        WrongMode,
        AlreadyApproved,
        NotApproved,
        TooManyAuthorities,
        InvalidTransition,
        GraduationNotReady,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::approve())]
        pub fn approve(origin: OriginFor<T>, who: T::AccountId) -> DispatchResult {
            T::FoundationOrigin::ensure_origin(origin)?;
            ensure!(
                Mode::<T>::get() == AdmissionMode::FoundationOnly,
                Error::<T>::WrongMode
            );
            Approved::<T>::try_mutate(|set| -> DispatchResult {
                ensure!(!set.contains(&who), Error::<T>::AlreadyApproved);
                set.try_insert(who.clone())
                    .map_err(|_| Error::<T>::TooManyAuthorities)?;
                Ok(())
            })?;
            Self::deposit_event(Event::Approved { who });
            Ok(())
        }
        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::remove())]
        pub fn remove(origin: OriginFor<T>, who: T::AccountId) -> DispatchResult {
            T::FoundationOrigin::ensure_origin(origin)?;
            ensure!(
                Mode::<T>::get() == AdmissionMode::FoundationOnly,
                Error::<T>::WrongMode
            );
            Approved::<T>::try_mutate(|set| -> DispatchResult {
                ensure!(set.remove(&who), Error::<T>::NotApproved);
                Ok(())
            })?;
            Self::deposit_event(Event::Removed { who });
            Ok(())
        }
        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::advance())]
        pub fn advance(origin: OriginFor<T>, mode: AdmissionMode) -> DispatchResult {
            match (Mode::<T>::get(), mode) {
                (AdmissionMode::FixedTestnet, AdmissionMode::FoundationOnly) => {
                    T::FoundationOrigin::ensure_origin(origin)?;
                }
                (AdmissionMode::FoundationOnly, AdmissionMode::PermissionlessEligibility) => {
                    // Anyone may trigger objective graduation once implemented;
                    // a collective vote is neither sufficient nor necessary.
                    ensure_signed(origin)?;
                    ensure!(T::Graduation::ready(), Error::<T>::GraduationNotReady)
                }
                _ => return Err(Error::<T>::InvalidTransition.into()),
            }
            Mode::<T>::put(mode);
            Self::deposit_event(Event::ModeAdvanced { mode });
            Ok(())
        }
        /// The Foundation's explicit Root-equivalent path, including runtime upgrades.
        /// The event carries inner failure; outer success alone is not proof of execution.
        #[pallet::call_index(3)]
        #[pallet::weight((T::WeightInfo::dispatch_as_root().saturating_add(call.get_dispatch_info().call_weight), call.get_dispatch_info().class))]
        pub fn dispatch_as_root(
            origin: OriginFor<T>,
            call: Box<T::GovernanceCall>,
        ) -> DispatchResult {
            T::FoundationOrigin::ensure_origin(origin)?;
            let result = call.dispatch_bypass_filter(frame_system::RawOrigin::Root.into());
            Self::deposit_event(Event::RootDispatched {
                result: result.map(|_| ()).map_err(|e| e.error),
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

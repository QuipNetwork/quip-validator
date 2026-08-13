//! Genesis-configured EIP-155 chain ID.
//!
//! `pallet-revive` reads `Config::ChainId: Get<u64>` at runtime. This pallet
//! owns that value as storage set at genesis so one runtime artifact can serve
//! local development (1337) and the public testnet (20033).
//!
//! There is no dispatchable. A running chain keeps the ID it started with.
//! Missing storage (a runtime upgrade of a chain that never wrote this key)
//! returns [`TESTNET_CHAIN_ID`].

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

use frame_support::traits::Get;

/// EIP-155 chain ID for the public Quip testnet.
pub const TESTNET_CHAIN_ID: u64 = 20_033;

/// Conventional EIP-155 chain ID for local development networks.
pub const LOCAL_CHAIN_ID: u64 = 1_337;

/// Fallback used when the storage key is absent (upgrade path).
pub struct DefaultChainId;

impl Get<u64> for DefaultChainId {
    fn get() -> u64 {
        TESTNET_CHAIN_ID
    }
}

/// `Get<u64>` adapter for `pallet_revive::Config::ChainId`.
pub struct ChainId<T>(core::marker::PhantomData<T>);

impl<T: Config> Get<u64> for ChainId<T> {
    fn get() -> u64 {
        Eip155ChainId::<T>::get()
    }
}

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;

    /// Pallet type for the genesis-configured EIP-155 chain ID.
    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Configuration for the EIP-155 chain ID pallet.
    #[pallet::config]
    pub trait Config: frame_system::Config {}

    /// Stored EIP-155 chain ID.
    #[pallet::storage]
    pub type Eip155ChainId<T: Config> = StorageValue<_, u64, ValueQuery, DefaultChainId>;

    /// Genesis input for the EIP-155 chain ID.
    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        /// Chain ID written at genesis. Must be greater than zero.
        pub chain_id: u64,
        /// Marker so the genesis config stays generic over the runtime.
        #[serde(skip)]
        pub _config: core::marker::PhantomData<T>,
    }

    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                chain_id: TESTNET_CHAIN_ID,
                _config: core::marker::PhantomData,
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> BuildGenesisConfig for GenesisConfig<T> {
        fn build(&self) {
            assert!(
                self.chain_id > 0,
                "evm-chain-id genesis: chain_id must be greater than 0"
            );
            Eip155ChainId::<T>::put(self.chain_id);
        }
    }
}

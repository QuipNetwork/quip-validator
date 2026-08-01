#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use core::marker::PhantomData;
use frame_support::weights::Weight;

/// Weight functions for `pallet-faucet-ops`.
pub trait WeightInfo {
	/// Weight of the root-only `mint` dispatchable.
	fn mint() -> Weight;
}

/// Default substrate database-backed weights for the faucet ops pallet.
pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config + pallet_balances::Config> WeightInfo for SubstrateWeight<T> {
	fn mint() -> Weight {
		// FaucetOps is an operational/development-only pallet, not a production
		// benchmarking target. Its only state transition follows Balances'
		// account-creating mint path, so reuse that upstream benchmarked weight
		// instead of maintaining a pallet-specific benchmark.
		<<T as pallet_balances::Config>::WeightInfo as pallet_balances::WeightInfo>::force_set_balance_creating()
	}
}

/// Test and fallback weights for the faucet ops pallet.
impl WeightInfo for () {
	fn mint() -> Weight {
		<() as pallet_balances::WeightInfo>::force_set_balance_creating()
	}
}

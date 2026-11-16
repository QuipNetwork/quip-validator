//! QUIP-31: close validator key selection before the BABE randomness boundary.
use super::{Babe, EpochDuration, Runtime, RuntimeCall};
use frame_support::{traits::Contains, weights::Weight};

/// Last 64 slots forbid key changes. Under independent slots with primary
/// probability c, a secondary-only tail this long has probability (1-c)^64,
/// approximately 1e-8 at c=1/4. Changing BABE's configuration requires reviewing
/// this bound; the genesis assertions below deliberately pin its assumptions.
pub const KEY_CHANGE_CUTOFF_SLOTS: u64 = 64;

const _: () = {
    assert!(KEY_CHANGE_CUTOFF_SLOTS > 0);
    assert!(KEY_CHANGE_CUTOFF_SLOTS < crate::network_params::EPOCH_DURATION_SLOTS);
    assert!(crate::BABE_GENESIS_EPOCH_CONFIG.c.0 == 1);
    assert!(crate::BABE_GENESIS_EPOCH_CONFIG.c.1 == 4);
    assert!(matches!(
        crate::BABE_GENESIS_EPOCH_CONFIG.allowed_slots,
        sp_consensus_babe::AllowedSlots::PrimaryAndSecondaryPlainSlots
    ));
};

pub struct SessionKeyCallFilter;
impl Contains<RuntimeCall> for SessionKeyCallFilter {
    fn contains(call: &RuntimeCall) -> bool {
        match call {
            RuntimeCall::Session(
                pallet_session::Call::set_keys { .. } | pallet_session::Call::purge_keys { .. },
            ) => key_changes_allowed(),
            _ => true,
        }
    }
}

/// Genesis key installation writes storage directly. At the first block BABE
/// establishes GenesisSlot and CurrentSlot before any extrinsic dispatch;
/// Session's ShouldEndSession=Babe also initializes BABE before epoch rotation.
/// A stale/future epoch index cannot occur on a healthy initialized chain.
/// Fail closed if corrupted or synthetic state violates these invariants.
pub(super) fn key_changes_allowed() -> bool {
    let duration = EpochDuration::get();
    // The SDK helper panics on an overflowing start slot. Check that arithmetic
    // before calling it so even malformed stored epoch state fails closed.
    if pallet_babe::EpochIndex::<Runtime>::get()
        .checked_mul(duration)
        .and_then(|offset| {
            offset.checked_add(u64::from(pallet_babe::GenesisSlot::<Runtime>::get()))
        })
        .is_none()
    {
        return false;
    }
    let start = u64::from(Babe::current_epoch_start());
    let current = u64::from(pallet_babe::CurrentSlot::<Runtime>::get());
    current.checked_sub(start).is_some_and(|offset| {
        // Last 64 slots are [duration-64, duration); never wrap a stale epoch.
        offset < duration && offset < duration.saturating_sub(KEY_CHANGE_CUTOFF_SLOTS)
    })
}

/// Conservative allowance for five reads of three fixed-size BABE values
/// (the SDK start helper rereads EpochIndex and GenesisSlot), arithmetic and
/// their proofs. Include in reference-machine measurement before release.
pub(super) fn filter_weight() -> Weight {
    Weight::from_parts(5_000_000, 8_000)
        .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads(5))
}

/// Charge nested key dispatches as well as direct extrinsics for the filter.
pub struct SessionWeights;
impl pallet_session::WeightInfo for SessionWeights {
    fn set_keys() -> Weight {
        <pallet_session::weights::SubstrateWeight<Runtime> as pallet_session::WeightInfo>::set_keys(
        )
        .saturating_add(filter_weight())
    }
    fn purge_keys() -> Weight {
        <pallet_session::weights::SubstrateWeight<Runtime> as pallet_session::WeightInfo>::purge_keys()
            .saturating_add(filter_weight())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod policy_tests;

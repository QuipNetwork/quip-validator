//! Equivocation reporting and the council's bounded discretionary slash route.
use super::{
    election::MaxBackersPerWinnerFinal,
    validator_onboarding::{BondingDuration, SessionsPerEra},
    EpochDuration,
};
use crate::{AccountId, Offences, Runtime, RuntimeEvent, Staking};
use alloc::vec::Vec;
use frame_support::{
    parameter_types,
    traits::{OnRuntimeUpgrade, StorageVersion},
    weights::Weight,
};
use sp_staking::offence::{Offence, OffenceError, ReportOffence};

parameter_types! {
    /// Transaction-pool revalidation horizon in blocks, not a proof-retention limit.
    pub const ReportLongevity: u64 = (BondingDuration::get() as u64)
        * (SessionsPerEra::get() as u64) * EpochDuration::get();
}

/// Keep this fixed while queued slashes exist; staking derives the slash era at application.
pub const SLASH_DEFER_ERAS: u32 = 27;
const _: () = assert!(SLASH_DEFER_ERAS < BondingDuration::get());

pub type ReportOffences = GuardedReports;
type HistoricalReports = pallet_staking::FilterHistoricalOffences<Staking, Offences>;
type Identification = pallet_session::historical::IdentificationTuple<Runtime>;

/// Match staking's session-to-era lookup, then require a bucket that has not been processed.
/// Shared by API preflight and the authoritative reporter check at dispatch.
pub(crate) fn has_pending_slash_bucket(session: sp_staking::SessionIndex) -> bool {
    let Some(active) = pallet_staking::ActiveEra::<Runtime>::get() else {
        return false;
    };
    let Some(start) = pallet_staking::ErasStartSessionIndex::<Runtime>::get(active.index) else {
        return false;
    };
    let era = if session >= start {
        active.index
    } else {
        let eras = pallet_staking::BondedEras::<Runtime>::get();
        let Some((era, _)) = eras.iter().rev().find(|(_, start)| *start <= session) else {
            return false;
        };
        *era
    };
    era.checked_add(SLASH_DEFER_ERAS)
        .and_then(|e| e.checked_add(1))
        .is_some_and(|apply_era| apply_era > active.index)
}

/// Reject before Offences or staking can record deduplication and max/span accounting.
pub struct GuardedReports;
impl<O: Offence<Identification>> ReportOffence<AccountId, Identification, O> for GuardedReports {
    fn report_offence(reporters: Vec<AccountId>, offence: O) -> Result<(), OffenceError> {
        // Enforce has_pending_slash_bucket again here: API preflight cannot cover direct or delayed reports.
        if !has_pending_slash_bucket(offence.session_index()) {
            // The consensus pallets map reporter errors to their generic report error.
            return Err(OffenceError::Other(0));
        }
        <HistoricalReports as ReportOffence<AccountId, Identification, O>>::report_offence(
            reporters, offence,
        )
    }
    fn is_known_offence(offenders: &[Identification], time_slot: &O::TimeSlot) -> bool {
        <HistoricalReports as ReportOffence<AccountId, Identification, O>>::is_known_offence(
            offenders, time_slot,
        )
    }
}

/// SDK weights plus the bounded report-age guard's three reads and era-list scan.
pub struct EquivocationWeights;
fn bucket_guard_weight() -> Weight {
    Weight::from_parts(10_000_000, 10_000)
        .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads(3))
}
impl pallet_babe::WeightInfo for EquivocationWeights {
    fn plan_config_change() -> Weight {
        <() as pallet_babe::WeightInfo>::plan_config_change()
    }
    fn report_equivocation(validators: u32, nominators: u32) -> Weight {
        <() as pallet_babe::WeightInfo>::report_equivocation(validators, nominators)
            .saturating_add(bucket_guard_weight())
    }
}
impl pallet_grandpa::WeightInfo for EquivocationWeights {
    fn note_stalled() -> Weight {
        <() as pallet_grandpa::WeightInfo>::note_stalled()
    }
    fn report_equivocation(validators: u32, nominators: u32) -> Weight {
        <() as pallet_grandpa::WeightInfo>::report_equivocation(validators, nominators)
            .saturating_add(bucket_guard_weight())
    }
}

impl pallet_offences::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type IdentificationTuple = pallet_session::historical::IdentificationTuple<Runtime>;
    type OnOffenceHandler = Staking;
}

const _: () = {
    assert!(
        <Runtime as pallet_babe::Config>::MaxNominators::get() >= MaxBackersPerWinnerFinal::get()
    );
    assert!(
        <Runtime as pallet_grandpa::Config>::MaxNominators::get()
            >= MaxBackersPerWinnerFinal::get()
    );
};

/// Adding Offences to an existing chain must install its version even without genesis.
pub struct InitializeOffences;
impl OnRuntimeUpgrade for InitializeOffences {
    fn on_runtime_upgrade() -> Weight {
        let db = <Runtime as frame_system::Config>::DbWeight::get();
        if StorageVersion::get::<Offences>() == StorageVersion::new(0) {
            StorageVersion::new(1).put::<Offences>();
            db.reads_writes(1, 1)
        } else {
            db.reads(1)
        }
    }

    #[cfg(feature = "try-runtime")]
    fn post_upgrade(_: alloc::vec::Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        frame_support::ensure!(
            StorageVersion::get::<Offences>() == StorageVersion::new(1),
            "Offences storage version not initialized"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;

parameter_types! {
    /// Conservative bounded-tree inspection overhead; include in reference weight regeneration.
    pub RootFilterWeight: Weight = Weight::from_parts(50_000_000, 0);
}

/// Check nested dispatch calls because Root dispatch bypasses BaseCallFilter.
/// Limit both recursion and work; overly large motions can be split by governance.
///
/// Changing these hard limits requires a runtime code upgrade. Governance is expected
/// to use a spec-versioned upgrade; the explicitly allowed emergency paths
/// (`set_code_without_checks`, `authorize_upgrade_without_checks`) do not enforce a
/// version bump, and using them for parameter changes is a process violation, not a
/// supported route.
pub struct CouncilRootCalls;
impl pallet_validator_admission::RootCallPolicy<crate::RuntimeCall> for CouncilRootCalls {
    fn check(call: &crate::RuntimeCall) -> Result<(), pallet_validator_admission::RootCallError> {
        use pallet_validator_admission::RootCallError;
        fn allowed(
            call: &crate::RuntimeCall,
            depth: u8,
            remaining: &mut u32,
        ) -> Result<(), RootCallError> {
            use crate::RuntimeCall;
            if depth > 8 {
                return Err(RootCallError::DepthLimit);
            }
            if *remaining == 0 {
                return Err(RootCallError::CallLimit);
            }
            *remaining = remaining.saturating_sub(1);
            match call {
                RuntimeCall::Staking(pallet_staking::Call::manual_slash { .. })
                | RuntimeCall::System(
                    frame_system::Call::set_storage { .. }
                    | frame_system::Call::kill_storage { .. }
                    | frame_system::Call::kill_prefix { .. },
                ) => Err(RootCallError::Denied),
                // Proxy/Multisig require signed origins and do not manufacture Root.
                // Inspect their call trees structurally; this does not imply Root propagation.
                RuntimeCall::Proxy(
                    pallet_proxy::Call::proxy { call, .. }
                    | pallet_proxy::Call::proxy_announced { call, .. },
                )
                | RuntimeCall::Multisig(
                    pallet_multisig::Call::as_multi { call, .. }
                    | pallet_multisig::Call::as_multi_threshold_1 { call, .. },
                ) => allowed(call, depth.saturating_add(1), remaining),
                RuntimeCall::ValidatorAdmission(
                    pallet_validator_admission::Call::dispatch_as_root { call },
                )
                | RuntimeCall::Revive(
                    pallet_revive::Call::eth_substrate_call { call, .. }
                    | pallet_revive::Call::dispatch_as_fallback_account { call },
                ) => allowed(call, depth.saturating_add(1), remaining),
                RuntimeCall::Foundation(
                    pallet_collective::Call::execute { proposal, .. }
                    | pallet_collective::Call::propose { proposal, .. },
                ) => allowed(proposal, depth.saturating_add(1), remaining),
                RuntimeCall::Utility(call) => match call {
                    pallet_utility::Call::batch { calls }
                    | pallet_utility::Call::batch_all { calls }
                    | pallet_utility::Call::force_batch { calls } => {
                        for inner in calls {
                            allowed(inner, depth.saturating_add(1), remaining)?;
                        }
                        Ok(())
                    }
                    pallet_utility::Call::as_derivative { call, .. }
                    | pallet_utility::Call::dispatch_as { call, .. }
                    | pallet_utility::Call::dispatch_as_fallible { call, .. }
                    | pallet_utility::Call::with_weight { call, .. } => {
                        allowed(call, depth.saturating_add(1), remaining)
                    }
                    pallet_utility::Call::if_else { main, fallback } => {
                        allowed(main, depth.saturating_add(1), remaining)?;
                        allowed(fallback, depth.saturating_add(1), remaining)
                    }
                    _ => Err(RootCallError::Denied),
                },
                _ => Ok(()),
            }
        }
        allowed(call, 0, &mut 256)
    }
}

impl pallet_slash_policy::Config for Runtime {
    type CouncilOrigin = super::validator_onboarding::FoundationOrRoot;
    type MaxBudgetWindow = <Runtime as pallet_staking::Config>::HistoryDepth;
    type SlashWeightInfo = pallet_slash_policy::weights::SubstrateWeight<Runtime>;
}

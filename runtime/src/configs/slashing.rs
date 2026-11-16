//! Equivocation reporting and the council's bounded discretionary slash route.
use super::{
    election::MaxBackersPerWinnerFinal,
    validator_onboarding::{BondingDuration, SessionsPerEra},
    EpochDuration,
};
use crate::{AccountId, Historical, Offences, Runtime, RuntimeEvent, Staking};
use alloc::vec::Vec;
use frame_support::{
    parameter_types,
    traits::{Get, KeyOwnerProofSystem},
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

/// Bind a current-session BABE proof to the active authority's stash.
/// Historical's current-session trie uses mutable NextKeys, which can already
/// assign a released signing key to a different validator. Past sessions keep
/// using their stored historical roots without this active-set cross-check.
pub struct ActiveBabeKeyOwner;
impl<D: AsRef<[u8]>> KeyOwnerProofSystem<(sp_runtime::KeyTypeId, D)> for ActiveBabeKeyOwner {
    type Proof = sp_session::MembershipProof;
    type IdentificationTuple = Identification;

    fn prove(key: (sp_runtime::KeyTypeId, D)) -> Option<Self::Proof> {
        Historical::prove(key)
    }

    fn check_proof(key: (sp_runtime::KeyTypeId, D), proof: Self::Proof) -> Option<Identification> {
        let active_index =
            if proof.session == pallet_session::CurrentIndex::<Runtime>::get() {
                if key.0 != sp_consensus_babe::KEY_TYPE {
                    return None;
                }
                Some(pallet_babe::Authorities::<Runtime>::get().iter().position(
                    |(authority, _)| AsRef::<[u8]>::as_ref(authority) == key.1.as_ref(),
                )?)
            } else {
                None
            };
        let owner = Historical::check_proof(key, proof)?;
        if let Some(index) = active_index {
            // Session supplies BABE authorities in the same order as Validators.
            if pallet_session::Validators::<Runtime>::get().get(index) != Some(&owner.0) {
                return None;
            }
        }
        Some(owner)
    }
}

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

/// SDK weights plus the report-age guard and BABE's active-owner cross-check.
pub struct EquivocationWeights;
fn bucket_guard_weight() -> Weight {
    Weight::from_parts(10_000_000, 10_000)
        .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads(3))
}

fn active_babe_owner_weight() -> Weight {
    // Charge the current session index and BOTH full bounded lists: 64 hybrid
    // BABE keys with weights, and the corresponding 64 validator account IDs.
    // Include compact-length/trie overhead and conservative CPU headroom. This
    // same check also runs during unsigned-pool validation, outside block weight.
    let max_authorities: u32 = <Runtime as pallet_babe::Config>::MaxAuthorities::get();
    let authorities = u64::from(max_authorities);
    let authority_bytes = <(
        sp_consensus_babe::AuthorityId,
        sp_consensus_babe::BabeAuthorityWeight,
    ) as codec::MaxEncodedLen>::max_encoded_len() as u64;
    let account_bytes = <AccountId as codec::MaxEncodedLen>::max_encoded_len() as u64;
    Weight::from_parts(
        50_000_000,
        authorities
            .saturating_mul(authority_bytes + account_bytes)
            .saturating_add(10_000),
    )
    .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads(3))
}
impl pallet_babe::WeightInfo for EquivocationWeights {
    fn plan_config_change() -> Weight {
        <() as pallet_babe::WeightInfo>::plan_config_change()
    }
    fn report_equivocation(validators: u32, nominators: u32) -> Weight {
        <() as pallet_babe::WeightInfo>::report_equivocation(validators, nominators)
            .saturating_add(bucket_guard_weight())
            .saturating_add(active_babe_owner_weight())
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

#[cfg(test)]
mod tests;

parameter_types! {
    /// Conservative bounded-tree inspection overhead; include in reference weight regeneration.
    pub RootFilterWeight: Weight = Weight::from_parts(50_000_000, 0)
        .saturating_add(super::session_keys::filter_weight());
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
            key_changes_allowed: &mut Option<bool>,
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
                // Root bypasses BaseCallFilter. Apply the same time window
                // to council motions, including nested re-originating calls.
                RuntimeCall::Session(
                    pallet_session::Call::set_keys { .. } | pallet_session::Call::purge_keys { .. },
                ) if !*key_changes_allowed
                    .get_or_insert_with(super::session_keys::key_changes_allowed) =>
                {
                    Err(RootCallError::Denied)
                }
                // Grant administration is signed custody only. Root must not
                // bypass a recipient curve or issue from an arbitrary source.
                RuntimeCall::Vesting(
                    pallet_vesting::Call::force_vested_transfer { .. }
                    | pallet_vesting::Call::force_remove_vesting_schedule { .. },
                )
                | RuntimeCall::Staking(pallet_staking::Call::manual_slash { .. })
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
                ) => allowed(
                    call,
                    depth.saturating_add(1),
                    remaining,
                    key_changes_allowed,
                ),
                RuntimeCall::ValidatorAdmission(
                    pallet_validator_admission::Call::dispatch_as_root { call },
                )
                | RuntimeCall::Revive(
                    pallet_revive::Call::eth_substrate_call { call, .. }
                    | pallet_revive::Call::dispatch_as_fallback_account { call },
                ) => allowed(
                    call,
                    depth.saturating_add(1),
                    remaining,
                    key_changes_allowed,
                ),
                RuntimeCall::Foundation(
                    pallet_collective::Call::execute { proposal, .. }
                    | pallet_collective::Call::propose { proposal, .. },
                ) => allowed(
                    proposal,
                    depth.saturating_add(1),
                    remaining,
                    key_changes_allowed,
                ),
                RuntimeCall::Utility(call) => match call {
                    pallet_utility::Call::batch { calls }
                    | pallet_utility::Call::batch_all { calls }
                    | pallet_utility::Call::force_batch { calls } => {
                        for inner in calls {
                            allowed(
                                inner,
                                depth.saturating_add(1),
                                remaining,
                                key_changes_allowed,
                            )?;
                        }
                        Ok(())
                    }
                    pallet_utility::Call::as_derivative { call, .. }
                    | pallet_utility::Call::dispatch_as { call, .. }
                    | pallet_utility::Call::dispatch_as_fallible { call, .. }
                    | pallet_utility::Call::with_weight { call, .. } => allowed(
                        call,
                        depth.saturating_add(1),
                        remaining,
                        key_changes_allowed,
                    ),
                    pallet_utility::Call::if_else { main, fallback } => {
                        allowed(
                            main,
                            depth.saturating_add(1),
                            remaining,
                            key_changes_allowed,
                        )?;
                        allowed(
                            fallback,
                            depth.saturating_add(1),
                            remaining,
                            key_changes_allowed,
                        )
                    }
                    _ => Err(RootCallError::Denied),
                },
                _ => Ok(()),
            }
        }
        // Read the block's window at most once, only if the tree actually
        // contains a key call. Unrelated structural checks remain storage-free.
        allowed(call, 0, &mut 256, &mut None)
    }
}

impl pallet_slash_policy::Config for Runtime {
    type CouncilOrigin = super::validator_onboarding::FoundationOrRoot;
    type MaxBudgetWindow = <Runtime as pallet_staking::Config>::HistoryDepth;
    type WeightInfo = pallet_slash_policy::weights::SubstrateWeight<Runtime>;
}

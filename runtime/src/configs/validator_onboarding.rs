//! FoundationOnly governance and bounded, admission-first elections.
use super::*;
use crate::{Foundation, Session, Staking};
use alloc::{collections::BinaryHeap, vec, vec::Vec};
use core::cmp::Reverse;
use frame_election_provider_support::{
    bounds::{DataProviderBounds, ElectionBounds, ElectionBoundsBuilder},
    data_provider, onchain, BoundedSupportsOf, ElectionDataProvider, ElectionProvider, PageIndex,
    SequentialPhragmen, VoterOf,
};
use frame_support::traits::{EitherOfDiverse, Nothing};

pub type FoundationOrigin =
    pallet_collective::EnsureProportionAtLeast<AccountId, pallet_collective::Instance1, 2, 3>;
/// Removing a council member needs a strictly stronger mandate than ordinary
/// decisions: 4/5 of the seats. At the V0 size of six that is five votes, so a
/// member can never be removed by the same threshold that appoints them, and a
/// bare majority cannot clear a seat.
pub type CouncilRemovalOrigin =
    pallet_collective::EnsureProportionAtLeast<AccountId, pallet_collective::Instance1, 4, 5>;
pub type FoundationOrRoot = EitherOfDiverse<frame_system::EnsureRoot<AccountId>, FoundationOrigin>;
pub type CouncilRemovalOrRoot =
    EitherOfDiverse<frame_system::EnsureRoot<AccountId>, CouncilRemovalOrigin>;
/// Foundation privileges require actual affirmative votes. Neither the prime
/// nor a simple majority may convert abstentions into approval at expiry.
pub struct AbstentionsAlwaysNo;
impl pallet_collective::DefaultVote for AbstentionsAlwaysNo {
    fn default_vote(_: Option<bool>, _: u32, _: u32, _: u32) -> bool {
        false
    }
}
parameter_types! {
    pub const MaxAdmittedAuthorities: u32 = 64;
    /// V0 validator-set cap (50, council-configurable later). Distinct from the
    /// 64-wide authority and election bounds, which are headroom above it.
    pub const MaxValidatorSetCap: u32 = 50;
    pub const SessionsPerEra: u32 = 6;
    pub const BondingDuration: u32 = 28;
    pub const SetIdSessionEntries: u64 = 28 * 6;
    pub const FoundationMotionDuration: BlockNumber = 24 * super::super::HOURS;
    pub FoundationProposalWeight: Weight = RuntimeBlockWeights::get().max_block / 2;
    // Both genesis and MBE fallback must fit every admitted candidate and the
    // configured nominator ceiling in their single page.
    pub ElectionBoundsConfig: ElectionBounds = ElectionBoundsBuilder::default()
        .voters_count(super::election::VoterSnapshotPerBlock::get().into())
        .targets_count(super::election::TargetSnapshotPerBlock::get().into()).build();
}
impl pallet_collective::Config<pallet_collective::Instance1> for Runtime {
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeEvent = RuntimeEvent;
    type Proposal = RuntimeCall;
    type MotionDuration = FoundationMotionDuration;
    type MaxProposals = ConstU32<32>;
    type MaxMembers = ConstU32<32>;
    type DefaultVote = AbstentionsAlwaysNo;
    type WeightInfo = pallet_collective::weights::SubstrateWeight<Runtime>;
    // Membership is the sole ordinary writer in production. Benchmark builds
    // accept Root so the upstream collective benchmarks can call set_members.
    #[cfg(not(feature = "runtime-benchmarks"))]
    type SetMembersOrigin = frame_system::EnsureNever<AccountId>;
    #[cfg(feature = "runtime-benchmarks")]
    type SetMembersOrigin = frame_system::EnsureRoot<AccountId>;
    type MaxProposalWeight = FoundationProposalWeight;
    type DisapproveOrigin = FoundationOrRoot;
    type KillOrigin = FoundationOrRoot;
    type Consideration = ();
}
impl pallet_membership::Config<pallet_membership::Instance1> for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type AddOrigin = FoundationOrRoot;
    // Appointment, swap and reset stay at the ordinary 2/3 threshold; only
    // removal is raised to 4/5 so a seat cannot be cleared by the body that
    // filled it.
    type RemoveOrigin = CouncilRemovalOrRoot;
    type SwapOrigin = FoundationOrRoot;
    type ResetOrigin = FoundationOrRoot;
    type PrimeOrigin = FoundationOrRoot;
    type MembershipInitialized = Foundation;
    type MembershipChanged = Foundation;
    type MaxMembers = ConstU32<32>;
    type WeightInfo = pallet_membership::weights::SubstrateWeight<Runtime>;
}
impl pallet_validator_admission::Config for Runtime {
    type FoundationOrigin = FoundationOrigin;
    type GovernanceCall = RuntimeCall;
    type MaxAuthorities = MaxAdmittedAuthorities;
    type Graduation = ();
    type WeightInfo = pallet_validator_admission::weights::SubstrateWeight<Runtime>;
}
impl pallet_session::historical::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type FullIdentification = sp_staking::Exposure<AccountId, Balance>;
    type FullIdentificationOf = pallet_staking::DefaultExposureOf<Runtime>;
}
pub struct StakingBenchmarkConfig;
impl pallet_staking::BenchmarkingConfig for StakingBenchmarkConfig {
    type MaxValidators = ConstU32<32>;
    // Exercise dormant nomination/slash paths in benchmark builds. Production
    // genesis still disables nominations; benchmark genesis allows SDK setup.
    type MaxNominators = ConstU32<32>;
}
impl pallet_staking::Config for Runtime {
    type OldCurrency = Balances;
    type Currency = Balances;
    type CurrencyBalance = Balance;
    type RuntimeHoldReason = RuntimeHoldReason;
    type UnixTime = Timestamp;
    type CurrencyToVote = sp_staking::currency_to_vote::U128CurrencyToVote;
    type RewardRemainder = ();
    type RuntimeEvent = RuntimeEvent;
    type Slash = ();
    type Reward = ();
    type SessionsPerEra = SessionsPerEra;
    type BondingDuration = BondingDuration;
    type SlashDeferDuration = ConstU32<0>;
    type AdminOrigin = FoundationOrRoot;
    type SessionInterface = Self;
    // Validators earn network fees plus emissions (MR !2 roles table), but the
    // validator share of 40M QUIP/year is not allocated in the companion spec.
    // Phase 0 #3 is open: mint nothing until specified; any nonzero payout needs
    // plan §6.4 reservation/accounting for delayed claims and era remainders.
    //
    // TODO(v0-governance): staking rewards. V0 plan §5.1 fixes the staking
    // emission share at 25% of the 40M QUIP/yr envelope (10M QUIP/yr), added as
    // a third emission route alongside ISING_SUBNET and QVRF_SUBNET (currently
    // 50/50, and `valid_routes` requires the shares to sum to 100%). The 25% is
    // TBD pending calibration against the final genesis supply: the payout is a
    // fixed annual amount, so APR = share / staked, which is ~2%/yr at 10M/yr
    // against 500M staked. EraPayout must then draw that share
    // (pallet_staking::inflation::compute_total_payout, min 2.5% / max 10% /
    // ideal stake 50% / falloff 5%) with RewardRemainder returning to the same
    // pot, and the routes need a migration.
    type EraPayout = ();
    type MaxExposurePageSize = ConstU32<64>;
    type NextNewSession = Session;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type ElectionProvider = AdmissionElection;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type GenesisElectionProvider = AdmissionElection;
    #[cfg(feature = "runtime-benchmarks")]
    type ElectionProvider = onchain::OnChainExecution<BenchmarkOnChainConfig>;
    #[cfg(feature = "runtime-benchmarks")]
    type GenesisElectionProvider = onchain::OnChainExecution<BenchmarkOnChainConfig>;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type VoterList = pallet_staking::UseNominatorsAndValidatorsMap<Self>;
    #[cfg(feature = "runtime-benchmarks")]
    type VoterList = benchmark_voter_list::BenchmarkVoterList;
    type TargetList = pallet_staking::UseValidatorsMap<Self>;
    type MaxValidatorSet = MaxValidatorSetCap;
    // Each nominator may back up to 16 validators. The SDK nominate benchmark
    // uses Linear<1, MaxNominations>, which is a real range at 16 (a singleton
    // range panics in median-slopes analysis), so benchmarks use the same quota.
    type NominationsQuota = pallet_staking::FixedNominationsQuota<16>;
    type MaxUnlockingChunks = ConstU32<32>;
    type HistoryDepth = ConstU32<84>;
    type MaxControllersInDeprecationBatch = ConstU32<32>;
    type BenchmarkingConfig = StakingBenchmarkConfig;
    type EventListeners = ();
    type WeightInfo = pallet_staking::weights::SubstrateWeight<Runtime>;
    type Filter = Nothing;
}

/// Iterate the bounded admission set FIRST. Filtering an already truncated
/// staking snapshot lets unapproved intents crowd approved validators out.
/// Staking remains the source of candidacy and voting stake. Nominations are
/// restricted to these same electable targets. TargetList stays unchanged.
pub struct AdmissionData;
impl AdmissionData {
    fn candidates() -> Vec<AccountId> {
        System::register_extra_weight_unchecked(
            Weight::from_parts(100_000_000, 128_000)
                .saturating_add(<Runtime as frame_system::Config>::DbWeight::get().reads(200)),
            DispatchClass::Mandatory,
        );
        pallet_validator_admission::Approved::<Runtime>::get()
            .into_iter()
            .filter(|who| {
                pallet_staking::Validators::<Runtime>::contains_key(who)
                    && pallet_session::NextKeys::<Runtime>::contains_key(who)
            })
            .collect()
    }
}
impl ElectionDataProvider for AdmissionData {
    type AccountId = AccountId;
    type BlockNumber = BlockNumber;
    // A nominator's vote list is its target list, bounded by the nominations quota.
    type MaxVotesPerVoter = ConstU32<16>;
    fn electable_targets(
        bounds: DataProviderBounds,
        page: PageIndex,
    ) -> data_provider::Result<Vec<AccountId>> {
        if page != 0 {
            return Err("only one election page is supported");
        }
        let targets = Self::candidates();
        if bounds.slice_exhausted(&targets) {
            return Err("approved target snapshot exceeds bounds");
        }
        Ok(targets)
    }
    fn electing_voters(
        bounds: DataProviderBounds,
        page: PageIndex,
    ) -> data_provider::Result<Vec<VoterOf<Self>>> {
        if page != 0 {
            return Err("only one election page is supported");
        }
        let targets = Self::candidates();
        let mut voters: Vec<VoterOf<Self>> = targets
            .iter()
            .map(|who| {
                let stake = Staking::weight_of(who);
                (
                    who.clone(),
                    stake,
                    vec![who.clone()].try_into().expect("one self vote fits"),
                )
            })
            .filter(|(_, stake, _)| *stake > 0)
            .collect();
        let mut eligible = voters.len() as u32;
        // The runtime ceiling applies even when a caller supplies no bound.
        let limit = bounds
            .count
            .map_or(super::election::VoterSnapshotPerBlock::get(), |bound| {
                bound.0.min(super::election::VoterSnapshotPerBlock::get())
            }) as usize;
        // Self-votes are the admission floor, even for an undersized caller
        // bound. Production bounds reserve room for all admitted targets.
        let self_votes = voters.len();
        let nominator_slots = limit.saturating_sub(self_votes);
        // Keep only the best candidates while scanning: memory stays bounded
        // even if council raises MaxNominatorsCount above the snapshot ceiling.
        // The worst retained voter is at the heap root. AccountId breaks equal
        // stakes deterministically, preferring the lower AccountId.
        let mut nominated = BinaryHeap::new();
        // Nominators are read by scanning the whole `Nominators` map, so charge for it.
        let nominators = pallet_staking::Nominators::<Runtime>::count();
        System::register_extra_weight_unchecked(
            <Runtime as frame_system::Config>::DbWeight::get().reads(nominators.into()),
            DispatchClass::Mandatory,
        );
        for (nominator, nominations) in pallet_staking::Nominators::<Runtime>::iter() {
            // Approved validators vote for themselves above, so their own
            // nominations are not counted.
            if nominations.suppressed || targets.contains(&nominator) {
                continue;
            }
            let votes: Vec<AccountId> = nominations
                .targets
                .into_iter()
                .filter(|target| targets.contains(target))
                .collect();
            if votes.is_empty() {
                continue;
            }
            let stake = Staking::weight_of(&nominator);
            if stake == 0 {
                continue;
            }
            let votes = votes
                .try_into()
                .map_err(|_| "nomination exceeds the vote bound")?;
            eligible = eligible.saturating_add(1);
            if nominator_slots > 0 {
                nominated.push((Reverse(stake), nominator, votes));
                if nominated.len() > nominator_slots {
                    nominated.pop();
                }
            }
        }
        voters.extend(
            nominated
                .into_sorted_vec()
                .into_iter()
                .map(|(Reverse(stake), nominator, votes)| (nominator, stake, votes)),
        );
        // Retain a prefix, so self-votes still precede stake-ranked nominators.
        // Account for SCALE's compact length prefix without repeatedly encoding
        // the full snapshot while removing voters from the tail.
        if let Some(size) = bounds.size {
            let mut payload_size = voters.iter().fold(0usize, |total, voter| {
                total.saturating_add(voter.encoded_size())
            });
            while voters.len() > self_votes
                && payload_size.saturating_add(codec::Compact(voters.len() as u32).encoded_size())
                    > size.0 as usize
            {
                let Some(voter) = voters.pop() else { break };
                payload_size = payload_size.saturating_sub(voter.encoded_size());
            }
        }
        let retained = voters.len() as u32;
        if retained < eligible {
            System::deposit_event(
                pallet_validator_admission::Event::<Runtime>::ElectionVotersTruncated {
                    eligible,
                    retained,
                },
            );
        }
        Ok(voters)
    }
    fn desired_targets() -> data_provider::Result<u32> {
        let desired = <Staking as ElectionDataProvider>::desired_targets()?;
        if desired > MaxValidatorSetCap::get() {
            return Err("requested validator count exceeds the admitted validator set cap");
        }
        // The MBE verifier requires exactly this many winners, including when
        // admission removal leaves fewer candidates than the configured target.
        Ok(desired.min(Self::candidates().len() as u32))
    }
    fn next_election_prediction(now: BlockNumber) -> BlockNumber {
        <Staking as ElectionDataProvider>::next_election_prediction(now)
    }
}
pub struct OnChainConfig;
impl onchain::Config for OnChainConfig {
    type Sort = ConstBool<true>;
    type System = Runtime;
    type Solver = SequentialPhragmen<AccountId, Perbill>;
    type DataProvider = AdmissionData;
    type WeightInfo = frame_election_provider_support::weights::SubstrateWeight<Runtime>;
    type Bounds = ElectionBoundsConfig;
    type MaxBackersPerWinner = super::election::MaxBackersPerWinner;
    type MaxWinnersPerPage = super::election::MaxWinnersPerPage;
}
// The SDK map has an unimplemented benchmark score-update hook. Delegate all
// actual list operations to that same map; it has no score-dependent ordering,
// so increasing/decreasing stake exercises its only update path.
#[cfg(feature = "runtime-benchmarks")]
mod benchmark_voter_list {
    use super::*;
    use alloc::boxed::Box;
    use frame_election_provider_support::SortedListProvider;
    type Map = pallet_staking::UseNominatorsAndValidatorsMap<Runtime>;

    pub struct BenchmarkVoterList;
    impl SortedListProvider<AccountId> for BenchmarkVoterList {
        type Error = <Map as SortedListProvider<AccountId>>::Error;
        type Score = <Map as SortedListProvider<AccountId>>::Score;
        fn iter() -> Box<dyn Iterator<Item = AccountId>> {
            Map::iter()
        }
        fn iter_from(
            start: &AccountId,
        ) -> Result<Box<dyn Iterator<Item = AccountId>>, Self::Error> {
            Map::iter_from(start)
        }
        fn count() -> u32 {
            Map::count()
        }
        fn contains(id: &AccountId) -> bool {
            Map::contains(id)
        }
        fn on_insert(id: AccountId, score: Self::Score) -> Result<(), Self::Error> {
            Map::on_insert(id, score)
        }
        fn on_update(id: &AccountId, score: Self::Score) -> Result<(), Self::Error> {
            Map::on_update(id, score)
        }
        fn get_score(id: &AccountId) -> Result<Self::Score, Self::Error> {
            Map::get_score(id)
        }
        fn on_remove(id: &AccountId) -> Result<(), Self::Error> {
            Map::on_remove(id)
        }
        fn unsafe_regenerate(
            all: impl IntoIterator<Item = AccountId>,
            score_of: Box<dyn Fn(&AccountId) -> Option<Self::Score>>,
        ) -> u32 {
            Map::unsafe_regenerate(all, score_of)
        }
        fn unsafe_clear() {
            Map::unsafe_clear()
        }
        fn lock() {
            Map::lock()
        }
        fn unlock() {
            Map::unlock()
        }
        #[cfg(feature = "try-runtime")]
        fn try_state() -> Result<(), sp_runtime::TryRuntimeError> {
            Map::try_state()
        }
        fn score_update_worst_case(who: &AccountId, is_increase: bool) -> Self::Score {
            let score = Staking::weight_of(who);
            if is_increase {
                score.saturating_mul(2)
            } else {
                score / 2
            }
        }
    }
}

// Upstream staking benchmarks create candidates without admission/session keys
// and require nominator exposures. Their new_era range includes 100 nominators
// plus 10 validators, independently of BenchmarkingConfig's snapshot ranges.
#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
    pub BenchmarkElectionBounds: ElectionBounds = ElectionBoundsBuilder::default()
        .voters_count(132.into()).targets_count(32.into()).build();
}
#[cfg(feature = "runtime-benchmarks")]
pub struct BenchmarkOnChainConfig;
#[cfg(feature = "runtime-benchmarks")]
impl onchain::Config for BenchmarkOnChainConfig {
    type Sort = ConstBool<true>;
    type System = Runtime;
    type Solver = SequentialPhragmen<AccountId, Perbill>;
    type DataProvider = Staking;
    type WeightInfo = frame_election_provider_support::weights::SubstrateWeight<Runtime>;
    type Bounds = BenchmarkElectionBounds;
    type MaxBackersPerWinner = ConstU32<132>;
    type MaxWinnersPerPage = ConstU32<32>;
}
type FilteredElection = onchain::OnChainExecution<OnChainConfig>;
/// The pinned staking Config requires DataProvider = Staking. This bridge keeps
/// that associated-type contract while the actual solver exclusively consumes
/// AdmissionData, for both normal and genesis elections.
pub struct AdmissionElection;
impl ElectionProvider for AdmissionElection {
    type AccountId = AccountId;
    type BlockNumber = BlockNumber;
    type Error = <FilteredElection as ElectionProvider>::Error;
    type MaxWinnersPerPage = super::election::MaxWinnersPerPage;
    type MaxBackersPerWinner = super::election::MaxBackersPerWinner;
    type MaxBackersPerWinnerFinal = super::election::MaxBackersPerWinnerFinal;
    type Pages = ConstU32<1>;
    type DataProvider = Staking;
    fn elect(page: PageIndex) -> Result<BoundedSupportsOf<Self>, Self::Error> {
        FilteredElection::elect(page)
    }
    fn start() -> Result<(), Self::Error> {
        FilteredElection::start()
    }
    fn duration() -> BlockNumber {
        0
    }
    fn status() -> Result<Option<Weight>, ()> {
        FilteredElection::status()
    }
}

/// Appending staking to legacy state must never default to unlimited nominators.
/// This does not activate a live network: rollout uses fresh genesis, or a
/// separately reviewed dormant-pallet then consensus-flip migration.
pub struct InitializeStakingLimits;
impl frame_support::traits::OnRuntimeUpgrade for InitializeStakingLimits {
    fn on_runtime_upgrade() -> Weight {
        if !pallet_staking::MaxNominatorsCount::<Runtime>::exists() {
            pallet_staking::MaxNominatorsCount::<Runtime>::put(0);
            return <Runtime as frame_system::Config>::DbWeight::get().reads_writes(1, 1);
        }
        <Runtime as frame_system::Config>::DbWeight::get().reads(1)
    }
    #[cfg(feature = "try-runtime")]
    fn post_upgrade(_: Vec<u8>) -> Result<(), sp_runtime::TryRuntimeError> {
        frame_support::ensure!(
            pallet_staking::MaxNominatorsCount::<Runtime>::get() == Some(0),
            "Phase 2 nominators must remain disabled"
        );
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::{FoundationMembership, RuntimeGenesisConfig, ValidatorAdmission};
    use frame_support::{
        assert_noop, assert_ok, dispatch::GetDispatchInfo, traits::OnRuntimeUpgrade,
    };
    use sp_runtime::{
        traits::{Dispatchable, Hash as _},
        BuildStorage,
    };

    pub(crate) fn ext() -> sp_io::TestExternalities {
        fn merge(target: &mut serde_json::Value, patch: serde_json::Value) {
            match (target, patch) {
                (serde_json::Value::Object(t), serde_json::Value::Object(p)) => {
                    for (k, v) in p {
                        merge(t.entry(k).or_insert(serde_json::Value::Null), v);
                    }
                }
                (t, p) => *t = p,
            }
        }
        let mut config = serde_json::to_value(RuntimeGenesisConfig::default()).unwrap();
        merge(
            &mut config,
            crate::genesis_config_presets::local_config_genesis(),
        );
        let config: RuntimeGenesisConfig = serde_json::from_value(config).unwrap();
        assert_eq!(
            config
                .session
                .keys
                .iter()
                .map(|(who, _, _)| who.clone())
                .collect::<Vec<_>>(),
            config.validator_admission.approved
        );
        assert_eq!(
            config
                .staking
                .stakers
                .iter()
                .map(|(who, _, _, _)| who.clone())
                .collect::<Vec<_>>(),
            config.validator_admission.approved
        );
        let mut ext = sp_io::TestExternalities::new(config.build_storage().unwrap());
        ext.execute_with(|| System::set_block_number(1));
        ext
    }
    fn foundation() -> RuntimeOrigin {
        pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(1, 1)
            .into()
    }
    fn collective_members(count: u8, prime: bool) -> Vec<AccountId> {
        let members: Vec<_> = (1..=count).map(|i| AccountId::new([i; 32])).collect();
        assert_ok!(FoundationMembership::reset_members(
            RuntimeOrigin::root(),
            members.clone()
        ));
        if prime {
            assert_ok!(FoundationMembership::set_prime(
                RuntimeOrigin::root(),
                crate::Address::Id(members[0].clone())
            ));
        }
        members
    }
    fn root_count_proposal() -> RuntimeCall {
        RuntimeCall::ValidatorAdmission(pallet_validator_admission::Call::dispatch_as_root {
            call: alloc::boxed::Box::new(RuntimeCall::Staking(
                pallet_staking::Call::set_validator_count { new: 3 },
            )),
        })
    }
    fn propose_and_vote(
        call: &RuntimeCall,
        members: &[AccountId],
        threshold: u32,
        ayes: usize,
    ) -> Hash {
        let hash = <Runtime as frame_system::Config>::Hashing::hash_of(call);
        assert_ok!(Foundation::propose(
            RuntimeOrigin::signed(members[0].clone()),
            threshold,
            alloc::boxed::Box::new(call.clone()),
            call.encoded_size() as u32,
        ));
        for member in members.iter().take(ayes) {
            assert_ok!(Foundation::vote(
                RuntimeOrigin::signed(member.clone()),
                hash,
                0,
                true
            ));
        }
        hash
    }
    fn close_proposal(
        call: &RuntimeCall,
        hash: Hash,
    ) -> frame_support::dispatch::DispatchResultWithPostInfo {
        Foundation::close(
            RuntimeOrigin::signed(AccountId::new([90; 32])),
            hash,
            0,
            call.get_dispatch_info().call_weight,
            call.encoded_size() as u32,
        )
    }
    #[test]
    fn foundation_expiry_never_promotes_majority_or_prime_abstentions() {
        for prime in [false, true] {
            for ayes in [1, 3] {
                ext().execute_with(|| {
                    let members = collective_members(5, prime);
                    let call = root_count_proposal();
                    let hash = propose_and_vote(&call, &members, 4, ayes);
                    assert_noop!(
                        close_proposal(&call, hash),
                        pallet_collective::Error::<Runtime, pallet_collective::Instance1>::TooEarly
                    );
                    System::set_block_number(1 + FoundationMotionDuration::get());
                    assert_ok!(close_proposal(&call, hash));
                    assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 2);
                    assert!(System::events().iter().any(|record| matches!(&record.event,
                        RuntimeEvent::Foundation(pallet_collective::Event::Closed { yes, no, .. })
                            if *yes == ayes as u32 && *no == 5 - ayes as u32)));
                    assert!(System::events().iter().any(|record| matches!(
                        &record.event,
                        RuntimeEvent::Foundation(pallet_collective::Event::Disapproved { .. })
                    )));
                    assert!(!System::events().iter().any(|record| matches!(
                        &record.event,
                        RuntimeEvent::ValidatorAdmission(
                            pallet_validator_admission::Event::RootDispatched { .. }
                        )
                    )));
                });
            }
        }
    }
    #[test]
    fn foundation_actual_two_thirds_dispatches_with_or_without_prime() {
        for (seats, ayes) in [(3, 2), (5, 4)] {
            for prime in [false, true] {
                for expired in [false, true] {
                    ext().execute_with(|| {
                        let members = collective_members(seats, prime);
                        let call = root_count_proposal();
                        let hash = propose_and_vote(&call, &members, ayes as u32, ayes);
                        if expired {
                            System::set_block_number(1 + FoundationMotionDuration::get());
                        }
                        assert_ok!(close_proposal(&call, hash));
                        assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 3);
                        assert!(System::events().iter().any(|record| matches!(
                            &record.event,
                            RuntimeEvent::Foundation(pallet_collective::Event::Executed {
                                result: Ok(()),
                                ..
                            })
                        )));
                        assert!(System::events().iter().any(|record| matches!(
                            &record.event,
                            RuntimeEvent::ValidatorAdmission(
                                pallet_validator_admission::Event::RootDispatched {
                                    result: Ok(())
                                }
                            )
                        )));
                    });
                }
            }
        }
    }
    #[test]
    fn foundation_low_proposal_threshold_does_not_bypass_privileged_origin() {
        ext().execute_with(|| {
            let members = collective_members(5, true);
            let call = root_count_proposal();
            let hash = propose_and_vote(&call, &members, 3, 3);
            assert_ok!(close_proposal(&call, hash));
            assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 2);
            assert!(System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::Foundation(pallet_collective::Event::Executed {
                    result: Err(sp_runtime::DispatchError::BadOrigin),
                    ..
                })
            )));
        });
    }
    #[test]
    fn foundation_receipt_preserves_failure_of_root_inner_call() {
        ext().execute_with(|| {
            let members = collective_members(3, true);
            // Faucet authority rotation rejects even Root. Collective execution
            // succeeds, while the nested Root receipt must retain BadOrigin.
            let call = RuntimeCall::ValidatorAdmission(
                pallet_validator_admission::Call::dispatch_as_root {
                    call: alloc::boxed::Box::new(RuntimeCall::FaucetOps(
                        pallet_faucet_ops::Call::set_authority { authority: None },
                    )),
                },
            );
            let before = pallet_faucet_ops::Authority::<Runtime>::get();
            let hash = propose_and_vote(&call, &members, 2, 2);
            assert_ok!(close_proposal(&call, hash));
            assert_eq!(pallet_faucet_ops::Authority::<Runtime>::get(), before);
            assert!(System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::Foundation(pallet_collective::Event::Executed { result: Ok(()), .. })
            )));
            assert!(System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::ValidatorAdmission(
                    pallet_validator_admission::Event::RootDispatched {
                        result: Err(sp_runtime::DispatchError::BadOrigin)
                    }
                )
            )));
        });
    }
    #[cfg(feature = "runtime-benchmarks")]
    #[test]
    fn foundation_benchmarks_cover_reachable_close_paths_with_strict_vote_policy() {
        use pallet_collective::DefaultVote;
        assert!(!<Runtime as pallet_collective::Config<
            pallet_collective::Instance1,
        >>::DefaultVote::default_vote(Some(true), 3, 0, 5));
        let encoded =
            crate::apis::api::dispatch("Benchmark_benchmark_metadata", &false.encode()).unwrap();
        let (list, _): (
            Vec<frame_benchmarking::BenchmarkList>,
            Vec<frame_support::traits::StorageInfo>,
        ) = Decode::decode(&mut encoded.as_slice()).unwrap();
        let foundation = list
            .iter()
            .find(|pallet| pallet.pallet == b"pallet_collective")
            .unwrap();
        for name in [
            b"close_early_approved".as_slice(),
            b"close_early_disapproved",
            b"close_disapproved",
        ] {
            assert!(foundation
                .benchmarks
                .iter()
                .any(|benchmark| benchmark.name == name));
        }
        assert!(!foundation
            .benchmarks
            .iter()
            .any(|benchmark| benchmark.name == b"close_approved"));
    }
    fn bond(who: AccountId) {
        use frame_support::traits::Currency;
        let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&who, 3000 * UNIT);
        assert_ok!(Staking::bond(
            RuntimeOrigin::signed(who.clone()),
            2_000 * UNIT,
            pallet_staking::RewardDestination::Staked
        ));
        assert_ok!(Staking::validate(
            RuntimeOrigin::signed(who),
            Default::default()
        ));
    }
    fn bond_nominator(who: AccountId) {
        use frame_support::traits::Currency;
        let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&who, 3000 * UNIT);
        assert_ok!(Staking::bond(
            RuntimeOrigin::signed(who),
            100 * UNIT,
            pallet_staking::RewardDestination::Staked
        ));
    }
    #[test]
    fn unapproved_intents_cannot_crowd_either_snapshot() {
        ext().execute_with(|| {
            let expected = AdmissionData::electable_targets(Default::default(), 0).unwrap();
            for i in 1..=96 {
                bond(AccountId::new([i; 32]));
            }
            assert_eq!(pallet_staking::Validators::<Runtime>::count(), 98);
            assert_eq!(pallet_staking::MaxValidatorsCount::<Runtime>::get(), None);
            let bounds = DataProviderBounds {
                count: Some(32.into()),
                size: None,
            };
            assert_eq!(
                AdmissionData::electable_targets(bounds, 0).unwrap(),
                expected
            );
            assert_eq!(
                AdmissionData::electing_voters(bounds, 0)
                    .unwrap()
                    .iter()
                    .map(|v| v.0.clone())
                    .collect::<Vec<_>>(),
                expected
            );
            assert_eq!(AdmissionElection::elect(0).unwrap().len(), 2);
            #[cfg(feature = "try-runtime")]
            assert_ok!(<Staking as frame_support::traits::Hooks<BlockNumber>>::try_state(1));
        });
    }
    #[test]
    fn removing_without_chill_filters_targets_and_self_votes() {
        ext().execute_with(|| {
            let who = Session::validators()[0].clone();
            assert_ok!(ValidatorAdmission::remove(foundation(), who.clone()));
            assert!(pallet_staking::Validators::<Runtime>::contains_key(&who));
            assert!(!AdmissionData::electable_targets(Default::default(), 0)
                .unwrap()
                .contains(&who));
            assert!(AdmissionData::electing_voters(Default::default(), 0)
                .unwrap()
                .iter()
                .all(|v| v.0 != who));
            assert!(AdmissionElection::elect(0)
                .unwrap()
                .iter()
                .all(|(id, _)| *id != who));
            #[cfg(feature = "try-runtime")]
            assert_ok!(<Staking as frame_support::traits::Hooks<BlockNumber>>::try_state(1));
        });
    }
    #[test]
    fn nominate_is_open_directly_and_through_utility() {
        ext().execute_with(|| {
            let direct = AccountId::new([99; 32]);
            let batched = AccountId::new([98; 32]);
            let target = Session::validators()[0].clone();
            bond_nominator(direct.clone());
            bond_nominator(batched.clone());
            assert_ok!(Staking::nominate(
                RuntimeOrigin::signed(direct.clone()),
                vec![crate::Address::Id(target.clone())]
            ));
            let call = RuntimeCall::Utility(pallet_utility::Call::batch_all {
                calls: vec![RuntimeCall::Staking(pallet_staking::Call::nominate {
                    targets: vec![crate::Address::Id(target)],
                })],
            });
            assert_ok!(call.dispatch(RuntimeOrigin::signed(batched.clone())));
            assert!(pallet_staking::Nominators::<Runtime>::contains_key(&direct));
            assert!(pallet_staking::Nominators::<Runtime>::contains_key(
                &batched
            ));
        });
    }
    #[test]
    fn nominator_backs_approved_validator_with_own_stake() {
        ext().execute_with(|| {
            let nominator = AccountId::new([97; 32]);
            let target = Session::validators()[0].clone();
            bond_nominator(nominator.clone());
            assert_ok!(Staking::nominate(
                RuntimeOrigin::signed(nominator.clone()),
                vec![crate::Address::Id(target.clone())]
            ));
            let voters = AdmissionData::electing_voters(Default::default(), 0).unwrap();
            let (_, stake, votes) = voters
                .iter()
                .find(|(who, _, _)| *who == nominator)
                .expect("nominator is an elector");
            assert_eq!(*stake, Staking::weight_of(&nominator));
            assert_eq!(votes.to_vec(), vec![target]);
        });
    }
    #[test]
    fn nominator_backing_only_unapproved_targets_is_absent() {
        ext().execute_with(|| {
            let unapproved = AccountId::new([96; 32]);
            let nominator = AccountId::new([95; 32]);
            bond(unapproved.clone());
            bond_nominator(nominator.clone());
            assert_ok!(Staking::nominate(
                RuntimeOrigin::signed(nominator.clone()),
                vec![crate::Address::Id(unapproved)]
            ));
            let voters = AdmissionData::electing_voters(Default::default(), 0).unwrap();
            assert!(voters.iter().all(|(who, _, _)| *who != nominator));
        });
    }
    #[test]
    fn nominator_votes_stay_inside_electable_targets() {
        ext().execute_with(|| {
            let unapproved = AccountId::new([94; 32]);
            let nominator = AccountId::new([93; 32]);
            let target = Session::validators()[0].clone();
            bond(unapproved.clone());
            bond_nominator(nominator.clone());
            assert_ok!(Staking::nominate(
                RuntimeOrigin::signed(nominator.clone()),
                vec![
                    crate::Address::Id(unapproved.clone()),
                    crate::Address::Id(target.clone())
                ]
            ));
            let electable = AdmissionData::electable_targets(Default::default(), 0).unwrap();
            let voters = AdmissionData::electing_voters(Default::default(), 0).unwrap();
            let (_, _, votes) = voters
                .iter()
                .find(|(who, _, _)| *who == nominator)
                .expect("nominator keeps its electable target");
            assert_eq!(votes.to_vec(), vec![target]);
            assert!(votes.iter().all(|vote| electable.contains(vote)));
            assert!(!votes.contains(&unapproved));
        });
    }
    fn nominate_with_stake(who: AccountId, stake: Balance, targets: Vec<AccountId>) {
        use frame_support::traits::Currency;
        let _ = Balances::make_free_balance_be(&who, stake + UNIT);
        assert_ok!(Staking::bond(
            RuntimeOrigin::signed(who.clone()),
            stake,
            pallet_staking::RewardDestination::Staked
        ));
        assert_ok!(Staking::nominate(
            RuntimeOrigin::signed(who),
            targets.into_iter().map(crate::Address::Id).collect()
        ));
    }

    #[test]
    fn voter_truncation_preserves_self_votes_and_ranks_eligible_nominators() {
        ext().execute_with(|| {
            let targets = AdmissionData::electable_targets(Default::default(), 0).unwrap();
            let unapproved = AccountId::new([185; 32]);
            bond(unapproved.clone());
            // The highest nominal stake backs only an unapproved target; it
            // must not displace any eligible nominator. Equal valid stakes use
            // AccountId order, independently of storage iteration order.
            for (id, stake) in [(180, 100), (183, 3_000), (182, 5_000), (181, 3_000)] {
                nominate_with_stake(AccountId::new([id; 32]), stake * UNIT, targets.clone());
            }
            nominate_with_stake(AccountId::new([184; 32]), 20_000 * UNIT, vec![unapproved]);
            let voters = AdmissionData::electing_voters(
                DataProviderBounds {
                    count: Some(4.into()),
                    size: None,
                },
                0,
            )
            .unwrap();
            let expected = targets
                .into_iter()
                .chain([AccountId::new([182; 32]), AccountId::new([181; 32])])
                .collect::<Vec<_>>();
            assert_eq!(
                voters.iter().map(|v| v.0.clone()).collect::<Vec<_>>(),
                expected
            );
            System::assert_last_event(
                pallet_validator_admission::Event::<Runtime>::ElectionVotersTruncated {
                    eligible: 6,
                    retained: 4,
                }
                .into(),
            );
            assert_eq!(
                AdmissionData::electing_voters(
                    DataProviderBounds {
                        count: Some(0.into()),
                        size: None
                    },
                    0
                )
                .unwrap(),
                voters[..2]
            );
            System::assert_last_event(
                pallet_validator_admission::Event::<Runtime>::ElectionVotersTruncated {
                    eligible: 6,
                    retained: 2,
                }
                .into(),
            );
        });
    }

    #[test]
    fn voter_byte_bounds_truncate_without_failure() {
        ext().execute_with(|| {
            let targets = AdmissionData::electable_targets(Default::default(), 0).unwrap();
            for id in 180..183 {
                nominate_with_stake(AccountId::new([id; 32]), 100 * UNIT, targets.clone());
            }
            let event_count = System::events().len();
            let full = AdmissionData::electing_voters(Default::default(), 0).unwrap();
            assert_eq!(System::events().len(), event_count); // Nothing dropped.
            for keep in 2..full.len() {
                let size = full[..keep].encoded_size() as u32;
                let bounds = DataProviderBounds {
                    count: None,
                    size: Some(size.into()),
                };
                let selected = AdmissionData::electing_voters(bounds, 0).unwrap();
                assert_eq!(selected, full[..keep]);
                assert!(!bounds.slice_exhausted(&selected));
                System::assert_last_event(
                    pallet_validator_admission::Event::<Runtime>::ElectionVotersTruncated {
                        eligible: 5,
                        retained: keep as u32,
                    }
                    .into(),
                );
            }
            // Even a caller bound below the admission floor retains self-votes.
            assert_eq!(
                AdmissionData::electing_voters(
                    DataProviderBounds {
                        count: None,
                        size: Some(0.into())
                    },
                    0,
                )
                .unwrap(),
                full[..2],
            );
        });
    }

    #[test]
    fn raising_nominator_limit_above_ceiling_preserves_snapshot_and_fallback() {
        use frame_support::traits::Hooks;
        use sp_runtime::traits::BlockNumberProvider;
        ext().execute_with(|| {
            use pallet_staking::ConfigOp::{Noop, Set};
            let config_call = RuntimeCall::Staking(pallet_staking::Call::set_staking_configs {
                min_nominator_bond: Noop,
                min_validator_bond: Noop,
                max_nominator_count: Set(1_101),
                max_validator_count: Noop,
                chill_threshold: Noop,
                min_commission: Noop,
                max_staked_rewards: Noop,
            });
            assert_ok!(ValidatorAdmission::dispatch_as_root(
                foundation(),
                alloc::boxed::Box::new(config_call)
            ));
            let targets = AdmissionData::electable_targets(Default::default(), 0).unwrap();
            for i in 0u32..1_101 {
                let mut bytes = [0; 32];
                bytes[..4].copy_from_slice(&i.to_le_bytes());
                nominate_with_stake(
                    AccountId::new(bytes),
                    (10 + Balance::from(i)) * UNIT,
                    targets.clone(),
                );
            }
            let voters = AdmissionData::electing_voters(Default::default(), 0).unwrap();
            assert_eq!(voters.len(), 1_100);
            assert_eq!(
                voters[..2].iter().map(|v| v.0.clone()).collect::<Vec<_>>(),
                targets
            );
            assert!(voters[2..].windows(2).all(|pair| pair[0].1 >= pair[1].1));
            System::assert_last_event(
                pallet_validator_admission::Event::<Runtime>::ElectionVotersTruncated {
                    eligible: 1_103,
                    retained: 1_100,
                }
                .into(),
            );
            // Exercise the SCALE length-prefix boundary as well as the count bound.
            for keep in [63, 64] {
                let bounds = DataProviderBounds {
                    count: None,
                    size: Some((voters[..keep].encoded_size() as u32).into()),
                };
                assert_eq!(
                    AdmissionData::electing_voters(bounds, 0).unwrap(),
                    voters[..keep]
                );
            }
            assert_ok!(crate::MultiBlockElection::start());
            for _ in 0..2 {
                let now = System::current_block_number() + 1;
                System::set_block_number(now);
                let mut meter = frame_support::weights::WeightMeter::with_limit(
                    RuntimeBlockWeights::get().max_block,
                );
                <crate::MultiBlockElection as Hooks<BlockNumber>>::on_poll(now, &mut meter);
            }
            let round = pallet_election_provider_multi_block::Round::<Runtime>::get();
            assert_eq!(
                pallet_election_provider_multi_block::PagedVoterSnapshot::<Runtime>::get(round, 0)
                    .unwrap()
                    .len(),
                1_100
            );
            // No solution was queued: the actual MBE export must use fallback.
            assert_eq!(crate::MultiBlockElection::elect(0).unwrap().len(), 2);
        });
    }

    #[test]
    fn nominator_bounds_stay_within_their_ceilings() {
        // `MaxNominators` bounds the proof-verification nominator count, so it
        // must cover a full exposure page; the vote list must hold a full quota.
        use frame_support::traits::Get;
        assert!(
            <<Runtime as pallet_babe::Config>::MaxNominators as Get<u32>>::get()
                >= <<Runtime as pallet_staking::Config>::MaxExposurePageSize as Get<u32>>::get()
        );
        assert!(
            <<AdmissionData as ElectionDataProvider>::MaxVotesPerVoter as Get<u32>>::get() >= 16
        );
    }
    #[test]
    fn migration_closes_missing_nominator_limit() {
        ext().execute_with(|| {
            pallet_staking::MaxNominatorsCount::<Runtime>::kill();
            InitializeStakingLimits::on_runtime_upgrade();
            assert_eq!(
                pallet_staking::MaxNominatorsCount::<Runtime>::get(),
                Some(0)
            );
        });
    }
    #[test]
    fn foundation_proposal_reaches_root_and_membership_updates_collective() {
        ext().execute_with(|| {
            let member = pallet_membership::Members::<Runtime, pallet_membership::Instance1>::get()
                [0]
            .clone();
            let call = RuntimeCall::ValidatorAdmission(
                pallet_validator_admission::Call::dispatch_as_root {
                    call: alloc::boxed::Box::new(RuntimeCall::Staking(
                        pallet_staking::Call::set_validator_count { new: 1 },
                    )),
                },
            );
            let len = call.encoded_size() as u32;
            assert_ok!(Foundation::propose(
                RuntimeOrigin::signed(member),
                1,
                alloc::boxed::Box::new(call),
                len
            ));
            assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 1);
            assert_ok!(FoundationMembership::add_member(
                foundation(),
                crate::Address::Id(AccountId::new([88; 32]))
            ));
            assert_eq!(
                pallet_collective::Members::<Runtime, pallet_collective::Instance1>::get().len(),
                2
            );
            assert_noop!(
                ValidatorAdmission::approve(RuntimeOrigin::root(), AccountId::new([77; 32])),
                sp_runtime::DispatchError::BadOrigin
            );
        });
    }
    #[test]
    fn election_rejects_out_of_bound_desired_count() {
        ext().execute_with(|| {
            pallet_staking::ValidatorCount::<Runtime>::put(51);
            assert!(AdmissionElection::elect(0).is_err());
        });
    }
    #[test]
    fn genesis_fallback_and_first_era_populate_exposures() {
        ext().execute_with(|| {
            let initial = Session::validators();
            assert_eq!(initial.len(), 2);
            assert!(
                pallet_staking::ErasStakersOverview::<Runtime>::iter_prefix(0)
                    .next()
                    .is_none()
            );
            for i in 1..=8 {
                System::set_block_number(i * 100);
                pallet_babe::CurrentSlot::<Runtime>::put(sp_consensus_babe::Slot::from(
                    u64::from(i) * 100,
                ));
                pallet_babe::Initialized::<Runtime>::put(
                    None::<sp_consensus_babe::digests::PreDigest>,
                );
                Session::rotate_session();
            }
            assert_eq!(Session::validators().len(), 2);
            let era = pallet_staking::CurrentEra::<Runtime>::get().unwrap();
            assert!(
                pallet_staking::ErasStakersOverview::<Runtime>::iter_prefix(era)
                    .next()
                    .is_some()
            );
        });
    }
    fn rotate() {
        let i = pallet_session::CurrentIndex::<Runtime>::get() + 1;
        System::set_block_number(i * 100);
        pallet_babe::CurrentSlot::<Runtime>::put(sp_consensus_babe::Slot::from(u64::from(i) * 100));
        pallet_babe::Initialized::<Runtime>::put(None::<sp_consensus_babe::digests::PreDigest>);
        Session::rotate_session();
        <crate::Grandpa as frame_support::traits::Hooks<BlockNumber>>::on_finalize(i * 100);
    }
    // Only the production staking provider filters elections through admission.
    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn both_consensus_sets_follow_removal_and_history_is_retained() {
        ext().execute_with(|| {
            let initial = Session::validators();
            let removed = initial[0].clone();
            let survivor = initial[1].clone();
            let survivor_keys = pallet_session::NextKeys::<Runtime>::get(&survivor).unwrap();
            assert_ok!(ValidatorAdmission::remove(foundation(), removed.clone()));
            for _ in 0..8 {
                rotate();
            }
            assert_eq!(Session::validators(), vec![survivor]);
            assert_eq!(
                pallet_babe::Authorities::<Runtime>::get().to_vec(),
                vec![(survivor_keys.0.babe.clone(), 1)]
            );
            assert_eq!(
                pallet_grandpa::Authorities::<Runtime>::get().to_vec(),
                vec![(survivor_keys.0.grandpa.clone(), 1)]
            );
            assert!(pallet_grandpa::CurrentSetId::<Runtime>::get() > 0);
            assert!(pallet_session::historical::StoredRange::<Runtime>::get().is_some());
            assert!(pallet_grandpa::SetIdSession::<Runtime>::iter()
                .next()
                .is_some());
            // Failure must keep this set, even if all approval is then removed.
            pallet_validator_admission::Approved::<Runtime>::kill();
            let era = pallet_staking::CurrentEra::<Runtime>::get();
            for _ in 0..8 {
                rotate();
            }
            assert_eq!(Session::validators().len(), 1);
            assert_eq!(pallet_staking::CurrentEra::<Runtime>::get(), era);
            assert_eq!(pallet_grandpa::Authorities::<Runtime>::get().len(), 1);
        });
    }
    #[test]
    fn foundation_rotates_faucet_key_without_changing_fuse_or_budget() {
        ext().execute_with(|| {
            let who = Session::validators()[0].clone();
            let operator = AccountId::new([97; 32]);
            assert_noop!(
                crate::FaucetOps::set_authority(RuntimeOrigin::root(), Some(operator.clone())),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                crate::FaucetOps::set_authority(
                    RuntimeOrigin::signed(who.clone()),
                    Some(operator.clone())
                ),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_ok!(crate::FaucetOps::set_authority(
                foundation(),
                Some(operator.clone())
            ));
            assert_noop!(
                crate::FaucetOps::mint(RuntimeOrigin::signed(who.clone()), who.clone(), UNIT),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_ok!(crate::FaucetOps::mint(
                RuntimeOrigin::signed(operator.clone()),
                who.clone(),
                UNIT
            ));
            assert_ok!(crate::FaucetOps::set_authority(foundation(), None));
            assert_noop!(
                crate::FaucetOps::mint(RuntimeOrigin::signed(operator), who.clone(), UNIT),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_noop!(
                crate::FaucetOps::mint(RuntimeOrigin::root(), who.clone(), UNIT),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_ok!(crate::FaucetOps::mint(foundation(), who.clone(), UNIT));
            let budget = pallet_emission_controller::FaucetBudget::<Runtime>::get();
            let issued = pallet_emission_controller::FaucetIssued::<Runtime>::get();
            assert_ok!(crate::FaucetOps::disable(foundation()));
            assert_ok!(crate::FaucetOps::set_authority(
                foundation(),
                Some(who.clone())
            ));
            assert_noop!(
                crate::FaucetOps::mint(RuntimeOrigin::signed(who.clone()), who, UNIT),
                pallet_faucet_ops::Error::<Runtime>::Disabled
            );
            assert_eq!(
                pallet_emission_controller::FaucetBudget::<Runtime>::get(),
                budget
            );
            assert_eq!(
                pallet_emission_controller::FaucetIssued::<Runtime>::get(),
                issued
            );
        });
    }
    #[test]
    fn bonded_candidate_with_owner_proof_joins_both_consensus_sets() {
        ext().execute_with(|| {
            use frame_support::traits::Currency;
            use quip_crypto_primitives::substrate::{ed25519_fndsa512, sr25519_fndsa512};
            use sp_core::proof_of_possession::ProofOfPossessionGenerator;
            use sp_core::Pair;
            let who = AccountId::new([91; 32]);
            let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&who, 3000 * UNIT);
            let mut babe = sr25519_fndsa512::Pair::from_string("//Charlie", None).unwrap();
            let mut grandpa = ed25519_fndsa512::Pair::from_string("//Charlie", None).unwrap();
            let keys: crate::BoxedSessionKeys = crate::SessionKeys {
                babe: babe.public().into(),
                grandpa: grandpa.public().into(),
            }
            .into();
            assert!(
                Session::set_keys(RuntimeOrigin::signed(who.clone()), keys.clone(), vec![])
                    .is_err()
            );
            let proof = (
                babe.generate_proof_of_possession(&who.encode()),
                grandpa.generate_proof_of_possession(&who.encode()),
            )
                .encode();
            let wrong_owner = AccountId::new([90; 32]);
            let wrong_owner_proof = (
                babe.generate_proof_of_possession(&wrong_owner.encode()),
                grandpa.generate_proof_of_possession(&wrong_owner.encode()),
            )
                .encode();
            let onboarding = |proof| {
                RuntimeCall::Utility(pallet_utility::Call::batch_all {
                    calls: vec![
                        RuntimeCall::Staking(pallet_staking::Call::bond {
                            value: 2_000 * UNIT,
                            payee: pallet_staking::RewardDestination::Staked,
                        }),
                        RuntimeCall::Session(pallet_session::Call::set_keys {
                            keys: keys.clone(),
                            proof,
                        }),
                        RuntimeCall::Staking(pallet_staking::Call::validate {
                            prefs: Default::default(),
                        }),
                    ],
                })
            };
            let before = Balances::free_balance(&who);
            for invalid_proof in [vec![], wrong_owner_proof] {
                assert!(onboarding(invalid_proof)
                    .dispatch(RuntimeOrigin::signed(who.clone()))
                    .is_err());
                assert_eq!(Balances::free_balance(&who), before);
                assert!(!pallet_staking::Bonded::<Runtime>::contains_key(&who));
                assert!(!pallet_session::NextKeys::<Runtime>::contains_key(&who));
                assert!(!pallet_staking::Validators::<Runtime>::contains_key(&who));
            }
            assert_ok!(onboarding(proof).dispatch(RuntimeOrigin::signed(who.clone())));
            assert!(!AdmissionData::electable_targets(Default::default(), 0)
                .unwrap()
                .contains(&who));
            assert_ok!(ValidatorAdmission::approve(foundation(), who.clone()));
            assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 2);
            assert_eq!(AdmissionElection::elect(0).unwrap().len(), 2);
            assert_ok!(ValidatorAdmission::dispatch_as_root(
                foundation(),
                alloc::boxed::Box::new(RuntimeCall::Staking(
                    pallet_staking::Call::set_validator_count { new: 3 }
                ))
            ));
            let elected = AdmissionElection::elect(0).unwrap();
            assert_eq!(elected.len(), 3);
            assert!(elected.iter().any(|(id, _)| id == &who));
            for _ in 0..8 {
                rotate();
            }
            assert!(Session::validators().contains(&who));
            assert!(pallet_babe::Authorities::<Runtime>::get()
                .iter()
                .any(|(key, _)| *key == keys.0.babe));
            assert!(pallet_grandpa::Authorities::<Runtime>::get()
                .iter()
                .any(|(key, _)| *key == keys.0.grandpa));
        });
    }
    #[test]
    fn foundation_requires_supermajority() {
        ext().execute_with(|| {
            let minority =
                pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(
                    1, 3,
                )
                .into();
            let majority =
                pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(
                    2, 3,
                )
                .into();
            assert_noop!(
                ValidatorAdmission::approve(minority, AccountId::new([87; 32])),
                sp_runtime::DispatchError::BadOrigin
            );
            assert_ok!(ValidatorAdmission::approve(
                majority,
                AccountId::new([87; 32])
            ));
        });
    }
    #[test]
    fn approved_intent_without_keys_cannot_be_elected() {
        ext().execute_with(|| {
            let who = AccountId::new([92; 32]);
            bond(who.clone());
            assert_ok!(ValidatorAdmission::approve(foundation(), who.clone()));
            assert!(!AdmissionData::electable_targets(Default::default(), 0)
                .unwrap()
                .contains(&who));
            assert!(AdmissionData::electing_voters(Default::default(), 0)
                .unwrap()
                .iter()
                .all(|v| v.0 != who));
        });
    }
    // The SDK benchmark provider deliberately uses its own election/floor path.
    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn minimum_floor_retains_old_set_on_over_aggressive_removal() {
        ext().execute_with(|| {
            let before = Session::validators();
            pallet_staking::MinimumValidatorCount::<Runtime>::put(2);
            assert_ok!(ValidatorAdmission::remove(foundation(), before[0].clone()));
            for _ in 0..8 {
                rotate();
            }
            assert_eq!(Session::validators(), before);
            assert_eq!(pallet_grandpa::Authorities::<Runtime>::get().len(), 2);
        });
    }
    #[test]
    fn zero_payout_does_not_mint() {
        ext().execute_with(|| {
            use pallet_staking::EraPayout;
            let before = Balances::total_issuance();
            assert_eq!(
                <() as EraPayout<Balance>>::era_payout(100 * UNIT, before, 60_000),
                (0, 0)
            );
            assert_eq!(Balances::total_issuance(), before);
        });
    }
}

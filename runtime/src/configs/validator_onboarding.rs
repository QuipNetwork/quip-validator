//! FoundationOnly governance and bounded, admission-first elections.
use super::*;
use crate::{Foundation, Session, Staking};
use alloc::{vec, vec::Vec};
use frame_election_provider_support::{
    bounds::{DataProviderBounds, ElectionBounds, ElectionBoundsBuilder},
    data_provider, onchain, BoundedSupportsOf, ElectionDataProvider, ElectionProvider, PageIndex,
    SequentialPhragmen, VoterOf,
};
use frame_support::traits::{EitherOfDiverse, Nothing};

pub type FoundationOrigin =
    pallet_collective::EnsureProportionAtLeast<AccountId, pallet_collective::Instance1, 2, 3>;
pub type FoundationOrRoot = EitherOfDiverse<frame_system::EnsureRoot<AccountId>, FoundationOrigin>;
parameter_types! {
    pub const SessionsPerEra: u32 = 6;
    pub const BondingDuration: u32 = 28;
    pub const SetIdSessionEntries: u64 = 28 * 6;
    pub const FoundationMotionDuration: BlockNumber = 24 * super::super::HOURS;
    pub FoundationProposalWeight: Weight = RuntimeBlockWeights::get().max_block / 2;
    pub ElectionBoundsConfig: ElectionBounds = ElectionBoundsBuilder::default()
        .voters_count(32.into()).targets_count(32.into()).build();
}
impl pallet_collective::Config<pallet_collective::Instance1> for Runtime {
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeEvent = RuntimeEvent;
    type Proposal = RuntimeCall;
    type MotionDuration = FoundationMotionDuration;
    type MaxProposals = ConstU32<32>;
    type MaxMembers = ConstU32<32>;
    type DefaultVote = pallet_collective::MoreThanMajorityThenPrimeDefaultVote;
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
    type RemoveOrigin = FoundationOrRoot;
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
    type MaxAuthorities = ConstU32<32>;
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
    type EraPayout = ();
    type MaxExposurePageSize = ConstU32<32>;
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
    type MaxValidatorSet = ConstU32<32>;
    #[cfg(not(feature = "runtime-benchmarks"))]
    type NominationsQuota = pallet_staking::FixedNominationsQuota<1>;
    // The SDK nominate benchmark uses Linear<1, MaxNominations>; a singleton
    // range panics in median-slopes analysis. Give benchmark builds two points.
    #[cfg(feature = "runtime-benchmarks")]
    type NominationsQuota = pallet_staking::FixedNominationsQuota<2>;
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
/// Staking remains the source of candidacy and voting stake; no nominators are
/// admitted in this phase. TargetList itself stays invariant-compatible.
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
    type MaxVotesPerVoter = ConstU32<1>;
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
        let voters: Vec<_> = Self::candidates()
            .into_iter()
            .map(|who| {
                let stake = Staking::weight_of(&who);
                (
                    who.clone(),
                    stake,
                    vec![who].try_into().expect("one self vote fits"),
                )
            })
            .filter(|(_, stake, _)| *stake > 0)
            .collect();
        if bounds.slice_exhausted(&voters) {
            return Err("approved voter snapshot exceeds bounds");
        }
        Ok(voters)
    }
    fn desired_targets() -> data_provider::Result<u32> {
        let desired = <Staking as ElectionDataProvider>::desired_targets()?;
        if desired > 32 {
            return Err("requested validator count exceeds authority bound");
        }
        Ok(desired)
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
    type MaxBackersPerWinner = ConstU32<32>;
    type MaxWinnersPerPage = ConstU32<32>;
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
    type MaxWinnersPerPage = ConstU32<32>;
    type MaxBackersPerWinner = ConstU32<32>;
    type MaxBackersPerWinnerFinal = ConstU32<32>;
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
mod tests {
    use super::*;
    use crate::{FoundationMembership, RuntimeGenesisConfig, ValidatorAdmission};
    use frame_support::{assert_noop, assert_ok, traits::OnRuntimeUpgrade};
    use sp_runtime::{traits::Dispatchable, BuildStorage};

    fn ext() -> sp_io::TestExternalities {
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
    fn bond(who: AccountId) {
        use frame_support::traits::Currency;
        let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&who, 1000 * UNIT);
        assert_ok!(Staking::bond(
            RuntimeOrigin::signed(who.clone()),
            100 * UNIT,
            pallet_staking::RewardDestination::Staked
        ));
        assert_ok!(Staking::validate(
            RuntimeOrigin::signed(who),
            Default::default()
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
    fn nominate_is_closed_directly_and_through_utility() {
        ext().execute_with(|| {
            let who = AccountId::new([99; 32]);
            bond(who.clone());
            let targets = vec![crate::Address::Id(Session::validators()[0].clone())];
            assert_noop!(
                Staking::nominate(RuntimeOrigin::signed(who.clone()), targets.clone()),
                pallet_staking::Error::<Runtime>::TooManyNominators
            );
            let call = RuntimeCall::Utility(pallet_utility::Call::batch_all {
                calls: vec![RuntimeCall::Staking(pallet_staking::Call::nominate {
                    targets,
                })],
            });
            assert!(call.dispatch(RuntimeOrigin::signed(who)).is_err());
            assert_eq!(pallet_staking::Nominators::<Runtime>::count(), 0);
        });
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
            pallet_staking::ValidatorCount::<Runtime>::put(33);
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
            use quip_crypto_primitives::substrate::{ed25519_fndsa512, sr25519_fndsa512};
            use sp_core::proof_of_possession::ProofOfPossessionGenerator;
            use sp_core::Pair;
            let who = AccountId::new([91; 32]);
            bond(who.clone());
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
            assert_ok!(Session::set_keys(
                RuntimeOrigin::signed(who.clone()),
                keys.clone(),
                proof
            ));
            assert!(!AdmissionData::electable_targets(Default::default(), 0)
                .unwrap()
                .contains(&who));
            assert_ok!(ValidatorAdmission::approve(foundation(), who.clone()));
            assert_ok!(Staking::set_validator_count(RuntimeOrigin::root(), 3));
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

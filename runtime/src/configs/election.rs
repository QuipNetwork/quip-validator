//! Single-page multi-block elections for classic staking.
use super::validator_onboarding::{AdmissionData, OnChainConfig};
use super::*;
use crate::{MultiBlockElection, MultiBlockElectionSigned, MultiBlockElectionVerifier, Staking};
use alloc::{vec, vec::Vec};
use frame_election_provider_support::{
    onchain, BoundedSupportsOf, ElectionProvider, PageIndex, SequentialPhragmen,
};
use pallet_election_provider_multi_block::{self as multi_block, unsigned::miner};
use sp_runtime::{transaction_validity::TransactionPriority, PerU16};

parameter_types! {
    pub const Pages: u32 = 1;
    /// Zero-based active-session offset within the planned staking era.
    pub const PlanningSession: u32 = 3;
    pub const MinimumElectionSlack: BlockNumber = 1_200;
    // Admission can contain 64 candidates, even though only 50 may win.
    pub const MaxNominators: u32 = 1024;
    pub const VoterSnapshotPerBlock: u32 = 1_100;
    pub const TargetSnapshotPerBlock: u32 = 64;
    pub const MaxWinnersPerPage: u32 = 50;
    pub const MaxBackersPerWinner: u32 = 64;
    pub const MaxBackersPerWinnerFinal: u32 = 64;
    // Signed submissions and their validation phase are disabled together.
    pub const SignedPhase: BlockNumber = 0;
    pub const SignedValidationPhase: BlockNumber = 0;
    pub const UnsignedPhase: BlockNumber = 300;
    pub const OffchainRepeat: BlockNumber = 20;
    pub const MinerTxPriority: TransactionPriority = TransactionPriority::MAX / 2;
    pub const MinerMaxLength: u32 = 128 * 1024;
    pub const ZeroRatio: Perbill = Perbill::zero();
}

// Keep the compile-time snapshot ceiling in sync with genesis. Admission can
// have more candidates than winners; reserving only 50 self-votes is not enough.
const _: () = assert!(
    TargetSnapshotPerBlock::get()
        >= <Runtime as pallet_validator_admission::Config>::MaxAuthorities::get()
);
const _: () = assert!(
    VoterSnapshotPerBlock::get()
        >= TargetSnapshotPerBlock::get().saturating_add(MaxNominators::get())
);

frame_election_provider_support::generate_solution_type!(
    #[compact]
    pub struct NposCompactSolution16::<
        VoterIndex = u32,
        TargetIndex = u16,
        Accuracy = PerU16,
        MaxVoters = VoterSnapshotPerBlock
    >(16)
);

impl multi_block::Config for Runtime {
    type Pages = Pages;
    type SignedPhase = SignedPhase;
    type SignedValidationPhase = SignedValidationPhase;
    type UnsignedPhase = UnsignedPhase;
    type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
    type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
    type DataProvider = AdmissionData;
    type MinerConfig = Self;
    type Fallback = onchain::OnChainExecution<OnChainConfig>;
    type Verifier = MultiBlockElectionVerifier;
    type Signed = MultiBlockElectionSigned;
    type AdminOrigin = validator_onboarding::FoundationOrRoot;
    type ManagerOrigin = validator_onboarding::FoundationOrRoot;
    type AreWeDone = multi_block::ProceedRegardlessOf<Self>;
    type OnRoundRotation = multi_block::CleanRound<Self>;
    type WeightInfo = multi_block::weights::polkadot::MultiBlockWeightInfo<Self>;
}

impl multi_block::verifier::Config for Runtime {
    type MaxWinnersPerPage = MaxWinnersPerPage;
    type MaxBackersPerWinner = MaxBackersPerWinner;
    type MaxBackersPerWinnerFinal = MaxBackersPerWinnerFinal;
    type SolutionDataProvider = MultiBlockElectionSigned;
    type WeightInfo = multi_block::weights::polkadot::MultiBlockVerifierWeightInfo<Self>;
}

impl multi_block::unsigned::Config for Runtime {
    type MinerPages = Pages;
    type OffchainRepeat = OffchainRepeat;
    type OffchainStorage = ConstBool<true>;
    type MinerTxPriority = MinerTxPriority;
    type OffchainSolver = SequentialPhragmen<AccountId, Perbill>;
    type WeightInfo = multi_block::weights::polkadot::MultiBlockUnsignedWeightInfo<Self>;
}

impl multi_block::signed::Config for Runtime {
    type Currency = Balances;
    type DepositBase = ConstU128<0>;
    type DepositPerPage = ConstU128<0>;
    type InvulnerableDeposit = ConstU128<0>;
    type RewardBase = ConstU128<0>;
    type MaxSubmissions = ConstU32<0>;
    type BailoutGraceRatio = ZeroRatio;
    type EjectGraceRatio = ZeroRatio;
    type EstimateCallFee = crate::TransactionPayment;
    type WeightInfo = multi_block::weights::polkadot::MultiBlockSignedWeightInfo<Self>;
}

impl miner::MinerConfig for Runtime {
    type AccountId = AccountId;
    type Hash = Hash;
    type Solution = NposCompactSolution16;
    type Solver = SequentialPhragmen<AccountId, Perbill>;
    type MaxLength = MinerMaxLength;
    type MaxVotesPerVoter = ConstU32<16>;
    type Pages = Pages;
    type VoterSnapshotPerBlock = VoterSnapshotPerBlock;
    type TargetSnapshotPerBlock = TargetSnapshotPerBlock;
    type MaxWinnersPerPage = MaxWinnersPerPage;
    type MaxBackersPerWinner = MaxBackersPerWinner;
    type MaxBackersPerWinnerFinal = MaxBackersPerWinnerFinal;
}

/// Classic staking requires its own data-provider associated type. The actual
/// MBE snapshot and fallback retain the admission-first provider.
pub struct AdmissionMultiBlockElection;
impl ElectionProvider for AdmissionMultiBlockElection {
    type AccountId = AccountId;
    type BlockNumber = BlockNumber;
    type Error = <MultiBlockElection as ElectionProvider>::Error;
    type DataProvider = Staking;
    type Pages = Pages;
    type MaxWinnersPerPage = MaxWinnersPerPage;
    type MaxBackersPerWinner = MaxBackersPerWinner;
    type MaxBackersPerWinnerFinal = MaxBackersPerWinnerFinal;

    fn elect(page: PageIndex) -> Result<BoundedSupportsOf<Self>, Self::Error> {
        // Session rotations can occur in consecutive blocks after missed BABE
        // slots. Do not consume/reset a round before its two snapshot polls and
        // the first unsigned block have elapsed. Normal rounds finish long
        // before staking asks; this protects forced eras and delayed snapshots.
        System::register_extra_weight_unchecked(
            <Runtime as frame_system::Config>::DbWeight::get().reads(1),
            DispatchClass::Mandatory,
        );
        match multi_block::CurrentPhase::<Runtime>::get() {
            multi_block::Phase::Snapshot(_) => {
                return Err(multi_block::ElectionError::Other("snapshot not ready"))
            }
            multi_block::Phase::Unsigned(left)
                if left >= UnsignedPhase::get().saturating_sub(1) =>
            {
                return Err(multi_block::ElectionError::Other(
                    "unsigned phase just opened",
                ));
            }
            _ => {}
        }
        // Classic staking does not reserve the MBE status/export weight (the
        // async staking consumer does). Charge export/cleanup here; the on-chain
        // fallback separately registers its solver weight.
        use multi_block::WeightInfo as _;
        System::register_extra_weight_unchecked(
            <Runtime as multi_block::Config>::WeightInfo::export_terminal(),
            DispatchClass::Mandatory,
        );
        MultiBlockElection::elect(page)
    }
    fn start() -> Result<(), Self::Error> {
        MultiBlockElection::start()
    }
    fn status() -> Result<Option<Weight>, ()> {
        MultiBlockElection::status()
    }
    fn duration() -> BlockNumber {
        MultiBlockElection::duration()
    }
}

type HistoricalStaking = pallet_session::historical::NoteHistoricalRoot<Runtime, Staking>;

/// Start rounds once per session rotation, after staking has attempted to plan
/// the next session. This ordering also handles ForceNew without starting and
/// consuming a new snapshot in the same block.
pub struct ElectionScheduler;
impl ElectionScheduler {
    /// Constants only: also evaluated at compile time, including Wasm builds.
    pub const fn integrity_test() {
        assert!(Pages::get() == 1, "classic staking only supports one page");
        assert!(SignedPhase::get() == 0 && SignedValidationPhase::get() == 0);
        assert!(UnsignedPhase::get() >= 2);
        // Reserve self-votes for every admitted target in addition to the
        // genesis nominator cap. A higher council-set cap truncates safely.
        assert!(
            VoterSnapshotPerBlock::get()
                >= TargetSnapshotPerBlock::get().saturating_add(MaxNominators::get())
        );
        let round_blocks = Pages::get()
            .saturating_add(1)
            .saturating_add(SignedPhase::get())
            .saturating_add(SignedValidationPhase::get())
            .saturating_add(UnsignedPhase::get());
        Self::assert_planning_window(round_blocks);
    }

    const fn assert_planning_window(round_blocks: u32) {
        // rotate_session enters i, then calls new_session(i+1). Staking elects
        // at i+1 = era_start+6: the deadline is active session offset 5, not 6.
        let deadline = validator_onboarding::SessionsPerEra::get().saturating_sub(1);
        assert!(
            PlanningSession::get() < deadline,
            "planning must precede election"
        );
        let window = (deadline.saturating_sub(PlanningSession::get()) as u64)
            .saturating_mul(EpochDuration::get());
        assert!(
            (round_blocks as u64).saturating_add(MinimumElectionSlack::get() as u64) <= window,
            "election phases do not leave two hours of planning slack"
        );
    }

    fn maybe_start() {
        // Bounded storage work, once per session; start itself reads/writes the
        // MBE phase. Account for the full path even on early returns.
        System::register_extra_weight_unchecked(
            <Runtime as frame_system::Config>::DbWeight::get().reads_writes(7, 1),
            DispatchClass::Mandatory,
        );
        let forcing = pallet_staking::ForceEra::<Runtime>::get();
        if forcing == pallet_staking::Forcing::ForceNone {
            // Pausing eras invalidates an in-flight snapshot/queued solution.
            // Resuming must collect current admission data rather than export
            // a round that could predate a validator's removal by weeks.
            if MultiBlockElection::status().is_ok() {
                use multi_block::WeightInfo as _;
                // Direct calls do not automatically charge dispatch weight.
                System::register_extra_weight_unchecked(
                    <Runtime as multi_block::Config>::WeightInfo::export_terminal(),
                    DispatchClass::Mandatory,
                );
                let _ = MultiBlockElection::manage(
                    frame_system::RawOrigin::Root.into(),
                    multi_block::ManagerOperation::ForceRotateRound,
                );
            }
            return;
        }
        // A failed genesis election may leave CurrentEra absent. Retry from the
        // initial session origin rather than requiring an era to exist first.
        let start = pallet_staking::CurrentEra::<Runtime>::get()
            .and_then(pallet_staking::ErasStartSessionIndex::<Runtime>::get)
            .unwrap_or(0);
        let active_session = pallet_session::CurrentIndex::<Runtime>::get();
        let due = active_session.saturating_sub(start) >= PlanningSession::get()
            || matches!(
                forcing,
                pallet_staking::Forcing::ForceNew | pallet_staking::Forcing::ForceAlways
            );
        // A failed export rotates MBE to Off while staking retains its era.
        // >= (not ==) lets the next attempt recover instead of freezing forever.
        if due && MultiBlockElection::status().is_err() {
            let _ = MultiBlockElection::start();
        }
    }
}

const _: () = ElectionScheduler::integrity_test();

impl pallet_session::SessionManager<AccountId> for ElectionScheduler {
    fn new_session(new_index: sp_staking::SessionIndex) -> Option<Vec<AccountId>> {
        let validators =
            <HistoricalStaking as pallet_session::SessionManager<AccountId>>::new_session(
                new_index,
            );
        Self::maybe_start();
        validators
    }
    fn new_session_genesis(new_index: sp_staking::SessionIndex) -> Option<Vec<AccountId>> {
        <HistoricalStaking as pallet_session::SessionManager<AccountId>>::new_session_genesis(
            new_index,
        )
    }
    fn start_session(start_index: sp_staking::SessionIndex) {
        <HistoricalStaking as pallet_session::SessionManager<AccountId>>::start_session(
            start_index,
        );
    }
    fn end_session(end_index: sp_staking::SessionIndex) {
        <HistoricalStaking as pallet_session::SessionManager<AccountId>>::end_session(end_index);
    }
}

impl<LocalCall> frame_system::offchain::CreateTransactionBase<LocalCall> for Runtime
where
    RuntimeCall: From<LocalCall>,
{
    type RuntimeCall = RuntimeCall;
    type Extrinsic = crate::UncheckedExtrinsic;
}

impl<LocalCall> frame_system::offchain::CreateBare<LocalCall> for Runtime
where
    RuntimeCall: From<LocalCall>,
{
    fn create_bare(call: Self::RuntimeCall) -> Self::Extrinsic {
        sp_runtime::generic::UncheckedExtrinsic::new_bare(call).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MultiBlockElection, MultiBlockElectionUnsigned};
    use frame_election_provider_support::ElectionProvider;
    use frame_support::{assert_noop, assert_ok, traits::Hooks, weights::WeightMeter};
    use multi_block::verifier::Verifier;
    use pallet_session::Validators;
    use sp_runtime::traits::BlockNumberProvider;

    fn poll() {
        let now = System::current_block_number().saturating_add(1);
        System::set_block_number(now);
        let mut meter = WeightMeter::with_limit(RuntimeBlockWeights::get().max_block);
        <MultiBlockElection as Hooks<BlockNumber>>::on_poll(now, &mut meter);
    }

    fn start_and_snapshot() {
        assert_ok!(MultiBlockElection::start());
        // Pages=1 needs a snapshot block and a transition into unsigned.
        poll();
        poll();
        assert!(matches!(
            multi_block::CurrentPhase::<Runtime>::get(),
            multi_block::Phase::Unsigned(_)
        ));
    }

    #[test]
    fn election_administration_requires_council_supermajority() {
        validator_onboarding::tests::ext().execute_with(|| {
            let council = |votes| -> RuntimeOrigin {
                pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(
                    votes, 6,
                )
                .into()
            };
            for origin in [RuntimeOrigin::signed(AccountId::new([99; 32])), council(3)] {
                assert_noop!(
                    MultiBlockElection::admin(
                        origin.clone(),
                        multi_block::AdminOperation::SetMinUntrustedScore(Default::default())
                    ),
                    sp_runtime::DispatchError::BadOrigin
                );
                assert_noop!(
                    MultiBlockElectionSigned::set_invulnerables(origin.clone(), vec![]),
                    sp_runtime::DispatchError::BadOrigin
                );
                assert_noop!(
                    MultiBlockElection::manage(
                        origin,
                        multi_block::ManagerOperation::ForceRotateRound
                    ),
                    sp_runtime::DispatchError::BadOrigin
                );
            }
            assert_ok!(MultiBlockElection::admin(
                council(4),
                multi_block::AdminOperation::SetMinUntrustedScore(Default::default())
            ));
            assert_ok!(MultiBlockElectionSigned::set_invulnerables(
                council(4),
                vec![]
            ));
            assert_ok!(MultiBlockElection::manage(
                council(4),
                multi_block::ManagerOperation::ForceRotateRound
            ));
        });
    }

    #[test]
    fn signed_submissions_cannot_register_even_in_a_forced_signed_phase() {
        validator_onboarding::tests::ext().execute_with(|| {
            multi_block::CurrentPhase::<Runtime>::put(multi_block::Phase::Signed(10));
            let who = Validators::<Runtime>::get()[0].clone();
            assert_noop!(
                MultiBlockElectionSigned::register(RuntimeOrigin::signed(who), Default::default()),
                multi_block::signed::Error::<Runtime>::QueueFull
            );
        });
    }

    #[test]
    fn queued_solution_is_exported_and_round_rotates() {
        validator_onboarding::tests::ext().execute_with(|| {
            start_and_snapshot();
            let solution = miner::OffchainWorkerMiner::<Runtime>::mine_solution(1, true)
                .expect("snapshot can be mined");
            assert_ok!(MultiBlockElectionUnsigned::submit_unsigned(
                RuntimeOrigin::none(),
                alloc::boxed::Box::new(solution)
            ));
            let queued = MultiBlockElectionVerifier::get_queued_solution_page(0)
                .expect("unsigned solution was verified");
            let round = multi_block::Round::<Runtime>::get();
            assert_eq!(MultiBlockElection::elect(0).unwrap(), queued);
            assert_eq!(multi_block::Round::<Runtime>::get(), round + 1);
            assert!(MultiBlockElection::status().is_err());
        });
    }

    #[test]
    fn offchain_worker_submits_a_valid_bare_runtime_extrinsic() {
        use sp_core::offchain::{
            testing::{TestOffchainExt, TestTransactionPoolExt},
            OffchainDbExt, OffchainWorkerExt, TransactionPoolExt,
        };
        use sp_runtime::traits::ExtrinsicLike;
        let mut ext = validator_onboarding::tests::ext();
        let (offchain, _) = TestOffchainExt::new();
        let (pool, state) = TestTransactionPoolExt::new();
        ext.register_extension(OffchainDbExt::new(offchain.clone()));
        ext.register_extension(OffchainWorkerExt::new(offchain));
        ext.register_extension(TransactionPoolExt::new(pool));
        ext.execute_with(|| {
            start_and_snapshot();
            <MultiBlockElectionUnsigned as Hooks<BlockNumber>>::offchain_worker(
                System::current_block_number(),
            );
            let transactions = &state.read().transactions;
            assert_eq!(transactions.len(), 1);
            let extrinsic = crate::UncheckedExtrinsic::decode(&mut &transactions[0][..])
                .expect("miner uses the runtime extrinsic encoding");
            assert!(extrinsic.is_bare());
            let RuntimeCall::MultiBlockElectionUnsigned(call) = extrinsic.0.function else {
                panic!("expected an unsigned election submission");
            };
            assert_ok!(<MultiBlockElectionUnsigned as frame_support::unsigned::ValidateUnsigned>::pre_dispatch(&call));
            let multi_block::unsigned::Call::submit_unsigned { paged_solution } = call;
            assert_ok!(MultiBlockElectionUnsigned::submit_unsigned(RuntimeOrigin::none(), paged_solution));
            assert!(MultiBlockElectionVerifier::get_queued_solution_page(0).is_some());
        });
    }

    #[test]
    fn unapproved_intents_cannot_break_multi_block_snapshot() {
        use frame_support::traits::Currency;
        validator_onboarding::tests::ext().execute_with(|| {
            for i in 80..150 {
                let who = AccountId::new([i; 32]);
                let _ = Balances::make_free_balance_be(&who, 3_000 * UNIT);
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
            assert!(pallet_staking::Validators::<Runtime>::count() > TargetSnapshotPerBlock::get());
            start_and_snapshot();
            assert_eq!(
                multi_block::PagedTargetSnapshot::<Runtime>::get(
                    multi_block::Round::<Runtime>::get(),
                    0
                )
                .unwrap()
                .len(),
                2
            );
            assert_eq!(MultiBlockElection::elect(0).unwrap().len(), 2);
        });
    }

    #[test]
    fn removal_updates_snapshot_and_unsigned_winner_count() {
        validator_onboarding::tests::ext().execute_with(|| {
            let removed = Validators::<Runtime>::get()[0].clone();
            start_and_snapshot();
            assert!(multi_block::PagedTargetSnapshot::<Runtime>::get(
                multi_block::Round::<Runtime>::get(),
                0
            )
            .unwrap()
            .contains(&removed));
            assert_eq!(MultiBlockElection::elect(0).unwrap().len(), 2);
            let council =
                pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(
                    1, 1,
                )
                .into();
            assert_ok!(crate::ValidatorAdmission::remove(council, removed.clone()));
            assert_eq!(pallet_staking::ValidatorCount::<Runtime>::get(), 2);
            start_and_snapshot();
            assert!(!multi_block::PagedTargetSnapshot::<Runtime>::get(
                multi_block::Round::<Runtime>::get(),
                0
            )
            .unwrap()
            .contains(&removed));
            assert_eq!(
                multi_block::DesiredTargets::<Runtime>::get(multi_block::Round::<Runtime>::get()),
                Some(1)
            );
            let solution = miner::OffchainWorkerMiner::<Runtime>::mine_solution(1, true).unwrap();
            assert_ok!(MultiBlockElectionUnsigned::submit_unsigned(
                RuntimeOrigin::none(),
                alloc::boxed::Box::new(solution)
            ));
            assert!(MultiBlockElectionVerifier::get_queued_solution_page(0).is_some());
            poll(); // the first unsigned block must elapse before classic staking exports
            let result = AdmissionMultiBlockElection::elect(0).unwrap();
            assert_eq!(result.len(), 1);
            assert!(result.iter().all(|(who, _)| *who != removed));
        });
    }

    #[test]
    fn missing_solution_uses_onchain_fallback() {
        validator_onboarding::tests::ext().execute_with(|| {
            let expected = onchain::OnChainExecution::<OnChainConfig>::elect(0).unwrap();
            start_and_snapshot();
            for _ in 0..UnsignedPhase::get() {
                poll();
            }
            assert!(MultiBlockElectionVerifier::get_queued_solution_page(0).is_none());
            assert_eq!(
                multi_block::CurrentPhase::<Runtime>::get(),
                multi_block::Phase::Done
            );
            assert!(!expected.is_empty());
            assert_eq!(MultiBlockElection::elect(0).unwrap(), expected);
        });
    }
    #[test]
    fn scheduler_integrity_test() {
        ElectionScheduler::integrity_test();
        assert_eq!(PlanningSession::get(), 3);
        assert_eq!(validator_onboarding::SessionsPerEra::get() - 1, 5);
        assert_eq!(EpochDuration::get(), 2_400);
    }

    #[test]
    #[should_panic(expected = "election phases do not leave two hours of planning slack")]
    fn scheduler_integrity_rejects_an_overlong_round() {
        ElectionScheduler::assert_planning_window(4_800);
    }

    #[cfg(not(feature = "runtime-benchmarks"))]
    mod scheduling {
        use super::*;
        use pallet_staking::{CurrentEra, ForceEra, Forcing};

        fn rotate_at(block: BlockNumber) {
            System::set_block_number(block);
            pallet_babe::CurrentSlot::<Runtime>::put(sp_consensus_babe::Slot::from(u64::from(
                block,
            )));
            pallet_babe::Initialized::<Runtime>::put(None::<sp_consensus_babe::digests::PreDigest>);
            crate::Session::rotate_session();
            <crate::Grandpa as Hooks<BlockNumber>>::on_finalize(block);
        }

        fn next_session() {
            let target =
                (pallet_session::CurrentIndex::<Runtime>::get() + 1) * EpochDuration::get() as u32;
            while System::current_block_number() < target - 1 {
                poll();
            }
            rotate_at(target);
        }

        fn finish_round() {
            for _ in 0..Pages::get() + 1 + UnsignedPhase::get() {
                poll();
            }
            assert_eq!(
                multi_block::CurrentPhase::<Runtime>::get(),
                multi_block::Phase::Done
            );
        }

        #[test]
        fn starts_at_session_three_and_elects_on_entry_to_five() {
            validator_onboarding::tests::ext().execute_with(|| {
                // Genesis and sessions 1/2 do not start an MBE round.
                assert!(MultiBlockElection::status().is_err());
                for _ in 0..2 {
                    next_session();
                    assert!(MultiBlockElection::status().is_err());
                }
                next_session();
                assert_eq!(pallet_session::CurrentIndex::<Runtime>::get(), 3);
                assert_eq!(
                    multi_block::CurrentPhase::<Runtime>::get(),
                    multi_block::Phase::Snapshot(1)
                );
                finish_round();
                let deadline = 5 * EpochDuration::get() as u32;
                assert!(deadline - System::current_block_number() >= MinimumElectionSlack::get());
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
                next_session();
                assert_eq!(pallet_session::CurrentIndex::<Runtime>::get(), 5);
                assert_eq!(CurrentEra::<Runtime>::get(), Some(1));
                assert!(MultiBlockElection::status().is_err());
                // Era 1 begins in session 6, so its planning offset 3 is session 9.
                for _ in 0..3 {
                    next_session();
                }
                assert!(MultiBlockElection::status().is_err());
                next_session();
                assert_eq!(pallet_session::CurrentIndex::<Runtime>::get(), 9);
                assert!(MultiBlockElection::status().is_ok());
            });
        }

        #[test]
        fn failed_export_restarts_and_recovers_on_a_later_session() {
            validator_onboarding::tests::ext().execute_with(|| {
                for _ in 0..3 {
                    next_session();
                }
                finish_round();
                // Simulate a missing snapshot page: no solution can be queued,
                // and the on-chain fallback must fail this particular round.
                multi_block::PagedVoterSnapshot::<Runtime>::remove(
                    multi_block::Round::<Runtime>::get(),
                    0,
                );
                next_session();
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
                assert_eq!(multi_block::Round::<Runtime>::get(), 1);
                assert_eq!(
                    multi_block::CurrentPhase::<Runtime>::get(),
                    multi_block::Phase::Snapshot(1)
                );
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(1));
                assert_eq!(Validators::<Runtime>::get().len(), 2);
            });
        }

        #[test]
        fn missing_initial_era_can_recover() {
            validator_onboarding::tests::ext().execute_with(|| {
                CurrentEra::<Runtime>::kill();
                for _ in 0..3 {
                    next_session();
                }
                assert_eq!(CurrentEra::<Runtime>::get(), None);
                assert!(MultiBlockElection::status().is_ok());
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
            });
        }

        #[test]
        fn force_new_mid_era_starts_then_elects() {
            validator_onboarding::tests::ext().execute_with(|| {
                next_session();
                assert_ok!(Staking::force_new_era(RuntimeOrigin::root()));
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
                assert!(MultiBlockElection::status().is_ok());
                assert_eq!(ForceEra::<Runtime>::get(), Forcing::ForceNew);
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(1));
                assert_eq!(ForceEra::<Runtime>::get(), Forcing::NotForcing);
            });
        }

        #[test]
        fn force_none_defers_planning_and_resuming_recovers() {
            validator_onboarding::tests::ext().execute_with(|| {
                assert_ok!(Staking::force_no_eras(RuntimeOrigin::root()));
                for _ in 0..7 {
                    next_session();
                }
                assert!(MultiBlockElection::status().is_err());
                assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
                ForceEra::<Runtime>::put(Forcing::NotForcing);
                next_session();
                assert!(MultiBlockElection::status().is_ok());
                next_session();
                assert_eq!(CurrentEra::<Runtime>::get(), Some(1));
            });
        }

        #[test]
        fn force_none_discards_completed_round_before_resuming_after_removal() {
            // Both stale queued solutions and fallback snapshots must be cleared.
            for queue_solution in [false, true] {
                validator_onboarding::tests::ext().execute_with(|| {
                    let removed = Validators::<Runtime>::get()[0].clone();
                    for _ in 0..3 {
                        next_session();
                    }
                    poll();
                    poll();
                    if queue_solution {
                        let solution =
                            miner::OffchainWorkerMiner::<Runtime>::mine_solution(1, true).unwrap();
                        assert_ok!(MultiBlockElectionUnsigned::submit_unsigned(
                            RuntimeOrigin::none(),
                            alloc::boxed::Box::new(solution)
                        ));
                    }
                    finish_round();
                    let old_round = multi_block::Round::<Runtime>::get();
                    assert!(
                        multi_block::PagedTargetSnapshot::<Runtime>::get(old_round, 0)
                            .unwrap()
                            .contains(&removed)
                    );
                    assert_eq!(
                        MultiBlockElectionVerifier::get_queued_solution_page(0).is_some(),
                        queue_solution,
                    );
                    assert_ok!(Staking::force_no_eras(RuntimeOrigin::root()));
                    for _ in 0..3 {
                        next_session();
                    }
                    assert!(MultiBlockElection::status().is_err());
                    assert_eq!(multi_block::Round::<Runtime>::get(), old_round + 1);
                    assert!(
                        multi_block::PagedTargetSnapshot::<Runtime>::get(old_round, 0).is_none()
                    );
                    assert!(
                        multi_block::PagedVoterSnapshot::<Runtime>::get(old_round, 0).is_none()
                    );
                    assert!(MultiBlockElectionVerifier::get_queued_solution_page(0).is_none());
                    let council = pallet_collective::RawOrigin::<
                        AccountId,
                        pallet_collective::Instance1,
                    >::Members(1, 1)
                    .into();
                    assert_ok!(crate::ValidatorAdmission::remove(council, removed.clone()));
                    next_session();
                    assert!(MultiBlockElection::status().is_err());
                    assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
                    ForceEra::<Runtime>::put(Forcing::NotForcing);
                    next_session();
                    assert!(MultiBlockElection::status().is_ok());
                    finish_round();
                    assert!(!multi_block::PagedTargetSnapshot::<Runtime>::get(
                        multi_block::Round::<Runtime>::get(),
                        0,
                    )
                    .unwrap()
                    .contains(&removed));
                    next_session();
                    assert_eq!(CurrentEra::<Runtime>::get(), Some(1));
                    next_session(); // Activate the newly planned validator set.
                    let elected = Validators::<Runtime>::get();
                    assert_eq!(elected.len(), 1);
                    assert!(!elected.contains(&removed));
                });
            }
        }

        #[test]
        fn consecutive_rotations_do_not_consume_an_unready_round() {
            validator_onboarding::tests::ext().execute_with(|| {
                assert_ok!(Staking::force_new_era_always(RuntimeOrigin::root()));
                for block in 2..5 {
                    rotate_at(block);
                    assert_eq!(CurrentEra::<Runtime>::get(), Some(0));
                    assert_eq!(multi_block::Round::<Runtime>::get(), 0);
                    let mut meter = WeightMeter::with_limit(RuntimeBlockWeights::get().max_block);
                    <MultiBlockElection as Hooks<BlockNumber>>::on_poll(block, &mut meter);
                }
                // Three blocks after start, the round can be safely consumed.
                rotate_at(5);
                assert_eq!(CurrentEra::<Runtime>::get(), Some(1));
                assert_eq!(multi_block::Round::<Runtime>::get(), 1);
                assert!(MultiBlockElection::status().is_ok()); // ForceAlways plans the next round
            });
        }
    }
}

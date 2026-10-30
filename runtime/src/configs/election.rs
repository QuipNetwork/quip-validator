//! Single-page multi-block elections for classic staking.
use super::validator_onboarding::{AdmissionData, OnChainConfig};
use super::*;
use crate::{MultiBlockElection, MultiBlockElectionSigned, MultiBlockElectionVerifier, Staking};
use alloc::vec;
use frame_election_provider_support::{
    onchain, BoundedSupportsOf, ElectionProvider, PageIndex, SequentialPhragmen,
};
use pallet_election_provider_multi_block::{self as multi_block, unsigned::miner};
use sp_runtime::{transaction_validity::TransactionPriority, PerU16};

parameter_types! {
    pub const Pages: u32 = 1;
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

    fn poll() {
        let now = System::block_number().saturating_add(1);
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
            let who = crate::Session::validators()[0].clone();
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
                System::block_number(),
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
            let removed = crate::Session::validators()[0].clone();
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
}

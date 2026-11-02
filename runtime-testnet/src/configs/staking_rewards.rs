//! V0 fixed staking budget; stake affects distribution, not the era's entitlement.
use crate::{Balance, Runtime, Staking, UNIT};
use pallet_emission_controller::{MILLISECONDS_PER_YEAR, STAKING_ANNUAL_EMISSION_QUIP};

frame_support::parameter_types! {
    pub const StakingAnnualEmission: Balance = STAKING_ANNUAL_EMISSION_QUIP.saturating_mul(UNIT);
}

// Assert the runtime-selected controller amount too, not only the shared policy.
const _: () = {
    let controller = <Runtime as pallet_emission_controller::Config>::AnnualEmission::get();
    assert!(controller == StakingAnnualEmission::get() * 3);
    assert!(
        controller + StakingAnnualEmission::get()
            == pallet_emission_controller::ANNUAL_EMISSION_QUIP * UNIT
    );
};

// Staking needs block-author points before any recorded era budget is claimable.
// Use the same BABE authority-index mapping already used by Revive.
impl pallet_authorship::Config for Runtime {
    type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Runtime, crate::Babe>;
    type EventHandler = Staking;
}

pub struct FixedStakingPayout;
impl pallet_staking::EraPayout<Balance> for FixedStakingPayout {
    fn era_payout(
        _total_staked: Balance,
        _total_issuance: Balance,
        era_duration_millis: u64,
    ) -> (Balance, Balance) {
        // This computes an entitlement only. The SDK mints on payout_stakers;
        // Reward = () drops the resulting debt and accounts for that issuance.
        let payout = StakingAnnualEmission::get()
            .saturating_mul(u128::from(era_duration_millis))
            .saturating_div(u128::from(MILLISECONDS_PER_YEAR));
        (payout, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Balances, EmissionController};
    use pallet_staking::EraPayout;

    const DAY_MILLIS: u64 = 24 * 60 * 60 * 1_000;
    type Payout = <Runtime as pallet_staking::Config>::EraPayout;

    #[test]
    fn fixed_budget_is_duration_proportional_and_independent_of_stake() {
        assert_eq!(StakingAnnualEmission::get(), 10_000_000 * UNIT);
        assert_eq!(Payout::era_payout(0, 0, 0), (0, 0));
        let day = Payout::era_payout(0, 0, DAY_MILLIS);
        assert_eq!(day.1, 0);
        // Reuse the controller's year, with no Perbill rate approximation.
        assert_eq!(day.0, 10_000_000 * UNIT / 365);
        assert_eq!(
            day,
            Payout::era_payout(100_000_000 * UNIT, 500_000_000 * UNIT, DAY_MILLIS)
        );
        assert_eq!(
            day,
            Payout::era_payout(Balance::MAX, Balance::MAX, DAY_MILLIS)
        );
        let half = Payout::era_payout(1, 1, DAY_MILLIS / 2).0;
        assert!(day.0.abs_diff(2 * half) <= 1);
        assert_eq!(
            Payout::era_payout(0, 0, MILLISECONDS_PER_YEAR),
            (10_000_000 * UNIT, 0)
        );
        // Long eras stay proportional instead of being capped at a Perbill's one year.
        assert_eq!(
            Payout::era_payout(0, 0, 2 * MILLISECONDS_PER_YEAR),
            (20_000_000 * UNIT, 0)
        );
        let extreme = Payout::era_payout(0, 0, u64::MAX);
        assert_eq!(
            extreme.0,
            StakingAnnualEmission::get() * u128::from(u64::MAX) / u128::from(MILLISECONDS_PER_YEAR)
        );
        assert_eq!(extreme.1, 0);
    }

    #[test]
    fn controller_and_daily_staking_budgets_share_one_40m_year() {
        super::super::validator_onboarding::tests::ext().execute_with(|| {
            let before = Balances::total_issuance();
            EmissionController::accrue(1); // Anchor without minting.
            let mut staking = 0;
            for day in 1..=365 {
                staking += Payout::era_payout(0, before, DAY_MILLIS).0;
                EmissionController::accrue(1 + day * DAY_MILLIS / 1_000);
            }
            EmissionController::accrue(1 + pallet_emission_controller::YEAR_SECONDS);
            assert!(staking.abs_diff(10_000_000 * UNIT) <= 365);
            let controller =
                EmissionController::entitlement(pallet_emission_controller::YEAR_SECONDS).unwrap();
            assert_eq!(controller, 30_000_000 * UNIT);
            assert_eq!(
                pallet_emission_controller::ScheduledIssued::<Runtime>::get(),
                controller
            );
            assert_eq!(Balances::total_issuance() - before, controller);
            assert!((controller + staking).abs_diff(40_000_000 * UNIT) <= 365);
            // No staking pot: each existing route gets 50% of the 30M controller budget.
            assert_eq!(
                pallet_emission_controller::Routes::<Runtime>::get().len(),
                2
            );
            for subnet in [
                pallet_emission_controller::ISING_SUBNET,
                pallet_emission_controller::QVRF_SUBNET,
            ] {
                assert_eq!(
                    pallet_emission_controller::SubnetIssued::<Runtime>::get(subnet),
                    15_000_000 * UNIT
                );
            }
        });
    }
    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn era_reward_mints_validator_commission_and_nominator_share_on_claim() {
        use crate::{AccountId, Address, BlockNumber, RuntimeOrigin, System, Timestamp};
        use codec::Encode;
        use frame_support::{
            assert_noop, assert_ok,
            traits::{Currency, Hooks},
            weights::WeightMeter,
        };
        use sp_runtime::{traits::BlockNumberProvider, Perbill};

        fn note_author(validator: &AccountId) {
            let index = pallet_session::Validators::<Runtime>::get()
                .iter()
                .position(|who| who == validator)
                .unwrap() as u32;
            let block = System::current_block_number() + 1;
            let pre_digest = sp_consensus_babe::digests::PreDigest::SecondaryPlain(
                sp_consensus_babe::digests::SecondaryPlainPreDigest {
                    authority_index: index,
                    slot: sp_consensus_babe::Slot::from(u64::from(block)),
                },
            );
            System::initialize(
                &block,
                &Default::default(),
                &sp_runtime::generic::Digest {
                    logs: alloc::vec![sp_runtime::generic::DigestItem::PreRuntime(
                        sp_consensus_babe::BABE_ENGINE_ID,
                        pre_digest.encode(),
                    )],
                },
            );
            <crate::Authorship as Hooks<BlockNumber>>::on_initialize(block);
            assert_eq!(crate::Authorship::author(), Some(validator.clone()));
            <crate::Authorship as Hooks<BlockNumber>>::on_finalize(block);
        }

        fn next_session() {
            let index = pallet_session::CurrentIndex::<Runtime>::get() + 1;
            let block = index * super::super::EpochDuration::get() as u32;
            System::set_block_number(block);
            pallet_babe::CurrentSlot::<Runtime>::put(sp_consensus_babe::Slot::from(u64::from(
                block,
            )));
            pallet_babe::Initialized::<Runtime>::put(None::<sp_consensus_babe::digests::PreDigest>);
            Timestamp::set_timestamp(1 + u64::from(block) * 6_000);
            crate::Session::rotate_session();
            <crate::Grandpa as Hooks<BlockNumber>>::on_finalize(block);
            <Staking as Hooks<BlockNumber>>::on_finalize(block);
            // Snapshot and transition blocks between the full-duration sessions.
            for offset in 1..=3 {
                System::set_block_number(block + offset);
                let mut meter =
                    WeightMeter::with_limit(crate::configs::RuntimeBlockWeights::get().max_block);
                <crate::MultiBlockElection as Hooks<BlockNumber>>::on_poll(
                    block + offset,
                    &mut meter,
                );
            }
        }

        super::super::validator_onboarding::tests::ext().execute_with(|| {
            let validator = pallet_session::Validators::<Runtime>::get()[0].clone();
            let nominator = AccountId::new([173; 32]);
            let _ = Balances::make_free_balance_be(&nominator, 1_001 * UNIT);
            assert_ok!(Staking::bond(
                RuntimeOrigin::signed(nominator.clone()),
                1_000 * UNIT,
                pallet_staking::RewardDestination::Stash
            ));
            assert_ok!(Staking::nominate(
                RuntimeOrigin::signed(nominator.clone()),
                alloc::vec![Address::Id(validator.clone())]
            ));
            assert_ok!(Staking::set_payee(
                RuntimeOrigin::signed(validator.clone()),
                pallet_staking::RewardDestination::Stash
            ));
            assert_ok!(Staking::validate(
                RuntimeOrigin::signed(validator.clone()),
                pallet_staking::ValidatorPrefs {
                    commission: Perbill::from_percent(10),
                    blocked: false
                }
            ));
            Timestamp::set_timestamp(1);
            <Staking as Hooks<BlockNumber>>::on_finalize(System::current_block_number());
            for _ in 0..6 {
                next_session();
            }
            let era = pallet_staking::ActiveEra::<Runtime>::get().unwrap().index;
            assert_eq!(era, 1);
            let exposure =
                pallet_staking::ErasStakersOverview::<Runtime>::get(era, &validator).unwrap();
            assert_eq!(exposure.own, 2_000 * UNIT);
            assert_eq!(exposure.total, 3_000 * UNIT);
            assert_eq!(
                pallet_staking::ErasStakersPaged::<Runtime>::get((era, &validator, 0))
                    .unwrap()
                    .others[0]
                    .who,
                nominator
            );
            note_author(&validator);
            let points = pallet_staking::ErasRewardPoints::<Runtime>::get(era);
            assert_eq!(points.total, 20);
            assert_eq!(points.individual.get(&validator), Some(&20));
            // Anchor controller accrual to the same era start for a full-year
            // issuance check through real staking claims below.
            EmissionController::accrue(
                <Timestamp as frame_support::traits::UnixTime>::now().as_secs(),
            );
            let issuance = Balances::total_issuance();
            let validator_before = Balances::free_balance(&validator);
            let nominator_before = Balances::free_balance(&nominator);
            for _ in 0..6 {
                next_session();
            }
            let era_millis = super::super::EpochDuration::get()
                * u64::from(super::super::validator_onboarding::SessionsPerEra::get())
                * crate::SLOT_DURATION;
            let eras_per_year = MILLISECONDS_PER_YEAR / era_millis;
            assert_eq!(MILLISECONDS_PER_YEAR % era_millis, 0);
            assert!(matches!(
                (era_millis, eras_per_year),
                (86_400_000, 365) | (21_600_000, 1_460)
            ));
            let budget = pallet_staking::ErasValidatorReward::<Runtime>::get(era).unwrap();
            assert_eq!(
                budget,
                Payout::era_payout(3_000 * UNIT, issuance, era_millis).0
            );
            assert_eq!(Balances::total_issuance(), issuance); // Budget calculation does not mint.
            assert_ok!(Staking::payout_stakers(
                RuntimeOrigin::signed(nominator.clone()),
                validator.clone(),
                era
            ));
            let rewarded: Balance = System::events()
                .into_iter()
                .filter_map(|record| match record.event {
                    crate::RuntimeEvent::Staking(pallet_staking::Event::Rewarded {
                        amount,
                        ..
                    }) => Some(amount),
                    _ => None,
                })
                .sum();
            let nominator_paid = Balances::free_balance(&nominator) - nominator_before;
            let validator_paid = Balances::free_balance(&validator) - validator_before;
            let commission = Perbill::from_percent(10) * budget;
            assert_eq!(
                nominator_paid,
                Perbill::from_rational(1u32, 3u32) * (budget - commission)
            );
            assert_eq!(
                validator_paid,
                commission + Perbill::from_rational(2u32, 3u32) * (budget - commission)
            );
            assert!(nominator_paid > 0);
            assert_eq!(rewarded, nominator_paid + validator_paid);
            assert!(rewarded <= budget);
            #[cfg(feature = "try-runtime")]
            assert_ok!(<EmissionController as Hooks<BlockNumber>>::try_state(
                System::current_block_number()
            ));
            assert_eq!(
                Balances::total_issuance() - issuance,
                nominator_paid + validator_paid
            );
            // Exposure fractions use Perbill; thirds can lose one part per billion.
            assert!(
                (nominator_paid + validator_paid).abs_diff(budget) <= budget / 1_000_000_000 + 2
            );
            let after_claim = Balances::total_issuance();
            assert_noop!(
                Staking::payout_stakers(RuntimeOrigin::signed(nominator), validator.clone(), era)
                    .map_err(|error| error.error),
                pallet_staking::Error::<Runtime>::AlreadyClaimed
            );
            assert_eq!(Balances::total_issuance(), after_claim);
            EmissionController::accrue(
                <Timestamp as frame_support::traits::UnixTime>::now().as_secs(),
            );
            // Claim a full accounting year: 365 daily eras or 1460 six-hour eras.
            // Payees use Stash, so rewards do not alter the exposure ratios.
            for _ in 1..eras_per_year {
                let era = pallet_staking::ActiveEra::<Runtime>::get().unwrap().index;
                note_author(&validator);
                for _ in 0..6 {
                    next_session();
                }
                assert_ok!(Staking::payout_stakers(
                    RuntimeOrigin::signed(validator.clone()),
                    validator.clone(),
                    era
                ));
                EmissionController::accrue(
                    <Timestamp as frame_support::traits::UnixTime>::now().as_secs(),
                );
            }
            let controller =
                EmissionController::entitlement(MILLISECONDS_PER_YEAR / 1_000).unwrap();
            let entitled = controller + u128::from(eras_per_year) * budget;
            let minted = Balances::total_issuance() - issuance;
            assert_eq!(
                pallet_emission_controller::ScheduledIssued::<Runtime>::get(),
                controller
            );
            // Includes SDK exposure rounding.
            assert!(minted.abs_diff(entitled) < UNIT);
            assert!(minted.abs_diff(40_000_000 * UNIT) < UNIT);
            #[cfg(feature = "try-runtime")]
            assert_ok!(<EmissionController as Hooks<BlockNumber>>::try_state(
                System::current_block_number()
            ));
        });
    }
}

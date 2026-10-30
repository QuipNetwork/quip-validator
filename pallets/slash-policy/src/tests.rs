use crate::{mock::*, *};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::Perbill;

#[test]
fn defaults_and_parameter_bounds_are_enforced() {
    ext().execute_with(|| {
        assert_eq!(
            MaxDiscretionarySlashPerMotion::<Test>::get(),
            Perbill::from_percent(1)
        );
        assert_eq!(
            MaxDiscretionarySlashCumulative::<Test>::get(),
            Perbill::from_percent(4)
        );
        assert_eq!(BudgetWindow::<Test>::get(), 28);
        assert_noop!(
            SlashPolicy::discretionary_slash(
                RuntimeOrigin::signed(1),
                1,
                1,
                Perbill::from_percent(1)
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        for (motion, cumulative) in [
            (Perbill::zero(), MIN_CUMULATIVE),
            (Perbill::from_parts(999_999), MIN_CUMULATIVE),
            (Perbill::from_percent(6), MAX_CUMULATIVE),
            (MIN_PER_MOTION, Perbill::from_parts(9_999_999)),
            (MIN_PER_MOTION, Perbill::from_percent(11)),
            (MAX_PER_MOTION, Perbill::from_percent(4)),
        ] {
            assert_noop!(
                SlashPolicy::set_limits(RuntimeOrigin::root(), motion, cumulative, 28),
                Error::<Test>::InvalidFraction
            );
        }
        for window in [27, 85] {
            assert_noop!(
                SlashPolicy::set_limits(
                    RuntimeOrigin::root(),
                    DEFAULT_PER_MOTION,
                    DEFAULT_CUMULATIVE,
                    window
                ),
                Error::<Test>::InvalidWindow
            );
        }
        assert_ok!(SlashPolicy::set_limits(
            RuntimeOrigin::root(),
            MIN_PER_MOTION,
            MIN_CUMULATIVE,
            28
        ));
        assert_ok!(SlashPolicy::set_limits(
            RuntimeOrigin::root(),
            MAX_PER_MOTION,
            MAX_CUMULATIVE,
            84
        ));
    });
}

#[test]
fn no_op_is_rolled_back_and_has_no_success_event_or_budget_charge() {
    ext().execute_with(|| {
        assert_ok!(SlashPolicy::discretionary_slash(
            RuntimeOrigin::root(),
            1,
            1,
            Perbill::from_percent(1)
        ));
        assert_noop!(
            SlashPolicy::discretionary_slash(
                RuntimeOrigin::root(),
                1,
                1,
                Perbill::from_parts(5_000_000)
            ),
            Error::<Test>::NoSlashScheduled
        );
        assert_eq!(
            SlashHistory::<Test>::get(1).as_slice(),
            &[(1, Perbill::from_percent(1))]
        );
        System::assert_last_event(
            Event::SlashScheduled {
                stash: 1,
                offence_era: 1,
                apply_era: 29,
                fraction: Perbill::from_percent(1),
                remaining_budget: Perbill::from_percent(3),
            }
            .into(),
        );
    });
}

#[test]
fn second_era_in_slashed_span_rejects_zero_value_queue_without_budget_charge() {
    ext().execute_with(|| {
        set_era(2);
        set_era(3);
        assert_ok!(SlashPolicy::discretionary_slash(
            RuntimeOrigin::root(),
            1,
            3,
            DEFAULT_PER_MOTION
        ));
        let first_queue = pallet_staking::UnappliedSlashes::<Test>::get(31);
        assert_eq!(first_queue.len(), 1);
        assert_eq!(first_queue[0].own, 100);
        assert!(first_queue[0].others.is_empty());
        let history = SlashHistory::<Test>::get(1);
        assert_eq!(history.as_slice(), &[(3, DEFAULT_PER_MOTION)]);
        let events = System::events();

        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 2, DEFAULT_PER_MOTION),
            Error::<Test>::NoSlashScheduled
        );
        assert!(pallet_staking::UnappliedSlashes::<Test>::get(30).is_empty());
        assert_eq!(
            pallet_staking::UnappliedSlashes::<Test>::get(31),
            first_queue
        );
        assert_eq!(SlashHistory::<Test>::get(1), history);
        assert_eq!(System::events(), events);
    });
}

#[test]
fn fractions_are_cumulative_across_spans_and_cancel_does_not_refund_budget() {
    ext().execute_with(|| {
        assert_ok!(SlashPolicy::discretionary_slash(
            RuntimeOrigin::root(),
            1,
            1,
            Perbill::from_percent(1)
        ));
        assert_ok!(Staking::cancel_deferred_slash(
            RuntimeOrigin::root(),
            29,
            vec![0]
        ));
        for era in 2..=4 {
            set_era(era);
            assert_ok!(SlashPolicy::discretionary_slash(
                RuntimeOrigin::root(),
                1,
                era,
                Perbill::from_percent(1)
            ));
        }
        System::assert_last_event(
            Event::SlashScheduled {
                stash: 1,
                offence_era: 4,
                apply_era: 32,
                fraction: Perbill::from_percent(1),
                remaining_budget: Perbill::zero(),
            }
            .into(),
        );
        set_era(5);
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 5, Perbill::from_percent(1)),
            Error::<Test>::BudgetExceeded
        );
        set_era(28);
        assert_noop!(
            SlashPolicy::discretionary_slash(
                RuntimeOrigin::root(),
                1,
                28,
                Perbill::from_percent(1)
            ),
            Error::<Test>::BudgetExceeded
        );
        set_era(29);
        assert_ok!(SlashPolicy::discretionary_slash(
            RuntimeOrigin::root(),
            1,
            29,
            Perbill::from_percent(1)
        ));
    });
}

#[test]
fn invalid_era_zero_exposure_and_invalid_fraction_are_rejected() {
    ext().execute_with(|| {
        pallet_staking::CurrentEra::<Test>::put(2);
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 2, Perbill::from_percent(1)),
            Error::<Test>::InvalidEra
        );
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 2, 1, Perbill::from_percent(1)),
            Error::<Test>::NoExposure
        );
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 1, Perbill::zero()),
            Error::<Test>::InvalidFraction
        );
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 1, Perbill::from_percent(2)),
            Error::<Test>::InvalidFraction
        );
        set_era(29);
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 1, Perbill::from_percent(1)),
            Error::<Test>::InvalidEra
        );
    });
}

#[test]
fn increasing_window_still_counts_retained_old_motions() {
    ext().execute_with(|| {
        for era in 1..=4 {
            set_era(era);
            assert_ok!(SlashPolicy::discretionary_slash(
                RuntimeOrigin::root(),
                1,
                era,
                Perbill::from_percent(1)
            ));
        }
        set_era(29);
        assert_ok!(SlashPolicy::set_limits(
            RuntimeOrigin::root(),
            DEFAULT_PER_MOTION,
            DEFAULT_CUMULATIVE,
            84
        ));
        assert_noop!(
            SlashPolicy::discretionary_slash(
                RuntimeOrigin::root(),
                1,
                29,
                Perbill::from_percent(1)
            ),
            Error::<Test>::BudgetExceeded
        );
    });
}

#[test]
fn lowering_the_budget_does_not_erase_previous_charges() {
    ext().execute_with(|| {
        for era in 1..=2 {
            set_era(era);
            assert_ok!(SlashPolicy::discretionary_slash(
                RuntimeOrigin::root(),
                1,
                era,
                DEFAULT_PER_MOTION
            ));
        }
        assert_ok!(SlashPolicy::set_limits(
            RuntimeOrigin::root(),
            DEFAULT_PER_MOTION,
            MIN_CUMULATIVE,
            28
        ));
        set_era(3);
        assert_noop!(
            SlashPolicy::discretionary_slash(RuntimeOrigin::root(), 1, 3, DEFAULT_PER_MOTION),
            Error::<Test>::BudgetExceeded
        );
    });
}

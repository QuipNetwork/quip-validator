use crate::{mock::*, *};
use frame_support::{assert_noop, assert_ok, traits::Hooks};
use sp_runtime::Perbill;

#[test]
fn fixed_schedule_is_proof_independent_and_perpetual() {
    new_test_ext().execute_with(|| {
        let initial = Balances::total_issuance();
        let annual = 40_000_000 * 1_000_000_000_000u128;
        EmissionController::accrue(99);
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        EmissionController::accrue(100 + YEAR_SECONDS);
        assert_eq!(ScheduledIssued::<Test>::get(), annual);
        assert_eq!(
            Balances::free_balance(EmissionController::pot(0)),
            annual / 2
        );
        assert_eq!(
            Balances::free_balance(EmissionController::pot(1)),
            annual / 2
        );
        assert_eq!(Balances::total_issuance(), initial + annual);
        EmissionController::accrue(100 + 11 * YEAR_SECONDS);
        assert_eq!(ScheduledIssued::<Test>::get(), 11 * annual);
    });
}
#[test]
fn repeated_calls_clock_regression_and_rounding_do_not_remint() {
    new_test_ext().execute_with(|| {
        for second in 101..200 {
            EmissionController::accrue(second);
        }
        let paid = ScheduledIssued::<Test>::get();
        assert_eq!(paid, EmissionController::entitlement(99).unwrap() / 2 * 2);
        EmissionController::accrue(199);
        EmissionController::accrue(150);
        assert_eq!(ScheduledIssued::<Test>::get(), paid);
        EmissionController::accrue(100 + YEAR_SECONDS);
        assert_eq!(
            ScheduledIssued::<Test>::get(),
            40_000_000 * 1_000_000_000_000u128
        );
    });
}
#[test]
fn rewards_transfer_without_issuance_and_cannot_spend_other_subnet() {
    new_test_ext().execute_with(|| {
        assert_eq!(SubnetRewards::<Test, 0>::pay(&2, 50), 0);
        System::assert_last_event(
            Event::<Test>::RewardUnavailable {
                subnet: 0,
                who: 2,
                requested: 50,
                error: None,
            }
            .into(),
        );
        EmissionController::accrue(101);
        let issuance = Balances::total_issuance();
        let other = Balances::free_balance(EmissionController::pot(1));
        assert_eq!(SubnetRewards::<Test, 0>::pay(&2, 50), 50);
        assert_eq!(Balances::free_balance(2), 50);
        assert_eq!(Balances::total_issuance(), issuance);
        let remaining = Balances::free_balance(EmissionController::pot(0)) - 10;
        assert_eq!(SubnetRewards::<Test, 0>::pay(&2, u128::MAX), remaining);
        assert_eq!(SubnetRewards::<Test, 0>::pay(&2, 1), 0);
        assert_eq!(Balances::free_balance(EmissionController::pot(1)), other);
        assert_eq!(Balances::total_issuance(), issuance);
    });
}
#[test]
fn testnet_budget_is_separate_and_overflow_is_atomic() {
    new_test_ext().execute_with(|| {
        assert_ok!(EmissionController::mint_faucet(&2, 10_000));
        assert_noop!(
            EmissionController::mint_faucet(&2, 1),
            Error::<Test>::BudgetExceeded
        );
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        FaucetBudget::<Test>::put(u128::MAX);
        assert_noop!(
            EmissionController::mint_faucet(&2, u128::MAX),
            Error::<Test>::Overflow
        );
        assert_eq!(FaucetIssued::<Test>::get(), 10_000);
    });
}
#[test]
fn no_epoch_backfill_for_new_genesis_and_missing_config_is_closed() {
    new_test_ext().execute_with(|| {
        StartAt::<Test>::kill();
        EmissionController::accrue(0);
        assert!(!StartAt::<Test>::exists());
        EmissionController::accrue(1_800_000_000);
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        assert_eq!(StartAt::<Test>::get(), Some(1_800_000_000));
        Enabled::<Test>::kill();
        EmissionController::accrue(1_900_000_000);
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        FaucetBudget::<Test>::kill();
        assert_noop!(
            EmissionController::mint_faucet(&2, 1),
            Error::<Test>::BudgetExceeded
        );
    });
}
#[test]
fn invalid_routes_rejected_and_accounts_distinct() {
    let full = Perbill::one();
    assert!(!EmissionController::valid_routes(
        &[(0, full), (0, full)],
        true
    ));
    assert!(!EmissionController::valid_routes(
        &[(0, Perbill::from_percent(50))],
        true
    ));
    assert!(!EmissionController::valid_routes(&[], true));
    assert!(EmissionController::valid_routes(&[], false));
    assert_ne!(EmissionController::pot(0), EmissionController::pot(1));
}
#[test]
fn root_issuance_is_an_explicit_exception_to_controller_accounting() {
    new_test_ext().execute_with(|| {
        let original = Balances::total_issuance();
        assert_ok!(Balances::force_set_balance(RuntimeOrigin::root(), 2, 123));
        assert_eq!(Balances::total_issuance(), original + 123);
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        assert_eq!(FaucetIssued::<Test>::get(), 0);
    });
}
#[test]
fn hook_and_upgrade_are_idempotent() {
    new_test_ext().execute_with(|| {
        System::set_block_number(200);
        EmissionController::on_initialize(200);
        let issuance = Balances::total_issuance();
        EmissionController::on_runtime_upgrade();
        EmissionController::on_initialize(200);
        assert_eq!(Balances::total_issuance(), issuance);
        #[cfg(feature = "try-runtime")]
        EmissionController::try_state(200).unwrap();
    });
}

#[test]
fn failed_deposit_does_not_consume_budget_or_emit_success() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            EmissionController::mint_faucet(&2, 1),
            Error::<Test>::DepositFailed
        );
        assert_eq!(FaucetIssued::<Test>::get(), 0);
        assert_eq!(Balances::free_balance(2), 0);
    });
}

#[test]
fn failed_accrual_is_visible_and_does_not_consume_entitlement() {
    new_test_ext().execute_with(|| {
        // Fault injection: no issuance headroom, independent of the recipient.
        pallet_balances::TotalIssuance::<Test>::put(u128::MAX);
        EmissionController::accrue(101);
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        System::assert_has_event(
            Event::<Test>::FundingFailed {
                subnet: 0,
                amount: EmissionController::entitlement(1).unwrap() / 2,
                error: Error::<Test>::Overflow.into(),
            }
            .into(),
        );
    });
}

use crate::{mock::*, *};
use frame_support::{assert_noop, assert_ok, traits::Hooks};
use sp_runtime::Perbill;

#[test]
fn fixed_schedule_is_proof_independent_and_perpetual() {
    new_test_ext().execute_with(|| {
        let initial = Balances::total_issuance();
        let annual = 30_000_000 * 1_000_000_000_000u128;
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
            30_000_000 * 1_000_000_000_000u128
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

#[test]
fn rate_reduction_rebases_paid_history_without_reminting_or_stalling() {
    use frame_support::traits::{GetStorageVersion, StorageVersion};
    for elapsed in [1, 123_457, YEAR_SECONDS] {
        new_test_ext().execute_with(|| {
            let old_start = 1_800_000_000;
            let now = old_start + elapsed;
            StartAt::<Test>::put(old_start);
            AnnualEmission::set(40_000_000 * 1_000_000_000_000);
            EmissionController::accrue(now);
            let issued = ScheduledIssued::<Test>::get();
            let counters = [SubnetIssued::<Test>::get(0), SubnetIssued::<Test>::get(1)];
            let balances = [
                Balances::free_balance(EmissionController::pot(0)),
                Balances::free_balance(EmissionController::pot(1)),
            ];
            let issuance = Balances::total_issuance();
            StorageVersion::new(1).put::<EmissionController>();
            AnnualEmission::set(30_000_000 * 1_000_000_000_000);
            System::set_block_number(now);
            #[cfg(feature = "try-runtime")]
            let pre = EmissionController::pre_upgrade().unwrap();
            EmissionController::on_runtime_upgrade();
            #[cfg(feature = "try-runtime")]
            EmissionController::post_upgrade(pre).unwrap();
            assert_eq!(
                EmissionController::on_chain_storage_version(),
                StorageVersion::new(2)
            );
            let start = StartAt::<Test>::get().unwrap();
            assert_eq!(
                start,
                now - (issued * u128::from(YEAR_SECONDS) / AnnualEmission::get()) as u64
            );
            assert!(start < old_start || elapsed == 1);
            assert_eq!(ScheduledIssued::<Test>::get(), issued);
            assert_eq!(
                [SubnetIssued::<Test>::get(0), SubnetIssued::<Test>::get(1)],
                counters
            );
            assert_eq!(
                [
                    Balances::free_balance(EmissionController::pot(0)),
                    Balances::free_balance(EmissionController::pot(1))
                ],
                balances
            );
            assert_eq!(Balances::total_issuance(), issuance);
            EmissionController::accrue(now);
            assert_eq!(Balances::total_issuance(), issuance);
            EmissionController::accrue(now + 1);
            let delta = ScheduledIssued::<Test>::get() - issued;
            assert!(delta > 0);
            assert!(delta <= AnnualEmission::get() / u128::from(YEAR_SECONDS) + 2);
            assert_eq!(Balances::total_issuance() - issuance, delta);
            // The version guard must not reset the clock on another upgrade call.
            System::set_block_number(now + 100);
            EmissionController::on_runtime_upgrade();
            assert_eq!(StartAt::<Test>::get(), Some(start));
            assert_eq!(Balances::total_issuance() - issuance, delta);
            #[cfg(feature = "try-runtime")]
            EmissionController::try_state(now + 100).unwrap();
        });
    }
}

#[test]
fn rebase_keeps_missing_and_disabled_schedules_closed() {
    use frame_support::traits::StorageVersion;
    new_test_ext().execute_with(|| {
        StartAt::<Test>::kill();
        StorageVersion::new(1).put::<EmissionController>();
        let issuance = Balances::total_issuance();
        EmissionController::on_runtime_upgrade();
        assert_eq!(StartAt::<Test>::get(), None);
        assert_eq!(Balances::total_issuance(), issuance);
        EmissionController::accrue(1_800_000_000);
        assert_eq!(ScheduledIssued::<Test>::get(), 0);
        assert_eq!(StartAt::<Test>::get(), Some(1_800_000_000));
        Enabled::<Test>::kill();
        StorageVersion::new(0).put::<EmissionController>();
        EmissionController::on_runtime_upgrade();
        assert!(!Enabled::<Test>::get());
        assert_eq!(StartAt::<Test>::get(), Some(1_800_000_000));
        assert_eq!(Balances::total_issuance(), issuance);
    });
}

#[test]
fn invalid_rebase_does_not_change_state_or_mark_upgrade_complete() {
    use frame_support::traits::{GetStorageVersion, StorageVersion};
    new_test_ext().execute_with(|| {
        // A clock before the required virtual anchor cannot represent the rebase.
        AnnualEmission::set(40_000_000 * 1_000_000_000_000);
        EmissionController::accrue(100 + YEAR_SECONDS);
        AnnualEmission::set(30_000_000 * 1_000_000_000_000);
        StorageVersion::new(1).put::<EmissionController>();
        let issued = ScheduledIssued::<Test>::get();
        let issuance = Balances::total_issuance();
        #[cfg(feature = "try-runtime")]
        assert!(EmissionController::pre_upgrade().is_err());
        EmissionController::on_runtime_upgrade();
        assert_eq!(
            EmissionController::on_chain_storage_version(),
            StorageVersion::new(1)
        );
        assert_eq!(StartAt::<Test>::get(), Some(100));
        assert_eq!(ScheduledIssued::<Test>::get(), issued);
        assert_eq!(Balances::total_issuance(), issuance);
    });
}

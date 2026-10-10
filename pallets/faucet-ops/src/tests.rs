use crate::{mock::*, Error, Event};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::DispatchError;

#[test]
fn authority_can_mint_to_specified_account() {
    new_test_ext().execute_with(|| {
        assert_ok!(FaucetOps::mint(RuntimeOrigin::signed(1), 2, 500));

        assert_eq!(Balances::free_balance(2), 500);
        System::assert_last_event(
            Event::Minted {
                who: 2,
                amount: 500,
            }
            .into(),
        );
    });
}

#[test]
fn mint_accumulates_on_existing_account() {
    new_test_ext().execute_with(|| {
        assert_ok!(FaucetOps::mint(RuntimeOrigin::signed(1), 1, 250));

        assert_eq!(Balances::free_balance(1), 1_250);
    });
}

#[test]
fn non_authority_cannot_mint() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(3), 2, 500),
            DispatchError::BadOrigin
        );
    });
}

#[test]
fn mint_rejects_zero_amount() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 0),
            Error::<Test>::ZeroAmount
        );
        // No Minted event should have been emitted; the account stays empty.
        assert_eq!(Balances::free_balance(2), 0);
    });
}

#[test]
fn root_is_not_the_faucet_authority() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::root(), 2, 500),
            DispatchError::BadOrigin
        );
        assert_noop!(
            FaucetOps::disable(RuntimeOrigin::root()),
            DispatchError::BadOrigin
        );
    });
}
#[test]
fn fuse_is_one_way_and_survives_repeated_upgrade() {
    use frame_support::traits::Hooks;
    new_test_ext().execute_with(|| {
        assert_ok!(FaucetOps::disable(RuntimeOrigin::signed(1)));
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 1),
            Error::<Test>::Disabled
        );
        assert_noop!(
            FaucetOps::disable(RuntimeOrigin::signed(1)),
            Error::<Test>::Disabled
        );
        FaucetOps::on_runtime_upgrade();
        assert_eq!(
            crate::State::<Test>::get(),
            crate::FaucetState::PermanentlyDisabled
        );
    });
}
#[test]
fn absent_state_and_legacy_upgrade_fail_closed() {
    use frame_support::traits::{Hooks, StorageVersion};
    new_test_ext().execute_with(|| {
        crate::State::<Test>::kill();
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 1),
            Error::<Test>::Disabled
        );
        StorageVersion::new(0).put::<FaucetOps>();
        #[cfg(feature = "try-runtime")]
        let before = FaucetOps::pre_upgrade().unwrap();
        FaucetOps::on_runtime_upgrade();
        #[cfg(feature = "try-runtime")]
        FaucetOps::post_upgrade(before).unwrap();
        assert_eq!(crate::State::<Test>::get(), crate::FaucetState::Paused);
        assert!(!crate::Authority::<Test>::exists());
    });
}
#[test]
fn faucet_mints_only_within_controller_budget() {
    new_test_ext().execute_with(|| {
        let issuance = Balances::total_issuance();
        assert_ok!(FaucetOps::mint(RuntimeOrigin::signed(1), 2, 10_000));
        assert_eq!(Balances::total_issuance(), issuance + 10_000);
        assert_eq!(
            pallet_emission_controller::FaucetIssued::<Test>::get(),
            10_000
        );
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 1),
            pallet_emission_controller::Error::<Test>::BudgetExceeded
        );
    });
}

#[test]
fn disabled_genesis_cannot_be_enabled_by_its_authority() {
    use frame_support::traits::BuildGenesisConfig;
    new_test_ext().execute_with(|| {
        crate::GenesisConfig::<Test> {
            state: crate::FaucetState::PermanentlyDisabled,
            authority: Some(1),
        }
        .build();
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 500),
            Error::<Test>::Disabled
        );
        assert_noop!(
            FaucetOps::disable(RuntimeOrigin::signed(1)),
            Error::<Test>::Disabled
        );
        assert_eq!(pallet_emission_controller::FaucetIssued::<Test>::get(), 0);
    });
}

#[test]
fn authority_rotation_revocation_and_disabled_state_are_independent() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            FaucetOps::set_authority(RuntimeOrigin::signed(1), Some(3)),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(FaucetOps::set_authority(RuntimeOrigin::root(), Some(3)));
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 500),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(FaucetOps::mint(RuntimeOrigin::signed(3), 2, 500));
        assert_ok!(FaucetOps::set_authority(RuntimeOrigin::root(), None));
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(3), 2, 500),
            sp_runtime::DispatchError::BadOrigin
        );
        crate::State::<Test>::put(crate::FaucetState::PermanentlyDisabled);
        assert_ok!(FaucetOps::set_authority(RuntimeOrigin::root(), Some(1)));
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(1), 2, 500),
            Error::<Test>::Disabled
        );
        assert_eq!(pallet_emission_controller::FaucetIssued::<Test>::get(), 500);
        crate::State::<Test>::put(crate::FaucetState::Paused);
        assert_ok!(FaucetOps::set_authority(RuntimeOrigin::root(), Some(3)));
        assert_noop!(
            FaucetOps::mint(RuntimeOrigin::signed(3), 2, 500),
            Error::<Test>::Disabled
        );
    });
}

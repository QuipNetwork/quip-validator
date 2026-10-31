use super::*;
use crate::{
    AccountId, Address, Multisig, RuntimeCall, RuntimeGenesisConfig, RuntimeOrigin, Vesting, UNIT,
};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, traits::Currency, weights::Weight};
use sp_runtime::BuildStorage;

type Schedule = pallet_vesting::VestingInfo<Balance, BlockNumber>;
fn account(n: u8) -> AccountId {
    AccountId::new([n; 32])
}
fn ext() -> sp_io::TestExternalities {
    let mut ext =
        sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
    ext.execute_with(|| System::set_block_number(1));
    ext
}
fn fund(who: &AccountId, amount: Balance) {
    let _ = Balances::make_free_balance_be(who, amount);
}
fn grant(
    source: &AccountId,
    recipient: &AccountId,
    schedule: Schedule,
) -> sp_runtime::DispatchResult {
    Vesting::vested_transfer(
        RuntimeOrigin::signed(source.clone()),
        Address::Id(recipient.clone()),
        schedule,
    )
}
fn schedule_count(who: &AccountId) -> usize {
    pallet_vesting::Vesting::<Runtime>::get(who).map_or(0, |s| s.len())
}

#[test]
fn provisional_defaults_and_empty_genesis_are_explicit() {
    assert_eq!(
        (
            DEFAULT_CLIFF_BLOCKS,
            DEFAULT_LINEAR_BLOCKS,
            DEFAULT_TRANCHE_CADENCE_MONTHS
        ),
        (5_256_000, 15_768_000, 3)
    );
    assert_eq!(
        <Runtime as pallet_vesting::Config>::MAX_VESTING_SCHEDULES,
        28
    );
    assert_eq!(MinVestedTransfer::get(), 100 * EXISTENTIAL_DEPOSIT);
    ext().execute_with(|| {
        assert_eq!(pallet_vesting::Vesting::<Runtime>::iter().count(), 0);
        assert_eq!(
            pallet_vesting::StorageVersion::<Runtime>::get(),
            pallet_vesting::Releases::V1
        );
    });
}

#[test]
fn issued_tranche_locks_transfers_until_vested_without_minting() {
    ext().execute_with(|| {
        let source = account(51);
        let recipient = account(52);
        let target = account(53);
        fund(&source, 100 * UNIT);
        let issuance = Balances::total_issuance();
        assert_ok!(grant(
            &source,
            &recipient,
            Schedule::new(20 * UNIT, 2 * UNIT, 10)
        ));
        assert_eq!(Balances::total_issuance(), issuance);
        assert_eq!(Balances::free_balance(&source), 80 * UNIT);
        assert_eq!(schedule_count(&recipient), 1);
        for block in [1, 9, 10] {
            System::set_block_number(block);
            assert_ok!(Vesting::vest_other(
                RuntimeOrigin::signed(source.clone()),
                Address::Id(recipient.clone())
            ));
            frame_support::assert_storage_noop!(assert!(Balances::transfer_allow_death(
                RuntimeOrigin::signed(recipient.clone()),
                Address::Id(target.clone()),
                UNIT
            )
            .is_err()));
        }
        System::set_block_number(11);
        assert_ok!(Vesting::vest(RuntimeOrigin::signed(recipient.clone())));
        assert_ok!(Balances::transfer_allow_death(
            RuntimeOrigin::signed(recipient.clone()),
            Address::Id(target.clone()),
            2 * UNIT
        ));
        frame_support::assert_storage_noop!(assert!(Balances::transfer_allow_death(
            RuntimeOrigin::signed(recipient.clone()),
            Address::Id(target.clone()),
            UNIT
        )
        .is_err()));
        System::set_block_number(20);
        assert_ok!(Vesting::vest(RuntimeOrigin::signed(recipient.clone())));
        assert_eq!(schedule_count(&recipient), 0);
        assert_ok!(Balances::transfer_allow_death(
            RuntimeOrigin::signed(recipient),
            Address::Id(target),
            18 * UNIT
        ));
        // Withholding the next tranche leaves custody's remaining balance untouched.
        assert_eq!(Balances::free_balance(&source), 80 * UNIT);
        assert_eq!(Balances::total_issuance(), issuance);
    });
}

#[test]
fn invalid_tranches_do_not_debit_custody_or_create_schedules() {
    ext().execute_with(|| {
        let source = account(51);
        let recipient = account(52);
        fund(&source, 100 * UNIT);
        for schedule in [
            Schedule::new(0, 1, 10),
            Schedule::new(MinVestedTransfer::get() - 1, 1, 10),
            Schedule::new(UNIT, 0, 10),
            Schedule::new(101 * UNIT, UNIT, 10),
        ] {
            frame_support::assert_storage_noop!(assert!(
                grant(&source, &recipient, schedule).is_err()
            ));
            assert_eq!(schedule_count(&recipient), 0);
        }
        let minimum = MinVestedTransfer::get();
        assert_ok!(grant(
            &source,
            &recipient,
            Schedule::new(minimum, minimum, 10)
        ));
        assert_eq!(Balances::free_balance(&recipient), minimum);
        assert_eq!(schedule_count(&recipient), 1);
    });
}

#[test]
fn capacity_requires_cleanup_before_another_tranche() {
    ext().execute_with(|| {
        let source = account(51);
        let recipient = account(52);
        fund(&source, 100 * UNIT);
        for _ in 0..28 {
            assert_ok!(grant(&source, &recipient, Schedule::new(UNIT, UNIT, 10)));
        }
        assert_eq!(schedule_count(&recipient), 28);
        let next = Schedule::new(UNIT, UNIT, 20);
        assert_noop!(
            grant(&source, &recipient, next),
            pallet_vesting::Error::<Runtime>::AtMaxVestingSchedules
        );
        System::set_block_number(11);
        assert_noop!(
            grant(&source, &recipient, next),
            pallet_vesting::Error::<Runtime>::AtMaxVestingSchedules
        );
        assert_ok!(Vesting::vest_other(
            RuntimeOrigin::signed(source.clone()),
            Address::Id(recipient.clone())
        ));
        assert_eq!(schedule_count(&recipient), 0);
        assert_ok!(grant(&source, &recipient, next));
    });
}

#[test]
fn rounding_past_starts_and_recipient_merge_follow_stock_semantics() {
    ext().execute_with(|| {
        let source = account(51);
        let recipient = account(52);
        fund(&source, 100 * UNIT);
        let rounded = Schedule::new(10 * UNIT, 3 * UNIT, 10);
        assert_eq!(rounded.ending_block_as_balance::<ConvertInto>(), 14);
        assert_eq!(rounded.locked_at::<ConvertInto>(13), UNIT);
        System::set_block_number(20);
        assert_ok!(grant(&source, &recipient, rounded));
        assert_eq!(schedule_count(&recipient), 0); // entirely vested when issued
        assert_ok!(grant(
            &source,
            &recipient,
            Schedule::new(10 * UNIT, UNIT, 30)
        ));
        assert_ok!(grant(
            &source,
            &recipient,
            Schedule::new(10 * UNIT, UNIT, 40)
        ));
        assert_ok!(Vesting::merge_schedules(
            RuntimeOrigin::signed(recipient.clone()),
            0,
            1
        ));
        let merged = pallet_vesting::Vesting::<Runtime>::get(&recipient).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].starting_block(), 40);
        assert_eq!(merged[0].locked(), 20 * UNIT);
    });
}

#[test]
fn existing_validator_can_bond_a_vested_grant() {
    crate::configs::validator_onboarding::tests::ext().execute_with(|| {
        let stash = pallet_session::Validators::<Runtime>::get()[0].clone();
        let source = account(51);
        fund(&stash, EXISTENTIAL_DEPOSIT);
        let held = frame_system::Account::<Runtime>::get(&stash).data.reserved;
        let ledger = pallet_staking::Ledger::<Runtime>::get(&stash)
            .unwrap()
            .total;
        assert_eq!(held, ledger);
        assert!(held > UNIT);
        // A grant larger than the old stake makes the freeze bind after bond_extra.
        let amount = 2 * held + 20 * UNIT;
        fund(&source, amount + UNIT);
        assert_ok!(grant(&source, &stash, Schedule::new(amount, UNIT, 100)));
        let before = frame_system::Account::<Runtime>::get(&stash).data;
        assert_eq!(before.frozen, amount);
        assert_eq!(before.reserved, held);
        assert_eq!(before.free, amount + EXISTENTIAL_DEPOSIT);
        assert!(before.total() >= before.frozen);
        assert_ok!(crate::Staking::bond_extra(
            RuntimeOrigin::signed(stash.clone()),
            20 * UNIT
        ));
        let bonded = frame_system::Account::<Runtime>::get(&stash).data;
        assert_eq!(bonded.frozen, amount);
        assert_eq!(bonded.reserved, held + 20 * UNIT);
        assert_eq!(bonded.total(), before.total());
        assert!(bonded.total() >= bonded.frozen);
        assert_eq!(
            pallet_staking::Ledger::<Runtime>::get(&stash)
                .unwrap()
                .total,
            ledger + 20 * UNIT
        );
        // Held funds collateralise this transfer; the remainder exactly meets the freeze.
        assert_ok!(Balances::transfer_allow_death(
            RuntimeOrigin::signed(stash.clone()),
            Address::Id(source.clone()),
            held + EXISTENTIAL_DEPOSIT
        ));
        let after = frame_system::Account::<Runtime>::get(&stash).data;
        assert_eq!(after.total(), after.frozen);
        assert_eq!(after.frozen, amount);
        assert!(after.free > UNIT + EXISTENTIAL_DEPOSIT);
        // There is ample free balance and ED. Only the binding freeze rejects this.
        assert_noop!(
            Balances::transfer_allow_death(
                RuntimeOrigin::signed(stash.clone()),
                Address::Id(source),
                UNIT
            ),
            sp_runtime::TokenError::Frozen
        );
        assert_eq!(schedule_count(&stash), 1);
    });
}

#[test]
fn prebonded_stake_can_collateralise_an_immediately_liquid_grant() {
    crate::configs::validator_onboarding::tests::ext().execute_with(|| {
        let stash = pallet_session::Validators::<Runtime>::get()[0].clone();
        let source = account(51);
        fund(&stash, EXISTENTIAL_DEPOSIT);
        fund(&source, 100 * UNIT);
        let held = frame_system::Account::<Runtime>::get(&stash).data.reserved;
        let amount = 20 * UNIT;
        assert!(held >= amount);
        assert_ok!(grant(&source, &stash, Schedule::new(amount, UNIT, 100)));
        assert_eq!(
            <System as sp_runtime::traits::BlockNumberProvider>::current_block_number(),
            1
        );
        assert_eq!(
            frame_system::Account::<Runtime>::get(&stash).data.frozen,
            amount
        );
        assert_ok!(Balances::transfer_allow_death(
            RuntimeOrigin::signed(stash.clone()),
            Address::Id(source),
            amount
        ));
        let after = frame_system::Account::<Runtime>::get(&stash).data;
        assert_eq!(after.free, EXISTENTIAL_DEPOSIT);
        assert_eq!(after.reserved, held);
        assert_eq!(after.frozen, amount);
        assert!(after.total() >= after.frozen);
        assert_eq!(schedule_count(&stash), 1);
    });
}

#[test]
fn fresh_recipient_can_initially_bond_from_a_future_start_grant() {
    ext().execute_with(|| {
        let source = account(51);
        let recipient = account(52);
        fund(&source, 100 * UNIT);
        assert_ok!(grant(
            &source,
            &recipient,
            Schedule::new(20 * UNIT, UNIT, 100)
        ));
        assert!(!pallet_staking::Ledger::<Runtime>::contains_key(&recipient));
        assert_ok!(crate::Staking::bond(
            RuntimeOrigin::signed(recipient.clone()),
            10 * UNIT,
            pallet_staking::RewardDestination::Staked
        ));
        let after = frame_system::Account::<Runtime>::get(&recipient).data;
        assert_eq!(
            <System as sp_runtime::traits::BlockNumberProvider>::current_block_number(),
            1
        );
        assert_eq!(after.free, 10 * UNIT);
        assert_eq!(after.reserved, 10 * UNIT);
        assert_eq!(after.frozen, 20 * UNIT);
        assert_eq!(after.total(), after.frozen);
        assert_eq!(
            pallet_staking::Ledger::<Runtime>::get(&recipient)
                .unwrap()
                .active,
            10 * UNIT
        );
        assert_noop!(
            Balances::transfer_allow_death(
                RuntimeOrigin::signed(recipient.clone()),
                Address::Id(source),
                UNIT
            ),
            sp_runtime::TokenError::Frozen
        );
        assert_eq!(schedule_count(&recipient), 1);
    });
}

#[test]
fn council_cannot_force_vesting_directly_or_through_wrappers() {
    ext().execute_with(|| {
        let source = account(51);
        let recipient = account(52);
        fund(&source, 100 * UNIT);
        assert_ok!(grant(&source, &recipient, Schedule::new(20 * UNIT, UNIT, 100)));
        let calls = [
            RuntimeCall::Vesting(pallet_vesting::Call::force_vested_transfer {
                source: Address::Id(source.clone()),
                target: Address::Id(recipient.clone()),
                schedule: Schedule::new(UNIT, UNIT, 100),
            }),
            RuntimeCall::Vesting(pallet_vesting::Call::force_remove_vesting_schedule {
                target: Address::Id(recipient),
                schedule_index: 0,
            }),
        ];
        for denied in calls {
            let wrappers = [
                denied.clone(),
                RuntimeCall::Utility(pallet_utility::Call::batch_all {
                    calls: vec![denied.clone()],
                }),
                RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
                    as_origin: Box::new(frame_system::RawOrigin::Root.into()),
                    call: Box::new(denied.clone()),
                }),
                RuntimeCall::ValidatorAdmission(pallet_validator_admission::Call::dispatch_as_root {
                    call: Box::new(denied.clone()),
                }),
                RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                    real: Address::Id(source.clone()),
                    force_proxy_type: None,
                    call: Box::new(denied.clone()),
                }),
                RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
                    delegate: Address::Id(source.clone()),
                    real: Address::Id(source.clone()),
                    force_proxy_type: None,
                    call: Box::new(denied.clone()),
                }),
                RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
                    other_signatories: vec![source.clone()],
                    call: Box::new(denied.clone()),
                }),
                RuntimeCall::Multisig(pallet_multisig::Call::as_multi {
                    threshold: 2,
                    other_signatories: vec![source.clone()],
                    maybe_timepoint: None,
                    call: Box::new(denied.clone()),
                    max_weight: Weight::MAX,
                }),
                RuntimeCall::Revive(pallet_revive::Call::eth_substrate_call {
                    call: Box::new(denied.clone()),
                    transaction_encoded: vec![],
                }),
                RuntimeCall::Revive(pallet_revive::Call::dispatch_as_fallback_account {
                    call: Box::new(denied),
                }),
            ];
            for call in wrappers {
                let council = pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(4, 6).into();
                assert_noop!(
                    crate::ValidatorAdmission::dispatch_as_root(council, Box::new(call)),
                    pallet_validator_admission::Error::<Runtime>::RootCallDenied
                );
            }
        }
    });
}

#[test]
fn custody_multisig_issues_only_after_threshold_and_checks_inner_result() {
    ext().execute_with(|| {
        let a = account(51);
        let b = account(52);
        let recipient = account(53);
        let custody = Multisig::multi_account_id(&[a.clone(), b.clone()], 2);
        fund(&b, 10 * UNIT);
        fund(&custody, 100 * UNIT);
        let call = RuntimeCall::Vesting(pallet_vesting::Call::vested_transfer {
            target: Address::Id(recipient.clone()),
            schedule: Schedule::new(20 * UNIT, UNIT, 100),
        });
        let hash = sp_io::hashing::blake2_256(&call.encode());
        frame_support::assert_storage_noop!(assert!(Multisig::as_multi(
            RuntimeOrigin::signed(a.clone()),
            2,
            vec![b.clone()],
            None,
            Box::new(call.clone()),
            Weight::MAX
        )
        .is_err()));
        fund(&a, 10 * UNIT);
        let issuance = Balances::total_issuance();
        assert_ok!(Multisig::as_multi(
            RuntimeOrigin::signed(a.clone()),
            2,
            vec![b.clone()],
            None,
            Box::new(call.clone()),
            Weight::MAX
        ));
        assert_eq!(Balances::free_balance(&custody), 100 * UNIT);
        assert_eq!(schedule_count(&recipient), 0);
        let pending = pallet_multisig::Multisigs::<Runtime>::get(&custody, hash).unwrap();
        assert!(Balances::reserved_balance(&a) > 0);
        assert_ok!(Multisig::as_multi(
            RuntimeOrigin::signed(b.clone()),
            2,
            vec![a.clone()],
            Some(pending.when),
            Box::new(call),
            Weight::MAX
        ));
        System::assert_last_event(
            pallet_multisig::Event::<Runtime>::MultisigExecuted {
                approving: account(52),
                timepoint: pending.when,
                multisig: custody.clone(),
                call_hash: hash,
                result: Ok(()),
            }
            .into(),
        );
        assert_eq!(Balances::free_balance(&custody), 80 * UNIT);
        assert_eq!(Balances::free_balance(&recipient), 20 * UNIT);
        assert_eq!(Balances::reserved_balance(&a), 0);
        assert_eq!(Balances::total_issuance(), issuance);
        assert_eq!(schedule_count(&recipient), 1);
        let next = RuntimeCall::Vesting(pallet_vesting::Call::vested_transfer {
            target: Address::Id(recipient.clone()),
            schedule: Schedule::new(UNIT, UNIT, 200),
        });
        let next_hash = sp_io::hashing::blake2_256(&next.encode());
        assert_ok!(Multisig::as_multi(
            RuntimeOrigin::signed(a.clone()),
            2,
            vec![b.clone()],
            None,
            Box::new(next),
            Weight::MAX
        ));
        let pending = pallet_multisig::Multisigs::<Runtime>::get(&custody, next_hash).unwrap();
        assert_ok!(Multisig::cancel_as_multi(
            RuntimeOrigin::signed(a.clone()),
            2,
            vec![b],
            pending.when,
            next_hash
        ));
        assert!(pallet_multisig::Multisigs::<Runtime>::get(&custody, next_hash).is_none());
        assert_eq!(Balances::reserved_balance(&a), 0);
        assert_eq!(Balances::free_balance(&custody), 80 * UNIT);
        assert_eq!(schedule_count(&recipient), 1);
    });
}

#[cfg(feature = "runtime-benchmarks")]
#[test]
fn benchmark_api_lists_every_vesting_weight_path() {
    use codec::Decode;
    ext().execute_with(|| {
        let data =
            crate::apis::api::dispatch("Benchmark_benchmark_metadata", &false.encode()).unwrap();
        let (list, _) = <(
            Vec<frame_benchmarking::BenchmarkList>,
            Vec<frame_support::traits::StorageInfo>,
        )>::decode(&mut &data[..])
        .unwrap();
        let entry = list.iter().find(|x| x.pallet == b"pallet_vesting").unwrap();
        for name in [
            "vest_locked",
            "vest_unlocked",
            "vest_other_locked",
            "vest_other_unlocked",
            "vested_transfer",
            "force_vested_transfer",
            "not_unlocking_merge_schedules",
            "unlocking_merge_schedules",
            "force_remove_vesting_schedule",
        ] {
            assert!(
                entry.benchmarks.iter().any(|x| x.name == name.as_bytes()),
                "missing {name}"
            );
        }
    });
}

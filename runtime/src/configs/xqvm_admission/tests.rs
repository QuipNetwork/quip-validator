use crate::{
    AccountId, Address, Balances, Runtime, RuntimeCall, RuntimeEvent, RuntimeOrigin, System, UNIT,
};
use codec::Encode;
use frame_support::{
    assert_noop, assert_ok,
    traits::{Contains, Currency},
    weights::Weight,
};
use sp_runtime::{
    traits::{Dispatchable, Hash as _},
    BuildStorage, DispatchError,
};

fn ext() -> sp_io::TestExternalities {
    let mut ext = sp_io::TestExternalities::new(
        crate::RuntimeGenesisConfig::default()
            .build_storage()
            .unwrap(),
    );
    ext.execute_with(|| {
        System::set_block_number(1);
        for n in [1, 2] {
            let _ = <Balances as Currency<AccountId>>::make_free_balance_be(
                &AccountId::new([n; 32]),
                100 * UNIT,
            );
        }
    });
    ext
}

fn upload_call() -> RuntimeCall {
    let program = xqvm::Program::new(vec![0xff]);
    assert!(xqvm::verifier::verify(&program).is_ok());
    RuntimeCall::Xqvm(pallet_xqvm::Call::store_program {
        bytecode: program.encode().try_into().unwrap(),
    })
}

fn filtered() -> DispatchError {
    frame_system::Error::<Runtime>::CallFiltered.into()
}

fn dispatch(who: &AccountId, call: RuntimeCall) -> Result<(), DispatchError> {
    call.dispatch(RuntimeOrigin::signed(who.clone()))
        .map(|_| ())
        .map_err(|e| e.error)
}

fn guarded_upload_call() -> RuntimeCall {
    let call = upload_call();
    // Check the configured filter before exercising wrapper dispatch paths.
    assert!(!<Runtime as frame_system::Config>::BaseCallFilter::contains(&call));
    call
}

#[test]
fn store_program_refused_before_decode_or_verification() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        assert_noop!(dispatch(&who, guarded_upload_call()), filtered());
        // Even an invalid container must report the filter error, not InvalidBytecode.
        assert_noop!(
            dispatch(
                &who,
                RuntimeCall::Xqvm(pallet_xqvm::Call::store_program {
                    bytecode: vec![0xff].try_into().unwrap()
                })
            ),
            filtered()
        );
        assert_eq!(pallet_xqvm::Programs::<Runtime>::iter().count(), 0);
        assert_ok!(dispatch(
            &who,
            RuntimeCall::System(frame_system::Call::remark { remark: vec![] })
        ));
    });
}

#[test]
fn signed_utility_paths_refuse_program_storage() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        let call = guarded_upload_call();
        for outer in [
            pallet_utility::Call::batch_all {
                calls: vec![call.clone()],
            },
            pallet_utility::Call::as_derivative {
                index: 0,
                call: Box::new(call.clone()),
            },
            pallet_utility::Call::if_else {
                main: Box::new(call.clone()),
                fallback: Box::new(call.clone()),
            },
        ] {
            assert_noop!(dispatch(&who, RuntimeCall::Utility(outer)), filtered());
        }
        let batch = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![call.clone()],
        });
        assert_ok!(dispatch(&who, batch));
        assert!(System::events().iter().any(|e| matches!(
            &e.event,
            RuntimeEvent::Utility(pallet_utility::Event::BatchInterrupted { error, .. })
                if *error == filtered()
        )));
        let force_batch =
            RuntimeCall::Utility(pallet_utility::Call::force_batch { calls: vec![call] });
        assert_ok!(dispatch(&who, force_batch));
        assert!(System::events().iter().any(|e| matches!(
            &e.event,
            RuntimeEvent::Utility(pallet_utility::Event::ItemFailed { error })
                if *error == filtered()
        )));
    });
}

#[test]
fn proxy_and_announced_proxy_refuse_program_storage() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        let delegate = AccountId::new([2; 32]);
        let call = guarded_upload_call();
        assert_ok!(crate::Proxy::add_proxy(
            RuntimeOrigin::signed(who.clone()),
            Address::Id(delegate.clone()),
            super::super::ProxyType::Any,
            0,
        ));
        for announced in [false, true] {
            System::reset_events();
            let outer = if announced {
                assert_ok!(crate::Proxy::announce(
                    RuntimeOrigin::signed(delegate.clone()),
                    Address::Id(who.clone()),
                    sp_io::hashing::blake2_256(&call.encode()).into(),
                ));
                RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
                    delegate: Address::Id(delegate.clone()),
                    real: Address::Id(who.clone()),
                    force_proxy_type: None,
                    call: Box::new(call.clone()),
                })
            } else {
                RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                    real: Address::Id(who.clone()),
                    force_proxy_type: None,
                    call: Box::new(call.clone()),
                })
            };
            assert_ok!(dispatch(&delegate, outer));
            assert!(System::events().iter().any(|e| matches!(
                &e.event,
                RuntimeEvent::Proxy(pallet_proxy::Event::ProxyExecuted { result })
                    if *result == Err(filtered())
            )));
        }
    });
}

#[test]
fn multisig_refuses_program_storage() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        let other = AccountId::new([2; 32]);
        let call = guarded_upload_call();
        let threshold_one = RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
            other_signatories: vec![other.clone()],
            call: Box::new(call.clone()),
        });
        assert_noop!(dispatch(&who, threshold_one), filtered());
        let timepoint = crate::Multisig::timepoint();
        assert_ok!(crate::Multisig::as_multi(
            RuntimeOrigin::signed(who.clone()),
            2,
            vec![other.clone()],
            None,
            Box::new(call.clone()),
            Weight::MAX,
        ));
        assert_ok!(crate::Multisig::as_multi(
            RuntimeOrigin::signed(other),
            2,
            vec![who],
            Some(timepoint),
            Box::new(call),
            Weight::MAX,
        ));
        assert!(System::events().iter().any(|e| matches!(
            &e.event,
            RuntimeEvent::Multisig(pallet_multisig::Event::MultisigExecuted { result, .. })
                if *result == Err(filtered())
        )));
    });
}

#[test]
fn revive_dispatch_paths_refuse_program_storage() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        let call = guarded_upload_call();
        assert_ok!(crate::Revive::eth_substrate_call(
            pallet_revive::Origin::<Runtime>::EthTransaction(who.clone()).into(),
            Box::new(call.clone()),
            vec![],
        ));
        assert!(System::events().iter().any(|e| matches!(
            &e.event,
            RuntimeEvent::Revive(pallet_revive::Event::EthExtrinsicRevert { dispatch_error })
                if *dispatch_error == filtered()
        )));
        let result =
            crate::Revive::dispatch_as_fallback_account(RuntimeOrigin::signed(who), Box::new(call));
        assert_eq!(result.unwrap_err().error, filtered());
    });
}

#[test]
fn existing_program_execution_and_job_registration_still_work() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        let bytes = xqvm::Program::new(vec![0xff]).encode();
        let hash = <Runtime as frame_system::Config>::Hashing::hash(&bytes);
        // Model a program admitted before the pause; do not bypass the filter to store it.
        let bounded = frame_support::BoundedVec::<
            u8,
            <Runtime as pallet_xqvm::Config>::MaxProgramSize,
        >::try_from(bytes)
        .unwrap();
        pallet_xqvm::Programs::<Runtime>::insert(hash, bounded);
        let execute = RuntimeCall::Xqvm(pallet_xqvm::Call::execute {
            program_hash: hash,
            calldata: Default::default(),
            output_slots: 0,
            step_limit: 10,
        });
        assert_ok!(dispatch(&who, execute));
        let call = RuntimeCall::QuantumComputeMempool(
            pallet_quantum_compute_mempool::Call::register_job_spec {
                builder: who,
                name: b"existing".to_vec().try_into().unwrap(),
                formulation: pallet_quantum_compute_mempool::types::Formulation::Ising,
                validation_program: Some(hash),
                transform_program: Some(hash),
            },
        );
        assert!(<Runtime as frame_system::Config>::BaseCallFilter::contains(
            &call
        ));
        assert_ok!(call.dispatch(RuntimeOrigin::root()));
    });
}

#[test]
fn raw_root_utility_bypass_is_governance_only() {
    ext().execute_with(|| {
        let who = AccountId::new([1; 32]);
        for fallible in [false, true] {
            // Use a benign valid program to demonstrate the accepted Root bypass.
            let program = if fallible {
                xqvm::Program::new(vec![0x11, 0x00, 0xff])
            } else {
                xqvm::Program::new(vec![0xff])
            };
            let call = Box::new(RuntimeCall::Xqvm(pallet_xqvm::Call::store_program {
                bytecode: program.encode().try_into().unwrap(),
            }));
            let as_origin = Box::new(frame_system::RawOrigin::Signed(who.clone()).into());
            let outer = RuntimeCall::Utility(if fallible {
                pallet_utility::Call::dispatch_as_fallible { as_origin, call }
            } else {
                pallet_utility::Call::dispatch_as { as_origin, call }
            });
            assert_noop!(dispatch(&who, outer.clone()), DispatchError::BadOrigin);
            assert_ok!(outer.dispatch(RuntimeOrigin::root()));
        }
        assert_eq!(pallet_xqvm::Programs::<Runtime>::iter().count(), 2);
    });
}

#[test]
fn block_builder_application_dispatches_through_the_filter() {
    use quip_transaction_crypto::{account_id_from_public, HybridPair, HybridTxSignature};
    use sp_core::Pair as _;
    ext().execute_with(|| {
        let pair = HybridPair::from_string("//Alice", None).unwrap();
        let who = account_id_from_public(&pair.public());
        let _ = <Balances as Currency<AccountId>>::make_free_balance_be(&who, 100 * UNIT);
        let call = guarded_upload_call();
        let extra = crate::native_tx_extension(sp_runtime::generic::Era::Immortal, 0, 0);
        let payload = crate::SignedPayload::new(call.clone(), extra.clone()).unwrap();
        let signature = payload.using_encoded(|encoded| HybridTxSignature::sign(&pair, encoded));
        let xt: crate::UncheckedExtrinsic = sp_runtime::generic::UncheckedExtrinsic::new_signed(
            call,
            Address::Id(who),
            signature,
            extra,
        )
        .into();
        // runtime/apis.rs exposes this exact Executive method as BlockBuilder_apply_extrinsic.
        assert_eq!(crate::Executive::apply_extrinsic(xt), Ok(Err(filtered())));
        assert_eq!(pallet_xqvm::Programs::<Runtime>::iter().count(), 0);
    });
}

#[cfg(feature = "try-runtime")]
#[test]
fn runtime_upgrade_hooks_accept_existing_runtime_state() {
    ext().execute_with(|| {
        let mut old = crate::VERSION;
        old.spec_version = 120;
        frame_system::LastRuntimeUpgrade::<Runtime>::put(
            frame_system::LastRuntimeUpgradeInfo::from(old),
        );
        assert_ok!(crate::Executive::try_runtime_upgrade(
            frame_try_runtime::UpgradeCheckSelect::PreAndPost
        ));
        assert_eq!(
            frame_system::LastRuntimeUpgrade::<Runtime>::get()
                .unwrap()
                .spec_version
                .0,
            121
        );
    });
}

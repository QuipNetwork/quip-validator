use super::*;
use crate::{AccountId, Address, BoxedSessionKeys, RuntimeEvent, RuntimeOrigin, System};
use codec::Encode;
use frame_support::{assert_noop, assert_ok, weights::Weight};
use sp_consensus_babe::{
    digests::{PreDigest, SecondaryPlainPreDigest},
    Slot,
};
use sp_core::{proof_of_possession::ProofOfPossessionGenerator, Pair as _};
use sp_runtime::{
    traits::{Dispatchable, Header as _},
    DispatchError,
};

fn ext() -> sp_io::TestExternalities {
    super::super::validator_onboarding::tests::ext()
}

fn validator() -> AccountId {
    quip_transaction_crypto::account_id_from_public(
        &quip_transaction_crypto::HybridPair::from_string("//Alice", None)
            .unwrap()
            .public(),
    )
}

fn keys(who: &AccountId) -> BoxedSessionKeys {
    pallet_session::NextKeys::<Runtime>::get(who).unwrap()
}

fn set(keys: BoxedSessionKeys) -> RuntimeCall {
    use quip_crypto_primitives::substrate::{ed25519_fndsa512, sr25519_fndsa512};
    let owner = validator().encode();
    let proof = (
        sr25519_fndsa512::Pair::from_string("//Alice", None)
            .unwrap()
            .generate_proof_of_possession(&owner),
        ed25519_fndsa512::Pair::from_string("//Alice", None)
            .unwrap()
            .generate_proof_of_possession(&owner),
    )
        .encode();
    RuntimeCall::Session(pallet_session::Call::set_keys { keys, proof })
}

fn purge() -> RuntimeCall {
    RuntimeCall::Session(pallet_session::Call::purge_keys {})
}

fn filtered() -> DispatchError {
    frame_system::Error::<Runtime>::CallFiltered.into()
}

fn dispatch(who: &AccountId, call: RuntimeCall) -> Result<(), DispatchError> {
    call.dispatch(RuntimeOrigin::signed(who.clone()))
        .map(|_| ())
        .map_err(|e| e.error)
}

fn at_offset(offset: u64) {
    pallet_babe::GenesisSlot::<Runtime>::put(Slot::from(1_000));
    pallet_babe::EpochIndex::<Runtime>::put(0);
    pallet_babe::CurrentSlot::<Runtime>::put(Slot::from(1_000 + offset));
}

fn assert_open(who: &AccountId) {
    let original = keys(who);
    assert_ok!(dispatch(who, purge()));
    assert!(!pallet_session::NextKeys::<Runtime>::contains_key(who));
    assert_ok!(dispatch(who, set(original.clone())));
    assert_eq!(keys(who), original);
}

fn assert_closed(who: &AccountId) {
    let original = keys(who);
    assert_noop!(dispatch(who, set(original.clone())), filtered());
    assert_noop!(dispatch(who, purge()), filtered());
    assert_eq!(keys(who), original);
}

fn start_block(number: u32, slot: u64) {
    let digest = PreDigest::SecondaryPlain(SecondaryPlainPreDigest {
        authority_index: 0,
        slot: Slot::from(slot),
    });
    let header = crate::Header::new(
        number,
        Default::default(),
        Default::default(),
        Default::default(),
        sp_runtime::Digest {
            logs: vec![sp_runtime::DigestItem::PreRuntime(
                sp_consensus_babe::BABE_ENGINE_ID,
                digest.encode(),
            )],
        },
    );
    crate::Executive::initialize_block(&header);
    assert_ok!(crate::Timestamp::set(
        RuntimeOrigin::none(),
        slot * crate::SLOT_DURATION
    ));
}

#[test]
fn cutoff_assumptions_match_both_networks_and_genesis_babe_configuration() {
    assert_eq!(KEY_CHANGE_CUTOFF_SLOTS, 64);
    assert!(matches!(EpochDuration::get(), 600 | 2_400));
    assert!(KEY_CHANGE_CUTOFF_SLOTS < EpochDuration::get());
    assert_eq!(crate::BABE_GENESIS_EPOCH_CONFIG.c, (1, 4));
    assert_eq!(
        crate::BABE_GENESIS_EPOCH_CONFIG.allowed_slots,
        sp_consensus_babe::AllowedSlots::PrimaryAndSecondaryPlainSlots
    );
    assert_eq!(crate::VERSION.spec_version, 120);
    assert_eq!(crate::VERSION.transaction_version, 7);
}

#[test]
fn both_key_calls_reject_exact_cutoff_and_every_remaining_slot() {
    ext().execute_with(|| {
        let who = validator();
        let cutoff = EpochDuration::get() - KEY_CHANGE_CUTOFF_SLOTS;
        at_offset(cutoff - 1);
        assert_open(&who);
        for offset in cutoff..EpochDuration::get() {
            at_offset(offset);
            assert_closed(&who);
        }
        // Unrelated calls stay available throughout the cutoff.
        assert_ok!(dispatch(
            &who,
            RuntimeCall::System(frame_system::Call::remark { remark: vec![] })
        ));
    });
}

#[test]
fn genesis_keys_and_real_block_one_accept_offset_zero() {
    ext().execute_with(|| {
        System::set_block_number(0);
        let who = validator();
        assert!(pallet_session::NextKeys::<Runtime>::contains_key(&who));
        start_block(1, 1_000);
        assert_eq!(
            pallet_babe::GenesisSlot::<Runtime>::get(),
            Slot::from(1_000)
        );
        assert_eq!(pallet_babe::EpochIndex::<Runtime>::get(), 0);
        assert_eq!(Babe::current_epoch_start(), Slot::from(1_000));
        assert_open(&who);
        crate::Executive::finalize_block();
    });
}

#[test]
fn real_session_rotation_reopens_next_epoch_and_handles_skipped_epochs() {
    for (epochs, offset, open) in [
        (1, 0, true),
        (4, 0, true),
        (4, EpochDuration::get() - 64, false),
    ] {
        ext().execute_with(|| {
            System::set_block_number(0);
            let who = validator();
            start_block(1, 1_000);
            crate::Executive::finalize_block();
            let old_session = pallet_session::CurrentIndex::<Runtime>::get();
            let slot = 1_000 + epochs * EpochDuration::get() + offset;
            start_block(2, slot);
            assert_eq!(pallet_babe::EpochIndex::<Runtime>::get(), epochs);
            assert_eq!(
                pallet_session::CurrentIndex::<Runtime>::get(),
                old_session + 1
            );
            assert_eq!(Babe::current_epoch_start(), Slot::from(slot - offset));
            if open {
                assert_open(&who);
            } else {
                assert_closed(&who);
            }
            crate::Executive::finalize_block();
        });
    }
}

#[test]
fn inconsistent_epoch_state_fails_closed_without_arithmetic_panics() {
    ext().execute_with(|| {
        let who = validator();
        at_offset(0);
        pallet_babe::CurrentSlot::<Runtime>::put(Slot::from(999));
        assert_closed(&who);
        at_offset(EpochDuration::get()); // epoch index was not advanced
        assert_closed(&who);
        at_offset(0);
        pallet_babe::EpochIndex::<Runtime>::put(u64::MAX);
        assert_closed(&who);
    });
}

#[test]
fn utility_wrappers_dispatch_through_the_filter() {
    ext().execute_with(|| {
        let who = validator();
        at_offset(EpochDuration::get()-64);
        for inner in [set(keys(&who)), purge()] {
            assert_noop!(dispatch(&who, RuntimeCall::Utility(pallet_utility::Call::batch_all {
                calls: vec![inner.clone()]
            })), filtered());
            assert_noop!(dispatch(&who, RuntimeCall::Utility(pallet_utility::Call::as_derivative {
                index: 0, call: Box::new(inner.clone())
            })), filtered());
            assert_ok!(dispatch(&who, RuntimeCall::Utility(pallet_utility::Call::batch {
                calls: vec![inner.clone()]
            })));
            assert!(System::events().iter().any(|e| matches!(&e.event,
                RuntimeEvent::Utility(pallet_utility::Event::BatchInterrupted { error, .. }) if *error == filtered())));
            System::reset_events();
            assert_ok!(dispatch(&who, RuntimeCall::Utility(pallet_utility::Call::force_batch {
                calls: vec![inner.clone()]
            })));
            assert!(System::events().iter().any(|e| matches!(&e.event,
                RuntimeEvent::Utility(pallet_utility::Event::ItemFailed { error }) if *error == filtered())));
            System::reset_events();
            assert_noop!(dispatch(&who, RuntimeCall::Utility(pallet_utility::Call::if_else {
                main: Box::new(inner.clone()), fallback: Box::new(inner)
            })), filtered());
        }
    });
}

#[test]
fn proxy_and_announced_proxy_dispatch_through_the_filter() {
    ext().execute_with(|| {
        let who = validator();
        let delegate = pallet_session::Validators::<Runtime>::get().into_iter().find(|v| *v != who).unwrap();
        assert_ok!(crate::Proxy::add_proxy(RuntimeOrigin::signed(who.clone()),
            Address::Id(delegate.clone()), super::super::ProxyType::Any, 0));
        at_offset(EpochDuration::get()-64);
        for inner in [set(keys(&who)), purge()] {
            for announced in [false, true] {
                System::reset_events();
                let call = if announced {
                    assert_ok!(crate::Proxy::announce(RuntimeOrigin::signed(delegate.clone()),
                        Address::Id(who.clone()), sp_io::hashing::blake2_256(&inner.encode()).into()));
                    RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
                        delegate: Address::Id(delegate.clone()), real: Address::Id(who.clone()),
                        force_proxy_type: None, call: Box::new(inner.clone())
                    })
                } else {
                    RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                        real: Address::Id(who.clone()), force_proxy_type: None,
                        call: Box::new(inner.clone())
                    })
                };
                assert_ok!(dispatch(&delegate, call));
                assert!(System::events().iter().any(|e| matches!(&e.event,
                    RuntimeEvent::Proxy(pallet_proxy::Event::ProxyExecuted { result }) if *result == Err(filtered()))));
                assert!(pallet_session::NextKeys::<Runtime>::contains_key(&who));
            }
        }
    });
}

#[test]
fn multisig_threshold_one_and_completed_threshold_two_are_filtered() {
    ext().execute_with(|| {
        let who = validator();
        let other = pallet_session::Validators::<Runtime>::get().into_iter().find(|v| *v != who).unwrap();
        at_offset(EpochDuration::get()-64);
        for inner in [set(keys(&who)), purge()] {
            assert_noop!(dispatch(&who, RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
                other_signatories: vec![other.clone()], call: Box::new(inner.clone())
            })), filtered());
            let timepoint = crate::Multisig::timepoint();
            assert_ok!(crate::Multisig::as_multi(RuntimeOrigin::signed(who.clone()), 2,
                vec![other.clone()], None, Box::new(inner.clone()), Weight::MAX));
            System::reset_events();
            assert_ok!(crate::Multisig::as_multi(RuntimeOrigin::signed(other.clone()), 2,
                vec![who.clone()], Some(timepoint), Box::new(inner), Weight::MAX));
            assert!(System::events().iter().any(|e| matches!(&e.event,
                RuntimeEvent::Multisig(pallet_multisig::Event::MultisigExecuted { result, .. }) if *result == Err(filtered()))));
        }
    });
}

#[test]
fn revive_eth_runtime_dispatch_and_fallback_dispatch_are_filtered() {
    ext().execute_with(|| {
        let who = validator();
        let original = keys(&who);
        at_offset(EpochDuration::get() - 65);
        // This is the actual inner path used for transactions addressed to
        // RUNTIME_PALLETS_ADDR, after the EVM layer validates its signer.
        assert_ok!(crate::Revive::eth_substrate_call(
            pallet_revive::Origin::<Runtime>::EthTransaction(who.clone()).into(),
            Box::new(set(original.clone())),
            vec![]
        ));
        assert!(!System::events().iter().any(|e| matches!(
            &e.event,
            RuntimeEvent::Revive(pallet_revive::Event::EthExtrinsicRevert { .. })
        )));
        at_offset(EpochDuration::get() - 64);
        for inner in [set(original.clone()), purge()] {
            System::reset_events();
            // Ethereum dispatch reports inner errors as a revert event while
            // the outer extrinsic returns Ok; checking only Ok is unsound.
            assert_ok!(crate::Revive::eth_substrate_call(
                pallet_revive::Origin::<Runtime>::EthTransaction(who.clone()).into(),
                Box::new(inner.clone()),
                vec![]
            ));
            assert!(System::events().iter().any(|e| matches!(&e.event,
                RuntimeEvent::Revive(pallet_revive::Event::EthExtrinsicRevert { dispatch_error })
                    if *dispatch_error == filtered())));
            assert_eq!(
                crate::Revive::dispatch_as_fallback_account(
                    RuntimeOrigin::signed(who.clone()),
                    Box::new(inner)
                )
                .unwrap_err()
                .error,
                filtered()
            );
        }
        assert_eq!(keys(&who), original);
    });
}

fn reoriginated(who: &AccountId, inner: RuntimeCall) -> RuntimeCall {
    RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
        as_origin: Box::new(frame_system::RawOrigin::Signed(who.clone()).into()),
        call: Box::new(inner),
    })
}

#[test]
fn council_cannot_reoriginate_key_changes_during_the_cutoff() {
    use pallet_validator_admission::RootCallPolicy;
    ext().execute_with(|| {
        let who = validator();
        let original = keys(&who);
        let council = || {
            pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(4, 6)
                .into()
        };
        at_offset(EpochDuration::get() - 64);
        for inner in [set(original.clone()), purge()] {
            let nested = RuntimeCall::Utility(pallet_utility::Call::batch_all {
                calls: vec![reoriginated(&who, inner.clone())],
            });
            for call in [inner, nested] {
                assert_eq!(
                    super::super::slashing::CouncilRootCalls::check(&call),
                    Err(pallet_validator_admission::RootCallError::Denied)
                );
                assert_noop!(
                    crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(call)),
                    pallet_validator_admission::Error::<Runtime>::RootCallDenied
                );
            }
        }
        at_offset(EpochDuration::get() - 65);
        assert_ok!(crate::ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(reoriginated(&who, purge()))
        ));
        assert!(!pallet_session::NextKeys::<Runtime>::contains_key(&who));
        assert_ok!(crate::ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(reoriginated(&who, set(original)))
        ));
        assert!(pallet_session::NextKeys::<Runtime>::contains_key(&who));
    });
}

#[test]
fn raw_root_bypass_exists_but_a_signed_validator_cannot_obtain_it() {
    ext().execute_with(|| {
        let who = validator();
        at_offset(EpochDuration::get() - 64);
        let call = reoriginated(&who, purge());
        assert_noop!(dispatch(&who, call.clone()), DispatchError::BadOrigin);
        assert_ok!(call.dispatch(RuntimeOrigin::root()));
        assert!(!pallet_session::NextKeys::<Runtime>::contains_key(&who));
    });
}

#[test]
fn foundation_execute_and_immediate_propose_cannot_bypass_the_filter() {
    ext().execute_with(|| {
        let who = validator();
        let member =
            pallet_collective::Members::<Runtime, pallet_collective::Instance1>::get()[0].clone();
        at_offset(EpochDuration::get() - 64);
        for inner in [set(keys(&who)), purge()] {
            for propose in [false, true] {
                System::reset_events();
                if propose {
                    assert_ok!(crate::Foundation::propose(
                        RuntimeOrigin::signed(member.clone()),
                        1,
                        Box::new(inner.clone()),
                        inner.encoded_size() as u32
                    ));
                } else {
                    assert_ok!(crate::Foundation::execute(
                        RuntimeOrigin::signed(member.clone()),
                        Box::new(inner.clone()),
                        inner.encoded_size() as u32
                    ));
                }
                assert!(System::events().iter().any(|e| match &e.event {
                    RuntimeEvent::Foundation(
                        pallet_collective::Event::Executed { result, .. }
                        | pallet_collective::Event::MemberExecuted { result, .. },
                    ) => *result == Err(filtered()),
                    _ => false,
                }));
            }
        }
    });
}

#[test]
fn council_policy_covers_all_root_utility_wrappers() {
    use pallet_validator_admission::{RootCallError, RootCallPolicy};
    ext().execute_with(|| {
        let who = validator();
        at_offset(EpochDuration::get() - 64);
        for inner in [set(keys(&who)), purge()] {
            let signed = || Box::new(frame_system::RawOrigin::Signed(who.clone()).into());
            let calls = vec![
                pallet_utility::Call::batch {
                    calls: vec![inner.clone()],
                },
                pallet_utility::Call::batch_all {
                    calls: vec![inner.clone()],
                },
                pallet_utility::Call::force_batch {
                    calls: vec![inner.clone()],
                },
                pallet_utility::Call::as_derivative {
                    index: 0,
                    call: Box::new(inner.clone()),
                },
                pallet_utility::Call::dispatch_as {
                    as_origin: signed(),
                    call: Box::new(inner.clone()),
                },
                pallet_utility::Call::dispatch_as_fallible {
                    as_origin: signed(),
                    call: Box::new(inner.clone()),
                },
                pallet_utility::Call::with_weight {
                    weight: Weight::MAX,
                    call: Box::new(inner.clone()),
                },
                pallet_utility::Call::if_else {
                    main: Box::new(inner.clone()),
                    fallback: Box::new(inner),
                },
            ];
            for call in calls {
                assert_eq!(
                    super::super::slashing::CouncilRootCalls::check(&RuntimeCall::Utility(call)),
                    Err(RootCallError::Denied)
                );
            }
        }
    });
}

#[test]
fn session_and_council_weights_include_the_added_filter_reads() {
    use pallet_session::WeightInfo;
    type Configured = <Runtime as pallet_session::Config>::WeightInfo;
    type Sdk = pallet_session::weights::SubstrateWeight<Runtime>;
    assert_eq!(
        Configured::set_keys(),
        Sdk::set_keys().saturating_add(filter_weight())
    );
    assert_eq!(
        Configured::purge_keys(),
        Sdk::purge_keys().saturating_add(filter_weight())
    );
    assert_eq!(
        super::super::slashing::RootFilterWeight::get(),
        Weight::from_parts(50_000_000, 0).saturating_add(filter_weight())
    );
    assert!(filter_weight().proof_size() >= 8_000);
}

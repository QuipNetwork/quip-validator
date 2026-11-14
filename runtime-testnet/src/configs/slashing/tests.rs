use super::*;
use crate::{Babe, Header, RuntimeCall, RuntimeOrigin, UncheckedExtrinsic};
use codec::{Decode, Encode};
use frame_support::{assert_ok, traits::KeyOwnerProofSystem};
use sp_consensus_babe::{
    digests::{CompatibleDigestItem, PreDigest, SecondaryPlainPreDigest},
    AuthorityPair, EquivocationProof, Slot,
};
use sp_core::{Pair, H256};
use sp_runtime::{
    traits::{Dispatchable, Header as _},
    transaction_validity::TransactionSource,
    Digest, DigestItem,
};

fn ext() -> sp_io::TestExternalities {
    let mut ext = super::super::validator_onboarding::tests::ext();
    ext.execute_with(|| {
        for _ in 0..12 {
            next_session();
            if pallet_staking::ActiveEra::<Runtime>::get().is_some_and(|e| e.index >= 1) {
                break;
            }
        }
        assert_eq!(
            pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
            1
        );
    });
    ext
}

fn api<T: Decode>(method: &str, args: impl Encode) -> T {
    let result = crate::apis::api::dispatch(method, &args.encode()).expect("runtime API exists");
    T::decode(&mut &result[..]).expect("runtime API result decodes")
}

fn babe_evidence() -> EquivocationProof<Header> {
    let pair = AuthorityPair::from_string("//Alice", None).unwrap();
    let slot = pallet_babe::GenesisSlot::<Runtime>::get()
        + Slot::from(
            pallet_babe::EpochIndex::<Runtime>::get() * super::super::EpochDuration::get() + 1,
        );
    let make_header = |root: u8| {
        let pre_digest = PreDigest::SecondaryPlain(SecondaryPlainPreDigest {
            authority_index: pallet_babe::Authorities::<Runtime>::get()
                .iter()
                .position(|(key, _)| *key == pair.public())
                .unwrap() as u32,
            slot,
        });
        let mut header = Header::new(
            1,
            H256::repeat_byte(root),
            H256::zero(),
            H256::zero(),
            Digest {
                logs: vec![DigestItem::babe_pre_digest(pre_digest)],
            },
        );
        let signature = pair.sign(header.hash().as_ref());
        header.digest_mut().push(DigestItem::babe_seal(signature));
        header
    };
    EquivocationProof {
        slot,
        offender: pair.public(),
        first_header: make_header(1),
        second_header: make_header(2),
    }
}

#[test]
fn babe_runtime_api_reports_real_hybrid_equivocation_and_queues_slash() {
    let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
    let mut ext = ext();
    ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
    ext.execute_with(|| {
        let evidence = babe_evidence();
        assert!(sp_consensus_babe::check_equivocation_proof(
            evidence.clone()
        ));
        let proof: Option<sp_consensus_babe::OpaqueKeyOwnershipProof> = api(
            "BabeApi_generate_key_ownership_proof",
            (evidence.slot, evidence.offender.clone()),
        );
        let proof: sp_session::MembershipProof = proof.unwrap().decode().unwrap();
        let (_, proof_exposure) = crate::Historical::check_proof(
            (sp_consensus_babe::KEY_TYPE, evidence.offender.clone()),
            proof.clone(),
        )
        .unwrap();
        // Identity proofs intentionally carry zero exposure. The queue below uses actual era stake.
        assert_eq!(proof_exposure, sp_staking::Exposure::default());
        let submitted: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (
                evidence,
                sp_consensus_babe::OpaqueKeyOwnershipProof::new(proof.encode()),
            ),
        );
        assert_eq!(submitted, Some(()));
        let transactions = &state.read().transactions;
        assert_eq!(transactions.len(), 1);
        let extrinsic = UncheckedExtrinsic::decode(&mut &transactions[0][..]).unwrap();
        let RuntimeCall::Babe(call) = &extrinsic.0.function else {
            panic!("expected BABE report")
        };
        let validity = Babe::validate_unsigned(TransactionSource::Local, call).unwrap();
        assert_eq!(validity.longevity, ReportLongevity::get());
        assert!(!validity.propagate);
        assert_ok!(extrinsic.0.function.clone().dispatch(RuntimeOrigin::none()));
        assert_eq!(pallet_offences::Reports::<Runtime>::iter().count(), 1);
        let era = pallet_staking::ActiveEra::<Runtime>::get().unwrap().index;
        let slashes = pallet_staking::UnappliedSlashes::<Runtime>::get(era + 28);
        assert_eq!(slashes.len(), 1);
        assert_eq!(
            slashes[0].validator,
            quip_transaction_crypto::account_id_from_public(
                &quip_transaction_crypto::HybridPair::from_string("//Alice", None)
                    .unwrap()
                    .public()
            )
        );
        assert!(slashes[0].own > 0);
        assert!(Babe::validate_unsigned(TransactionSource::Local, call).is_err());
    });
}

#[test]
fn runtime_api_rejects_malformed_membership_proof() {
    let mut ext = ext();
    ext.execute_with(|| {
        let submitted: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (
                babe_evidence(),
                sp_consensus_babe::OpaqueKeyOwnershipProof::new(vec![255]),
            ),
        );
        assert_eq!(submitted, None);
        assert_eq!(pallet_offences::Reports::<Runtime>::iter().count(), 0);
    });
}

fn grandpa_evidence() -> sp_consensus_grandpa::EquivocationProof<H256, u32> {
    use sp_consensus_grandpa::{AuthorityPair, Equivocation, EquivocationProof};
    let pair = AuthorityPair::from_string("//Alice", None).unwrap();
    let round = 1;
    let set_id = pallet_grandpa::CurrentSetId::<Runtime>::get();
    let vote = |root| {
        let prevote = finality_grandpa::Prevote {
            target_hash: H256::repeat_byte(root),
            target_number: 1u32,
        };
        let payload = sp_consensus_grandpa::localized_payload(
            round,
            set_id,
            &finality_grandpa::Message::Prevote(prevote.clone()),
        );
        (prevote, pair.sign(&payload))
    };
    let evidence = EquivocationProof::new(
        set_id,
        Equivocation::Prevote(finality_grandpa::Equivocation {
            round_number: round,
            identity: pair.public(),
            first: vote(1),
            second: vote(2),
        }),
    );
    evidence
}

#[test]
fn grandpa_runtime_api_reports_real_hybrid_equivocation_and_queues_slash() {
    use sp_consensus_grandpa::OpaqueKeyOwnershipProof;
    let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
    let mut ext = ext();
    ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
    ext.execute_with(|| {
        let evidence = grandpa_evidence();
        let set_id = evidence.set_id();
        assert!(sp_consensus_grandpa::check_equivocation_proof(
            evidence.clone()
        ));
        let proof: Option<OpaqueKeyOwnershipProof> = api(
            "GrandpaApi_generate_key_ownership_proof",
            (set_id, evidence.offender().clone()),
        );
        let proof = proof.unwrap();
        let (_, proof_exposure) = crate::Historical::check_proof(
            (sp_consensus_grandpa::KEY_TYPE, evidence.offender().clone()),
            proof.decode().unwrap(),
        )
        .unwrap();
        // Identity proofs intentionally carry zero exposure. The queue below uses actual era stake.
        assert_eq!(proof_exposure, sp_staking::Exposure::default());
        let submitted: Option<()> = api(
            "GrandpaApi_submit_report_equivocation_unsigned_extrinsic",
            (evidence, proof),
        );
        assert_eq!(submitted, Some(()));
        let transactions = &state.read().transactions;
        assert_eq!(transactions.len(), 1);
        let extrinsic = UncheckedExtrinsic::decode(&mut &transactions[0][..]).unwrap();
        let RuntimeCall::Grandpa(call) = &extrinsic.0.function else {
            panic!("expected GRANDPA report")
        };
        assert_ok!(crate::Grandpa::validate_unsigned(
            TransactionSource::Local,
            call
        ));
        assert_ok!(extrinsic.0.function.dispatch(RuntimeOrigin::none()));
        assert_eq!(pallet_offences::Reports::<Runtime>::iter().count(), 1);
        let era = pallet_staking::ActiveEra::<Runtime>::get().unwrap().index;
        let slashes = pallet_staking::UnappliedSlashes::<Runtime>::get(era + 28);
        assert_eq!(slashes.len(), 1);
        assert!(slashes[0].own > 0);
    });
}

fn next_session() {
    use crate::{BlockNumber, System, Timestamp};
    use frame_support::{traits::Hooks, weights::WeightMeter};
    let index = pallet_session::CurrentIndex::<Runtime>::get() + 1;
    let block = index * super::super::EpochDuration::get() as u32;
    System::set_block_number(block);
    pallet_babe::CurrentSlot::<Runtime>::put(Slot::from(u64::from(block)));
    pallet_babe::Initialized::<Runtime>::put(None::<PreDigest>);
    Timestamp::set_timestamp(1 + u64::from(block) * 6_000);
    crate::Session::rotate_session();
    <crate::Grandpa as Hooks<BlockNumber>>::on_finalize(block);
    <Staking as Hooks<BlockNumber>>::on_finalize(block);
    for offset in 1..=3 {
        System::set_block_number(block + offset);
        let mut meter = WeightMeter::with_limit(super::super::RuntimeBlockWeights::get().max_block);
        <crate::MultiBlockElection as Hooks<BlockNumber>>::on_poll(block + offset, &mut meter);
    }
}

fn advance_to_era(target: u32) {
    for _ in 0..200 {
        if pallet_staking::ActiveEra::<Runtime>::get().unwrap().index >= target {
            break;
        }
        next_session();
    }
    assert_eq!(
        pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
        target
    );
}

#[test]
fn deferred_consensus_slash_is_applied_at_scheduled_era_and_burns_issuance() {
    use frame_support::traits::KeyOwnerProofSystem;
    ext().execute_with(|| {
        let proof = babe_evidence();
        let owner = crate::Historical::prove((sp_consensus_babe::KEY_TYPE, proof.offender.clone()))
            .unwrap();
        let (stash, _) = crate::Historical::check_proof(
            (sp_consensus_babe::KEY_TYPE, proof.offender.clone()),
            owner.clone(),
        )
        .unwrap();
        assert_ok!(Babe::report_equivocation_unsigned(
            RuntimeOrigin::none(),
            Box::new(proof),
            owner
        ));
        let apply_era = 29;
        assert_eq!(
            pallet_staking::UnappliedSlashes::<Runtime>::get(apply_era).len(),
            1
        );
        let before = crate::Balances::total_issuance();
        let stake = Staking::slashable_balance_of(&stash);
        advance_to_era(apply_era - 1);
        assert_eq!(Staking::slashable_balance_of(&stash), stake);
        advance_to_era(apply_era);
        assert!(pallet_staking::UnappliedSlashes::<Runtime>::get(apply_era).is_empty());
        assert_eq!(Staking::slashable_balance_of(&stash), 0);
        assert_eq!(before - crate::Balances::total_issuance(), stake);
        // Automatic slashing does not remove the admission permission.
        assert!(pallet_validator_admission::Approved::<Runtime>::get().contains(&stash));
    });
}

#[test]
fn processed_slash_bucket_report_is_rejected_before_any_slash_accounting() {
    use frame_support::assert_noop;
    let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
    let mut ext = ext();
    ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
    ext.execute_with(|| {
        let babe = babe_evidence();
        let babe_owner =
            crate::Historical::prove((sp_consensus_babe::KEY_TYPE, babe.offender.clone())).unwrap();
        let grandpa = grandpa_evidence();
        let grandpa_owner =
            crate::Historical::prove((sp_consensus_grandpa::KEY_TYPE, grandpa.offender().clone()))
                .unwrap();
        // Last safe reporting era: the bucket is still in the future.
        advance_to_era(28);
        assert!(has_pending_slash_bucket(babe_owner.session));
        let result: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (
                babe.clone(),
                sp_consensus_babe::OpaqueKeyOwnershipProof::new(babe_owner.encode()),
            ),
        );
        assert_eq!(result, Some(()));
        let result: Option<()> = api(
            "GrandpaApi_submit_report_equivocation_unsigned_extrinsic",
            (
                grandpa.clone(),
                sp_consensus_grandpa::OpaqueKeyOwnershipProof::new(grandpa_owner.encode()),
            ),
        );
        assert_eq!(result, Some(()));
        assert_eq!(state.read().transactions.len(), 2);
        // The next rotation has already drained bucket29 before any extrinsic can report.
        advance_to_era(29);
        assert_eq!(
            pallet_staking::BondedEras::<Runtime>::get()
                .first()
                .unwrap()
                .0,
            29u32.saturating_sub(BondingDuration::get())
        );
        assert!(!has_pending_slash_bucket(babe_owner.session));
        let result: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (
                babe.clone(),
                sp_consensus_babe::OpaqueKeyOwnershipProof::new(babe_owner.encode()),
            ),
        );
        assert_eq!(result, None);
        let result: Option<()> = api(
            "GrandpaApi_submit_report_equivocation_unsigned_extrinsic",
            (
                grandpa.clone(),
                sp_consensus_grandpa::OpaqueKeyOwnershipProof::new(grandpa_owner.encode()),
            ),
        );
        assert_eq!(result, None);
        // These reports were submitted while age27 was valid. SDK pool validation
        // still admits them at age28, but dispatch must reject without any slash writes.
        let transactions = &state.read().transactions;
        assert_eq!(transactions.len(), 2);
        for encoded in transactions {
            let extrinsic = UncheckedExtrinsic::decode(&mut &encoded[..]).unwrap();
            let expected: sp_runtime::DispatchError = match &extrinsic.0.function {
                RuntimeCall::Babe(call) => {
                    assert_ok!(Babe::validate_unsigned(TransactionSource::Local, call));
                    pallet_babe::Error::<Runtime>::DuplicateOffenceReport.into()
                }
                RuntimeCall::Grandpa(call) => {
                    assert_ok!(crate::Grandpa::validate_unsigned(
                        TransactionSource::Local,
                        call
                    ));
                    pallet_grandpa::Error::<Runtime>::DuplicateOffenceReport.into()
                }
                _ => panic!("expected consensus report"),
            };
            assert_noop!(
                extrinsic
                    .0
                    .function
                    .dispatch(RuntimeOrigin::none())
                    .map_err(|e| e.error),
                expected
            );
        }
        // Bypass the APIs: dispatch must still reject without modifying deduplication/max/span state.
        // SDK consensus pallets map every reporter error to DuplicateOffenceReport.
        assert_noop!(
            Babe::report_equivocation_unsigned(
                RuntimeOrigin::none(),
                Box::new(babe.clone()),
                babe_owner.clone()
            ),
            pallet_babe::Error::<Runtime>::DuplicateOffenceReport
        );
        assert_noop!(
            Babe::report_equivocation(
                RuntimeOrigin::signed(crate::AccountId::new([9; 32])),
                Box::new(babe),
                babe_owner
            ),
            pallet_babe::Error::<Runtime>::DuplicateOffenceReport
        );
        assert_noop!(
            crate::Grandpa::report_equivocation_unsigned(
                RuntimeOrigin::none(),
                Box::new(grandpa.clone()),
                grandpa_owner.clone()
            ),
            pallet_grandpa::Error::<Runtime>::DuplicateOffenceReport
        );
        assert_noop!(
            crate::Grandpa::report_equivocation(
                RuntimeOrigin::signed(crate::AccountId::new([9; 32])),
                Box::new(grandpa),
                grandpa_owner
            ),
            pallet_grandpa::Error::<Runtime>::DuplicateOffenceReport
        );
        assert_eq!(pallet_offences::Reports::<Runtime>::iter().count(), 0);
        assert!(pallet_staking::UnappliedSlashes::<Runtime>::get(29).is_empty());
        assert_eq!(
            pallet_staking::ValidatorSlashInEra::<Runtime>::iter().count(),
            0
        );
        assert_eq!(pallet_staking::SpanSlash::<Runtime>::iter().count(), 0);
    });
}

fn council() -> RuntimeOrigin {
    pallet_collective::RawOrigin::<crate::AccountId, pallet_collective::Instance1>::Members(4, 6)
        .into()
}

#[test]
fn council_cannot_bypass_manual_slash_filter_through_nested_wrappers() {
    use frame_support::{assert_noop, weights::Weight};
    ext().execute_with(|| {
        let stash = pallet_session::Validators::<Runtime>::get()[0].clone();
        let denied = RuntimeCall::Staking(pallet_staking::Call::manual_slash {
            validator_stash: stash.clone(),
            era: 1,
            slash_fraction: sp_runtime::Perbill::from_percent(100),
        });
        assert_noop!(
            Staking::manual_slash(council(), stash, 1, sp_runtime::Perbill::from_percent(100)),
            sp_runtime::DispatchError::BadOrigin
        );
        let remark = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
        let wrappers = vec![
            denied.clone(),
            RuntimeCall::ValidatorAdmission(pallet_validator_admission::Call::dispatch_as_root {
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Revive(pallet_revive::Call::eth_substrate_call {
                transaction_encoded: vec![],
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Revive(pallet_revive::Call::dispatch_as_fallback_account {
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Foundation(pallet_collective::Call::execute {
                proposal: Box::new(denied.clone()),
                length_bound: u32::MAX,
            }),
            RuntimeCall::Foundation(pallet_collective::Call::propose {
                threshold: 4,
                proposal: Box::new(denied.clone()),
                length_bound: u32::MAX,
            }),
            RuntimeCall::Proxy(pallet_proxy::Call::proxy {
                real: crate::Address::Id(crate::AccountId::new([3; 32])),
                force_proxy_type: None,
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Proxy(pallet_proxy::Call::proxy_announced {
                delegate: crate::Address::Id(crate::AccountId::new([4; 32])),
                real: crate::Address::Id(crate::AccountId::new([3; 32])),
                force_proxy_type: None,
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Multisig(pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: vec![crate::AccountId::new([3; 32])],
                maybe_timepoint: None,
                call: Box::new(denied.clone()),
                max_weight: Weight::MAX,
            }),
            RuntimeCall::Multisig(pallet_multisig::Call::as_multi_threshold_1 {
                other_signatories: vec![crate::AccountId::new([3; 32])],
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Utility(pallet_utility::Call::batch {
                calls: vec![denied.clone()],
            }),
            RuntimeCall::Utility(pallet_utility::Call::batch_all {
                calls: vec![denied.clone()],
            }),
            RuntimeCall::Utility(pallet_utility::Call::force_batch {
                calls: vec![denied.clone()],
            }),
            RuntimeCall::Utility(pallet_utility::Call::as_derivative {
                index: 0,
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
                as_origin: Box::new(frame_system::RawOrigin::Root.into()),
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Utility(pallet_utility::Call::dispatch_as_fallible {
                as_origin: Box::new(frame_system::RawOrigin::Root.into()),
                call: Box::new(denied.clone()),
            }),
            RuntimeCall::Utility(pallet_utility::Call::with_weight {
                call: Box::new(denied.clone()),
                weight: Weight::zero(),
            }),
            RuntimeCall::Utility(pallet_utility::Call::if_else {
                main: Box::new(remark.clone()),
                fallback: Box::new(denied.clone()),
            }),
            RuntimeCall::Utility(pallet_utility::Call::if_else {
                main: Box::new(denied),
                fallback: Box::new(remark.clone()),
            }),
        ];
        for call in wrappers {
            // Two nested wrappers exercise recursion as well as each immediate variant.
            let nested =
                RuntimeCall::Utility(pallet_utility::Call::batch_all { calls: vec![call] });
            assert!(!is_allowed(&nested));
            assert_noop!(
                crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(nested)),
                pallet_validator_admission::Error::<Runtime>::RootCallDenied
            );
        }
        assert_ok!(crate::ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(remark)
        ));
    });
}

#[test]
fn root_filter_retains_slash_veto_and_unstake_and_bounds_traversal() {
    let stash = crate::AccountId::new([1; 32]);
    assert!(is_allowed(&RuntimeCall::Staking(
        pallet_staking::Call::force_unstake {
            stash,
            num_slashing_spans: 0
        }
    )));
    assert!(is_allowed(&RuntimeCall::Staking(
        pallet_staking::Call::cancel_deferred_slash {
            era: 1,
            slash_indices: vec![0]
        }
    )));
    let remark = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
    let deep = (0..9).fold(remark.clone(), |call, _| {
        RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![call] })
    });
    assert!(!is_allowed(&deep));
    assert!(!is_allowed(&RuntimeCall::Utility(
        pallet_utility::Call::batch {
            calls: vec![remark; 256]
        }
    )));
}

#[test]
fn report_one_era_before_application_is_still_accepted_and_applied() {
    for babe in [true, false] {
        let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
        let mut ext = ext();
        ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
        ext.execute_with(|| {
            // Preserve valid era1 evidence and its ownership proof across the rotations.
            let (method, encoded) = if babe {
                let proof = babe_evidence();
                let owner =
                    crate::Historical::prove((sp_consensus_babe::KEY_TYPE, proof.offender.clone()))
                        .unwrap();
                (
                    "BabeApi_submit_report_equivocation_unsigned_extrinsic",
                    (
                        proof,
                        sp_consensus_babe::OpaqueKeyOwnershipProof::new(owner.encode()),
                    )
                        .encode(),
                )
            } else {
                let proof = grandpa_evidence();
                let owner = crate::Historical::prove((
                    sp_consensus_grandpa::KEY_TYPE,
                    proof.offender().clone(),
                ))
                .unwrap();
                (
                    "GrandpaApi_submit_report_equivocation_unsigned_extrinsic",
                    (
                        proof,
                        sp_consensus_grandpa::OpaqueKeyOwnershipProof::new(owner.encode()),
                    )
                        .encode(),
                )
            };
            advance_to_era(28);
            let result = crate::apis::api::dispatch(method, &encoded).unwrap();
            assert_eq!(Option::<()>::decode(&mut &result[..]).unwrap(), Some(()));
            let transactions = &state.read().transactions;
            assert_eq!(transactions.len(), 1);
            let extrinsic = UncheckedExtrinsic::decode(&mut &transactions[0][..]).unwrap();
            assert_ok!(extrinsic.0.function.dispatch(RuntimeOrigin::none()));
            assert_eq!(pallet_offences::Reports::<Runtime>::iter().count(), 1);
            assert_eq!(
                pallet_staking::UnappliedSlashes::<Runtime>::get(29).len(),
                1
            );
            let before = crate::Balances::total_issuance();
            advance_to_era(29);
            assert!(pallet_staking::UnappliedSlashes::<Runtime>::get(29).is_empty());
            assert!(crate::Balances::total_issuance() < before);
        });
    }
}

#[test]
fn council_wrapper_defaults_schedule_and_root_path_can_cancel_then_force_unstake() {
    use crate::{SlashPolicy, ValidatorAdmission};
    use frame_support::assert_noop;
    use sp_runtime::Perbill;
    ext().execute_with(|| {
        let stash = pallet_session::Validators::<Runtime>::get()[0].clone();
        assert_eq!(
            pallet_slash_policy::MaxDiscretionarySlashPerMotion::<Runtime>::get(),
            Perbill::from_percent(1)
        );
        assert_eq!(
            pallet_slash_policy::MaxDiscretionarySlashCumulative::<Runtime>::get(),
            Perbill::from_percent(4)
        );
        assert_eq!(
            pallet_slash_policy::BudgetWindow::<Runtime>::get(),
            BondingDuration::get()
        );
        assert_eq!(pallet_slash_policy::BudgetWindow::<Runtime>::get(), 30);
        let minority: RuntimeOrigin = pallet_collective::RawOrigin::<
            crate::AccountId,
            pallet_collective::Instance1,
        >::Members(3, 5)
        .into();
        assert_noop!(
            SlashPolicy::discretionary_slash(minority, stash.clone(), 1, Perbill::from_percent(1)),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(SlashPolicy::discretionary_slash(
            council(),
            stash.clone(),
            1,
            Perbill::from_percent(1)
        ));
        crate::System::assert_last_event(
            pallet_slash_policy::Event::SlashScheduled {
                stash: stash.clone(),
                offence_era: 1,
                apply_era: 29,
                fraction: Perbill::from_percent(1),
                remaining_budget: Perbill::from_percent(3),
            }
            .into(),
        );
        assert_eq!(
            pallet_staking::UnappliedSlashes::<Runtime>::get(29).len(),
            1
        );
        assert_ok!(ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(RuntimeCall::Staking(
                pallet_staking::Call::cancel_deferred_slash {
                    era: 29,
                    slash_indices: vec![0]
                }
            ))
        ));
        crate::System::assert_last_event(
            pallet_validator_admission::Event::RootDispatched { result: Ok(()) }.into(),
        );
        assert!(pallet_staking::UnappliedSlashes::<Runtime>::get(29).is_empty());
        assert_eq!(
            pallet_slash_policy::SlashHistory::<Runtime>::get(&stash).as_slice(),
            &[(1, Perbill::from_percent(1))]
        );
        let spans = pallet_staking::SlashingSpans::<Runtime>::get(&stash)
            .unwrap()
            .prior
            .len() as u32
            + 1;
        assert_ok!(ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(RuntimeCall::Staking(pallet_staking::Call::force_unstake {
                stash: stash.clone(),
                num_slashing_spans: spans
            }))
        ));
        crate::System::assert_last_event(
            pallet_validator_admission::Event::RootDispatched { result: Ok(()) }.into(),
        );
        assert!(!pallet_staking::Bonded::<Runtime>::contains_key(&stash));
    });
}

fn is_allowed(call: &RuntimeCall) -> bool {
    <CouncilRootCalls as pallet_validator_admission::RootCallPolicy<RuntimeCall>>::check(call)
        .is_ok()
}

#[test]
fn raw_storage_is_refused_bare_and_nested_without_changing_any_state() {
    use frame_support::assert_noop;
    ext().execute_with(|| {
        let key = pallet_slash_policy::BudgetWindow::<Runtime>::hashed_key().to_vec();
        let calls = vec![
            RuntimeCall::System(frame_system::Call::set_storage {
                items: vec![(key.clone(), 1u32.encode())],
            }),
            RuntimeCall::System(frame_system::Call::kill_storage {
                keys: vec![key.clone()],
            }),
            RuntimeCall::System(frame_system::Call::kill_prefix {
                prefix: key,
                subkeys: 1,
            }),
        ];
        for call in calls {
            assert_noop!(
                crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(call.clone())),
                pallet_validator_admission::Error::<Runtime>::RootCallDenied
            );
            let nested = RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![call] });
            assert_noop!(
                crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(nested)),
                pallet_validator_admission::Error::<Runtime>::RootCallDenied
            );
        }
    });
}

#[test]
fn root_dispatch_reports_distinct_depth_and_call_count_limits() {
    use frame_support::assert_noop;
    ext().execute_with(|| {
        let remark = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
        let at_limit = (0..8).fold(remark.clone(), |call, _| {
            RuntimeCall::Utility(pallet_utility::Call::batch { calls: vec![call] })
        });
        assert_ok!(crate::ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(at_limit.clone())
        ));
        let beyond = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![at_limit],
        });
        assert_noop!(
            crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(beyond)),
            pallet_validator_admission::Error::<Runtime>::RootCallTooDeep
        );
        let too_many = RuntimeCall::Utility(pallet_utility::Call::batch {
            calls: vec![remark; 256],
        });
        assert_noop!(
            crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(too_many)),
            pallet_validator_admission::Error::<Runtime>::RootCallTooLarge
        );
    });
}

#[test]
fn root_upgrade_and_heap_paths_remain_allowed() {
    let mut ext = ext();
    ext.register_extension(sp_core::traits::ReadRuntimeVersionExt::new(
        sc_executor::WasmExecutor::<sp_io::SubstrateHostFunctions>::builder().build(),
    ));
    ext.execute_with(|| {
        let code = vec![];
        let code_hash = H256::repeat_byte(1);
        for call in [
            frame_system::Call::set_code { code: code.clone() },
            frame_system::Call::set_code_without_checks { code: code.clone() },
            frame_system::Call::authorize_upgrade { code_hash },
            frame_system::Call::authorize_upgrade_without_checks { code_hash },
            frame_system::Call::apply_authorized_upgrade { code },
            frame_system::Call::set_heap_pages { pages: 64 },
            frame_system::Call::remark { remark: vec![] },
            frame_system::Call::remark_with_event { remark: vec![] },
        ] {
            assert!(is_allowed(&RuntimeCall::System(call)));
        }
        // Empty code reaches the checked SDK upgrade path and is rejected there, not by policy.
        assert_ok!(crate::ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(RuntimeCall::System(frame_system::Call::set_code {
                code: vec![]
            }))
        ));
        crate::System::assert_last_event(
            pallet_validator_admission::Event::RootDispatched {
                result: Err(frame_system::Error::<Runtime>::FailedToExtractRuntimeVersion.into()),
            }
            .into(),
        );
        assert_ok!(crate::ValidatorAdmission::dispatch_as_root(
            council(),
            Box::new(RuntimeCall::System(frame_system::Call::set_heap_pages {
                pages: 64
            }))
        ));
        crate::System::assert_last_event(
            pallet_validator_admission::Event::RootDispatched { result: Ok(()) }.into(),
        );
    });
}

// Ported from the QUIP-22 audit handover; exercises the real BABE hooks.
// Append this test to quip-validator/runtime/src/configs/slashing/tests.rs.
// It reuses that module's pinned runtime genesis and genuine BABE equivocation helpers.
fn vv_rehearsal_ext() -> sp_io::TestExternalities {
    use crate::{RuntimeGenesisConfig, System};
    use sp_runtime::BuildStorage;

    fn merge(target: &mut serde_json::Value, patch: serde_json::Value) {
        match (target, patch) {
            (serde_json::Value::Object(t), serde_json::Value::Object(p)) => {
                for (k, v) in p {
                    merge(t.entry(k).or_insert(serde_json::Value::Null), v);
                }
            }
            (t, p) => *t = p,
        }
    }
    let mut config = serde_json::to_value(RuntimeGenesisConfig::default()).unwrap();
    merge(
        &mut config,
        crate::genesis_config_presets::rehearsal_config_genesis(),
    );
    let config: RuntimeGenesisConfig = serde_json::from_value(config).unwrap();
    let mut ext = sp_io::TestExternalities::new(config.build_storage().unwrap());
    ext.execute_with(|| {
        System::set_block_number(1);
        for _ in 0..12 {
            next_session();
            if pallet_staking::ActiveEra::<Runtime>::get().is_some_and(|e| e.index >= 1) {
                break;
            }
        }
        assert_eq!(
            pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
            1
        );
    });
    ext
}

fn vv_assigned_secondary_slot(authority_index: u32, after: Slot) -> Slot {
    let authorities = pallet_babe::Authorities::<Runtime>::get();
    let randomness = pallet_babe::Randomness::<Runtime>::get();
    (1..=super::super::EpochDuration::get())
        .map(|delta| after + Slot::from(delta))
        .find(|slot| {
            let hash = (randomness, *slot).using_encoded(sp_core::hashing::blake2_256);
            let assigned =
                sp_core::U256::from_big_endian(&hash) % sp_core::U256::from(authorities.len());
            assigned == sp_core::U256::from(authority_index)
        })
        .expect("offender has a later secondary slot in this epoch")
}

#[test]
fn vv_poc_offence_disable_bypass() {
    use crate::{Session, System};
    use frame_support::traits::Hooks;

    let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
    let mut ext = vv_rehearsal_ext();
    ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
    ext.execute_with(|| {
        assert!(
            Session::validators().len() >= 4,
            "disable limit needs four validators"
        );
        let evidence = babe_evidence();
        let offender_index = pallet_babe::Authorities::<Runtime>::get()
            .iter()
            .position(|(key, _)| *key == evidence.offender)
            .unwrap() as u32;
        let later_slot = vv_assigned_secondary_slot(offender_index, evidence.slot);
        let proof: Option<sp_consensus_babe::OpaqueKeyOwnershipProof> = api(
            "BabeApi_generate_key_ownership_proof",
            (evidence.slot, evidence.offender.clone()),
        );
        let submitted: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (evidence, proof.expect("ownership proof")),
        );
        assert_eq!(submitted, Some(()));
        let transactions = &state.read().transactions;
        assert_eq!(transactions.len(), 1);
        let extrinsic = UncheckedExtrinsic::decode(&mut &transactions[0][..]).unwrap();
        assert_ok!(extrinsic.0.function.dispatch(RuntimeOrigin::none()));
        assert!(Session::disabled_validators().contains(&offender_index));

        // Initialize a later block through the real System and BABE hooks with
        // the offender's BABE pre-digest. The session helper skips the prior
        // BABE finalizer, so clear its temporary state first.
        pallet_babe::Initialized::<Runtime>::kill();
        let next_block = System::block_number() + 1;
        let digest = Digest {
            logs: vec![DigestItem::babe_pre_digest(PreDigest::SecondaryPlain(
                SecondaryPlainPreDigest {
                    authority_index: offender_index,
                    slot: later_slot,
                },
            ))],
        };
        System::initialize(&next_block, &H256::zero(), &digest);
        <Babe as Hooks<crate::BlockNumber>>::on_initialize(next_block);
        assert!(pallet_babe::Initialized::<Runtime>::get().is_some());
        let finalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            <Babe as Hooks<crate::BlockNumber>>::on_finalize(next_block);
        }));
        if finalized.is_ok() {
            println!("VV-BUG-REPRODUCED: disabled offender authored a later BABE block");
            panic!("BABE accepted a disabled author");
        }
    });
}

// Adversarial boundary: the two-validator local preset has a zero disable
// budget, so a reported validator must remain an eligible BABE author.
#[test]
fn vv_control_offence_disable_limit() {
    use crate::{Session, System};
    use frame_support::traits::Hooks;

    let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
    let mut ext = ext();
    ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
    ext.execute_with(|| {
        assert_eq!(Session::validators().len(), 2);
        let evidence = babe_evidence();
        let offender_index = pallet_babe::Authorities::<Runtime>::get()
            .iter()
            .position(|(key, _)| *key == evidence.offender)
            .unwrap() as u32;
        let later_slot = vv_assigned_secondary_slot(offender_index, evidence.slot);
        let proof: Option<sp_consensus_babe::OpaqueKeyOwnershipProof> = api(
            "BabeApi_generate_key_ownership_proof",
            (evidence.slot, evidence.offender.clone()),
        );
        let submitted: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (evidence, proof.expect("ownership proof")),
        );
        assert_eq!(submitted, Some(()));
        let transactions = &state.read().transactions;
        assert_eq!(transactions.len(), 1);
        let extrinsic = UncheckedExtrinsic::decode(&mut &transactions[0][..]).unwrap();
        assert_ok!(extrinsic.0.function.dispatch(RuntimeOrigin::none()));
        assert!(Session::disabled_validators().is_empty());

        pallet_babe::Initialized::<Runtime>::kill();
        let next_block = System::block_number() + 1;
        let digest = Digest {
            logs: vec![DigestItem::babe_pre_digest(PreDigest::SecondaryPlain(
                SecondaryPlainPreDigest {
                    authority_index: offender_index,
                    slot: later_slot,
                },
            ))],
        };
        System::initialize(&next_block, &H256::zero(), &digest);
        <Babe as Hooks<crate::BlockNumber>>::on_initialize(next_block);
        assert!(pallet_babe::Initialized::<Runtime>::get().is_some());
        let finalized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            <Babe as Hooks<crate::BlockNumber>>::on_finalize(next_block);
        }));
        assert!(
            finalized.is_ok(),
            "a validator never disabled by Session must still author"
        );
    });
}

// Ported from the QUIP-21 audit handover.
#[test]
fn vv_poc_parent_session_proof_rejects_boundary_equivocation() {
    let (pool, state) = sp_core::offchain::testing::TestTransactionPoolExt::new();
    let mut ext = ext();
    ext.register_extension(sp_core::offchain::TransactionPoolExt::new(pool));
    ext.execute_with(|| {
        let pair = AuthorityPair::from_string("//Alice", None).unwrap();
        let old_session = pallet_session::CurrentIndex::<Runtime>::get();
        let next_epoch_slot = pallet_babe::GenesisSlot::<Runtime>::get()
            + Slot::from(
                (pallet_babe::EpochIndex::<Runtime>::get() + 1)
                    * super::super::EpochDuration::get(),
            );
        let parent_proof: Option<sp_consensus_babe::OpaqueKeyOwnershipProof> = api(
            "BabeApi_generate_key_ownership_proof",
            (next_epoch_slot, pair.public()),
        );
        let parent_proof = parent_proof.map(|proof| {
            let decoded: sp_session::MembershipProof = proof.decode().unwrap();
            assert_eq!(decoded.session, old_session);
            sp_consensus_babe::OpaqueKeyOwnershipProof::new(decoded.encode())
        });
        let used_parent = parent_proof.is_some();

        next_session();
        assert_eq!(pallet_session::CurrentIndex::<Runtime>::get(), old_session + 1);
        let make_header = |root: u8| {
            let pre_digest = PreDigest::SecondaryPlain(SecondaryPlainPreDigest {
                authority_index: pallet_babe::Authorities::<Runtime>::get()
                    .iter()
                    .position(|(key, _)| *key == pair.public())
                    .unwrap() as u32,
                slot: next_epoch_slot,
            });
            let mut header = Header::new(
                1,
                H256::repeat_byte(root),
                H256::zero(),
                H256::zero(),
                Digest {
                    logs: vec![DigestItem::babe_pre_digest(pre_digest)],
                },
            );
            let signature = pair.sign(header.hash().as_ref());
            header.digest_mut().push(DigestItem::babe_seal(signature));
            header
        };
        let evidence = EquivocationProof {
            slot: next_epoch_slot,
            offender: pair.public(),
            first_header: make_header(1),
            second_header: make_header(2),
        };
        assert!(sp_consensus_babe::check_equivocation_proof(evidence.clone()));
        let selected_proof = parent_proof.or_else(|| {
            api::<Option<sp_consensus_babe::OpaqueKeyOwnershipProof>>(
                "BabeApi_generate_key_ownership_proof",
                (evidence.slot, evidence.offender.clone()),
            )
        }).expect("continuing authority is provable");
        let submitted: Option<()> = api(
            "BabeApi_submit_report_equivocation_unsigned_extrinsic",
            (evidence, selected_proof),
        );
        assert_eq!(submitted, Some(()));
        let transactions = &state.read().transactions;
        assert_eq!(transactions.len(), 1);
        let extrinsic = UncheckedExtrinsic::decode(&mut &transactions[0][..]).unwrap();
        let _dispatch = extrinsic.0.function.dispatch(RuntimeOrigin::none());
        let reports = pallet_offences::Reports::<Runtime>::iter().count();
        if reports == 0 {
            println!(
                "VV-BUG-REPRODUCED: boundary slot old_session={} current={} used_parent={} reports=0",
                old_session,
                pallet_session::CurrentIndex::<Runtime>::get(),
                used_parent
            );
        }
        assert_eq!(reports, 1, "stock client selection must report boundary equivocation");
    });
}

#[test]
fn babe_key_ownership_proof_excludes_next_epoch_slots() {
    ext().execute_with(|| {
        let authority = AuthorityPair::from_string("//Alice", None)
            .unwrap()
            .public();
        let boundary =
            Babe::current_epoch_start().saturating_add(super::super::EpochDuration::get());
        let last_current: Option<sp_consensus_babe::OpaqueKeyOwnershipProof> = api(
            "BabeApi_generate_key_ownership_proof",
            (boundary - Slot::from(1), authority.clone()),
        );
        assert!(last_current.is_some());
        for slot in [boundary, boundary + Slot::from(1), Slot::from(u64::MAX)] {
            let proof: Option<sp_consensus_babe::OpaqueKeyOwnershipProof> = api(
                "BabeApi_generate_key_ownership_proof",
                (slot, authority.clone()),
            );
            assert!(
                proof.is_none(),
                "future epoch slot must retry against newer state"
            );
        }
    });
}

// Ported from the QUIP-09/part-a audit handover.
#[test]
fn vv_poc_post_snapshot_unbond_outlives_deferred_slash() {
    use frame_support::traits::KeyOwnerProofSystem;

    ext().execute_with(|| {
        let current_evidence = babe_evidence();
        let current_owner = crate::Historical::prove((
            sp_consensus_babe::KEY_TYPE,
            current_evidence.offender.clone(),
        ))
        .unwrap();
        let (stash, _) = crate::Historical::check_proof(
            (sp_consensus_babe::KEY_TYPE, current_evidence.offender),
            current_owner,
        )
        .unwrap();
        assert_ok!(Staking::bond_extra(
            RuntimeOrigin::signed(stash.clone()),
            1_000 * crate::UNIT,
        ));
        // Staking announces era 2 while era 1 remains active. The next-era
        // election has already taken its voter snapshot at this point.
        for _ in 0..8 {
            if pallet_staking::CurrentEra::<Runtime>::get() == Some(2)
                && pallet_staking::ActiveEra::<Runtime>::get().unwrap().index == 1
            {
                break;
            }
            next_session();
        }
        assert_eq!(pallet_staking::CurrentEra::<Runtime>::get(), Some(2));
        assert_eq!(
            pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
            1
        );

        let before = pallet_staking::Ledger::<Runtime>::get(&stash).unwrap();
        assert_eq!(before.active, 3_000 * crate::UNIT);
        assert_ok!(Staking::unbond(
            RuntimeOrigin::signed(stash.clone()),
            1_000 * crate::UNIT,
        ));
        let after_unbond = pallet_staking::Ledger::<Runtime>::get(&stash).unwrap();
        assert_eq!(after_unbond.active, 2_000 * crate::UNIT);
        assert_eq!(after_unbond.unlocking.len(), 1);
        assert_eq!(
            after_unbond.unlocking[0].era,
            pallet_staking::CurrentEra::<Runtime>::get().unwrap() + BondingDuration::get()
        );

        advance_to_era(2);
        let exposure = pallet_staking::ErasStakersOverview::<Runtime>::get(2, &stash).unwrap();
        assert_eq!(exposure.total, 3_000 * crate::UNIT);

        let evidence = babe_evidence();
        let owner =
            crate::Historical::prove((sp_consensus_babe::KEY_TYPE, evidence.offender.clone()))
                .unwrap();
        let (offender_stash, _) = crate::Historical::check_proof(
            (sp_consensus_babe::KEY_TYPE, evidence.offender.clone()),
            owner.clone(),
        )
        .unwrap();
        assert_eq!(offender_stash, stash);
        assert_ok!(Babe::report_equivocation_unsigned(
            RuntimeOrigin::none(),
            Box::new(evidence),
            owner,
        ));
        assert_eq!(
            pallet_staking::UnappliedSlashes::<Runtime>::get(30).len(),
            1
        );

        advance_to_era(29);
        for _ in 0..8 {
            if pallet_staking::CurrentEra::<Runtime>::get() == Some(30) {
                break;
            }
            next_session();
        }
        assert_eq!(pallet_staking::CurrentEra::<Runtime>::get(), Some(30));
        assert_eq!(
            pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
            29
        );
        assert_eq!(
            pallet_staking::UnappliedSlashes::<Runtime>::get(30).len(),
            1
        );

        let total_before_withdrawal = pallet_staking::Ledger::<Runtime>::get(&stash)
            .unwrap()
            .total;
        assert_ok!(Staking::withdraw_unbonded(
            RuntimeOrigin::signed(stash.clone()),
            0,
        ));
        let total_after_withdrawal = pallet_staking::Ledger::<Runtime>::get(&stash)
            .unwrap()
            .total;
        let escaped = total_before_withdrawal - total_after_withdrawal;
        assert_eq!(
            escaped, 0,
            "snapshotted validator stake must remain slashable"
        );
        let issuance_before_slash = crate::Balances::total_issuance();
        advance_to_era(30);
        let burned = issuance_before_slash - crate::Balances::total_issuance();
        assert_eq!(burned, exposure.total);
    });
}

// Ported from the QUIP-09/part-b audit handover.
// The nominator case needs 30 eras: 29 closes only the validator variant.

fn vv_poll_election_block() {
    use frame_support::{traits::Hooks, weights::WeightMeter};
    let now = crate::System::block_number() + 1;
    crate::System::set_block_number(now);
    let mut meter = WeightMeter::with_limit(super::super::RuntimeBlockWeights::get().max_block);
    <crate::MultiBlockElection as Hooks<crate::BlockNumber>>::on_poll(now, &mut meter);
}

fn vv_next_session_blockwise() {
    use frame_support::traits::Hooks;
    let index = pallet_session::CurrentIndex::<Runtime>::get() + 1;
    let block = index * super::super::EpochDuration::get() as u32;
    while crate::System::block_number() < block - 1 {
        vv_poll_election_block();
    }
    crate::System::set_block_number(block);
    pallet_babe::CurrentSlot::<Runtime>::put(Slot::from(u64::from(block)));
    pallet_babe::Initialized::<Runtime>::put(None::<PreDigest>);
    crate::Timestamp::set_timestamp(1 + u64::from(block) * 6_000);
    crate::Session::rotate_session();
    <crate::Grandpa as Hooks<crate::BlockNumber>>::on_finalize(block);
    <Staking as Hooks<crate::BlockNumber>>::on_finalize(block);
}

fn vv_finish_election_round() {
    for _ in
        0..super::super::election::Pages::get() + 1 + super::super::election::UnsignedPhase::get()
    {
        vv_poll_election_block();
    }
    assert_eq!(
        pallet_election_provider_multi_block::CurrentPhase::<Runtime>::get(),
        pallet_election_provider_multi_block::Phase::Done,
    );
}

fn vv_advance_active_era_blockwise(target: u32) {
    for _ in 0..200 {
        if pallet_staking::ActiveEra::<Runtime>::get().is_some_and(|e| e.index >= target) {
            break;
        }
        vv_next_session_blockwise();
    }
    assert_eq!(
        pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
        target,
    );
}

fn vv_setup_era_offset_nominator() -> (crate::AccountId, crate::AccountId) {
    use frame_support::traits::Currency;
    vv_advance_active_era_blockwise(1);
    let validator = quip_transaction_crypto::account_id_from_public(
        &quip_transaction_crypto::HybridPair::from_string("//Alice", None)
            .unwrap()
            .public(),
    );
    assert!(crate::Session::validators().contains(&validator));
    let nominator = crate::AccountId::new([181; 32]);
    let stake = 3_000 * crate::UNIT;
    let _ = crate::Balances::make_free_balance_be(&nominator, stake + 2 * crate::UNIT);
    assert_ok!(Staking::bond(
        RuntimeOrigin::signed(nominator.clone()),
        stake,
        pallet_staking::RewardDestination::Staked,
    ));
    assert_ok!(Staking::nominate(
        RuntimeOrigin::signed(nominator.clone()),
        vec![crate::Address::Id(validator.clone())],
    ));
    while pallet_session::CurrentIndex::<Runtime>::get() < 9 {
        vv_next_session_blockwise();
    }
    assert_eq!(
        pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
        1,
    );
    assert_eq!(pallet_staking::CurrentEra::<Runtime>::get(), Some(1));
    (validator, nominator)
}

#[test]
fn vv_poc_era_offset_after_snapshot() {
    use frame_support::traits::KeyOwnerProofSystem;
    super::super::validator_onboarding::tests::ext().execute_with(|| {
        let (validator, nominator) = vv_setup_era_offset_nominator();
        let stake = 3_000 * crate::UNIT;
        vv_finish_election_round();
        let round = pallet_election_provider_multi_block::Round::<Runtime>::get();
        let snapshot =
            pallet_election_provider_multi_block::PagedVoterSnapshot::<Runtime>::get(round, 0)
                .expect("voter page exists after the election round");
        assert!(snapshot.iter().any(|(who, weight, _)| {
            *who == nominator && *weight == Staking::weight_of(&nominator)
        }));
        assert_eq!(pallet_staking::CurrentEra::<Runtime>::get(), Some(1));
        assert_eq!(
            pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
            1
        );

        // Keep a small active ledger alongside the snapshotted unlocking stake.
        assert_ok!(Staking::bond_extra(
            RuntimeOrigin::signed(nominator.clone()),
            crate::UNIT,
        ));
        assert_ok!(Staking::chill(RuntimeOrigin::signed(nominator.clone())));
        assert_ok!(Staking::unbond(
            RuntimeOrigin::signed(nominator.clone()),
            stake
        ));
        let chunk = pallet_staking::Ledger::<Runtime>::get(&nominator)
            .unwrap()
            .unlocking[0]
            .clone();
        assert_eq!(chunk.value, stake);
        assert_eq!(
            chunk.era,
            pallet_staking::CurrentEra::<Runtime>::get().unwrap() + BondingDuration::get()
        );

        vv_advance_active_era_blockwise(2);
        let overview = pallet_staking::ErasStakersOverview::<Runtime>::get(2, &validator).unwrap();
        let page = pallet_staking::ErasStakersPaged::<Runtime>::get((2, &validator, 0)).unwrap();
        assert_eq!(overview.total, 5_000 * crate::UNIT);
        assert!(page
            .others
            .iter()
            .any(|other| other.who == nominator && other.value == stake));

        let evidence = babe_evidence();
        let owner =
            crate::Historical::prove((sp_consensus_babe::KEY_TYPE, evidence.offender.clone()))
                .unwrap();
        let (offender, _) = crate::Historical::check_proof(
            (sp_consensus_babe::KEY_TYPE, evidence.offender.clone()),
            owner.clone(),
        )
        .unwrap();
        assert_eq!(offender, validator);
        assert_ok!(Babe::report_equivocation_unsigned(
            RuntimeOrigin::none(),
            Box::new(evidence),
            owner,
        ));
        let pending = pallet_staking::UnappliedSlashes::<Runtime>::get(30);
        assert_eq!(pending.len(), 1);
        assert!(pending[0]
            .others
            .iter()
            .any(|(who, amount)| *who == nominator && *amount == stake));

        let queued = pending[0].own
            + pending[0]
                .others
                .iter()
                .map(|(_, amount)| *amount)
                .sum::<crate::Balance>();
        vv_advance_active_era_blockwise(29);
        for _ in 0..8 {
            if pallet_staking::CurrentEra::<Runtime>::get() == Some(30) {
                break;
            }
            vv_next_session_blockwise();
        }
        assert_eq!(pallet_staking::CurrentEra::<Runtime>::get(), Some(30));
        assert_eq!(
            pallet_staking::ActiveEra::<Runtime>::get().unwrap().index,
            29
        );
        assert_eq!(
            pallet_staking::UnappliedSlashes::<Runtime>::get(30).len(),
            1
        );
        let total_before = pallet_staking::Ledger::<Runtime>::get(&nominator)
            .unwrap()
            .total;
        let spans = pallet_staking::SlashingSpans::<Runtime>::get(&nominator)
            .map_or(0, |spans| spans.prior.len() as u32 + 1);
        assert_ok!(Staking::withdraw_unbonded(
            RuntimeOrigin::signed(nominator.clone()),
            spans,
        ));
        let total_after =
            pallet_staking::Ledger::<Runtime>::get(&nominator).map_or(0, |ledger| ledger.total);
        let released = total_before - total_after;
        assert_eq!(
            released, 0,
            "snapshotted stake must remain slashable until era 30"
        );
        let issuance_before = crate::Balances::total_issuance();
        vv_advance_active_era_blockwise(30);
        let burned = issuance_before - crate::Balances::total_issuance();
        assert_eq!(
            burned, queued,
            "the full queued validator and nominator slash must burn"
        );
        assert!(pallet_staking::UnappliedSlashes::<Runtime>::get(30).is_empty());
        assert_eq!(
            pallet_staking::Ledger::<Runtime>::get(&nominator)
                .unwrap()
                .total,
            total_before - stake,
            "the full snapshotted nominator stake must be slashed before withdrawal"
        );
    });
}

#[test]
fn vv_poc_era_offset_control_before_snapshot() {
    super::super::validator_onboarding::tests::ext().execute_with(|| {
        let (validator, nominator) = vv_setup_era_offset_nominator();
        assert_ok!(Staking::chill(RuntimeOrigin::signed(nominator.clone())));
        assert_ok!(Staking::unbond(
            RuntimeOrigin::signed(nominator.clone()),
            3_000 * crate::UNIT,
        ));
        vv_finish_election_round();
        let round = pallet_election_provider_multi_block::Round::<Runtime>::get();
        let snapshot =
            pallet_election_provider_multi_block::PagedVoterSnapshot::<Runtime>::get(round, 0)
                .expect("voter page exists after the election round");
        assert!(snapshot.iter().all(|(who, _, _)| *who != nominator));
        vv_advance_active_era_blockwise(2);
        let overview = pallet_staking::ErasStakersOverview::<Runtime>::get(2, &validator).unwrap();
        assert_eq!(overview.total, 2_000 * crate::UNIT);
        let page = pallet_staking::ErasStakersPaged::<Runtime>::get((2, &validator, 0));
        assert!(page.is_none_or(|page| page.others.iter().all(|other| other.who != nominator)));
    });
}

// Ported from the R13 audit handover.

#[test]
fn vv_poc_grandpa_retention() {
    use quip_crypto_primitives::substrate::{ed25519_fndsa512, sr25519_fndsa512};
    use sp_consensus_grandpa::{AuthorityPair, Equivocation, EquivocationProof};
    use sp_core::proof_of_possession::ProofOfPossessionGenerator;

    fn change_keys(owner: &str, seed: &str) {
        let who = quip_transaction_crypto::account_id_from_public(
            &quip_transaction_crypto::HybridPair::from_string(owner, None)
                .unwrap()
                .public(),
        );
        let mut babe = sr25519_fndsa512::Pair::from_string(seed, None).unwrap();
        let mut grandpa = ed25519_fndsa512::Pair::from_string(seed, None).unwrap();
        let keys = crate::SessionKeys {
            babe: babe.public().into(),
            grandpa: grandpa.public().into(),
        }
        .into();
        let proof = (
            babe.generate_proof_of_possession(&who.encode()),
            grandpa.generate_proof_of_possession(&who.encode()),
        )
            .encode();
        assert_ok!(crate::Session::set_keys(
            RuntimeOrigin::signed(who),
            keys,
            proof
        ));
    }

    let mut ext = ext();
    ext.execute_with(|| {
        // A queued change becomes effective one session after it is first queued.
        change_keys("//Alice", "//Charlie");
        while pallet_staking::ActiveEra::<Runtime>::get().unwrap().index < 2 {
            if pallet_session::CurrentIndex::<Runtime>::get() ==
                pallet_staking::ErasStartSessionIndex::<Runtime>::get(1).unwrap() + 5 {
                change_keys("//Bob", "//Eve");
            }
            next_session();
        }
        // Restore Bob NextKeys to the keys used when session 12 historical root
        // was planned; its already-queued Eve keys still activate next session.
        change_keys("//Bob", "//Bob");
        let offence_era = pallet_staking::ActiveEra::<Runtime>::get().unwrap().index;
        let pair = AuthorityPair::from_string("//Charlie", None).unwrap();
        assert!(pallet_grandpa::Authorities::<Runtime>::get()
            .iter().any(|(key, _)| key == &pair.public()));
        let set_id = pallet_grandpa::CurrentSetId::<Runtime>::get();
        assert!(set_id > 0);
        let round = 7;
        let vote = |root| {
            let prevote = finality_grandpa::Prevote {
                target_hash: H256::repeat_byte(root), target_number: 1u32,
            };
            let payload = sp_consensus_grandpa::localized_payload(
                round, set_id, &finality_grandpa::Message::Prevote(prevote.clone()),
            );
            (prevote, pair.sign(&payload))
        };
        let evidence = EquivocationProof::new(set_id, Equivocation::Prevote(
            finality_grandpa::Equivocation {
                round_number: round, identity: pair.public(),
                first: vote(1), second: vote(2),
            },
        ));
        assert!(sp_consensus_grandpa::check_equivocation_proof(evidence.clone()));
        let proof: sp_session::MembershipProof =
            crate::Historical::prove((sp_consensus_grandpa::KEY_TYPE, pair.public())).unwrap();
        let offence_session = proof.session;
        assert!(pallet_grandpa::SetIdSession::<Runtime>::get(set_id - 1).is_some());
        let check_proof = || {
            assert!(crate::Historical::check_proof(
                (sp_consensus_grandpa::KEY_TYPE, pair.public()), proof.clone(),
            ).is_some());
            assert!(has_pending_slash_bucket(offence_session));
        };
        check_proof();
        let mut changes = 0u64;
        let mut alternate = false;
        let mut before_boundary = None;
        let mut control_accepted = false;
        while pallet_staking::ActiveEra::<Runtime>::get().unwrap().index < offence_era + 27
            || pallet_session::CurrentIndex::<Runtime>::get()
                < pallet_staking::ErasStartSessionIndex::<Runtime>::get(offence_era + 27).unwrap() + 5
        {
            change_keys("//Alice", if alternate { "//Charlie" } else { "//Dave" });
            alternate = !alternate;
            let old = pallet_grandpa::CurrentSetId::<Runtime>::get();
            next_session();
            let new = pallet_grandpa::CurrentSetId::<Runtime>::get();
            if new != old {
                assert_eq!(new, old + 1);
                changes += 1;
            }
            if pallet_grandpa::SetIdSession::<Runtime>::get(set_id - 1).is_some() {
                before_boundary = Some((pallet_session::CurrentIndex::<Runtime>::get(), changes));
                if !control_accepted && pallet_staking::ActiveEra::<Runtime>::get().unwrap().index == offence_era + 27 {
                    check_proof();
                    sp_io::storage::start_transaction();
                    assert_ok!(crate::Grandpa::report_equivocation_unsigned(
                        RuntimeOrigin::none(), Box::new(evidence.clone()), proof.clone(),
                    ));
                    assert_eq!(pallet_staking::UnappliedSlashes::<Runtime>::get(offence_era + 28).len(), 1);
                    sp_io::storage::rollback_transaction();
                    control_accepted = true;
                }
            }
        }
        check_proof();
        assert!(control_accepted, "valid report before pruning must queue slash");
        let last_kept = before_boundary.expect("pre-pruning boundary");
        assert!(changes > 0, "ordinary key rotations must change sets");
        assert_eq!(changes, 167, "each eligible session should change sets");
        if pallet_grandpa::SetIdSession::<Runtime>::get(set_id - 1).is_none() {
            assert!(last_kept.1 < changes);
        }
        assert_eq!(pallet_staking::ActiveEra::<Runtime>::get().unwrap().index, offence_era + 27);
        let apply_era = offence_era + 28;
        assert_eq!(pallet_staking::UnappliedSlashes::<Runtime>::get(apply_era).len(), 0);
        // Security expectation: any historically valid equivocation within a pending bucket
        // reaches offence accounting despite ordinary validator-owned rotations.
        let result = crate::Grandpa::report_equivocation_unsigned(
            RuntimeOrigin::none(), Box::new(evidence), proof,
        );
        if let Err(error) = &result {
            assert_eq!(error.error, pallet_grandpa::Error::<Runtime>::InvalidEquivocationProof.into());
            assert_eq!(pallet_staking::UnappliedSlashes::<Runtime>::get(apply_era).len(), 0);
            println!("VV-BUG-REPRODUCED: era={offence_era} current_era={} set={set_id} changes={changes} prior_boundary={last_kept:?}",
                pallet_staking::ActiveEra::<Runtime>::get().unwrap().index);
        }
        assert_ok!(result);
        assert_eq!(pallet_staking::UnappliedSlashes::<Runtime>::get(apply_era).len(), 1);
    });
}

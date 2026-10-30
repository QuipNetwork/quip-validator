use super::*;
use crate::{Babe, Header, RuntimeCall, RuntimeOrigin, UncheckedExtrinsic};
use codec::{Decode, Encode};
use frame_support::{
    assert_ok,
    traits::{KeyOwnerProofSystem, OnRuntimeUpgrade},
};
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

#[test]
fn offences_upgrade_initializes_version_once_without_clearing_reports() {
    ext().execute_with(|| {
        let id = H256::repeat_byte(7);
        let report = sp_staking::offence::OffenceDetails {
            offender: (crate::AccountId::new([7; 32]), Default::default()),
            reporters: vec![],
        };
        pallet_offences::Reports::<Runtime>::insert(id, report.clone());
        StorageVersion::new(0).put::<Offences>();
        InitializeOffences::on_runtime_upgrade();
        assert_eq!(pallet_offences::Reports::<Runtime>::get(id), Some(report));
        assert_eq!(StorageVersion::get::<Offences>(), StorageVersion::new(1));
        assert_eq!(
            InitializeOffences::on_runtime_upgrade(),
            <Runtime as frame_system::Config>::DbWeight::get().reads(1)
        );
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
fn oldest_bonded_report_is_rejected_before_any_slash_accounting() {
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
            1
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
    pallet_collective::RawOrigin::<crate::AccountId, pallet_collective::Instance1>::Members(4, 5)
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
    use crate::{ProtocolParams, ValidatorAdmission};
    use frame_support::assert_noop;
    use sp_runtime::Perbill;
    ext().execute_with(|| {
        let stash = pallet_session::Validators::<Runtime>::get()[0].clone();
        assert_eq!(
            pallet_protocol_params::MaxDiscretionarySlashPerMotion::<Runtime>::get(),
            Perbill::from_percent(1)
        );
        assert_eq!(
            pallet_protocol_params::MaxDiscretionarySlashCumulative::<Runtime>::get(),
            Perbill::from_percent(4)
        );
        assert_eq!(
            pallet_protocol_params::BudgetWindow::<Runtime>::get(),
            BondingDuration::get()
        );
        let minority: RuntimeOrigin = pallet_collective::RawOrigin::<
            crate::AccountId,
            pallet_collective::Instance1,
        >::Members(3, 5)
        .into();
        assert_noop!(
            ProtocolParams::discretionary_slash(
                minority,
                stash.clone(),
                1,
                Perbill::from_percent(1)
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(ProtocolParams::discretionary_slash(
            council(),
            stash.clone(),
            1,
            Perbill::from_percent(1)
        ));
        crate::System::assert_last_event(
            pallet_protocol_params::Event::SlashScheduled {
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
            pallet_protocol_params::SlashHistory::<Runtime>::get(&stash).as_slice(),
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
        let key = pallet_protocol_params::BudgetWindow::<Runtime>::hashed_key().to_vec();
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

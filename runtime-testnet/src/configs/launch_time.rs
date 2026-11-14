//! Keep fresh-launch emissions and staking anchored to the intended launch instant.
use crate::Runtime;
use frame_support::traits::OnTimestampSet;

/// Reject an early timestamp inherent before staking can anchor its first era.
/// StartAt uses seconds; Timestamp uses milliseconds. Division avoids overflow.
pub struct TimestampFloor;
impl OnTimestampSet<u64> for TimestampFloor {
    fn on_timestamp_set(now_ms: u64) {
        if let Some(start_at) = pallet_emission_controller::StartAt::<Runtime>::get() {
            assert!(
                now_ms / 1_000 >= start_at,
                "Timestamp precedes configured launch instant"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Balance, EmissionController, Staking, UNIT};
    use codec::Encode;
    use frame_support::assert_ok;
    use sp_runtime::traits::Header as _;

    const LAUNCH_MS: u64 = 1_763_000_000_000;

    // Executive path from the QUIP-01 part-a handover, including a BABE slot digest.
    fn drive_block(number: u32, timestamp_ms: u64, parent: crate::Hash) -> crate::Hash {
        let pre_digest = sp_consensus_babe::digests::PreDigest::SecondaryPlain(
            sp_consensus_babe::digests::SecondaryPlainPreDigest {
                authority_index: 0,
                slot: sp_consensus_babe::Slot::from(timestamp_ms / 6_000),
            },
        );
        let header = crate::Header::new(
            number,
            Default::default(),
            Default::default(),
            parent,
            sp_runtime::generic::Digest {
                logs: alloc::vec![sp_runtime::generic::DigestItem::PreRuntime(
                    sp_consensus_babe::BABE_ENGINE_ID,
                    pre_digest.encode(),
                )],
            },
        );
        crate::Executive::initialize_block(&header);
        assert_ok!(crate::Timestamp::set(
            crate::RuntimeOrigin::none(),
            timestamp_ms
        ));
        crate::Executive::finalize_block().hash()
    }

    #[test]
    #[should_panic(expected = "Timestamp precedes configured launch instant")]
    fn vv_poc_low_slot_genesis_backfill_through_executive() {
        super::super::validator_onboarding::tests::ext().execute_with(|| {
            crate::System::set_block_number(0);
            pallet_emission_controller::StartAt::<Runtime>::put(LAUNCH_MS / 1_000);
            drive_block(1, 6_000, Default::default());
        });
    }

    #[test]
    fn vv_control_wall_clock_launch_has_no_backfill() {
        super::super::validator_onboarding::tests::ext().execute_with(|| {
            crate::System::set_block_number(0);
            pallet_emission_controller::StartAt::<Runtime>::put(LAUNCH_MS / 1_000);
            let parent = drive_block(1, LAUNCH_MS, Default::default());
            assert_eq!(
                pallet_staking::ActiveEra::<Runtime>::get().unwrap().start,
                Some(LAUNCH_MS)
            );
            let parent = drive_block(2, LAUNCH_MS + 6_000, parent);
            drive_block(3, LAUNCH_MS + 12_000, parent);
            assert_eq!(
                pallet_emission_controller::StartAt::<Runtime>::get(),
                Some(LAUNCH_MS / 1_000)
            );
            let issued = pallet_emission_controller::ScheduledIssued::<Runtime>::get();
            let expected = EmissionController::entitlement(6).unwrap();
            assert!(issued.abs_diff(expected) <= 2);
        });
    }

    #[test]
    fn timestamp_floor_uses_seconds_and_preserves_unanchored_presets() {
        sp_io::TestExternalities::default().execute_with(|| {
            TimestampFloor::on_timestamp_set(1);
            pallet_emission_controller::StartAt::<Runtime>::put(10);
            assert!(std::panic::catch_unwind(|| TimestampFloor::on_timestamp_set(9_999)).is_err());
            TimestampFloor::on_timestamp_set(10_000);
            TimestampFloor::on_timestamp_set(u64::MAX);
            pallet_emission_controller::StartAt::<Runtime>::put(u64::MAX);
            assert!(
                std::panic::catch_unwind(|| TimestampFloor::on_timestamp_set(u64::MAX)).is_err()
            );
        });
    }

    // Ported QUIP-01 part-b: reject the low launch and retain the honest payout control.
    #[cfg(not(feature = "runtime-benchmarks"))]
    #[test]
    fn vv_poc_low_slot_era_start_backfills_staking() {
        use crate::{AccountId, BlockNumber, RuntimeOrigin, System, Timestamp};
        use codec::Encode;
        use frame_support::{assert_ok, traits::Hooks, weights::WeightMeter};
        use sp_runtime::traits::Header as _;

        const WALL_MS: u64 = 1_763_000_000_000;

        fn seal(number: u32, timestamp_ms: u64, parent: crate::Hash) -> crate::Hash {
            let pre_digest = sp_consensus_babe::digests::PreDigest::SecondaryPlain(
                sp_consensus_babe::digests::SecondaryPlainPreDigest {
                    authority_index: 0,
                    slot: sp_consensus_babe::Slot::from(timestamp_ms / 6_000),
                },
            );
            let header = crate::Header::new(
                number,
                Default::default(),
                Default::default(),
                parent,
                sp_runtime::generic::Digest {
                    logs: alloc::vec![sp_runtime::generic::DigestItem::PreRuntime(
                        sp_consensus_babe::BABE_ENGINE_ID,
                        pre_digest.encode(),
                    )],
                },
            );
            crate::Executive::initialize_block(&header);
            assert_ok!(Timestamp::set(RuntimeOrigin::none(), timestamp_ms));
            crate::Executive::finalize_block().hash()
        }

        fn note_author(validator: &AccountId) {
            let block = System::block_number() + 1;
            let pre_digest = sp_consensus_babe::digests::PreDigest::SecondaryPlain(
                sp_consensus_babe::digests::SecondaryPlainPreDigest {
                    authority_index: 0,
                    slot: sp_consensus_babe::Slot::from(WALL_MS / 6_000 + 1),
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

        fn run_case(first_ms: u64) -> (Balance, Balance, u64) {
            super::super::validator_onboarding::tests::ext().execute_with(|| {
                System::set_block_number(0);
                pallet_emission_controller::StartAt::<Runtime>::put(WALL_MS / 1_000);
                assert_eq!(pallet_staking::ActiveEra::<Runtime>::get().unwrap().start, None);
                let parent = seal(1, first_ms, Default::default());
                let first_anchor = pallet_staking::ActiveEra::<Runtime>::get().unwrap().start;
                if first_ms == WALL_MS {
                    assert_eq!(first_anchor, Some(WALL_MS));
                }
                let second_ms = if first_ms == WALL_MS { WALL_MS + 6_000 } else { WALL_MS };
                let _ = seal(2, second_ms, parent);
                let anchor = pallet_staking::ActiveEra::<Runtime>::get().unwrap().start;
                let validator: AccountId = pallet_session::Validators::<Runtime>::get()[0].clone();
                note_author(&validator);
                let points = pallet_staking::ErasRewardPoints::<Runtime>::get(0);
                assert!(points.total >= 20);
                assert!(points.individual.get(&validator).copied().unwrap_or(0) >= 20);

                for index in 1..=6_u32 {
                    let block = index * super::super::EpochDuration::get() as u32;
                    System::set_block_number(block);
                    let timestamp_ms = WALL_MS + u64::from(block) * 6_000;
                    pallet_babe::CurrentSlot::<Runtime>::put(sp_consensus_babe::Slot::from(
                        timestamp_ms / 6_000,
                    ));
                    pallet_babe::Initialized::<Runtime>::put(
                        None::<sp_consensus_babe::digests::PreDigest>,
                    );
                    Timestamp::set_timestamp(timestamp_ms);
                    crate::Session::rotate_session();
                    <crate::Grandpa as Hooks<BlockNumber>>::on_finalize(block);
                    <Staking as Hooks<BlockNumber>>::on_finalize(block);
                    for offset in 1..=3 {
                        System::set_block_number(block + offset);
                        let mut meter = WeightMeter::with_limit(
                            crate::configs::RuntimeBlockWeights::get().max_block,
                        );
                        <crate::MultiBlockElection as Hooks<BlockNumber>>::on_poll(
                            block + offset,
                            &mut meter,
                        );
                    }
                }

                let reward = pallet_staking::ErasValidatorReward::<Runtime>::get(0).unwrap();
                let issuance_before = pallet_balances::TotalIssuance::<Runtime>::get();
                assert_ok!(Staking::payout_stakers(
                    RuntimeOrigin::signed(validator.clone()),
                    validator,
                    0,
                ));
                let minted = pallet_balances::TotalIssuance::<Runtime>::get() - issuance_before;
                println!("VV-STAKING first_ms={first_ms} anchor={anchor:?} reward={reward} minted={minted}");
                (reward, minted, anchor.unwrap())
            })
        }

        let low = std::panic::catch_unwind(|| run_case(6_000));
        let panic = low.expect_err("the early inherent must invalidate block one");
        let reason = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(reason.contains("Timestamp precedes configured launch instant"));
        let (wall_reward, wall_minted, wall_anchor) = run_case(WALL_MS);
        assert_eq!(wall_anchor, WALL_MS);
        assert!(wall_reward > 0 && wall_reward < 100_000 * UNIT);
        assert!(wall_minted > 0 && wall_minted <= wall_reward);
    }
}

//! Authority matrix for V0 governance, asserted as a test suite.
//!
//! Spec 120 removed `pallet-sudo`, so every privileged path now funnels through
//! the council. This module pins the resulting matrix so a later threshold or
//! origin change cannot widen it silently:
//!
//! | Call                                   | Origin                       |
//! | -------------------------------------- | ---------------------------- |
//! | `ValidatorAdmission::approve/remove`   | council 2/3                  |
//! | `ValidatorAdmission::dispatch_as_root` | council 2/3 → `RawOrigin::Root` |
//! | `FoundationMembership::add/swap/reset` | council 2/3                  |
//! | `FoundationMembership::remove`         | council 2/3                  |
//! | `Staking::set_staking_configs`         | Root only, i.e. via `dispatch_as_root` |
//! | `Staking::cancel_deferred_slash` etc.  | Root only, i.e. via `dispatch_as_root` |
//!
//! At the V0 size of six seats, 2/3 is exactly four votes. The council may
//! remove its own members directly, through Root, or by resetting membership.

#[cfg(test)]
mod tests {
    use crate::{
        AccountId, FoundationMembership, Runtime, RuntimeCall, RuntimeGenesisConfig, RuntimeOrigin,
        ValidatorAdmission,
    };
    use frame_support::{assert_noop, assert_ok};
    use pallet_collective::{Instance1, RawOrigin};
    use sp_runtime::{BuildStorage, DispatchError, MultiAddress};

    /// Six council seats, as configured for V0.
    const SEATS: u32 = 6;

    fn council() -> Vec<AccountId> {
        (1u8..=SEATS as u8)
            .map(|i| AccountId::from([i; 32]))
            .collect()
    }

    fn new_test_ext() -> sp_io::TestExternalities {
        let mut config = RuntimeGenesisConfig::default();
        config.foundation_membership.members = council()
            .try_into()
            .expect("six council seats fit in MaxMembers");
        // `approve` only works once admission has left the fixed-testnet mode.
        config.validator_admission.mode = pallet_validator_admission::AdmissionMode::FoundationOnly;
        config.validator_admission.approved = Vec::new();
        sp_io::TestExternalities::new(config.build_storage().expect("genesis builds"))
    }

    fn council_origin(approvals: u32) -> RuntimeOrigin {
        RawOrigin::<AccountId, Instance1>::Members(approvals, SEATS).into()
    }

    /// Membership calls take a `MultiAddress`, not a bare account.
    fn into_member(who: AccountId) -> MultiAddress<AccountId, ()> {
        MultiAddress::Id(who)
    }

    fn candidate() -> AccountId {
        AccountId::from([0xAA; 32])
    }

    /// Root is dispatched through the council, not through a sudo key: a
    /// `set_heap_pages` call only succeeds from `dispatch_as_root`.
    fn root_only_call() -> Box<RuntimeCall> {
        Box::new(RuntimeCall::System(frame_system::Call::set_heap_pages {
            pages: 16,
        }))
    }

    #[test]
    fn genesis_installs_six_council_seats() {
        new_test_ext().execute_with(|| {
            let members = pallet_collective::Members::<Runtime, Instance1>::get();
            assert_eq!(members.len(), SEATS as usize);
            assert_eq!(members, council());
        });
    }

    #[test]
    fn council_decision_requires_two_thirds() {
        new_test_ext().execute_with(|| {
            // A plain signed account has no governance power at all.
            assert_noop!(
                ValidatorAdmission::approve(RuntimeOrigin::signed(candidate()), candidate()),
                DispatchError::BadOrigin
            );
            // Three of six is half — below 2/3.
            assert_noop!(
                ValidatorAdmission::approve(council_origin(3), candidate()),
                DispatchError::BadOrigin
            );
            // Four of six is exactly 2/3.
            assert_ok!(ValidatorAdmission::approve(council_origin(4), candidate()));
            assert!(pallet_validator_admission::Approved::<Runtime>::get().contains(&candidate()));
        });
    }

    #[test]
    fn root_is_reachable_only_through_the_council() {
        new_test_ext().execute_with(|| {
            assert_noop!(
                ValidatorAdmission::dispatch_as_root(
                    RuntimeOrigin::signed(candidate()),
                    root_only_call()
                ),
                DispatchError::BadOrigin
            );
            assert_noop!(
                ValidatorAdmission::dispatch_as_root(council_origin(3), root_only_call()),
                DispatchError::BadOrigin
            );
            assert_ok!(ValidatorAdmission::dispatch_as_root(
                council_origin(4),
                root_only_call()
            ));

            // The set_staking_configs path is Root-only in this SDK revision, so
            // the council can only reach it by dispatching as Root.
            assert_noop!(
                crate::Staking::set_staking_configs(
                    council_origin(4),
                    pallet_staking::ConfigOp::Noop,
                    pallet_staking::ConfigOp::Noop,
                    pallet_staking::ConfigOp::Noop,
                    pallet_staking::ConfigOp::Noop,
                    pallet_staking::ConfigOp::Noop,
                    pallet_staking::ConfigOp::Noop,
                    pallet_staking::ConfigOp::Noop,
                ),
                DispatchError::BadOrigin
            );
            assert_ok!(ValidatorAdmission::dispatch_as_root(
                council_origin(4),
                Box::new(RuntimeCall::Staking(
                    pallet_staking::Call::set_staking_configs {
                        min_nominator_bond: pallet_staking::ConfigOp::Noop,
                        min_validator_bond: pallet_staking::ConfigOp::Noop,
                        max_nominator_count: pallet_staking::ConfigOp::Noop,
                        max_validator_count: pallet_staking::ConfigOp::Noop,
                        chill_threshold: pallet_staking::ConfigOp::Noop,
                        min_commission: pallet_staking::ConfigOp::Noop,
                        max_staked_rewards: pallet_staking::ConfigOp::Noop,
                    }
                ))
            ));
        });
    }

    #[test]
    fn membership_removal_requires_two_thirds() {
        new_test_ext().execute_with(|| {
            let outgoing = council()[5].clone();
            assert_noop!(
                FoundationMembership::remove_member(
                    council_origin(3),
                    into_member(outgoing.clone())
                ),
                DispatchError::BadOrigin
            );
            assert_ok!(FoundationMembership::remove_member(
                council_origin(4),
                into_member(outgoing.clone())
            ));
            assert!(!pallet_collective::Members::<Runtime, Instance1>::get().contains(&outgoing));
            assert_eq!(
                pallet_collective::Members::<Runtime, Instance1>::get().len(),
                5
            );
        });
    }

    #[test]
    fn membership_removal_through_root_requires_two_thirds() {
        new_test_ext().execute_with(|| {
            let outgoing = council()[5].clone();
            let call = Box::new(RuntimeCall::FoundationMembership(
                pallet_membership::Call::remove_member {
                    who: into_member(outgoing.clone()),
                },
            ));
            assert_noop!(
                ValidatorAdmission::dispatch_as_root(council_origin(3), call.clone()),
                DispatchError::BadOrigin
            );
            assert_ok!(ValidatorAdmission::dispatch_as_root(
                council_origin(4),
                call
            ));
            // Check the inner dispatch took effect, not just the wrapper result.
            assert!(!pallet_collective::Members::<Runtime, Instance1>::get().contains(&outgoing));
            assert_eq!(
                pallet_collective::Members::<Runtime, Instance1>::get().len(),
                5
            );
        });
    }

    #[test]
    fn membership_reset_requires_two_thirds() {
        new_test_ext().execute_with(|| {
            assert_noop!(
                FoundationMembership::reset_members(council_origin(3), Vec::new()),
                DispatchError::BadOrigin
            );
            assert_ok!(FoundationMembership::reset_members(
                council_origin(4),
                Vec::new()
            ));
            assert!(pallet_membership::Members::<Runtime, Instance1>::get().is_empty());
            assert!(pallet_collective::Members::<Runtime, Instance1>::get().is_empty());
        });
    }

    #[test]
    fn membership_appointment_stays_at_two_thirds() {
        new_test_ext().execute_with(|| {
            let incoming = AccountId::from([0x42; 32]);
            assert_noop!(
                FoundationMembership::add_member(council_origin(3), into_member(incoming.clone())),
                DispatchError::BadOrigin
            );
            assert_ok!(FoundationMembership::add_member(
                council_origin(4),
                into_member(incoming.clone())
            ));
            assert!(pallet_collective::Members::<Runtime, Instance1>::get().contains(&incoming));
        });
    }

    /// QBlock reward changes are council business, bounded by the runtime
    /// constants, and never need a runtime upgrade.
    #[test]
    fn qblock_reward_is_council_settable_and_bounded() {
        use frame_support::traits::Get;

        new_test_ext().execute_with(|| {
            let reward = 5 * crate::UNIT;
            assert_noop!(
                crate::QuantumPow::set_block_reward(council_origin(3), reward),
                DispatchError::BadOrigin
            );
            assert_ok!(crate::QuantumPow::set_block_reward(
                council_origin(4),
                reward
            ));
            assert_eq!(crate::QuantumPow::block_reward(), reward);

            let max = <crate::configs::QuantumPowMaxBlockReward as Get<u128>>::get();
            assert_noop!(
                crate::QuantumPow::set_block_reward(council_origin(4), max + 1),
                pallet_quantum_pow::Error::<Runtime>::BlockRewardOutOfBounds
            );
        });
    }

    /// QBlock round length is the second half of the same council knob: the
    /// difficulty controller's decay gate and hardening band both read it.
    #[test]
    fn qblock_round_length_is_council_settable_and_bounded() {
        use frame_support::traits::Get;

        new_test_ext().execute_with(|| {
            let max = <crate::configs::QuantumPowMaxTargetProofBlocks as Get<u32>>::get();
            assert_noop!(
                crate::QuantumPow::set_target_proof_blocks(council_origin(3), 200),
                DispatchError::BadOrigin
            );
            assert_ok!(crate::QuantumPow::set_target_proof_blocks(
                council_origin(4),
                200
            ));
            assert_eq!(crate::QuantumPow::target_proof_blocks(), 200);
            assert_noop!(
                crate::QuantumPow::set_target_proof_blocks(council_origin(4), max + 1),
                pallet_quantum_pow::Error::<Runtime>::TargetProofBlocksOutOfBounds
            );
        });
    }

    /// The council retains commission-floor authority at 2/3 through the
    /// audited Root path; direct council origins no longer satisfy `AdminOrigin`.
    #[test]
    fn staking_admin_calls_go_through_root() {
        use sp_runtime::Perbill;

        new_test_ext().execute_with(|| {
            assert_noop!(
                crate::Staking::set_min_commission(council_origin(3), Perbill::from_percent(10)),
                DispatchError::BadOrigin
            );
            assert_noop!(
                crate::Staking::set_min_commission(council_origin(4), Perbill::from_percent(10)),
                DispatchError::BadOrigin
            );
            assert_ok!(ValidatorAdmission::dispatch_as_root(
                council_origin(4),
                Box::new(RuntimeCall::Staking(
                    pallet_staking::Call::set_min_commission {
                        new: Perbill::from_percent(10),
                    }
                ))
            ));
            assert_eq!(
                pallet_staking::MinCommission::<Runtime>::get(),
                Perbill::from_percent(10)
            );
        });
    }
}

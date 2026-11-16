use crate::{AccountId, Runtime, RuntimeCall, RuntimeOrigin};
use frame_support::{assert_noop, assert_ok};
use pallet_validator_admission::{RootCallError, RootCallPolicy};
use sp_consensus_babe::{digests::NextConfigDescriptor, AllowedSlots, Slot};

#[test]
fn council_cannot_change_babe_configuration_directly_or_through_nested_calls() {
    super::super::validator_onboarding::tests::ext().execute_with(|| {
        let council = || {
            pallet_collective::RawOrigin::<AccountId, pallet_collective::Instance1>::Members(4, 6)
                .into()
        };
        for offset in [0, super::super::EpochDuration::get() - 64] {
            pallet_babe::CurrentSlot::<Runtime>::put(Slot::from(offset));
            for c in [(1, 4), (1, 2)] {
                let call = RuntimeCall::Babe(pallet_babe::Call::plan_config_change {
                    config: NextConfigDescriptor::V1 {
                        c,
                        allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots,
                    },
                });
                let nested = RuntimeCall::Utility(pallet_utility::Call::batch_all {
                    calls: vec![RuntimeCall::Utility(pallet_utility::Call::dispatch_as {
                        as_origin: Box::new(frame_system::RawOrigin::Root.into()),
                        call: Box::new(call.clone()),
                    })],
                });
                for call in [call, nested] {
                    assert_eq!(
                        super::super::slashing::CouncilRootCalls::check(&call),
                        Err(RootCallError::Denied)
                    );
                    assert_noop!(
                        crate::ValidatorAdmission::dispatch_as_root(council(), Box::new(call)),
                        pallet_validator_admission::Error::<Runtime>::RootCallDenied
                    );
                    assert!(pallet_babe::PendingEpochConfigChange::<Runtime>::get().is_none());
                }
            }
        }
        // The SDK Root-only API still exists. Ordinary validators cannot call
        // it, and the only governed Root dispatcher now rejects it above.
        let config = NextConfigDescriptor::V1 {
            c: (1, 2),
            allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots,
        };
        assert_noop!(
            crate::Babe::plan_config_change(
                RuntimeOrigin::signed(AccountId::new([1; 32])),
                config.clone()
            ),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(crate::Babe::plan_config_change(
            RuntimeOrigin::root(),
            config
        ));
    });
}

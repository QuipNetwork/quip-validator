use super::*;
use crate as pallet_validator_admission;
use frame_support::{assert_noop, assert_ok, derive_impl, traits::ConstU32};
use sp_runtime::BuildStorage;

frame_support::construct_runtime!(pub enum Test { System: frame_system, Admission: pallet_validator_admission });
#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = frame_system::mocking::MockBlock<Test>;
}
impl Config for Test {
    type FoundationOrigin =
        frame_system::EnsureSignedBy<frame_support::traits::IsInVec<Foundation>, u64>;
    type GovernanceCall = RuntimeCall;
    type MaxAuthorities = ConstU32<32>;
    type Graduation = ();
    type WeightInfo = ();
}
frame_support::parameter_types! { pub Foundation: Vec<u64> = vec![1]; }
pub(crate) fn ext() -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    GenesisConfig::<Test> {
        mode: AdmissionMode::FoundationOnly,
        approved: vec![2],
    }
    .assimilate_storage(&mut storage)
    .unwrap();
    let mut ext = sp_io::TestExternalities::new(storage);
    ext.execute_with(|| System::set_block_number(1));
    ext
}
#[test]
fn only_foundation_can_manage_admission_or_dispatch_root() {
    ext().execute_with(|| {
        assert_noop!(
            Admission::approve(RuntimeOrigin::signed(9), 3),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            Admission::approve(RuntimeOrigin::root(), 3),
            sp_runtime::DispatchError::BadOrigin
        );
        let call = Box::new(RuntimeCall::System(frame_system::Call::remark {
            remark: vec![],
        }));
        assert_noop!(
            Admission::dispatch_as_root(RuntimeOrigin::signed(9), call.clone()),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_ok!(Admission::dispatch_as_root(RuntimeOrigin::signed(1), call));
        System::assert_last_event(Event::RootDispatched { result: Ok(()) }.into());
    });
}
#[test]
fn admission_is_bounded_and_removal_releases_capacity() {
    ext().execute_with(|| {
        for who in 3..=33 {
            assert_ok!(Admission::approve(RuntimeOrigin::signed(1), who));
        }
        assert_eq!(Approved::<Test>::get().len(), 32);
        assert_noop!(
            Admission::approve(RuntimeOrigin::signed(1), 34),
            Error::<Test>::TooManyAuthorities
        );
        assert_noop!(
            Admission::approve(RuntimeOrigin::signed(1), 2),
            Error::<Test>::AlreadyApproved
        );
        assert_ok!(Admission::remove(RuntimeOrigin::signed(1), 2));
        assert_ok!(Admission::approve(RuntimeOrigin::signed(1), 34));
        assert_noop!(
            Admission::remove(RuntimeOrigin::signed(1), 2),
            Error::<Test>::NotApproved
        );
    });
}
#[test]
fn modes_are_one_way_and_graduation_is_closed() {
    ext().execute_with(|| {
        assert_noop!(
            Admission::advance(RuntimeOrigin::signed(1), AdmissionMode::FixedTestnet),
            Error::<Test>::InvalidTransition
        );
        assert_noop!(
            Admission::advance(
                RuntimeOrigin::signed(1),
                AdmissionMode::PermissionlessEligibility
            ),
            Error::<Test>::GraduationNotReady
        );
        Mode::<Test>::put(AdmissionMode::FixedTestnet);
        assert_noop!(
            Admission::approve(RuntimeOrigin::signed(1), 3),
            Error::<Test>::WrongMode
        );
        assert_noop!(
            Admission::remove(RuntimeOrigin::signed(1), 2),
            Error::<Test>::WrongMode
        );
        assert_ok!(Admission::advance(
            RuntimeOrigin::signed(1),
            AdmissionMode::FoundationOnly
        ));
        assert_noop!(
            Admission::advance(RuntimeOrigin::signed(1), AdmissionMode::FoundationOnly),
            Error::<Test>::InvalidTransition
        );
    });
}
#[test]
fn root_dispatch_records_inner_failure() {
    ext().execute_with(|| {
        let call = Box::new(RuntimeCall::Admission(Call::approve { who: 3 }));
        assert_ok!(Admission::dispatch_as_root(RuntimeOrigin::signed(1), call));
        System::assert_last_event(
            Event::RootDispatched {
                result: Err(sp_runtime::DispatchError::BadOrigin),
            }
            .into(),
        );
    });
}

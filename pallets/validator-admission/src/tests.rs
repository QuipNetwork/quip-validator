use super::*;
use crate as pallet_validator_admission;
use frame_support::{assert_noop, assert_ok, derive_impl, traits::ConstU32};
use sp_runtime::BuildStorage;

frame_support::construct_runtime!(pub enum Test { System: frame_system, Admission: pallet_validator_admission });
#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = frame_system::mocking::MockBlock<Test>;
    type SystemWeightInfo = CountingSystemWeights;
}

std::thread_local! {
    /// How many times `remark`'s weight function ran on this thread.
    static REMARK_WEIGHT_CALLS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

/// frame_system weights that count how often `remark`'s weight function is
/// evaluated: each `get_dispatch_info` of a `remark` evaluates it once, so a
/// call nesting a `remark` counts the leaf evaluations its own dispatch info
/// costs (W3-003). Every weight is zero.
pub struct CountingSystemWeights;
impl frame_system::WeightInfo for CountingSystemWeights {
    fn remark(_: u32) -> Weight {
        REMARK_WEIGHT_CALLS.with(|c| c.set(c.get().saturating_add(1)));
        Weight::zero()
    }
    fn remark_with_event(_: u32) -> Weight {
        Weight::zero()
    }
    fn set_heap_pages() -> Weight {
        Weight::zero()
    }
    fn set_code() -> Weight {
        Weight::zero()
    }
    fn set_storage(_: u32) -> Weight {
        Weight::zero()
    }
    fn kill_storage(_: u32) -> Weight {
        Weight::zero()
    }
    fn kill_prefix(_: u32) -> Weight {
        Weight::zero()
    }
    fn authorize_upgrade() -> Weight {
        Weight::zero()
    }
    fn apply_authorized_upgrade() -> Weight {
        Weight::zero()
    }
}
impl Config for Test {
    type FoundationOrigin =
        frame_system::EnsureSignedBy<frame_support::traits::IsInVec<Foundation>, u64>;
    type GovernanceCall = RuntimeCall;
    type MaxAuthorities = ConstU32<32>;
    type Graduation = ();
    type OnRemoval = ();
    type RootCallFilter = frame_support::traits::Everything;
    type RootCallFilterWeight = ();
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

/// W3-003: `dispatch_as_root`'s weight annotation must read its inner call's
/// dispatch info once. Reading it once for the weight and again for the class
/// doubles the work at every nesting level, so a call nesting `d` levels costs
/// 2^d leaf evaluations -- before dispatch, in fee estimation and in pool
/// validation, where the root-call depth limit does not apply. At depth 16 the
/// double read costs 65,536 evaluations of the leaf's weight; one read costs 1.
#[test]
fn nested_dispatch_as_root_reads_the_inner_dispatch_info_once() {
    use frame_support::dispatch::GetDispatchInfo;
    let depth = 16;
    let leaf = RuntimeCall::System(frame_system::Call::remark { remark: vec![] });
    let nested = (0..depth).fold(leaf.clone(), |call, _| {
        RuntimeCall::Admission(Call::dispatch_as_root {
            call: Box::new(call),
        })
    });
    REMARK_WEIGHT_CALLS.with(|c| c.set(0));
    let info = nested.get_dispatch_info();
    assert_eq!(
        REMARK_WEIGHT_CALLS.with(|c| c.get()),
        1,
        "the leaf's weight must be evaluated once for a {depth}-deep nest"
    );
    // One read still carries both halves: the leaf's class, and every level's
    // own weight on top of the leaf's (zero under these weights).
    assert_eq!(info.class, leaf.get_dispatch_info().class);
    let per_level = <() as WeightInfo>::dispatch_as_root()
        .saturating_add(<Test as Config>::RootCallFilterWeight::get());
    assert_eq!(info.call_weight, per_level.saturating_mul(depth));
}

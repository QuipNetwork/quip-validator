use crate::{mock::*, ChainId, Eip155ChainId, LOCAL_CHAIN_ID, TESTNET_CHAIN_ID};
use frame_support::traits::Get;

#[test]
fn genesis_persists_local_chain_id() {
    ext_with_chain_id(LOCAL_CHAIN_ID).execute_with(|| {
        assert_eq!(Eip155ChainId::<Test>::get(), LOCAL_CHAIN_ID);
        assert_eq!(<ChainId<Test> as Get<u64>>::get(), LOCAL_CHAIN_ID);
    });
}

#[test]
fn genesis_persists_testnet_chain_id() {
    ext_with_chain_id(TESTNET_CHAIN_ID).execute_with(|| {
        assert_eq!(Eip155ChainId::<Test>::get(), TESTNET_CHAIN_ID);
        assert_eq!(<ChainId<Test> as Get<u64>>::get(), TESTNET_CHAIN_ID);
    });
}

#[test]
#[should_panic(expected = "chain_id must be greater than 0")]
fn genesis_rejects_zero_chain_id() {
    let _ = ext_with_chain_id(0);
}

#[test]
fn unset_storage_returns_testnet_default() {
    empty_ext().execute_with(|| {
        assert_eq!(Eip155ChainId::<Test>::get(), TESTNET_CHAIN_ID);
        assert_eq!(<ChainId<Test> as Get<u64>>::get(), TESTNET_CHAIN_ID);
    });
}

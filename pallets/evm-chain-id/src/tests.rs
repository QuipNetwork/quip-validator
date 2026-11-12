use crate::{mock::*, ChainId, Eip155ChainId, LOCAL_CHAIN_ID, TESTNET_CHAIN_ID};
use frame_support::traits::Get;

#[test]
fn genesis_persists_chain_id() {
    let cases = [("local", LOCAL_CHAIN_ID), ("testnet", TESTNET_CHAIN_ID)];

    for (name, chain_id) in cases {
        ext_with_chain_id(chain_id).execute_with(|| {
            assert_eq!(Eip155ChainId::<Test>::get(), chain_id, "case: {name}");
            assert_eq!(<ChainId<Test> as Get<u64>>::get(), chain_id, "case: {name}");
        });
    }
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

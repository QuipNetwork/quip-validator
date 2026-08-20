use crate as pallet_evm_chain_id;
use frame_support::derive_impl;
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

#[frame_support::runtime]
mod runtime {
    #[runtime::runtime]
    #[runtime::derive(
        RuntimeCall,
        RuntimeEvent,
        RuntimeError,
        RuntimeOrigin,
        RuntimeFreezeReason,
        RuntimeHoldReason,
        RuntimeSlashReason,
        RuntimeLockId,
        RuntimeTask,
        RuntimeViewFunction
    )]
    pub struct Test;

    #[runtime::pallet_index(0)]
    pub type System = frame_system::Pallet<Test>;

    #[runtime::pallet_index(1)]
    pub type EvmChainId = pallet_evm_chain_id::Pallet<Test>;
}

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
}

impl pallet_evm_chain_id::Config for Test {}

/// Externalities with this pallet's genesis omitted, so the storage key is absent.
pub fn empty_ext() -> sp_io::TestExternalities {
    frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap()
        .into()
}

/// Externalities with an explicit genesis chain ID.
pub fn ext_with_chain_id(chain_id: u64) -> sp_io::TestExternalities {
    let mut storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();

    pallet_evm_chain_id::GenesisConfig::<Test> {
        chain_id,
        _config: core::marker::PhantomData,
    }
    .assimilate_storage(&mut storage)
    .unwrap();

    storage.into()
}

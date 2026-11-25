#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

// QUI-2549: benchmark builds open pallet-revive deployment to any signed
// account (`configs::ReviveDeployOrigin`). A release build must never carry
// that carve-out.
#[cfg(all(feature = "runtime-benchmarks", feature = "on-chain-release-build"))]
compile_error!(
    "`runtime-benchmarks` must not be combined with `on-chain-release-build`: benchmark \
     builds open pallet-revive deployment to any signed account (QUI-2549)"
);

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

pub mod apis;
#[cfg(feature = "runtime-benchmarks")]
mod benchmarks;
pub mod configs;
mod network_params;
pub mod weights;

extern crate alloc;
use alloc::vec::Vec;
use pallet_revive::evm::runtime::EthExtra;
use quip_transaction_crypto::HybridTxSignature;
use sp_runtime::{
    generic, impl_opaque_keys,
    traits::{BlakeTwo256, IdentifyAccount, Verify},
    MultiAddress,
};
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;

pub mod genesis_config_presets;

/// Opaque types. These are used by the CLI to instantiate machinery that don't need to know
/// the specifics of the runtime. They can then be made to be agnostic over specific formats
/// of data like extrinsics, allowing for them to continue syncing the network through upgrades
/// to even the core data structures.
pub mod opaque {
    use super::*;
    use sp_runtime::{
        generic,
        traits::{BlakeTwo256, Hash as HashT},
    };

    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

    /// Opaque block header type.
    pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
    /// Opaque block type.
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    /// Opaque block identifier type.
    pub type BlockId = generic::BlockId<Block>;
    /// Opaque block hash type.
    pub type Hash = <BlakeTwo256 as HashT>::Output;
}

impl_opaque_keys! {
    pub struct SessionKeys {
        pub babe: Babe,
        pub grandpa: Grandpa,
    }
}

/// Heap-boxed session keys keep `pallet_session::Call::set_keys` from making
/// the entire `RuntimeCall` enum larger than Utility's 1 KiB batching limit.
/// `Box<T>` and this single-field wrapper are SCALE-transparent, so the
/// existing `Session.set_keys` wire encoding is unchanged.
#[derive(
    Clone,
    PartialEq,
    Eq,
    codec::Encode,
    codec::Decode,
    codec::DecodeWithMemTracking,
    scale_info::TypeInfo,
    serde::Serialize,
    serde::Deserialize,
    Debug,
)]
pub struct BoxedSessionKeys(alloc::boxed::Box<SessionKeys>);

impl From<SessionKeys> for BoxedSessionKeys {
    fn from(keys: SessionKeys) -> Self {
        Self(alloc::boxed::Box::new(keys))
    }
}

impl sp_runtime::traits::OpaqueKeys for BoxedSessionKeys {
    type KeyTypeIdProviders = <SessionKeys as sp_runtime::traits::OpaqueKeys>::KeyTypeIdProviders;

    fn key_ids() -> &'static [sp_core::crypto::KeyTypeId] {
        SessionKeys::key_ids()
    }

    fn get_raw(&self, key_type: sp_core::crypto::KeyTypeId) -> &[u8] {
        self.0.get_raw(key_type)
    }

    fn ownership_proof_is_valid(&self, owner: &[u8], proof: &[u8]) -> bool {
        self.0.ownership_proof_is_valid(owner, proof)
    }
}

#[cfg(test)]
mod boxed_session_keys_tests {
    use super::{BoxedSessionKeys, SessionKeys};
    use codec::Encode;
    use quip_crypto_primitives::substrate::{
        ed25519_fndsa512::Pair as HybridGrandpaPair, sr25519_fndsa512::Pair as HybridBabePair,
    };
    use sp_core::Pair as _;

    #[test]
    fn boxed_session_keys_preserve_scale_wire_encoding() {
        let keys = SessionKeys {
            babe: HybridBabePair::from_string("//Alice", None)
                .expect("Alice BABE seed is valid")
                .public()
                .into(),
            grandpa: HybridGrandpaPair::from_string("//Alice", None)
                .expect("Alice GRANDPA seed is valid")
                .public()
                .into(),
        };
        let original_encoding = keys.encode();
        let boxed_encoding = BoxedSessionKeys::from(keys).encode();

        assert_eq!(boxed_encoding, original_encoding);
    }
}

// To learn more about runtime versioning, see:
// https://docs.substrate.io/main-docs/build/upgrade#runtime-versioning
#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: alloc::borrow::Cow::Borrowed("quip-testnet"),
    impl_name: alloc::borrow::Cow::Borrowed("quip"),
    authoring_version: 1,
    // The version of the runtime specification. A full node will not attempt to use its native
    //   runtime in substitute for the on-chain Wasm runtime unless all of `spec_name`,
    //   `spec_version`, and `authoring_version` are the same between Wasm and native.
    // Bumped to 101 (and `transaction_version` to 2) when the signed-extrinsic
    // wire format switched from `MultiSignature` to the hybrid envelope. Without
    // these bumps, peers/clients could treat the new format as the old one.
    // Bumped to 102 for v0.2.0: adds `pallet_faucet_ops` (idx 11) and
    // `pallet_session` (idx 12). New dispatchables, events, and storage entries
    // change the runtime metadata; the signed-extrinsic wire format is
    // unchanged, so `transaction_version` stays at 2.
    // Bumped to 103 for QUI-567: adds the canonical default plain Ising job
    // spec, root-gates `QuantumComputeMempool::register_job_spec`, and changes
    // that call's argument encoding, so `transaction_version` moves to 3.
    // Bumped to 104 for the topology-upgrade path: adds
    // `QuantumPow::set_default_topology` (call_index 5) and makes the
    // difficulty energy curve spec-aware (h/J magnitudes derived from the
    // default topology's allowed-value specs instead of hardcoded ternary-h /
    // binary-J). Existing call encodings are unchanged, so
    // `transaction_version` stays at 3.
    // Bumped to 105 for indexer-free quantum reads: adds monotonic qblock ids,
    // qblock/hardness runtime APIs, and the mempool open-order recovery index.
    // Existing call encodings are unchanged, so `transaction_version` stays at
    // 3.
    // Bumped to 106 for on-chain miner descriptors and qblock participation:
    // adds `MinerRegistry` (idx 13) with descriptor/participation calls,
    // events, and storage. Existing call encodings are unchanged, so
    // `transaction_version` stays at 3.
    // Bumped to 107 for the participants-per-qblock reverse index
    // (`ParticipantsByQBlock`, `ParticipantCountByQBlock`) and the
    // `MinerRegistryApi` runtime API. Call encodings are unchanged, so
    // `transaction_version` stays at 3.
    // Bumped to 108 for per-topology difficulty + the mineable-topology
    // whitelist: `QuantumPow.Difficulty` (global StorageValue) becomes
    // `Difficulties` (StorageMap keyed by topology hash), `MineableTopologies`
    // is added, `set_difficulty` gains a `topology_hash` argument, and
    // `add_mineable_topology`/`remove_mineable_topology` (call_index 6/7) are
    // added. `set_difficulty`'s argument encoding changed, so
    // `transaction_version` moves to 4. Pallet storage version 2 → 3 with a
    // carry-forward migration.
    // Bumped to 109 to restore on-chain `system_info`: `MinerRegistry` adds a
    // schema-v2 descriptor input (`NodeDescriptorInput::V2`) carrying an
    // optional typed hardware survey, plus a v1 → v2 storage migration that
    // drops existing descriptors (miners re-file on restart). The V1 call
    // variant keeps index 0 and encodes identically, so `transaction_version`
    // stays at 4. MinerRegistry pallet storage version 1 → 2.
    // Bumped to 110 to add the optional `runtime` block (node software identity:
    // python / quip_version / protocol_version / in_docker / docker_image) to
    // the MinerRegistry V2 descriptor. Additive trailing field on the V2 input;
    // V1 is unaffected and V2 was not yet deployed, so `transaction_version`
    // stays at 4 and no new migration is needed (the v1 → v2 migration already
    // wipes descriptors; pallet storage version stays 2).
    // Bumped to 111 (110 had already shipped in v0.2.1-rc11 when these
    // landed) for two QuantumPow changes — this is what the chain deployed:
    // - `QBlock` gains a trailing `topology_hash` so a block records which
    //   topology it was mined against. This changes the persisted `QBlocks`
    //   value layout, so QuantumPow pallet storage version goes 3 → 4 with a
    //   v3 → v4 migration that re-encodes existing entries, backfilling
    //   `topology_hash` with the default topology. Read-only runtime API
    //   shape change (`QBlock`/`QBlockWithNonce`). Includes the sudo-only
    //   per-topology curve `c` override (`set_topology_curve`, new call).
    // - `submit_proof` weight becomes dimension-scaled (QIP-03): charged
    //   weight now depends on the registered topology's node/edge counts and
    //   the proof's solution count instead of a flat 60M placeholder.
    // No call encodings changed in 111, so `transaction_version` stayed at 4.
    // Bumped to 112 (111 was already deployed when this landed):
    // `QuantumProof` gains a trailing `device_access_time_us: u64`
    // (miner-reported compute time: QPU access time for QPU wins, wall clock
    // for CPU/GPU), carried through `ProofRecord` and persisted as a trailing
    // field on `QBlock`. QuantumPow pallet storage version goes 4 → 5: the
    // deployed-v4 path appends `device_access_time_us = 0` preserving each
    // block's stored `topology_hash`; the pre-v4 path re-encodes from the
    // 7-field layout backfilling both trailing fields. Read-only runtime API
    // shape change (`QBlock`/`QBlockWithNonce`). `submit_proof`'s argument
    // encoding changed, so `transaction_version` moves to 5.
    // Bumped for pallet-revive (idx 14), its Ethereum runtime APIs, EVM-aware
    // unchecked-extrinsic wrapper, and `EthSetOrigin` transaction extension.
    // The extension set and accepted extrinsic forms change, so
    // `transaction_version` moves to 6. First shipped as 114 in the v0.2.2-rc
    // tags (113 was only an intermediate branch value and never released).
    // Bumped to 115 for the post-tag main build after the benchmark weight
    // regeneration. No call encodings changed, so `transaction_version` stays
    // at 6. 115 has not shipped. Later storage-only changes (pallet-evm-chain-id)
    // stay on 115 until a 115 runtime is live.
    // Bumped to 116 for the H2/H4 chain-wipe relaunch and to publish Metadata
    // V16 from the legacy metadata runtime API (`state_getMetadata`), which
    // previously returned the V14 inherent default. The versioned metadata API
    // keeps serving 14/15/16. The H1/H3 (ML-DSA-44) to H2/H4 (FN-DSA-512)
    // scheme change uses new public-key and signature encodings, so old signed
    // extrinsics are incompatible and `transaction_version` moves to 7.
    // Bumped to 117 to hard-invalidate 116 nodes: the crates.io pqhybridsign
    // rc5 switch and repins carry no interface change, but a spec bump makes
    // any node still on 116 refuse the new runtime outright.
    // Extended 117 with custody pallets starting at index 16, keeping the
    // existing extrinsic format, so transaction version 7 stayed. 117 shipped
    // on aglais in the v0.3.x tags; every consensus change after it bumps.
    // Bumped to 118: quantum-pow difficulty decay is continuous per block
    // instead of stepped per 100-block epoch and eases faster past the
    // target round length, per-win hardening is capped at one epoch of decay
    // and always nets harder at or under target, and the dominant-winner
    // easing rule is gone with its WinnerStreak storage (pallet storage v6).
    // Same calls; a 117 node computes a different threshold from identical
    // state, so the version must move. `transaction_version` stays at 7.
    // 118 also moves pallet-xqvm from the aglais git pin (xquad `3cf0b07`),
    // which 117 shipped, to crates.io xqvm 0.4.0. Stored programs use the
    // XQBC format, and execution results change: checked arithmetic, a VM
    // memory budget, per-step charges for the O(model) opcodes, XQMX/XSMX
    // rejecting k < 2 (the domain stays {0, ..., k-1}), sample-domain checks
    // on SETLINE/ADDLINE, model registers required by the quadratic opcodes
    // and joins, and allocations capped at u32::MAX. `store_program` now
    // verifies bytecode, bounds the CFG and takes a deposit; `execute` is
    // priced by program size and steps and rejects a zero step limit;
    // `Error<T>` has a variant per VM fault; `remove_program`/`evict_program`
    // are new calls. pallet-xqvm declares storage version 1 with no migration,
    // since no programs were stored before the wipe. Its weights were
    // regenerated on the reference machine for 0.4.0.
    // `store_program` and `execute` keep their call indices and arguments, so
    // `transaction_version` stays at 7.
    // 119 includes the fused faucet and fixed-schedule subnet emission controller.
    // QuantumPow decay starts after 100 blocks, and each win hardens
    // by a bounded step toward its achieved energy from the live threshold.
    // 119 carries three QuantumComputeMempool changes. `IsingParams` renames
    // `min_energy_milli` to `max_energy_milli` (same SCALE position and type).
    // `TopNWeighted` splits by rank instead of energy magnitude.
    // `on_initialize` finalises orders from their effective expiry block: new
    // `OrdersExpiringAt` and `NextExpiryBucket` storage, pallet storage v2 -> v3
    // with an index-building migration, and `ResultReady` plus `OrderResults`
    // move from `claim_reward` to expiry. Call encodings are unchanged, so
    // `transaction_version` stays at 7.
    // Bumped to 120 for V0 governance. `pallet-sudo` (index 6) is removed, so
    // Root is reachable only through the council's
    // `ValidatorAdmission::dispatch_as_root`; pallet index 6 is retired and
    // must not be reused. Membership changes, including removal of the
    // council's own members, require 2/3. No call encoding changes, so
    // `transaction_version` stays at 7.
    spec_version: 120,
    impl_version: 1,
    apis: apis::RUNTIME_API_VERSIONS,
    transaction_version: 7,
    system_version: 1,
};

mod block_times {
    /// This determines the average expected block time that we are targeting. Blocks will be
    /// produced at a minimum duration defined by `SLOT_DURATION`. `SLOT_DURATION` is picked up by
    /// `pallet_timestamp` which is in turn picked up by `pallet_babe`.
    ///
    /// Change this to adjust the block time.
    pub const MILLI_SECS_PER_BLOCK: u64 = 6000;

    // NOTE: Currently it is not possible to change the slot duration after the chain has started.
    // Attempting to do so will brick block production.
    pub const SLOT_DURATION: u64 = MILLI_SECS_PER_BLOCK;
}
pub use block_times::*;

// Time is measured by number of blocks.
pub const MINUTES: BlockNumber = 60_000 / (MILLI_SECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

pub const BLOCK_HASH_COUNT: BlockNumber = 2400;

// Unit = the base number of indivisible units for balances
pub const UNIT: Balance = 1_000_000_000_000;
pub const MILLI_UNIT: Balance = 1_000_000_000;
pub const MICRO_UNIT: Balance = 1_000_000;

/// Existential deposit.
pub const EXISTENTIAL_DEPOSIT: Balance = MILLI_UNIT;

/// The BABE epoch configuration at genesis.
pub const BABE_GENESIS_EPOCH_CONFIG: sp_consensus_babe::BabeEpochConfiguration =
    sp_consensus_babe::BabeEpochConfiguration {
        c: (1, 4),
        allowed_slots: sp_consensus_babe::AllowedSlots::PrimaryAndSecondaryPlainSlots,
    };

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

/// Hybrid transaction signature used for runtime extrinsics.
pub type Signature = HybridTxSignature;

/// Some way of identifying an account on the chain. We intentionally make it equivalent
/// to the public key of our transaction signing scheme.
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;

/// Balance of an account.
pub type Balance = u128;

/// Index of a transaction in the chain.
pub type Nonce = u32;

/// A hash of some data used by the chain.
pub type Hash = sp_core::H256;

/// An index to a block.
pub type BlockNumber = u32;

/// The address format for describing accounts.
pub type Address = MultiAddress<AccountId, ()>;

/// Block header type as expected by this runtime.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;

/// Block type as expected by this runtime.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;

/// A Block signed with a Justification
pub type SignedBlock = generic::SignedBlock<Block>;

/// BlockId type as expected by this runtime.
pub type BlockId = generic::BlockId<Block>;

/// The `TransactionExtension` to the basic transaction logic.
pub type TxExtension = (
    frame_system::AuthorizeCall<Runtime>,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
    frame_metadata_hash_extension::CheckMetadataHash<Runtime>,
    pallet_revive::evm::tx_extension::SetOrigin<Runtime>,
    frame_system::WeightReclaim<Runtime>,
);

fn tx_extension(
    era: generic::Era,
    nonce: Nonce,
    tip: Balance,
    revive_origin: pallet_revive::evm::tx_extension::SetOrigin<Runtime>,
) -> TxExtension {
    (
        frame_system::AuthorizeCall::<Runtime>::new(),
        frame_system::CheckNonZeroSender::<Runtime>::new(),
        frame_system::CheckSpecVersion::<Runtime>::new(),
        frame_system::CheckTxVersion::<Runtime>::new(),
        frame_system::CheckGenesis::<Runtime>::new(),
        frame_system::CheckEra::<Runtime>::from(era),
        frame_system::CheckNonce::<Runtime>::from(nonce),
        frame_system::CheckWeight::<Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
        frame_metadata_hash_extension::CheckMetadataHash::<Runtime>::new(false),
        revive_origin,
        frame_system::WeightReclaim::<Runtime>::new(),
    )
}

/// Construct the extension tuple used by Quip's native signed transactions.
/// Keeping this in the runtime prevents node-side transaction builders from
/// drifting when the ordered extension set changes.
pub fn native_tx_extension(era: generic::Era, nonce: Nonce, tip: Balance) -> TxExtension {
    tx_extension(era, nonce, tip, Default::default())
}

/// pallet-revive's dispatch-time limits that bound a legitimate
/// `eth_transact` (QUI-2548 follow-up, A10b). Revive keeps them in a private
/// module (`limits.rs` at the pinned SDK), so they are restated here and
/// pinned to revive's dispatch behaviour by the test
/// `revive_limits_restated_here_match_dispatch`:
/// - `BLOB_BYTES` (`limits.rs:113`): the largest contract blob an instantiate
///   may carry (`BlobTooLarge` above it);
/// - `CALLDATA_BYTES` (`limits.rs:71`): the largest call or constructor data
///   code is run with (`CallDataTooLarge` above it).
pub const REVIVE_BLOB_BYTES: u32 = 1024 * 1024;
pub const REVIVE_CALLDATA_BYTES: u32 = 128 * 1024;
/// RLP framing and signature around an Ethereum transaction's input: the
/// type byte, the list and input headers (at most 5 bytes each), six integer
/// fields (chain id, nonce, the gas prices, gas, value; at most 33 bytes
/// each), `to` (21) and the signature (at most 99): 334 bytes at most, so
/// 1 KiB leaves about 690 bytes for an access list.
pub const ETH_TRANSACT_RLP_OVERHEAD: u32 = 1024;
/// The largest `eth_transact` payload this runtime accepts (A10b):
/// revive's largest instantiate input (a `BLOB_BYTES` blob plus
/// `CALLDATA_BYTES` of constructor data -- revive's own
/// `MAX_TRANSACTION_PAYLOAD_SIZE`) plus the RLP framing, 1,180,672 bytes.
///
/// Enforced before the payload is RLP-decoded (see [`EthExtraImpl`]), because
/// the decode and signer recovery hold several further copies of the input: a
/// valid block with one 2 MiB + 768 B `eth_transact` and fillers held
/// 65.975 MiB in the importer, above `VM_BLOCK_RESIDENCY` (claude_reviewer's
/// A10 R8). Below the 2 MiB allocator order those copies stay small.
///
/// Nothing that can succeed at dispatch with code involved is larger: a call
/// into code is limited to `CALLDATA_BYTES` of input, a PVM instantiate to a
/// `BLOB_BYTES` blob plus `CALLDATA_BYTES` of data, and an EVM instantiate to
/// 49,152 bytes of init code (EIP-3860). Two kinds of transaction above the cap
/// are refused that dispatch would have accepted: a value transfer to an
/// account with no code carrying more input than the cap (the input is not
/// read), and an `eth_substrate_call` (`to` = the runtime-pallets address)
/// wrapping a call larger than the cap, which can be submitted natively.
pub const MAX_ETH_TRANSACT_PAYLOAD: u32 =
    REVIVE_BLOB_BYTES + REVIVE_CALLDATA_BYTES + ETH_TRANSACT_RLP_OVERHEAD;
/// `InvalidTransaction::Custom` code for an `eth_transact` payload over
/// [`MAX_ETH_TRANSACT_PAYLOAD`]. Custom rather than `ExhaustsResources`: the
/// payload is invalid in every block, not too large for this one, and block
/// authors treat `ExhaustsResources` as a sign the block is full.
pub const ETH_TRANSACT_PAYLOAD_TOO_LARGE: u8 = 1;

/// Revive's own conversion of an `eth_transact` into a checked extrinsic,
/// unchanged; [`EthExtraImpl`] wraps it with the payload cap.
struct ReviveEthExtra;

impl EthExtra for ReviveEthExtra {
    type Config = Runtime;
    type Extension = TxExtension;

    fn get_eth_extension(nonce: u32, tip: Balance) -> Self::Extension {
        tx_extension(
            generic::Era::Immortal,
            nonce,
            tip,
            pallet_revive::evm::tx_extension::SetOrigin::<Runtime>::new_from_eth_transaction(),
        )
    }
}

/// Builds the normal transaction extensions used by an Ethereum transaction
/// after Revive has recovered and validated its secp256k1 signer, and refuses
/// a payload over [`MAX_ETH_TRANSACT_PAYLOAD`] before anything decodes it.
/// `Checkable::check` calls this for every unsigned `eth_transact`, so the cap
/// holds in pool validation and in block import alike.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EthExtraImpl;

impl EthExtra for EthExtraImpl {
    type Config = Runtime;
    type Extension = TxExtension;

    fn get_eth_extension(nonce: u32, tip: Balance) -> Self::Extension {
        <ReviveEthExtra as EthExtra>::get_eth_extension(nonce, tip)
    }

    fn try_into_checked_extrinsic(
        payload: &[u8],
        encoded_len: usize,
    ) -> Result<
        generic::CheckedExtrinsic<AccountId, RuntimeCall, TxExtension>,
        sp_runtime::transaction_validity::InvalidTransaction,
    > {
        if payload.len() > MAX_ETH_TRANSACT_PAYLOAD as usize {
            return Err(
                sp_runtime::transaction_validity::InvalidTransaction::Custom(
                    ETH_TRANSACT_PAYLOAD_TOO_LARGE,
                ),
            );
        }
        <ReviveEthExtra as EthExtra>::try_into_checked_extrinsic(payload, encoded_len)
    }
}

/// Unchecked extrinsic type as expected by this runtime.
pub type UncheckedExtrinsic =
    pallet_revive::evm::runtime::UncheckedExtrinsic<Address, Signature, EthExtraImpl>;

/// The payload being signed in transactions.
pub type SignedPayload = generic::SignedPayload<RuntimeCall, TxExtension>;

/// Executive: handles dispatch to the various modules.
///
/// Runtime 120 launches from fresh genesis, so legacy migrations are removed.
/// Future in-place upgrades add migrations normally; storage versions remain.
pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

#[cfg(test)]
mod mempool_vm_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use codec::Encode;
    use quip_transaction_crypto::{account_id_from_public, HybridPair, HybridTxSignature};
    use sp_core::Pair as _;
    use sp_runtime::{traits::Checkable, transaction_validity::InvalidTransaction, BuildStorage};

    fn signed_test_extrinsic(
        sender: &HybridPair,
        address: Address,
        call: RuntimeCall,
        nonce: u32,
    ) -> UncheckedExtrinsic {
        let tx_ext = native_tx_extension(generic::Era::Immortal, nonce, 0);

        let payload = SignedPayload::new(call.clone(), tx_ext.clone()).unwrap();
        let signature = payload.using_encoded(|encoded| HybridTxSignature::sign(sender, encoded));

        generic::UncheckedExtrinsic::new_signed(call, address, signature, tx_ext).into()
    }

    #[test]
    fn hybrid_signed_extrinsic_checks_successfully() {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let sender = HybridPair::from_string("//Alice", None).unwrap();
            let account_id = account_id_from_public(&sender.public());
            let xt = signed_test_extrinsic(
                &sender,
                account_id.clone().into(),
                SystemCall::remark { remark: vec![] }.into(),
                0,
            );

            let lookup = frame_system::ChainContext::<Runtime>::default();
            let checked =
                <UncheckedExtrinsic as Checkable<frame_system::ChainContext<Runtime>>>::check(
                    xt, &lookup,
                );

            assert!(checked.is_ok());
        });
    }

    // QUI-2548 (owner decision of 2026-11-25): `BlockLength` is Normal 3.5 MiB,
    // Operational 3.75 MiB, Mandatory 5 MiB, enforced by `CheckWeight` when a
    // transaction is validated.
    mod block_length {
        use super::*;
        use frame_support::dispatch::{DispatchClass, GetDispatchInfo};
        use sp_runtime::transaction_validity::{TransactionSource, TransactionValidity};

        fn ext_with_funded(who: &AccountId) -> sp_io::TestExternalities {
            let mut ext = sp_io::TestExternalities::new(
                RuntimeGenesisConfig::default().build_storage().unwrap(),
            );
            ext.execute_with(|| {
                System::set_block_number(1);
                let _ =
                    <Balances as frame_support::traits::Currency<AccountId>>::make_free_balance_be(
                        who,
                        1_000_000_000 * UNIT,
                    );
            });
            ext
        }

        fn validate(xt: UncheckedExtrinsic) -> TransactionValidity {
            Executive::validate_transaction(TransactionSource::External, xt, System::block_hash(0))
        }

        /// The payload that makes `make(payload)`'s encoding exactly `len`
        /// bytes: from 2^14 up every compact prefix is four bytes, so the
        /// encoding grows byte for byte with the payload.
        fn payload_for_len(make: &dyn Fn(usize) -> UncheckedExtrinsic, len: usize) -> usize {
            let probe = 64 * 1024;
            let overhead = make(probe).encode().len() - probe;
            let payload = len - overhead;
            assert_eq!(make(payload).encode().len(), len);
            payload
        }

        #[test]
        fn block_length_limits_are_the_qui_2548_decision() {
            let lengths = configs::RuntimeBlockLength::get();
            assert_eq!(*lengths.max.get(DispatchClass::Normal), 3_670_016);
            assert_eq!(*lengths.max.get(DispatchClass::Operational), 3_932_160);
            assert_eq!(*lengths.max.get(DispatchClass::Mandatory), 5_242_880);
        }

        /// Each class admits an extrinsic that fills its limit exactly and
        /// refuses one byte more with `ExhaustsResources`: a signed `set_code`
        /// and a `Foundation::propose` wrapping a remark (both Operational, and
        /// both included even when they fail at dispatch), and a plain remark
        /// (Normal). `CheckWeight` counts the whole block against the limit:
        /// the empty header and digest that `frame_system::initialize` books
        /// (`BlockSize`), plus every extrinsic.
        #[test]
        fn extrinsics_over_their_class_block_length_are_refused_at_validation() {
            let sender = HybridPair::from_string("//Alice", None).unwrap();
            let who = account_id_from_public(&sender.public());
            let sign =
                |call: RuntimeCall| signed_test_extrinsic(&sender, who.clone().into(), call, 0);
            let remark = |p: usize| -> RuntimeCall {
                SystemCall::remark {
                    remark: vec![0u8; p],
                }
                .into()
            };
            let set_code = |p: usize| sign(SystemCall::set_code { code: vec![0u8; p] }.into());
            let propose = |p: usize| {
                sign(
                    pallet_collective::Call::<Runtime, pallet_collective::Instance1>::propose {
                        threshold: 2,
                        proposal: Box::new(remark(p)),
                        // What a submitter would pass; `u32::MAX` would inflate
                        // `propose`'s weight past the Operational limit.
                        length_bound: remark(p).encoded_size() as u32,
                    }
                    .into(),
                )
            };
            let normal = |p: usize| sign(remark(p));
            let cases: [(
                &str,
                &dyn Fn(usize) -> UncheckedExtrinsic,
                DispatchClass,
                u32,
            ); 3] = [
                (
                    "signed set_code",
                    &set_code,
                    DispatchClass::Operational,
                    configs::OPERATIONAL_BLOCK_LENGTH,
                ),
                (
                    "Foundation::propose(remark)",
                    &propose,
                    DispatchClass::Operational,
                    configs::OPERATIONAL_BLOCK_LENGTH,
                ),
                (
                    "remark",
                    &normal,
                    DispatchClass::Normal,
                    configs::NORMAL_BLOCK_LENGTH,
                ),
            ];
            for (label, make, class, cap) in cases {
                ext_with_funded(&who).execute_with(|| {
                    // `validate_transaction` initializes the next block with an
                    // empty digest.
                    let header = <Header as sp_runtime::traits::Header>::new(
                        System::block_number() + 1,
                        Default::default(),
                        Default::default(),
                        System::block_hash(0),
                        Default::default(),
                    );
                    let overhead =
                        header.encoded_size() + sp_runtime::Digest::default().encoded_size();
                    let fits = payload_for_len(make, cap as usize - overhead);
                    assert_eq!(
                        make(fits).0.function.get_dispatch_info().class,
                        class,
                        "{label}"
                    );
                    assert!(
                        validate(make(fits)).is_ok(),
                        "{label}: {} B + the {overhead}-B header fills the {cap}-B limit and is \
                         admitted: {:?}",
                        cap as usize - overhead,
                        validate(make(fits))
                    );
                    assert_eq!(
                        validate(make(fits + 1)),
                        Err(InvalidTransaction::ExhaustsResources.into()),
                        "{label}: one byte more is refused"
                    );
                });
            }
        }

        /// The runtime upgrade path still fits: the Foundation's
        /// `dispatch_as_root(set_code)` carrying this runtime's real Wasm is
        /// Operational and admitted at validation.
        #[test]
        fn foundation_runtime_upgrade_fits_the_operational_block_length() {
            let wasm = WASM_BINARY.expect("tests build the Wasm runtime");
            let sender = HybridPair::from_string("//Alice", None).unwrap();
            let who = account_id_from_public(&sender.public());
            ext_with_funded(&who).execute_with(|| {
                pallet_membership::Members::<Runtime, pallet_membership::Instance1>::put(
                    frame_support::BoundedVec::try_from(vec![who.clone()]).unwrap(),
                );
                let xt = signed_test_extrinsic(
                    &sender,
                    who.clone().into(),
                    pallet_validator_admission::Call::<Runtime>::dispatch_as_root {
                        call: Box::new(SystemCall::set_code { code: wasm.to_vec() }.into()),
                    }
                    .into(),
                    0,
                );
                let len = xt.encode().len();
                println!(
                    "runtime upgrade extrinsic: {len} B (Wasm {} B); Operational limit {} B, {} B spare",
                    wasm.len(),
                    configs::OPERATIONAL_BLOCK_LENGTH,
                    configs::OPERATIONAL_BLOCK_LENGTH as usize - len
                );
                assert!(wasm.len() > 1024 * 1024, "the real runtime Wasm");
                assert_eq!(xt.0.function.get_dispatch_info().class, DispatchClass::Operational);
                assert!(validate(xt.clone()).is_ok(), "{:?}", validate(xt));
            });
        }
    }

    // A10b (QUI-2548 follow-up): the `eth_transact` payload cap.
    mod eth_transact_cap {
        use super::*;
        use frame_support::assert_err_ignore_postinfo;
        use pallet_revive::evm::{
            Account as EthAccount, Bytes, TransactionLegacyUnsigned, TransactionUnsigned,
        };
        use sp_core::{H160, U256};
        use sp_runtime::transaction_validity::{TransactionSource, TransactionValidity};

        const CAP: usize = MAX_ETH_TRANSACT_PAYLOAD as usize;

        fn validate(xt: UncheckedExtrinsic) -> TransactionValidity {
            Executive::validate_transaction(TransactionSource::External, xt, System::block_hash(0))
        }

        fn bare(payload: Vec<u8>) -> UncheckedExtrinsic {
            generic::UncheckedExtrinsic::<Address, RuntimeCall, Signature, TxExtension>::new_bare(
                pallet_revive::Call::<Runtime>::eth_transact { payload }.into(),
            )
            .into()
        }

        /// The signed RLP payload of a legacy transaction from `eth`.
        fn payload(eth: &EthAccount, to: Option<H160>, input: Vec<u8>, gas: U256) -> Vec<u8> {
            use frame_support::traits::Get as _;
            let tx = TransactionLegacyUnsigned {
                chain_id: Some(U256::from(
                    <Runtime as pallet_revive::Config>::ChainId::get(),
                )),
                gas,
                gas_price: pallet_revive::Pallet::<Runtime>::evm_base_fee(),
                input: Bytes(input),
                nonce: U256::zero(),
                to,
                value: U256::zero(),
                ..Default::default()
            };
            eth.sign_transaction(TransactionUnsigned::TransactionLegacyUnsigned(tx))
                .signed_payload()
        }

        /// A signed call to an account with no code whose payload is exactly
        /// `len` bytes: the input is sized from the measured framing, which can
        /// move by a byte with the signature's leading zeros.
        fn call_of_len(eth: &EthAccount, len: usize, gas: U256) -> Vec<u8> {
            let to = Some(H160::repeat_byte(0x11));
            let mut input = len - 512;
            for _ in 0..8 {
                let p = payload(eth, to, vec![0x5a; input], gas);
                if p.len() == len {
                    return p;
                }
                input = input + len - p.len();
            }
            panic!("no input gives a {len}-B payload");
        }

        fn eth_ext(eth: &EthAccount) -> sp_io::TestExternalities {
            let mut ext = revive_ext();
            ext.execute_with(|| funded(&eth.substrate_account()));
            ext
        }

        /// A gas limit the fee checks accept for a payload at the cap.
        fn gas_for_cap(eth: &EthAccount) -> U256 {
            [
                100_000_000u64,
                1_000_000_000,
                10_000_000_000,
                100_000_000_000,
            ]
            .into_iter()
            .map(U256::from)
            .find(|&gas| validate(bare(call_of_len(eth, CAP, gas))).is_ok())
            .expect("some gas limit admits a payload at the cap")
        }

        #[test]
        fn the_cap_is_revives_largest_instantiate_plus_framing() {
            assert_eq!(REVIVE_BLOB_BYTES + REVIVE_CALLDATA_BYTES, 1_179_648);
            assert_eq!(MAX_ETH_TRANSACT_PAYLOAD, 1_180_672);
        }

        /// `cap - 1` and `cap` are admitted; `cap + 1` is refused with
        /// `Custom(ETH_TRANSACT_PAYLOAD_TOO_LARGE)`, and before any decode: a
        /// payload that is not even RLP gets the same refusal at `cap + 1`, and
        /// a decode failure (`Call`) at `cap`.
        #[test]
        fn eth_transact_payload_cap_is_enforced_at_validation() {
            let eth = EthAccount::default();
            eth_ext(&eth).execute_with(|| {
                let gas = gas_for_cap(&eth);
                for len in [CAP - 1, CAP] {
                    let xt = bare(call_of_len(&eth, len, gas));
                    assert!(validate(xt.clone()).is_ok(), "{len} B: {:?}", validate(xt));
                }
                let too_large = InvalidTransaction::Custom(ETH_TRANSACT_PAYLOAD_TOO_LARGE).into();
                assert_eq!(
                    validate(bare(call_of_len(&eth, CAP + 1, gas))),
                    Err(too_large)
                );
                assert_eq!(validate(bare(vec![0xff; CAP + 1])), Err(too_large));
                assert_eq!(
                    validate(bare(vec![0xff; CAP])),
                    Err(InvalidTransaction::Call.into())
                );
            });
        }

        /// The largest instantiate revive accepts at dispatch -- a PVM blob of
        /// exactly `BLOB_BYTES` (a header naming that length) plus
        /// `CALLDATA_BYTES` of constructor data -- is still admitted.
        #[test]
        fn largest_legitimate_instantiate_is_admitted() {
            let eth = EthAccount::default();
            eth_ext(&eth).execute_with(|| {
                let gas = gas_for_cap(&eth);
                let blob = REVIVE_BLOB_BYTES as usize;
                let mut input = vec![0u8; blob + REVIVE_CALLDATA_BYTES as usize];
                input[..4].copy_from_slice(b"PVM\0");
                input[5..13].copy_from_slice(&(blob as u64).to_le_bytes());
                let p = payload(&eth, None, input, gas);
                assert!(p.len() <= CAP && p.len() > CAP - 1024, "{} B", p.len());
                let xt = bare(p);
                assert!(validate(xt.clone()).is_ok(), "{:?}", validate(xt));
            });
        }

        /// Pins the restated limits to revive's dispatch: a blob one byte over
        /// `REVIVE_BLOB_BYTES` is `BlobTooLarge` and one at it is not, and call
        /// data one byte over `REVIVE_CALLDATA_BYTES` is `CallDataTooLarge`
        /// and at it is not.
        #[test]
        fn revive_limits_restated_here_match_dispatch() {
            use pallet_revive::Error;
            revive_ext().execute_with(|| {
                let deployer = configs::ReviveRootDeployer::get();
                funded(&deployer);
                let blob = |len: usize| {
                    let mut code = vec![0u8; len];
                    code[..4].copy_from_slice(b"PVM\0");
                    code
                };
                let upload =
                    |len| Revive::upload_code(RuntimeOrigin::root(), blob(len), 1_000_000 * UNIT);
                assert_eq!(
                    upload(REVIVE_BLOB_BYTES as usize + 1),
                    Err(Error::<Runtime>::BlobTooLarge.into())
                );
                assert_ne!(
                    upload(REVIVE_BLOB_BYTES as usize),
                    Err(Error::<Runtime>::BlobTooLarge.into())
                );

                assert_ok!(instantiate_call(evm_initcode(&[0x00]), 9)
                    .dispatch_bypass_filter(RuntimeOrigin::root()));
                let contract = System::events()
                    .iter()
                    .find_map(|record| match &record.event {
                        RuntimeEvent::Revive(pallet_revive::Event::Instantiated {
                            contract,
                            ..
                        }) => Some(*contract),
                        _ => None,
                    })
                    .expect("the contract is instantiated");
                let caller = signer("//Alice");
                funded(&caller);
                assert_ok!(Revive::map_account(RuntimeOrigin::signed(caller.clone())));
                let call = |len: usize| {
                    Revive::call(
                        RuntimeOrigin::signed(caller.clone()),
                        contract,
                        0,
                        Weight::from_parts(10_000_000_000, 10 * 1024 * 1024),
                        1_000 * UNIT,
                        vec![0u8; len],
                    )
                };
                assert_err_ignore_postinfo!(
                    call(REVIVE_CALLDATA_BYTES as usize + 1),
                    Error::<Runtime>::CallDataTooLarge
                );
                assert_ok!(call(REVIVE_CALLDATA_BYTES as usize));
            });
        }
    }

    // QUI-2549: pallet-revive deployment is restricted to Root and Foundation
    // members (`configs::ReviveDeployOrigin`). These run in the normal build,
    // which pins the restriction against the `runtime-benchmarks` carve-out.
    use frame_support::{
        assert_ok, dispatch::DispatchResultWithPostInfo, traits::UnfilteredDispatchable,
        weights::Weight,
    };

    /// claude_reviewer's EVM initcode wrapper (`z_revive_evm3`): copy the
    /// runtime code that follows a 12-byte prefix, and return it.
    fn evm_initcode(runtime: &[u8]) -> Vec<u8> {
        let len = u8::try_from(runtime.len()).expect("short runtime code");
        let mut code = vec![
            0x60, len, 0x60, 0x0C, 0x60, 0x00, 0x39, 0x60, len, 0x60, 0x00, 0xF3,
        ];
        code.extend_from_slice(runtime);
        code
    }

    /// claude_reviewer's recursion contract (`z_revive_evm3`, "mem 1 MiB +
    /// self-recursion"): touch memory at 1 MiB, then call itself with all
    /// remaining gas. The shape whose import the VM budget does not cover.
    fn recursion_contract() -> Vec<u8> {
        let mut runtime = vec![0x60_u8, 0x01, 0x62, 0x0F, 0xFF, 0xFF, 0x53];
        runtime.extend_from_slice(&[
            0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x60, 0x00, 0x30, 0x5A, 0xF1,
        ]);
        runtime.push(0x00);
        evm_initcode(&runtime)
    }

    fn revive_ext() -> sp_io::TestExternalities {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| System::set_block_number(1));
        ext
    }

    fn funded(who: &AccountId) {
        use frame_support::traits::fungible::Mutate;
        let _ = <Balances as Mutate<AccountId>>::set_balance(who, 1_000_000 * UNIT);
    }

    fn signer(seed: &str) -> AccountId {
        account_id_from_public(&HybridPair::from_string(seed, None).unwrap().public())
    }

    fn instantiate_call(code: Vec<u8>, salt: u8) -> pallet_revive::Call<Runtime> {
        pallet_revive::Call::<Runtime>::instantiate_with_code {
            value: 0,
            weight_limit: Weight::from_parts(10_000_000_000, 10 * 1024 * 1024),
            storage_deposit_limit: 1_000 * UNIT,
            code,
            data: vec![],
            salt: Some([salt; 32]),
        }
    }

    fn instantiated(deployer: &AccountId) -> bool {
        use pallet_revive::AddressMapper;
        let address = <Runtime as pallet_revive::Config>::AddressMapper::to_address(deployer);
        System::events().iter().any(|record| {
            matches!(
                &record.event,
                RuntimeEvent::Revive(pallet_revive::Event::Instantiated { deployer: d, .. })
                    if *d == address
            )
        })
    }

    /// A signed account outside the Foundation cannot deploy through any path:
    /// `upload_code`, `instantiate_with_code` (claude_reviewer's recursion
    /// contract among them), `instantiate` of an existing hash, the Ethereum
    /// create path (`eth_instantiate_with_code`, what `eth_transact` with no
    /// `to` becomes) and `eth_substrate_call` wrapping a deploy.
    #[test]
    fn revive_deployment_is_refused_to_non_members_on_every_path() {
        use frame_support::{assert_err_ignore_postinfo, assert_noop};
        use sp_runtime::DispatchError::BadOrigin;
        revive_ext().execute_with(|| {
            let outsider = signer("//Alice");
            funded(&outsider);
            assert_ok!(Revive::map_account(RuntimeOrigin::signed(outsider.clone())));
            let signed = || RuntimeOrigin::signed(outsider.clone());

            assert_noop!(
                Revive::upload_code(signed(), evm_initcode(&[0x00]), 1_000 * UNIT),
                BadOrigin
            );
            for (salt, code) in [(1, evm_initcode(&[0x00])), (2, recursion_contract())] {
                let call = instantiate_call(code, salt);
                assert_err_ignore_postinfo!(call.dispatch_bypass_filter(signed()), BadOrigin);
            }
            assert_err_ignore_postinfo!(
                Revive::instantiate(
                    signed(),
                    0,
                    Weight::from_parts(10_000_000_000, 10 * 1024 * 1024),
                    1_000 * UNIT,
                    sp_core::H256::repeat_byte(7),
                    vec![],
                    None,
                ),
                BadOrigin
            );

            // The Ethereum paths: `eth_transact` is converted to these calls
            // with the `EthTransaction` origin, which becomes the signer.
            let eth = || {
                RuntimeOrigin::from(pallet_revive::Origin::<Runtime>::EthTransaction(
                    outsider.clone(),
                ))
            };
            let created = Revive::eth_instantiate_with_code(
                eth(),
                0u32.into(),
                Weight::from_parts(10_000_000_000, 10 * 1024 * 1024),
                u64::MAX.into(),
                recursion_contract(),
                vec![],
                vec![],
                0u32.into(),
                0,
            );
            let wrapped = Revive::eth_substrate_call(
                eth(),
                Box::new(instantiate_call(evm_initcode(&[0x00]), 3).into()),
                vec![],
            );
            let refused = |result: &DispatchResultWithPostInfo| {
                result.as_ref().err().map(|e| e.error) == Some(BadOrigin)
                    || System::events().iter().any(|record| {
                        matches!(
                            &record.event,
                            RuntimeEvent::Revive(pallet_revive::Event::EthExtrinsicRevert {
                                dispatch_error: BadOrigin
                            })
                        )
                    })
            };
            assert!(refused(&created), "eth create: {created:?}");
            assert!(refused(&wrapped), "eth_substrate_call: {wrapped:?}");

            // `dispatch_as_fallback_account` dispatches its inner call as the
            // outsider's (non-member) fallback account.
            let fallback = Revive::dispatch_as_fallback_account(
                signed(),
                Box::new(instantiate_call(evm_initcode(&[0x00]), 4).into()),
            );
            assert_eq!(fallback.map_err(|e| e.error), Err(BadOrigin));

            // The dry-run API reports the same refusal.
            use pallet_revive::runtime_decl_for_revive_api::ReviveApi as _;
            let dry_run = <Runtime>::upload_code(outsider.clone(), evm_initcode(&[0x00]), None);
            assert_eq!(dry_run.map(|_| ()), Err(BadOrigin));
            assert!(!instantiated(&outsider), "no contract was deployed");
        });
    }

    /// A Foundation member deploys from its own signed extrinsic and owns the
    /// contract.
    #[test]
    fn revive_deployment_is_open_to_foundation_members() {
        revive_ext().execute_with(|| {
            let member = signer("//Bob");
            funded(&member);
            pallet_membership::Members::<Runtime, pallet_membership::Instance1>::put(
                frame_support::BoundedVec::try_from(vec![member.clone()]).unwrap(),
            );
            assert_ok!(Revive::map_account(RuntimeOrigin::signed(member.clone())));
            assert_ok!(instantiate_call(evm_initcode(&[0x00]), 1)
                .dispatch_bypass_filter(RuntimeOrigin::signed(member.clone())));
            assert!(instantiated(&member));
        });
    }

    /// Root -- a council motion through `dispatch_as_root` -- deploys as the
    /// keyless `ReviveRootDeployer`: refused cleanly while it is unfunded (the
    /// storage deposit cannot be held), and accepted once governance funds it.
    #[test]
    fn revive_root_deploys_as_the_funded_root_deployer() {
        revive_ext().execute_with(|| {
            let council: RuntimeOrigin = pallet_collective::RawOrigin::<
                AccountId,
                pallet_collective::Instance1,
            >::Members(4, 6)
            .into();
            let deployer = configs::ReviveRootDeployer::get();
            let root_deploy = |salt: u8| {
                assert_ok!(ValidatorAdmission::dispatch_as_root(
                    council.clone(),
                    Box::new(instantiate_call(evm_initcode(&[0x00]), salt).into()),
                ));
                match System::events().last().map(|r| r.event.clone()) {
                    Some(RuntimeEvent::ValidatorAdmission(
                        pallet_validator_admission::Event::RootDispatched { result },
                    )) => result,
                    other => panic!("expected RootDispatched, got {other:?}"),
                }
            };
            assert!(
                root_deploy(1).is_err(),
                "an unfunded Root deployer cannot deploy"
            );
            assert!(!instantiated(&deployer));
            funded(&deployer);
            assert_eq!(root_deploy(2), Ok(()));
            assert!(instantiated(&deployer));
        });
    }

    /// Confirms the runtime's `CanonicalDefaultIsingSpecId` resolves to the
    /// same hash that the pallet's mock test pins. SDKs and downstream docs
    /// embed this hash; a mock-vs-runtime divergence would silently break
    /// every client that hardcodes it.
    #[test]
    fn default_ising_spec_id_matches_pinned_hash() {
        use frame_support::traits::Get as _;
        let id = <Runtime as pallet_quantum_compute_mempool::Config>::DefaultIsingSpecId::get();
        assert_eq!(
            format!("{id:?}"),
            "0x8f46f3a31321d1d093314fc769c42cbe7a83d71a0b69e6571a0f68e2a04067f0",
        );
    }

    #[test]
    fn hybrid_signed_extrinsic_rejects_wrong_account() {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let sender = HybridPair::from_string("//Alice", None).unwrap();
            let wrong = HybridPair::from_string("//Bob", None).unwrap();
            let wrong_account = account_id_from_public(&wrong.public());
            let xt = signed_test_extrinsic(
                &sender,
                wrong_account.into(),
                SystemCall::remark { remark: vec![] }.into(),
                0,
            );

            let lookup = frame_system::ChainContext::<Runtime>::default();
            let checked =
                <UncheckedExtrinsic as Checkable<frame_system::ChainContext<Runtime>>>::check(
                    xt, &lookup,
                );

            assert_eq!(checked.unwrap_err(), InvalidTransaction::BadProof.into());
        });
    }

    #[test]
    fn balances_support_plain_reserve_round_trip() {
        use frame_support::traits::{Currency, ReservableCurrency};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            let account =
                account_id_from_public(&HybridPair::from_string("//Alice", None).unwrap().public());
            let reserve = 5 * UNIT;
            <Balances as Currency<AccountId>>::make_free_balance_be(&account, 10 * UNIT);

            assert!(Balances::reserve(&account, reserve).is_ok());
            assert_eq!(Balances::reserved_balance(&account), reserve);
            assert_eq!(Balances::unreserve(&account, reserve), 0);
            assert_eq!(Balances::reserved_balance(&account), 0);
        });
    }

    #[test]
    fn permissionless_reward_settlement_charges_signed_caller() {
        use frame_support::{
            assert_ok,
            traits::{Currency, Hooks},
        };
        use pallet_quantum_compute_mempool::{
            types::{Formulation, JobMode, MinerType, ResultDelivery, RewardResolution},
            JobOrders, Solvers,
        };
        use sp_runtime::traits::Hash as _;

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| {
            System::set_block_number(1);
            let signer = HybridPair::from_string("//Alice", None).unwrap();
            let caller = account_id_from_public(&signer.public());
            let proposer = AccountId::new([1; 32]);
            let winner = AccountId::new([2; 32]);
            let initial = 100 * UNIT;
            let reward = 2 * UNIT;
            for account in [&caller, &proposer, &winner] {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, initial);
            }
            let name: frame_support::BoundedVec<u8, frame_support::traits::ConstU32<128>> =
                b"settlement-fees".to_vec().try_into().unwrap();
            let spec_id = <Runtime as frame_system::Config>::Hashing::hash_of(&(
                name.clone(), Formulation::Ising, None::<Hash>, None::<Hash>,
            ));
            assert_ok!(QuantumComputeMempool::register_job_spec(
                RuntimeOrigin::root(), proposer.clone(), name, Formulation::Ising, None, None,
            ));
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(winner.clone()), MinerType::Cpu,
            ));
            assert_ok!(QuantumComputeMempool::propose_job(
                RuntimeOrigin::signed(proposer.clone()),
                spec_id,
                pallet_quantum_compute_mempool::types::IsingParams {
                    nodes: vec![0, 1].try_into().unwrap(),
                    edges: vec![(0, 1)].try_into().unwrap(),
                    h_values: vec![0, 0].try_into().unwrap(),
                    j_values: vec![-1_000].try_into().unwrap(),
                    max_energy_milli: None,
                    min_diversity_milli: None,
                    min_solutions: None,
                },
                reward, JobMode::Open, RewardResolution::SingleBest, 2, 1,
                ResultDelivery::OnChainOnly,
            ));
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(winner.clone()), 0,
                vec![vec![1, 1].try_into().unwrap()].try_into().unwrap(),
            ));
            System::set_block_number(2);
            QuantumComputeMempool::on_initialize(2);
            assert!(!Solvers::<Runtime>::contains_key(&caller));
            let call = pallet_quantum_compute_mempool::Call::claim_reward { order_id: 0 }.into();
            let xt = signed_test_extrinsic(&signer, Address::Id(caller.clone()), call, 0);
            let expected_fee = TransactionPayment::query_info(xt.clone(), xt.encode().len() as u32)
                .partial_fee;
            assert!(expected_fee > 0);
            assert_ok!(Executive::apply_extrinsic(xt).unwrap());
            assert_eq!(Balances::free_balance(&caller), initial - expected_fee);
            assert_eq!(Balances::free_balance(&winner), initial - EXISTENTIAL_DEPOSIT + reward);
            assert_eq!(Balances::free_balance(&proposer), initial - reward);
            assert_eq!(Balances::reserved_balance(&proposer), 0);
            assert!(!Solvers::<Runtime>::contains_key(&caller));
            assert_eq!(Solvers::<Runtime>::get(&winner).unwrap().rewards_earned, reward);
            assert_eq!(JobOrders::<Runtime>::get(0).unwrap().status,
                pallet_quantum_compute_mempool::types::OrderStatus::Closed);
            assert!(System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::TransactionPayment(pallet_transaction_payment::Event::TransactionFeePaid {
                    who, actual_fee, tip,
                }) if who == &caller && *actual_fee == expected_fee && *tip == 0
            )));
        });
    }

    fn prepare_two_winner_settlement(
        proposer: &AccountId,
        winners: &[AccountId; 2],
        reward: Balance,
    ) -> Hash {
        use frame_support::assert_ok;
        use pallet_quantum_compute_mempool::types::{
            Formulation, JobMode, MinerType, ResultDelivery, RewardResolution,
        };
        use sp_runtime::traits::Hash as _;

        let name: frame_support::BoundedVec<u8, frame_support::traits::ConstU32<128>> =
            b"settlement-rollback".to_vec().try_into().unwrap();
        let spec_id = <Runtime as frame_system::Config>::Hashing::hash_of(&(
            name.clone(),
            Formulation::Ising,
            None::<Hash>,
            None::<Hash>,
        ));
        assert_ok!(QuantumComputeMempool::register_job_spec(
            RuntimeOrigin::root(),
            proposer.clone(),
            name,
            Formulation::Ising,
            None,
            None,
        ));
        for winner in winners {
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(winner.clone()),
                MinerType::Cpu,
            ));
        }
        assert_ok!(QuantumComputeMempool::propose_job(
            RuntimeOrigin::signed(proposer.clone()),
            spec_id,
            pallet_quantum_compute_mempool::types::IsingParams {
                nodes: vec![0, 1].try_into().unwrap(),
                edges: vec![(0, 1)].try_into().unwrap(),
                h_values: vec![0, 0].try_into().unwrap(),
                j_values: vec![-1_000].try_into().unwrap(),
                max_energy_milli: None,
                min_diversity_milli: None,
                min_solutions: None,
            },
            reward,
            JobMode::Bid {
                miners: Some(winners.to_vec().try_into().unwrap()),
                miner_types: None
            },
            RewardResolution::TopNEqual { n: 2 },
            2,
            1,
            ResultDelivery::CallbackWithPoll {
                endpoint: b"https://solver.example/poll".to_vec().try_into().unwrap()
            },
        ));
        for (index, winner) in winners.iter().enumerate() {
            let spins = if index == 0 { vec![1, 1] } else { vec![1, -1] };
            assert_ok!(QuantumComputeMempool::submit_solution(
                RuntimeOrigin::signed(winner.clone()),
                0,
                vec![spins.try_into().unwrap()].try_into().unwrap(),
            ));
        }
        spec_id
    }

    #[test]
    fn signed_settlement_failure_rolls_back_payouts_but_keeps_fee_and_nonce() {
        use frame_support::{
            assert_ok,
            traits::{Currency, Hooks, LockableCurrency, WithdrawReasons},
        };
        use pallet_quantum_compute_mempool::{
            JobOrders, JobSpecs, OpenOrders, OrderResults, Solvers,
        };

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| {
            System::set_block_number(1);
            let signer = HybridPair::from_string("//Alice", None).unwrap();
            let caller = account_id_from_public(&signer.public());
            let proposer = AccountId::new([1; 32]);
            let winners = [AccountId::new([2; 32]), AccountId::new([3; 32])];
            let initial = 100 * UNIT;
            let reward = 2 * UNIT + 1;
            for account in [&caller, &proposer, &winners[0], &winners[1]] {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, initial);
            }
            let spec_id = prepare_two_winner_settlement(&proposer, &winners, reward);
            // W1-006: settlement pays from the order's escrow account, which
            // never carries a lock in production; one is placed only to force a
            // later payout to fail. The first payout (UNIT+1) leaves UNIT, which
            // covers the UNIT/2 lock; the second (UNIT) then crosses it.
            let escrow = QuantumComputeMempool::escrow_account(0);
            Balances::set_lock(*b"settle!!", &escrow, UNIT / 2, WithdrawReasons::TRANSFER);
            System::set_block_number(2);
            QuantumComputeMempool::on_initialize(2);
            System::reset_events();
            let order_before = JobOrders::<Runtime>::get(0).unwrap().encode();
            let spec_before = JobSpecs::<Runtime>::get(spec_id).unwrap();
            let solvers_before = winners.each_ref().map(|w| Solvers::<Runtime>::get(w).unwrap());
            let open_before = OpenOrders::<Runtime>::get(0);
            let result_before = OrderResults::<Runtime>::get(0);
            assert!(result_before.is_some(), "hook must publish before settlement");
            assert!(open_before.is_none());
            assert_eq!(JobOrders::<Runtime>::get(0).unwrap().status,
                pallet_quantum_compute_mempool::types::OrderStatus::Expired);
            assert!(!Solvers::<Runtime>::contains_key(&caller));
            let call: RuntimeCall = pallet_quantum_compute_mempool::Call::claim_reward { order_id: 0 }.into();
            let xt = signed_test_extrinsic(&signer, Address::Id(caller.clone()), call.clone(), 0);
            let failed_fee = TransactionPayment::query_info(xt.clone(), xt.encode().len() as u32).partial_fee;
            let error: sp_runtime::DispatchError = sp_runtime::TokenError::Frozen.into();
            assert_eq!(Executive::apply_extrinsic(xt).unwrap(), Err(error));

            assert_eq!(Balances::free_balance(&proposer), initial - reward);
            assert_eq!(Balances::free_balance(&escrow), reward);
            for (winner, solver_before) in winners.iter().zip(&solvers_before) {
                assert_eq!(Balances::free_balance(winner), initial - EXISTENTIAL_DEPOSIT);
                assert_eq!(Solvers::<Runtime>::get(winner).unwrap(), *solver_before);
            }
            assert_eq!(JobOrders::<Runtime>::get(0).unwrap().encode(), order_before);
            assert_eq!(JobSpecs::<Runtime>::get(spec_id).unwrap(), spec_before);
            assert_eq!(OpenOrders::<Runtime>::get(0), open_before);
            assert_eq!(OrderResults::<Runtime>::get(0), result_before);
            assert!(failed_fee > 0);
            assert_eq!(Balances::free_balance(&caller), initial - failed_fee);
            assert_eq!(System::account_nonce(&caller), 1);
            let events = System::events();
            assert!(!events.iter().any(|record| matches!(&record.event,
                RuntimeEvent::QuantumComputeMempool(_) |
                RuntimeEvent::Balances(pallet_balances::Event::Transfer { .. } | pallet_balances::Event::Unreserved { .. })
            )));
            assert!(events.iter().any(|record| matches!(&record.event,
                RuntimeEvent::System(frame_system::Event::ExtrinsicFailed { dispatch_error, .. })
                    if dispatch_error == &error
            )));
            assert!(events.iter().any(|record| matches!(&record.event,
                RuntimeEvent::TransactionPayment(pallet_transaction_payment::Event::TransactionFeePaid { who, actual_fee, tip })
                    if who == &caller && *actual_fee == failed_fee && *tip == 0
            )));

            Balances::remove_lock(*b"settle!!", &escrow);
            System::reset_events();
            let retry = signed_test_extrinsic(&signer, Address::Id(caller.clone()), call.clone(), 1);
            let retry_fee = TransactionPayment::query_info(retry.clone(), retry.encode().len() as u32).partial_fee;
            assert_ok!(Executive::apply_extrinsic(retry).unwrap());
            assert_eq!(System::account_nonce(&caller), 2);
            assert_eq!(Balances::free_balance(&caller), initial - failed_fee - retry_fee);
            assert_eq!(Balances::free_balance(&escrow), 0);
            assert!(!System::account_exists(&escrow));
            assert_eq!(Balances::free_balance(&proposer), initial - reward);
            for (winner, payout) in winners.iter().zip([UNIT + 1, UNIT]) {
                assert_eq!(Balances::free_balance(winner), initial - EXISTENTIAL_DEPOSIT + payout);
                assert_eq!(Solvers::<Runtime>::get(winner).unwrap().rewards_earned, payout);
            }
            assert_eq!(JobSpecs::<Runtime>::get(spec_id).unwrap().successful_orders, 1);
            assert!(!OpenOrders::<Runtime>::contains_key(0));
            assert_eq!(JobOrders::<Runtime>::get(0).unwrap().status,
                pallet_quantum_compute_mempool::types::OrderStatus::Closed);
            assert_eq!(OrderResults::<Runtime>::get(0).unwrap().winners.len(), 2);
            assert_eq!(System::events().iter().filter(|record| matches!(&record.event,
                RuntimeEvent::QuantumComputeMempool(pallet_quantum_compute_mempool::Event::OrderClosed { successful: true, .. })
            )).count(), 1);
            let repeat = signed_test_extrinsic(&signer, Address::Id(caller.clone()), call, 2);
            assert_eq!(Executive::apply_extrinsic(repeat).unwrap(),
                Err(pallet_quantum_compute_mempool::Error::<Runtime>::AlreadyClaimed.into()));
            assert_eq!(JobSpecs::<Runtime>::get(spec_id).unwrap().successful_orders, 1);
            assert_eq!(Balances::free_balance(&winners[0]), initial - EXISTENTIAL_DEPOSIT + UNIT + 1);
            assert_eq!(Balances::free_balance(&winners[1]), initial - EXISTENTIAL_DEPOSIT + UNIT);
        });
    }

    #[test]
    fn settlement_pays_from_escrow_even_after_the_proposer_is_reaped() {
        // W1-006: the reward sits in the order's escrow account, so settlement
        // never touches the proposer's balance. A proposer who empties and
        // reaps their own account after posting cannot block or shrink the
        // payout, and settlement burns nothing.
        use frame_support::{
            assert_ok,
            traits::{Currency, Hooks},
        };
        use pallet_quantum_compute_mempool::{JobSpecs, OrderResults, Solvers};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| {
            System::set_block_number(1);
            let proposer = AccountId::new([1; 32]);
            let winners = [AccountId::new([2; 32]), AccountId::new([3; 32])];
            let caller = AccountId::new([4; 32]);
            let reward = 2 * UNIT;
            <Balances as Currency<AccountId>>::make_free_balance_be(&proposer, 100 * UNIT);
            for winner in &winners {
                <Balances as Currency<AccountId>>::make_free_balance_be(
                    winner,
                    2 * EXISTENTIAL_DEPOSIT,
                );
            }
            let spec_id = prepare_two_winner_settlement(&proposer, &winners, reward);
            let escrow = QuantumComputeMempool::escrow_account(0);
            assert_eq!(Balances::free_balance(&escrow), reward);
            // The proposer empties its account after posting and is reaped.
            <Balances as Currency<AccountId>>::make_free_balance_be(&proposer, 0);
            assert!(!System::account_exists(&proposer));
            System::set_block_number(2);
            QuantumComputeMempool::on_initialize(2);
            System::reset_events();
            let issuance_before = Balances::total_issuance();
            // Direct dispatch isolates settlement from signed-extrinsic fees.
            assert_ok!(QuantumComputeMempool::claim_reward(
                RuntimeOrigin::signed(caller),
                0
            ));
            assert!(!System::account_exists(&proposer));
            assert_eq!(Balances::total_issuance(), issuance_before);
            assert_eq!(Balances::free_balance(&escrow), 0);
            assert!(!System::account_exists(&escrow));
            for winner in &winners {
                assert_eq!(Balances::free_balance(winner), EXISTENTIAL_DEPOSIT + UNIT);
                assert_eq!(Balances::reserved_balance(winner), EXISTENTIAL_DEPOSIT);
                assert_eq!(
                    Solvers::<Runtime>::get(winner).unwrap().rewards_earned,
                    UNIT
                );
            }
            assert_eq!(
                JobSpecs::<Runtime>::get(spec_id).unwrap().successful_orders,
                1
            );
            assert_eq!(OrderResults::<Runtime>::get(0).unwrap().winners.len(), 2);
            assert!(!System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::Balances(pallet_balances::Event::DustLost { .. })
            )));
        });
    }

    #[test]
    fn settlement_proof_envelope_and_normal_dispatch_limits_are_valid() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Hooks};
        use pallet_quantum_compute_mempool::weights::{
            claim_reward_proof_envelope, CLAIM_REWARD_PROOF_FLOOR,
        };
        <QuantumComputeMempool as Hooks<BlockNumber>>::integrity_test();
        assert_eq!(claim_reward_proof_envelope::<Runtime>(), 1_008_157);
        assert_eq!(
            pallet_quantum_compute_mempool::weights::reclaim_order_proof_envelope::<Runtime>(),
            671_247
        );
        let submission =
            pallet_quantum_compute_mempool::weights::submit_solution_proof_envelope::<Runtime>();
        assert_eq!(submission, 803_680);
        let proposal =
            pallet_quantum_compute_mempool::weights::propose_job_proof_envelope::<Runtime>();
        assert_eq!(proposal, 691_400);
        assert!(submission <= pallet_quantum_compute_mempool::weights::SUBMIT_SOLUTION_PROOF_FLOOR);
        let call: RuntimeCall =
            pallet_quantum_compute_mempool::Call::claim_reward { order_id: 0 }.into();
        let info = call.get_dispatch_info();
        assert!(info.call_weight.proof_size() >= CLAIM_REWARD_PROOF_FLOOR);
        let limits = <Runtime as frame_system::Config>::BlockWeights::get();
        let normal = limits.get(frame_support::dispatch::DispatchClass::Normal);
        assert!(info.call_weight.all_lte(normal.max_extrinsic.unwrap()));
    }

    #[test]
    fn signed_payee_batch_binds_signer_and_rolls_back_failed_correlation() {
        use frame_support::{assert_ok, traits::Currency};
        use pallet_quantum_compute_mempool::{
            types::*, NextOrderId, OrderPayeeRequirements, OrderPayees,
        };
        let mut genesis = RuntimeGenesisConfig::default();
        genesis.quantum_compute_mempool.default_ising_spec_builder = Some(AccountId::new([1; 32]));
        let mut ext = sp_io::TestExternalities::new(genesis.build_storage().unwrap());
        ext.execute_with(|| {
            System::set_block_number(1);
            let signer = HybridPair::from_string("//Alice", None).unwrap();
            let caller = account_id_from_public(&signer.public());
            <Balances as Currency<AccountId>>::make_free_balance_be(&caller, 100 * UNIT);
            assert_ok!(QuantumComputeMempool::register_solver(
                RuntimeOrigin::signed(caller.clone()),
                MinerType::Cpu
            ));
            let spec = QuantumComputeMempool::default_ising_spec_id();
            // Runtime genesis must seed the canonical spec and empty payee maps.
            assert!(pallet_quantum_compute_mempool::JobSpecs::<Runtime>::contains_key(spec));
            assert!(OrderPayees::<Runtime>::iter().next().is_none());
            assert!(OrderPayeeRequirements::<Runtime>::iter().next().is_none());
            for (nonce, chain_id) in [(0, 1), (1, 20033)] {
                let destination = WinnerPayee::V1 {
                    chain_id,
                    address: sp_core::H160::repeat_byte(7),
                };
                let calls = vec![
                    RuntimeCall::System(frame_system::Call::remark_with_event {
                        remark: b"correlation-test".to_vec(),
                    }),
                    RuntimeCall::QuantumComputeMempool(
                        pallet_quantum_compute_mempool::Call::propose_job_with_payee_requirement {
                            spec_id: spec,
                            ising_params: IsingParams {
                                nodes: vec![0, 1].try_into().unwrap(),
                                edges: vec![(0, 1)].try_into().unwrap(),
                                h_values: vec![0, 0].try_into().unwrap(),
                                j_values: vec![-1000].try_into().unwrap(),
                                max_energy_milli: None,
                                min_diversity_milli: None,
                                min_solutions: None,
                            },
                            reward: UNIT,
                            mode: JobMode::Open,
                            resolution: RewardResolution::SingleBest,
                            deadline_blocks: 10,
                            block_wait: 5,
                            delivery: ResultDelivery::OnChainOnly,
                            required_chain_id: 20033,
                        },
                    ),
                    RuntimeCall::QuantumComputeMempool(
                        pallet_quantum_compute_mempool::Call::submit_solution_with_payee {
                            order_id: 0,
                            solutions: vec![vec![1, 1].try_into().unwrap()].try_into().unwrap(),
                            payee: destination,
                        },
                    ),
                ];
                let call = RuntimeCall::Utility(pallet_utility::Call::batch_all { calls });
                let xt = signed_test_extrinsic(&signer, Address::Id(caller.clone()), call, nonce);
                let result = Executive::apply_extrinsic(xt).unwrap();
                if nonce == 0 {
                    assert_eq!(
                        result.unwrap_err(),
                        pallet_quantum_compute_mempool::Error::<Runtime>::PayeeChainMismatch.into()
                    );
                    assert_eq!(NextOrderId::<Runtime>::get(), 0);
                    assert_eq!(Balances::reserved_balance(&caller), EXISTENTIAL_DEPOSIT);
                    assert!(OrderPayeeRequirements::<Runtime>::get(0).is_none());
                    assert!(OrderPayees::<Runtime>::get(0, &caller).is_none());
                    assert!(!System::events().iter().any(|event| matches!(
                        event.event,
                        RuntimeEvent::System(frame_system::Event::Remarked { .. })
                    )));
                } else {
                    assert_ok!(result);
                    assert_eq!(NextOrderId::<Runtime>::get(), 1);
                    // W1-006: the reward is in the order's escrow account; the
                    // caller's reserve is only its solver deposit.
                    assert_eq!(Balances::reserved_balance(&caller), EXISTENTIAL_DEPOSIT);
                    assert_eq!(
                        Balances::free_balance(QuantumComputeMempool::escrow_account(0)),
                        UNIT
                    );
                    assert_eq!(OrderPayees::<Runtime>::get(0, &caller), Some(destination));
                    assert_eq!(OrderPayees::<Runtime>::iter_prefix(0).count(), 1);
                    assert_eq!(OrderPayeeRequirements::<Runtime>::get(0), Some(20033));
                }
            }
        });
    }

    #[test]
    fn production_proposer_limit_counts_outstanding_orders() {
        use frame_support::{
            assert_noop, assert_ok,
            traits::{Currency, Hooks},
        };
        use pallet_quantum_compute_mempool::{types::*, JobSpecs, ProposerOrders};
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| {
            System::set_block_number(1);
            let proposer = AccountId::new([1; 32]);
            assert!(ProposerOrders::<Runtime>::get(&proposer).is_empty());
            assert_eq!(
                <Runtime as pallet_quantum_compute_mempool::Config>::MaxOrdersPerProposer::get(),
                32
            );
            <Balances as Currency<AccountId>>::make_free_balance_be(&proposer, 100 * UNIT);
            assert_ok!(QuantumComputeMempool::register_job_spec(
                RuntimeOrigin::root(),
                proposer.clone(),
                b"cap-test".to_vec().try_into().unwrap(),
                Formulation::Ising,
                None,
                None
            ));
            let spec = JobSpecs::<Runtime>::iter_keys().next().unwrap();
            let propose = || {
                QuantumComputeMempool::propose_job(
                    RuntimeOrigin::signed(proposer.clone()),
                    spec,
                    IsingParams {
                        nodes: vec![0, 1].try_into().unwrap(),
                        edges: vec![(0, 1)].try_into().unwrap(),
                        h_values: vec![0, 0].try_into().unwrap(),
                        j_values: vec![-1000].try_into().unwrap(),
                        max_energy_milli: None,
                        min_diversity_milli: None,
                        min_solutions: None,
                    },
                    UNIT,
                    JobMode::Open,
                    RewardResolution::SingleBest,
                    1,
                    1,
                    ResultDelivery::OnChainOnly,
                )
            };
            for _ in 0..32 {
                assert_ok!(propose());
            }
            assert_noop!(
                propose(),
                pallet_quantum_compute_mempool::Error::<Runtime>::OrderLimitReached
            );
            for block in 2..=6 {
                System::set_block_number(block);
                QuantumComputeMempool::on_initialize(block);
            }
            assert_eq!(ProposerOrders::<Runtime>::get(&proposer).len(), 32);
            assert_ok!(QuantumComputeMempool::reclaim_order(
                RuntimeOrigin::signed(proposer.clone()),
                0
            ));
            assert_ok!(propose());
            assert_eq!(ProposerOrders::<Runtime>::get(&proposer).len(), 32);
        });
    }

    #[test]
    fn h4_multisig_uses_hash_approval_then_executes_full_call() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Currency, weights::Weight};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let target = account_id_from_public(
                &HybridPair::from_string("//Dave", None).unwrap().public(),
            );
            let mut signatories = vec![
                account_id_from_public(&alice.public()),
                account_id_from_public(&bob.public()),
                account_id_from_public(&charlie.public()),
            ];
            signatories.sort();

            for account in &signatories {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }
            let multisig = Multisig::multi_account_id(&signatories, 2);
            <Balances as Currency<AccountId>>::make_free_balance_be(&multisig, 10 * UNIT);

            let inner: RuntimeCall = BalancesCall::transfer_allow_death {
                dest: Address::Id(target.clone()),
                value: 3 * UNIT,
            }
            .into();
            let call_hash = sp_io::hashing::blake2_256(&inner.encode());

            let first_account = account_id_from_public(&alice.public());
            let mut first_others = signatories
                .iter()
                .filter(|account| **account != first_account)
                .cloned()
                .collect::<Vec<_>>();
            first_others.sort();
            let approval: RuntimeCall = pallet_multisig::Call::approve_as_multi {
                threshold: 2,
                other_signatories: first_others,
                maybe_timepoint: None,
                call_hash,
                max_weight: Weight::zero(),
            }
            .into();
            let approval_xt = signed_test_extrinsic(
                &alice,
                Address::Id(first_account),
                approval,
                0,
            );
            let approval_size = approval_xt.encode().len();
            assert!(Executive::apply_extrinsic(approval_xt).unwrap().is_ok());

            let timepoint = pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash)
                .expect("first approval creates multisig state")
                .when;
            let final_account = account_id_from_public(&bob.public());
            let mut final_others = signatories
                .iter()
                .filter(|account| **account != final_account)
                .cloned()
                .collect::<Vec<_>>();
            final_others.sort();
            let final_call: RuntimeCall = pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: final_others,
                maybe_timepoint: Some(timepoint),
                max_weight: inner.get_dispatch_info().call_weight,
                call: alloc::boxed::Box::new(inner),
            }
            .into();
            let final_xt =
                signed_test_extrinsic(&bob, Address::Id(final_account), final_call, 0);
            let final_size = final_xt.encode().len();
            assert!(Executive::apply_extrinsic(final_xt).unwrap().is_ok());

            eprintln!(
                "H4 multisig extrinsic sizes: approve_as_multi={approval_size}, as_multi={final_size}"
            );
            assert_eq!(Balances::free_balance(target), 3 * UNIT);
            assert!(pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash).is_none());
        });
    }

    #[test]
    fn h4_utility_batch_all_and_derivative_transfer_work() {
        use frame_support::traits::Currency;
        use sp_core::crypto::Ss58Codec;

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let alice_account = account_id_from_public(&alice.public());
            <Balances as Currency<AccountId>>::make_free_balance_be(&alice_account, 100 * UNIT);

            let recipients = ["//Bob", "//Charlie", "//Dave"].map(|uri| {
                account_id_from_public(&HybridPair::from_string(uri, None).unwrap().public())
            });
            let calls = recipients
                .iter()
                .enumerate()
                .map(|(index, recipient)| {
                    BalancesCall::transfer_allow_death {
                        dest: Address::Id(recipient.clone()),
                        value: (index as Balance + 1) * UNIT,
                    }
                    .into()
                })
                .collect();
            let batch: RuntimeCall = pallet_utility::Call::batch_all { calls }.into();
            let batch_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), batch, 0);
            assert!(Executive::apply_extrinsic(batch_xt).unwrap().is_ok());
            assert_eq!(Balances::free_balance(&recipients[0]), UNIT);
            assert_eq!(Balances::free_balance(&recipients[1]), 2 * UNIT);
            assert_eq!(Balances::free_balance(&recipients[2]), 3 * UNIT);

            let derivative = pallet_utility::derivative_account_id(alice_account.clone(), 7);
            <Balances as Currency<AccountId>>::make_free_balance_be(&derivative, 5 * UNIT);
            let derivative_target =
                account_id_from_public(&HybridPair::from_string("//Eve", None).unwrap().public());
            let derivative_call: RuntimeCall = pallet_utility::Call::as_derivative {
                index: 7,
                call: alloc::boxed::Box::new(
                    BalancesCall::transfer_allow_death {
                        dest: Address::Id(derivative_target.clone()),
                        value: 2 * UNIT,
                    }
                    .into(),
                ),
            }
            .into();
            let derivative_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account), derivative_call, 1);
            assert!(Executive::apply_extrinsic(derivative_xt).unwrap().is_ok());

            eprintln!(
                "utility derivative index 7 address: {}",
                derivative.to_ss58check()
            );
            assert_eq!(Balances::free_balance(derivative_target), 2 * UNIT);
        });
    }

    #[test]
    fn runtime_call_fits_utility_batching_limit() {
        assert!(core::mem::size_of::<RuntimeCall>() <= 1024);
    }

    #[test]
    fn h4_multisig_can_execute_utility_batch() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Currency, weights::Weight};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let mut signatories = vec![
                account_id_from_public(&alice.public()),
                account_id_from_public(&bob.public()),
                account_id_from_public(&charlie.public()),
            ];
            signatories.sort();
            for account in &signatories {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }
            let multisig = Multisig::multi_account_id(&signatories, 2);
            <Balances as Currency<AccountId>>::make_free_balance_be(&multisig, 10 * UNIT);

            let recipients = ["//Dave", "//Eve"].map(|uri| {
                account_id_from_public(&HybridPair::from_string(uri, None).unwrap().public())
            });
            let inner: RuntimeCall = pallet_utility::Call::batch_all {
                calls: recipients
                    .iter()
                    .map(|recipient| {
                        BalancesCall::transfer_allow_death {
                            dest: Address::Id(recipient.clone()),
                            value: 2 * UNIT,
                        }
                        .into()
                    })
                    .collect(),
            }
            .into();
            let call_hash = sp_io::hashing::blake2_256(&inner.encode());

            let alice_account = account_id_from_public(&alice.public());
            let mut alice_others = signatories
                .iter()
                .filter(|account| **account != alice_account)
                .cloned()
                .collect::<Vec<_>>();
            alice_others.sort();
            let approval: RuntimeCall = pallet_multisig::Call::approve_as_multi {
                threshold: 2,
                other_signatories: alice_others,
                maybe_timepoint: None,
                call_hash,
                max_weight: Weight::zero(),
            }
            .into();
            let approval_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account), approval, 0);
            assert!(Executive::apply_extrinsic(approval_xt).unwrap().is_ok());

            let timepoint = pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash)
                .expect("first approval creates multisig state")
                .when;
            let bob_account = account_id_from_public(&bob.public());
            let mut bob_others = signatories
                .iter()
                .filter(|account| **account != bob_account)
                .cloned()
                .collect::<Vec<_>>();
            bob_others.sort();
            let execution: RuntimeCall = pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: bob_others,
                maybe_timepoint: Some(timepoint),
                max_weight: inner.get_dispatch_info().call_weight,
                call: alloc::boxed::Box::new(inner),
            }
            .into();
            let execution_xt = signed_test_extrinsic(&bob, Address::Id(bob_account), execution, 0);
            assert!(Executive::apply_extrinsic(execution_xt).unwrap().is_ok());

            assert_eq!(Balances::free_balance(&recipients[0]), 2 * UNIT);
            assert_eq!(Balances::free_balance(&recipients[1]), 2 * UNIT);
        });
    }

    #[test]
    fn h4_proxy_filters_calls_rejects_announcements_and_creates_pure_accounts() {
        use frame_support::traits::Currency;
        use sp_runtime::traits::{BlakeTwo256, Hash as _};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let alice_account = account_id_from_public(&alice.public());
            let bob_account = account_id_from_public(&bob.public());
            let charlie_account = account_id_from_public(&charlie.public());
            for account in [&alice_account, &bob_account, &charlie_account] {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }

            let add_immediate: RuntimeCall = pallet_proxy::Call::add_proxy {
                delegate: Address::Id(bob_account.clone()),
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 0,
            }
            .into();
            let add_immediate_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), add_immediate, 0);
            assert!(Executive::apply_extrinsic(add_immediate_xt)
                .unwrap()
                .is_ok());

            let recipient =
                account_id_from_public(&HybridPair::from_string("//Dave", None).unwrap().public());
            let transfer: RuntimeCall = BalancesCall::transfer_allow_death {
                dest: Address::Id(recipient.clone()),
                value: 4 * UNIT,
            }
            .into();
            let proxied_transfer: RuntimeCall = pallet_proxy::Call::proxy {
                real: Address::Id(alice_account.clone()),
                force_proxy_type: Some(configs::ProxyType::TransferOnly),
                call: alloc::boxed::Box::new(transfer),
            }
            .into();
            let transfer_xt =
                signed_test_extrinsic(&bob, Address::Id(bob_account.clone()), proxied_transfer, 0);
            assert!(Executive::apply_extrinsic(transfer_xt).unwrap().is_ok());
            assert_eq!(Balances::free_balance(&recipient), 4 * UNIT);

            let rejected: RuntimeCall = pallet_proxy::Call::proxy {
                real: Address::Id(alice_account.clone()),
                force_proxy_type: Some(configs::ProxyType::TransferOnly),
                call: alloc::boxed::Box::new(SystemCall::remark { remark: vec![1] }.into()),
            }
            .into();
            let rejected_xt =
                signed_test_extrinsic(&bob, Address::Id(bob_account.clone()), rejected, 1);
            assert!(Executive::apply_extrinsic(rejected_xt).unwrap().is_ok());
            assert!(System::events().iter().any(|record| matches!(
                &record.event,
                RuntimeEvent::Proxy(pallet_proxy::Event::ProxyExecuted { result: Err(_) })
            )));

            let add_delayed: RuntimeCall = pallet_proxy::Call::add_proxy {
                delegate: Address::Id(charlie_account.clone()),
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 3,
            }
            .into();
            let add_delayed_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), add_delayed, 1);
            assert!(Executive::apply_extrinsic(add_delayed_xt).unwrap().is_ok());

            let delayed_call: RuntimeCall = BalancesCall::transfer_keep_alive {
                dest: Address::Id(recipient),
                value: UNIT,
            }
            .into();
            let delayed_hash = BlakeTwo256::hash_of(&alloc::boxed::Box::new(delayed_call.clone()));
            let announce: RuntimeCall = pallet_proxy::Call::announce {
                real: Address::Id(alice_account.clone()),
                call_hash: delayed_hash,
            }
            .into();
            let announce_xt =
                signed_test_extrinsic(&charlie, Address::Id(charlie_account.clone()), announce, 0);
            assert!(Executive::apply_extrinsic(announce_xt).unwrap().is_ok());
            assert_eq!(
                pallet_proxy::Announcements::<Runtime>::get(&charlie_account)
                    .0
                    .len(),
                1
            );

            // The real account can reject the announced call immediately,
            // before the three-block execution delay has elapsed.
            let reject: RuntimeCall = pallet_proxy::Call::reject_announcement {
                delegate: Address::Id(charlie_account.clone()),
                call_hash: delayed_hash,
            }
            .into();
            let reject_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), reject, 2);
            assert!(Executive::apply_extrinsic(reject_xt).unwrap().is_ok());
            assert!(
                pallet_proxy::Announcements::<Runtime>::get(&charlie_account)
                    .0
                    .is_empty()
            );

            let create_pure: RuntimeCall = pallet_proxy::Call::create_pure {
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 0,
                index: 9,
            }
            .into();
            let create_pure_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account.clone()), create_pure, 3);
            assert!(Executive::apply_extrinsic(create_pure_xt).unwrap().is_ok());
            let pure = System::events()
                .iter()
                .find_map(|record| match &record.event {
                    RuntimeEvent::Proxy(pallet_proxy::Event::PureCreated { pure, .. }) => {
                        Some(pure.clone())
                    }
                    _ => None,
                })
                .expect("create_pure emits the custody account");
            assert!(pallet_proxy::Proxies::<Runtime>::contains_key(pure));
        });
    }

    #[test]
    fn h4_multisig_account_can_delegate_to_transfer_proxy() {
        use frame_support::{dispatch::GetDispatchInfo, traits::Currency, weights::Weight};

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            System::set_block_number(1);

            let alice = HybridPair::from_string("//Alice", None).unwrap();
            let bob = HybridPair::from_string("//Bob", None).unwrap();
            let charlie = HybridPair::from_string("//Charlie", None).unwrap();
            let delegate = HybridPair::from_string("//Dave", None).unwrap();
            let mut signatories = vec![
                account_id_from_public(&alice.public()),
                account_id_from_public(&bob.public()),
                account_id_from_public(&charlie.public()),
            ];
            signatories.sort();
            for account in &signatories {
                <Balances as Currency<AccountId>>::make_free_balance_be(account, 100 * UNIT);
            }
            let delegate_account = account_id_from_public(&delegate.public());
            <Balances as Currency<AccountId>>::make_free_balance_be(&delegate_account, 100 * UNIT);
            let multisig = Multisig::multi_account_id(&signatories, 2);
            <Balances as Currency<AccountId>>::make_free_balance_be(&multisig, 20 * UNIT);

            let add_proxy: RuntimeCall = pallet_proxy::Call::add_proxy {
                delegate: Address::Id(delegate_account.clone()),
                proxy_type: configs::ProxyType::TransferOnly,
                delay: 0,
            }
            .into();
            let call_hash = sp_io::hashing::blake2_256(&add_proxy.encode());
            let alice_account = account_id_from_public(&alice.public());
            let mut alice_others = signatories
                .iter()
                .filter(|account| **account != alice_account)
                .cloned()
                .collect::<Vec<_>>();
            alice_others.sort();
            let approval: RuntimeCall = pallet_multisig::Call::approve_as_multi {
                threshold: 2,
                other_signatories: alice_others,
                maybe_timepoint: None,
                call_hash,
                max_weight: Weight::zero(),
            }
            .into();
            let approval_xt =
                signed_test_extrinsic(&alice, Address::Id(alice_account), approval, 0);
            assert!(Executive::apply_extrinsic(approval_xt).unwrap().is_ok());

            let timepoint = pallet_multisig::Multisigs::<Runtime>::get(&multisig, call_hash)
                .expect("first approval creates multisig state")
                .when;
            let bob_account = account_id_from_public(&bob.public());
            let mut bob_others = signatories
                .iter()
                .filter(|account| **account != bob_account)
                .cloned()
                .collect::<Vec<_>>();
            bob_others.sort();
            let execute_add: RuntimeCall = pallet_multisig::Call::as_multi {
                threshold: 2,
                other_signatories: bob_others,
                maybe_timepoint: Some(timepoint),
                max_weight: add_proxy.get_dispatch_info().call_weight,
                call: alloc::boxed::Box::new(add_proxy),
            }
            .into();
            let execute_add_xt =
                signed_test_extrinsic(&bob, Address::Id(bob_account), execute_add, 0);
            assert!(Executive::apply_extrinsic(execute_add_xt).unwrap().is_ok());
            assert_eq!(pallet_proxy::Proxies::<Runtime>::get(&multisig).0.len(), 1);

            let recipient =
                account_id_from_public(&HybridPair::from_string("//Eve", None).unwrap().public());
            let proxy_transfer: RuntimeCall = pallet_proxy::Call::proxy {
                real: Address::Id(multisig.clone()),
                force_proxy_type: Some(configs::ProxyType::TransferOnly),
                call: alloc::boxed::Box::new(
                    BalancesCall::transfer_allow_death {
                        dest: Address::Id(recipient.clone()),
                        value: 3 * UNIT,
                    }
                    .into(),
                ),
            }
            .into();
            let proxy_transfer_xt =
                signed_test_extrinsic(&delegate, Address::Id(delegate_account), proxy_transfer, 0);
            assert!(Executive::apply_extrinsic(proxy_transfer_xt)
                .unwrap()
                .is_ok());
            assert_eq!(Balances::free_balance(recipient), 3 * UNIT);
        });
    }

    #[test]
    fn revive_configuration_matches_network_build() {
        assert_eq!(
            <Revive as frame_support::traits::PalletInfoAccess>::index(),
            14
        );
        assert_eq!(
            <EvmChainId as frame_support::traits::PalletInfoAccess>::index(),
            15
        );

        assert_eq!(configs::ReviveDepositPerByte::get(), 10 * MICRO_UNIT);
        assert_eq!(configs::ReviveDepositPerItem::get(), 200 * MILLI_UNIT);
        assert_eq!(
            configs::ReviveDepositPerChildTrieItem::get(),
            2 * MILLI_UNIT
        );
        assert_eq!(
            configs::ReviveCodeHashLockupDepositPercent::get(),
            sp_runtime::Perbill::from_percent(30)
        );
        assert_eq!(
            <<Runtime as pallet_revive::Config>::NativeToEthRatio as frame_support::traits::Get<
                u32,
            >>::get(),
            10u32.pow(18 - 12)
        );

        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());
        ext.execute_with(|| {
            assert_eq!(
                <pallet_evm_chain_id::ChainId<Runtime> as frame_support::traits::Get<u64>>::get(),
                pallet_evm_chain_id::TESTNET_CHAIN_ID
            );
            assert_eq!(
                Revive::evm_base_fee(),
                sp_core::U256::from(1_000_000_000u64)
            );
        });
    }

    #[test]
    fn genesis_initializes_revive_account() {
        let mut ext =
            sp_io::TestExternalities::new(RuntimeGenesisConfig::default().build_storage().unwrap());

        ext.execute_with(|| {
            let account = Revive::account_id();
            assert!(System::account_exists(&account));

            assert_eq!(
                frame_system::Account::<Runtime>::get(&account).data.free,
                EXISTENTIAL_DEPOSIT
            );
        });
    }
}

// Create the runtime by composing the FRAME pallets that were previously configured.
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
    pub struct Runtime;

    #[runtime::pallet_index(0)]
    pub type System = frame_system;

    #[runtime::pallet_index(1)]
    pub type Timestamp = pallet_timestamp;

    #[runtime::pallet_index(2)]
    pub type Babe = pallet_babe;

    #[runtime::pallet_index(3)]
    pub type Grandpa = pallet_grandpa;

    #[runtime::pallet_index(4)]
    pub type Balances = pallet_balances;

    #[runtime::pallet_index(5)]
    pub type TransactionPayment = pallet_transaction_payment;

    // Pallet index 6 is retired. Spec 120 removed `pallet-sudo` so that Root is
    // reachable only through the council: a 2/3 motion dispatching via
    // `ValidatorAdmission::dispatch_as_root`. Never reuse index 6 for a new
    // pallet — its index is part of every stored storage key and extrinsic.

    // Include the custom logic from the pallet-template in the runtime.
    #[runtime::pallet_index(7)]
    pub type Template = pallet_template;

    #[runtime::pallet_index(8)]
    pub type Xqvm = pallet_xqvm;

    #[runtime::pallet_index(9)]
    pub type QuantumComputeMempool = pallet_quantum_compute_mempool;

    #[runtime::pallet_index(10)]
    pub type QuantumPow = pallet_quantum_pow;

    #[runtime::pallet_index(11)]
    pub type FaucetOps = pallet_faucet_ops;

    #[runtime::pallet_index(12)]
    pub type Session = pallet_session;

    #[runtime::pallet_index(13)]
    pub type MinerRegistry = pallet_miner_registry;

    #[runtime::pallet_index(14)]
    pub type Revive = pallet_revive;

    #[runtime::pallet_index(15)]
    pub type EvmChainId = pallet_evm_chain_id;

    #[runtime::pallet_index(16)]
    pub type Multisig = pallet_multisig;

    #[runtime::pallet_index(17)]
    pub type Utility = pallet_utility;

    #[runtime::pallet_index(18)]
    pub type Proxy = pallet_proxy;

    #[runtime::pallet_index(19)]
    pub type EmissionController = pallet_emission_controller;
    #[runtime::pallet_index(20)]
    pub type Foundation = pallet_collective<Instance1>;
    #[runtime::pallet_index(21)]
    pub type FoundationMembership = pallet_membership<Instance1>;
    #[runtime::pallet_index(22)]
    pub type ValidatorAdmission = pallet_validator_admission;
    #[runtime::pallet_index(23)]
    pub type Staking = pallet_staking;
    #[runtime::pallet_index(24)]
    pub type Historical = pallet_session::historical;
    #[runtime::pallet_index(25)]
    pub type MultiBlockElection = pallet_election_provider_multi_block;
    #[runtime::pallet_index(26)]
    pub type MultiBlockElectionVerifier = pallet_election_provider_multi_block::verifier;
    #[runtime::pallet_index(27)]
    pub type MultiBlockElectionUnsigned = pallet_election_provider_multi_block::unsigned;
    #[runtime::pallet_index(28)]
    pub type MultiBlockElectionSigned = pallet_election_provider_multi_block::signed;
    #[runtime::pallet_index(29)]
    pub type Authorship = pallet_authorship;
    #[runtime::pallet_index(30)]
    pub type Offences = pallet_offences;
    #[runtime::pallet_index(31)]
    pub type SlashPolicy = pallet_slash_policy;

    #[runtime::pallet_index(32)]
    pub type Vesting = pallet_vesting;
}

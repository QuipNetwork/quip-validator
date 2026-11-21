#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub mod weights;
pub use weights::*;

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{pallet_prelude::BoundedVec, traits::Currency};
use scale_info::TypeInfo;

type BlockNumberOf<T> = frame_system::pallet_prelude::BlockNumberFor<T>;
type BalanceOf<T> =
    <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

type NodeIdOf<T> = BoundedVec<u8, <T as Config>::MaxNodeIdBytes>;
type NodeNameOf<T> = BoundedVec<u8, <T as Config>::MaxNodeNameBytes>;
type PublicHostOf<T> = BoundedVec<u8, <T as Config>::MaxPublicHostBytes>;
type RpcEndpointOf<T> = BoundedVec<u8, <T as Config>::MaxRpcEndpointBytes>;
type RpcEndpointsOf<T> = BoundedVec<RpcEndpointOf<T>, <T as Config>::MaxRpcEndpoints>;
type MinerLabelOf<T> = BoundedVec<u8, <T as Config>::MaxMinerLabelBytes>;
type MinerBackendOf<T> = BoundedVec<u8, <T as Config>::MaxMinerBackendBytes>;
type MinerDeviceIdOf<T> = BoundedVec<u8, <T as Config>::MaxMinerDeviceIdBytes>;
type MinerSpecOf<T> = MinerSpec<MinerLabelOf<T>, MinerBackendOf<T>, MinerDeviceIdOf<T>>;
type MinerSpecsOf<T> = BoundedVec<MinerSpecOf<T>, <T as Config>::MaxMinerSpecs>;

// Bounded field aliases for the schema-v2 `system_info` hardware survey.
type OsStringOf<T> = BoundedVec<u8, <T as Config>::MaxOsStringBytes>;
type CpuBrandOf<T> = BoundedVec<u8, <T as Config>::MaxCpuBrandBytes>;
type ArchOf<T> = BoundedVec<u8, <T as Config>::MaxArchBytes>;
type GpuVendorOf<T> = BoundedVec<u8, <T as Config>::MaxGpuVendorBytes>;
type GpuNameOf<T> = BoundedVec<u8, <T as Config>::MaxGpuNameBytes>;
type GpuOf<T> = GpuInfo<GpuVendorOf<T>, GpuNameOf<T>>;
type GpusOf<T> = BoundedVec<GpuOf<T>, <T as Config>::MaxGpus>;
type SystemInfoOf<T> = SystemInfo<OsStringOf<T>, CpuBrandOf<T>, ArchOf<T>, GpusOf<T>>;

// Bounded field aliases for the schema-v2 `runtime` (node software) block.
type RuntimeVersionOf<T> = BoundedVec<u8, <T as Config>::MaxRuntimeVersionBytes>;
type DockerImageOf<T> = BoundedVec<u8, <T as Config>::MaxDockerImageBytes>;
type RuntimeInfoOf<T> = RuntimeInfo<RuntimeVersionOf<T>, DockerImageOf<T>>;

type NodeDescriptorV1InputOf<T> = NodeDescriptorV1Input<
    NodeIdOf<T>,
    NodeNameOf<T>,
    PublicHostOf<T>,
    RpcEndpointsOf<T>,
    MinerSpecsOf<T>,
>;
type NodeDescriptorV2InputOf<T> = NodeDescriptorV2Input<
    NodeIdOf<T>,
    NodeNameOf<T>,
    PublicHostOf<T>,
    RpcEndpointsOf<T>,
    MinerSpecsOf<T>,
    SystemInfoOf<T>,
    RuntimeInfoOf<T>,
>;
type NodeDescriptorInputOf<T> =
    NodeDescriptorInput<NodeDescriptorV1InputOf<T>, NodeDescriptorV2InputOf<T>>;
type NodeDescriptorOf<T> = NodeDescriptor<
    NodeIdOf<T>,
    NodeNameOf<T>,
    PublicHostOf<T>,
    RpcEndpointsOf<T>,
    MinerSpecsOf<T>,
    BlockNumberOf<T>,
    BalanceOf<T>,
    SystemInfoOf<T>,
    RuntimeInfoOf<T>,
>;
type ParticipationRecordOf<T> = ParticipationRecord<BlockNumberOf<T>>;

/// Schema version stamped onto every `NodeDescriptor::V1` stored by this pallet.
pub const NODE_DESCRIPTOR_SCHEMA_V1: u16 = 1;

/// Schema version stamped onto descriptors filed via the V2 input, which carry
/// an optional typed `system_info` hardware survey.
pub const NODE_DESCRIPTOR_SCHEMA_V2: u16 = 2;

/// Read-only view into the proof-of-work pallet's qblock numbering.
///
/// Lets the miner registry decide which qblock a participation declaration
/// targets without depending on `pallet-quantum-pow` directly.
pub trait QBlockIdProvider {
    /// Highest qblock id minted so far, or `None` if none has been produced yet.
    fn latest_qblock_id() -> Option<u64>;

    /// Id of the qblock miners should currently be working towards — one past
    /// the latest, treating the empty chain as candidate id `1`.
    fn candidate_qblock_id() -> u64 {
        Self::latest_qblock_id().unwrap_or(0).saturating_add(1)
    }
}

impl QBlockIdProvider for () {
    fn latest_qblock_id() -> Option<u64> {
        None
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
/// Verbosity a node advertises for its off-chain logging.
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
/// Class of compute backend a miner runs on.
pub enum MinerKind {
    Cpu,
    Gpu,
    QpuDwave,
    QpuIbm,
    QpuIonq,
    QpuPasqal,
    Asic,
    /// Apple Metal GPU backend (Apple Silicon). Appended last to keep the
    /// SCALE variant indices of the existing kinds stable on a live chain.
    Metal,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Operating-system identity from a node's hardware survey. `S` is a bounded
/// byte string (`OsStringOf<T>`).
pub struct OsInfo<S> {
    /// OS family, e.g. "Linux" / "Darwin" / "Windows".
    pub system: S,
    /// Kernel or build string.
    pub release: S,
    /// Machine architecture, e.g. "x86_64" / "arm64".
    pub machine: S,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// CPU identity from a node's hardware survey.
pub struct CpuInfo<Brand, Arch> {
    pub logical_cores: Option<u32>,
    pub physical_cores: Option<u32>,
    /// Marketing/brand string, e.g. "AMD EPYC 7763".
    pub brand: Brand,
    /// Instruction-set architecture, e.g. "x86_64".
    pub arch: Arch,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// A single GPU from a node's hardware survey.
pub struct GpuInfo<Vendor, Name> {
    /// Position in the node's GPU enumeration; rigs stay well under 256.
    pub index: u8,
    /// Vendor string, e.g. "NVIDIA" / "Apple" / "AMD".
    pub vendor: Vendor,
    /// Product name, e.g. "NVIDIA H100 80GB HBM3".
    pub name: Name,
    pub memory_mb: Option<u32>,
    /// Utilization percentage, constrained to `0..=100` on store.
    pub utilization_pct: Option<u8>,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Optional typed hardware survey carried by schema-v2 descriptors. Every
/// variable-length field is a `BoundedVec`, so `MaxEncodedLen` stays finite.
/// GPU vendor/name bounds live inside `Gpus` (`GpuOf<T>`), so they are not
/// separate generics here.
pub struct SystemInfo<OsStr, Brand, Arch, Gpus> {
    pub os: OsInfo<OsStr>,
    pub cpu: CpuInfo<Brand, Arch>,
    pub memory_mb: Option<u32>,
    /// `BoundedVec<GpuInfo<Vendor, Name>, MaxGpus>`.
    pub gpus: Gpus,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Optional node-software identity carried by schema-v2 descriptors: the
/// interpreter/build versions and container context. `Ver` and `Image` are
/// bounded byte strings (`RuntimeVersionOf<T>` / `DockerImageOf<T>`).
pub struct RuntimeInfo<Ver, Image> {
    /// Python interpreter version, e.g. "3.13.1".
    pub python: Ver,
    /// Node software version, e.g. "0.2.1".
    pub quip_version: Ver,
    pub protocol_version: u32,
    pub in_docker: bool,
    /// Container image identity, present only when running in a container.
    pub docker_image: Option<Image>,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// A single miner advertised within a node descriptor.
pub struct MinerSpec<Label, Backend, DeviceId> {
    pub kind: MinerKind,
    pub label: Option<Label>,
    pub backend: Option<Backend>,
    pub device_id: Option<DeviceId>,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Caller-supplied descriptor payload for schema v1, validated before storage.
pub struct NodeDescriptorV1Input<NodeId, NodeName, PublicHost, RpcEndpoints, MinerSpecs> {
    pub node_id: NodeId,
    pub node_name: NodeName,
    pub public_host: Option<PublicHost>,
    pub public_port: Option<u16>,
    pub rpc_endpoints: RpcEndpoints,
    pub auto_mine: bool,
    pub log_level: LogLevel,
    pub miners: MinerSpecs,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Caller-supplied descriptor payload for schema v2: identical to v1 plus an
/// optional `system_info` hardware survey and an optional `runtime` block. Field
/// order matches v1 so the shared prefix encodes the same way.
pub struct NodeDescriptorV2Input<
    NodeId,
    NodeName,
    PublicHost,
    RpcEndpoints,
    MinerSpecs,
    SystemInfoInput,
    RuntimeInfoInput,
> {
    pub node_id: NodeId,
    pub node_name: NodeName,
    pub public_host: Option<PublicHost>,
    pub public_port: Option<u16>,
    pub rpc_endpoints: RpcEndpoints,
    pub auto_mine: bool,
    pub log_level: LogLevel,
    pub miners: MinerSpecs,
    pub system_info: Option<SystemInfoInput>,
    pub runtime: Option<RuntimeInfoInput>,
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Versioned wrapper over descriptor inputs so future schemas can be added
/// without breaking the extrinsic signature. The variant indices are pinned so
/// that V1 always encodes to byte `0x00`: in-flight V1 signed extrinsics must
/// keep decoding identically regardless of how variants are ordered in source.
pub enum NodeDescriptorInput<V1, V2> {
    #[codec(index = 0)]
    V1(V1),
    #[codec(index = 1)]
    V2(V2),
}

#[derive(
    Clone, Debug, Decode, DecodeWithMemTracking, Encode, Eq, MaxEncodedLen, PartialEq, TypeInfo,
)]
/// Stored, validated node descriptor: the canonical record kept on-chain
/// together with its payload hash and the deposit reserved for it.
pub struct NodeDescriptor<
    NodeId,
    NodeName,
    PublicHost,
    RpcEndpoints,
    MinerSpecs,
    BlockNumber,
    Balance,
    SystemInfo,
    RuntimeInfo,
> {
    pub schema_version: u16,
    pub node_id: NodeId,
    pub node_name: NodeName,
    pub public_host: Option<PublicHost>,
    pub public_port: Option<u16>,
    pub rpc_endpoints: RpcEndpoints,
    pub auto_mine: bool,
    pub log_level: LogLevel,
    pub miners: MinerSpecs,
    pub payload_hash: sp_core::H256,
    pub updated_at: BlockNumber,
    pub deposit: Balance,
    /// Hardware survey, present only for descriptors filed via the V2 input.
    pub system_info: Option<SystemInfo>,
    /// Node-software identity, present only for descriptors filed via the V2
    /// input with a `runtime` block.
    pub runtime: Option<RuntimeInfo>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
/// An account's most recent participation declaration, used to reject more
/// than one declaration per qblock.
pub struct ParticipationRecord<BlockNumber> {
    pub qblock_id: u64,
    pub kind: MinerKind,
    pub budget_seconds: Option<u32>,
    pub updated_at: BlockNumber,
}

sp_api::decl_runtime_apis! {
    /// Read-only access to miner participation per qblock.
    #[api_version(2)]
    pub trait MinerRegistryApi<AccountId, BlockNumber>
    where
        AccountId: codec::Codec + Ord,
        BlockNumber: codec::Codec,
    {
        /// Participants of `qblock_id` with their records, in the double map's
        /// storage order — the Blake2_128Concat order of the account keys, not
        /// account-id order (the version-2 contract, O-D2). `start_after`
        /// returns only entries strictly after that account's storage key, and
        /// works even when the cursor account no longer has a record; `limit` is
        /// capped server-side. The page is read directly from the second-key
        /// iterator, so a call costs `O(limit)`, not `O(participants)` (W1-009).
        fn participants_by_qblock(
            qblock_id: u64,
            start_after: Option<AccountId>,
            limit: u32,
        ) -> Vec<(AccountId, crate::ParticipationRecord<BlockNumber>)>;

        /// Number of participants recorded for `qblock_id`.
        fn participant_count_by_qblock(qblock_id: u64) -> u32;
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::{
        pallet_prelude::*,
        traits::{ReservableCurrency, StorageVersion},
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::{SaturatedConversion, Saturating, Zero};

    // v3 (W1-009): adds `PruneCursor` and `on_idle` retention pruning of
    // `ParticipantsByQBlock` / `ParticipantCountByQBlock`. No migration: the new
    // cursor defaults to 0 (prune from the oldest qblock) and no existing item
    // changes shape.
    const STORAGE_VERSION: StorageVersion = StorageVersion::new(3);

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_balances::Config {
        #[allow(deprecated)]
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        type Currency: ReservableCurrency<Self::AccountId>;

        type QBlockIds: QBlockIdProvider;

        #[pallet::constant]
        type MaxNodeIdBytes: Get<u32>;
        #[pallet::constant]
        type MaxNodeNameBytes: Get<u32>;
        #[pallet::constant]
        type MaxPublicHostBytes: Get<u32>;
        #[pallet::constant]
        type MaxRpcEndpointBytes: Get<u32>;
        #[pallet::constant]
        type MaxRpcEndpoints: Get<u32>;
        #[pallet::constant]
        type MaxMinerSpecs: Get<u32>;
        #[pallet::constant]
        type MaxMinerLabelBytes: Get<u32>;
        #[pallet::constant]
        type MaxMinerBackendBytes: Get<u32>;
        #[pallet::constant]
        type MaxMinerDeviceIdBytes: Get<u32>;
        #[pallet::constant]
        type MaxOsStringBytes: Get<u32>;
        #[pallet::constant]
        type MaxCpuBrandBytes: Get<u32>;
        #[pallet::constant]
        type MaxArchBytes: Get<u32>;
        #[pallet::constant]
        type MaxGpuVendorBytes: Get<u32>;
        #[pallet::constant]
        type MaxGpuNameBytes: Get<u32>;
        #[pallet::constant]
        type MaxGpus: Get<u32>;
        #[pallet::constant]
        type MaxRuntimeVersionBytes: Get<u32>;
        #[pallet::constant]
        type MaxDockerImageBytes: Get<u32>;
        #[pallet::constant]
        type DescriptorDepositBase: Get<BalanceOf<Self>>;
        #[pallet::constant]
        type DescriptorDepositPerByte: Get<BalanceOf<Self>>;

        /// Number of most-recent qblocks of participation history to retain.
        /// `on_idle` prunes `ParticipantsByQBlock` / `ParticipantCountByQBlock`
        /// for every qblock id older than `candidate - ParticipationRetention`.
        ///
        /// Testnet is 5,040 (O-D1). The subnet coordinator that consumes this
        /// index reads only the current candidate qblock's participants, so the
        /// on-chain record exists purely for recent-era auditing, not for the
        /// live protocol. 5,040 = 28 x 180: 180 qblocks form per day (one every
        /// 80 blocks x 6 s = 480 s, and 86,400 s / 480 s = 180), retained for 28
        /// days. Beyond that window no on-chain or off-chain reader consults the
        /// record, so it is pruned to bound unbounded growth (W1-009).
        #[pallet::constant]
        type ParticipationRetention: Get<u64>;

        /// Maximum participation entries `on_idle` removes in one block — the
        /// `clear_prefix` limit. Bounds the per-block pruning cost, so a qblock
        /// with many participants is cleared over several blocks rather than in
        /// a single oversized `on_idle`.
        #[pallet::constant]
        type MaxParticipationPrunePerBlock: Get<u32>;

        type WeightInfo: WeightInfo;
    }

    #[pallet::storage]
    pub type NodeDescriptors<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, NodeDescriptorOf<T>>;

    #[pallet::storage]
    pub type LatestParticipation<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ParticipationRecordOf<T>>;

    /// Reverse index of participation: for each qblock id, the record of every
    /// account that participated. Enables enumerating all participants of a
    /// qblock (which `LatestParticipation` cannot, as it only keeps each
    /// account's most recent record). Grows with qblocks, like `QBlocks` in
    /// `quantum-pow`; each `(qblock_id, account)` is written at most once
    /// because participation is one-per-account-per-qblock.
    #[pallet::storage]
    pub type ParticipantsByQBlock<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        u64,
        Blake2_128Concat,
        T::AccountId,
        ParticipationRecordOf<T>,
    >;

    /// Number of participants recorded for each qblock id, maintained
    /// alongside [`ParticipantsByQBlock`] for O(1) count queries.
    #[pallet::storage]
    pub type ParticipantCountByQBlock<T: Config> =
        StorageMap<_, Blake2_128Concat, u64, u32, ValueQuery>;

    /// The next qblock id `on_idle` will consider for retention pruning. Starts
    /// at zero and only advances, so pruning never revisits a cleared qblock.
    /// Only the qblock id is persisted: `clear_prefix` deletes keys in place, so
    /// a qblock that is only partially cleared in one block resumes from its
    /// surviving keys next block, with no sub-cursor to store (W1-009).
    #[pallet::storage]
    pub type PruneCursor<T: Config> = StorageValue<_, u64, ValueQuery>;

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Prune one old qblock's participation per block, within the block's
        /// leftover weight. Does nothing until the block can afford a full pass
        /// (so a full block is never pushed over budget) and nothing once the
        /// cursor has caught up to the retention window.
        fn on_idle(_now: BlockNumberFor<T>, remaining: Weight) -> Weight {
            Self::prune_participation(remaining)
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// A node descriptor was created or replaced for `who`.
        DescriptorUpdated {
            who: T::AccountId,
            payload_hash: sp_core::H256,
            payload_len: u32,
        },
        /// A node descriptor was removed and its deposit returned to `who`.
        DescriptorCleared { who: T::AccountId },
        /// `who` declared participation in `qblock_id` with the given backend.
        MinerParticipated {
            qblock_id: u64,
            who: T::AccountId,
            kind: MinerKind,
            budget_seconds: Option<u32>,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        EmptyNodeId,
        EmptyNodeName,
        EmptyPublicHost,
        EmptyRpcEndpoint,
        EmptyMinerLabel,
        EmptyMinerBackend,
        EmptyMinerDeviceId,
        EmptyOsSystem,
        EmptyCpuBrand,
        EmptyCpuArch,
        EmptyGpuVendor,
        EmptyGpuName,
        InvalidGpuUtilization,
        EmptyPythonVersion,
        EmptyQuipVersion,
        EmptyDockerImage,
        NoMiners,
        InvalidPort,
        DescriptorNotFound,
        DescriptorRequired,
        InvalidQBlockId,
        DuplicateParticipation,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create or replace the caller's node descriptor.
        ///
        /// Validates the payload, then reserves (or refunds) the difference
        /// between the new and any previous deposit so the held amount always
        /// matches the current descriptor size.
        ///
        /// The fixed benchmark weight covers replacement with a fully
        /// populated, maximally bounded V2 descriptor. Descriptor validation,
        /// hashing, deposit calculation, and SCALE storage encoding traverse
        /// several independent byte strings and collections; charging the
        /// bounded maximum avoids repeating those traversals pre-dispatch just
        /// to derive a dynamic component.
        #[pallet::call_index(0)]
        #[pallet::weight(<T as Config>::WeightInfo::set_descriptor())]
        pub fn set_descriptor(
            origin: OriginFor<T>,
            descriptor: NodeDescriptorInputOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let payload_hash =
                sp_core::H256::from(sp_io::hashing::blake2_256(&descriptor.encode()));
            let (stored, payload_len) = Self::validate_and_store_descriptor(descriptor)?;
            Self::replace_descriptor(&who, stored, payload_hash)?;

            Self::deposit_event(Event::DescriptorUpdated {
                who,
                payload_hash,
                payload_len,
            });
            Ok(())
        }

        /// Remove the caller's node descriptor, return its deposit, and drop the
        /// associated participation record. Fails if no descriptor is stored.
        #[pallet::call_index(1)]
        #[pallet::weight(<T as Config>::WeightInfo::clear_descriptor())]
        pub fn clear_descriptor(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let descriptor =
                NodeDescriptors::<T>::take(&who).ok_or(Error::<T>::DescriptorNotFound)?;
            // The reserved amount always equals the stored deposit, so the full
            // amount must be returned. A non-zero remainder signals reserve/unreserve
            // accounting drift and is caught in tests / try-runtime.
            let remaining = T::Currency::unreserve(&who, descriptor.deposit);
            debug_assert!(remaining.is_zero());
            // Drop the stale participation record so a cleared account does not keep
            // a dangling row in storage after its descriptor is gone.
            LatestParticipation::<T>::remove(&who);
            Self::deposit_event(Event::DescriptorCleared { who });
            Ok(())
        }

        /// Declare that the caller's node is working on the current candidate
        /// qblock. Requires a stored descriptor, the qblock id to match the
        /// current candidate, and at most one declaration per qblock.
        #[pallet::call_index(2)]
        #[pallet::weight(<T as Config>::WeightInfo::participate())]
        pub fn participate(
            origin: OriginFor<T>,
            qblock_id: u64,
            kind: MinerKind,
            budget_seconds: Option<u32>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(
                NodeDescriptors::<T>::contains_key(&who),
                Error::<T>::DescriptorRequired
            );
            ensure!(
                qblock_id == T::QBlockIds::candidate_qblock_id(),
                Error::<T>::InvalidQBlockId
            );
            ensure!(
                LatestParticipation::<T>::get(&who)
                    .map(|record| record.qblock_id != qblock_id)
                    .unwrap_or(true),
                Error::<T>::DuplicateParticipation
            );

            let record = ParticipationRecord {
                qblock_id,
                kind,
                budget_seconds,
                updated_at: frame_system::Pallet::<T>::block_number(),
            };

            LatestParticipation::<T>::insert(&who, record);
            // The `DuplicateParticipation` guard above ensures `(qblock_id, who)`
            // is written at most once, so the count increment never
            // double-counts an account.
            ParticipantsByQBlock::<T>::insert(qblock_id, &who, record);
            ParticipantCountByQBlock::<T>::mutate(qblock_id, |count| {
                *count = count.saturating_add(1)
            });

            Self::deposit_event(Event::MinerParticipated {
                qblock_id,
                who,
                kind,
                budget_seconds,
            });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Number of participants recorded for `qblock_id`.
        pub fn participant_count_by_qblock(qblock_id: u64) -> u32 {
            ParticipantCountByQBlock::<T>::get(qblock_id)
        }

        /// Participants of `qblock_id` with their records, in storage order
        /// (W1-009). `start_after` returns only entries strictly after the
        /// cursor; `limit` is capped at 1,000.
        ///
        /// This reads at most `limit` entries: the page is taken directly from
        /// the double map's second-key iterator, starting strictly after the
        /// cursor's storage key, with no full-prefix scan and no sort. The order
        /// is the map's storage (Blake2_128Concat) order, not account-id order,
        /// which is the `#[api_version(2)]` contract (O-D2). The cursor works
        /// even when the cursor account no longer has a record, because the raw
        /// key sets the iterator start position directly.
        pub fn participants_by_qblock(
            qblock_id: u64,
            start_after: Option<T::AccountId>,
            limit: u32,
        ) -> Vec<(T::AccountId, ParticipationRecordOf<T>)> {
            let limit = limit.min(1_000) as usize;

            if limit == 0 {
                return Vec::new();
            }

            match start_after {
                Some(cursor) => {
                    // `hashed_key_for` is the full storage key of `(qblock_id,
                    // cursor)`; `iter_prefix_from` starts strictly after it, so
                    // the cursor itself is excluded and its presence is not
                    // required.
                    let from = ParticipantsByQBlock::<T>::hashed_key_for(qblock_id, &cursor);
                    ParticipantsByQBlock::<T>::iter_prefix_from(qblock_id, from)
                        .take(limit)
                        .collect()
                }
                None => ParticipantsByQBlock::<T>::iter_prefix(qblock_id)
                    .take(limit)
                    .collect(),
            }
        }

        /// One retention-pruning step, called from `on_idle`. Removes up to
        /// `MaxParticipationPrunePerBlock` participation entries of the oldest
        /// un-pruned qblock that has fallen outside the retention window, via a
        /// single bounded `clear_prefix`. The cursor advances only once a qblock
        /// is fully cleared, so a qblock with more than the per-block limit is
        /// finished over consecutive blocks. Returns the weight consumed.
        fn prune_participation(remaining: Weight) -> Weight {
            let db = T::DbWeight::get();
            let limit = T::MaxParticipationPrunePerBlock::get();
            // Pre-charge the worst case (a full qblock-clearing pass) and skip
            // when the block cannot afford it, so `on_idle` never overruns.
            let worst = <T as Config>::WeightInfo::prune_participation(limit);
            if remaining.any_lt(worst) {
                return Weight::zero();
            }
            let candidate = T::QBlockIds::candidate_qblock_id();
            let prune_before = candidate.saturating_sub(T::ParticipationRetention::get());
            let cursor = PruneCursor::<T>::get();
            if cursor >= prune_before {
                // Caught up: the oldest un-pruned qblock is still inside the
                // retention window. Charge the two reads taken to decide that.
                return db.reads(2);
            }
            // The two gating reads above are already covered by the `worst`
            // charge below; charge the clear step itself.
            db.reads(2).saturating_add(Self::prune_qblock_step(cursor))
        }

        /// Clear up to `MaxParticipationPrunePerBlock` entries of `qblock_id`'s
        /// participation in one bounded `clear_prefix`, advancing the cursor past
        /// it once fully cleared. This is the measured body of the pruning step
        /// (its `prune_participation` benchmark exercises it directly, without the
        /// candidate/window gating). Returns the weight of the entries removed.
        pub(crate) fn prune_qblock_step(qblock_id: u64) -> Weight {
            let limit = T::MaxParticipationPrunePerBlock::get();
            let result = ParticipantsByQBlock::<T>::clear_prefix(qblock_id, limit, None);
            if result.maybe_cursor.is_none() {
                // The qblock is fully cleared: drop its count and advance past
                // it. A qblock that never had participants clears in one pass
                // with `unique == 0`, so empty ids are skipped a block at a time.
                ParticipantCountByQBlock::<T>::remove(qblock_id);
                PruneCursor::<T>::put(qblock_id.saturating_add(1));
            }
            // Charge the entries actually removed; bounded above by the per-block
            // limit, which the caller already showed the block can afford.
            <T as Config>::WeightInfo::prune_participation(result.unique)
        }

        fn validate_and_store_descriptor(
            descriptor: NodeDescriptorInputOf<T>,
        ) -> Result<(NodeDescriptorOf<T>, u32), DispatchError> {
            match descriptor {
                NodeDescriptorInput::V1(input) => Self::validate_and_store_v1(input),
                NodeDescriptorInput::V2(input) => Self::validate_and_store_v2(input),
            }
        }

        /// Reject empty required identity fields, a zero port, and empty RPC
        /// endpoints. Shared by the V1 and V2 store paths.
        fn validate_node_identity(
            node_id: &NodeIdOf<T>,
            node_name: &NodeNameOf<T>,
            public_host: Option<&PublicHostOf<T>>,
            public_port: Option<u16>,
            rpc_endpoints: &RpcEndpointsOf<T>,
        ) -> DispatchResult {
            ensure!(!node_id.is_empty(), Error::<T>::EmptyNodeId);
            ensure!(!node_name.is_empty(), Error::<T>::EmptyNodeName);
            if let Some(host) = public_host {
                ensure!(!host.is_empty(), Error::<T>::EmptyPublicHost);
            }
            if let Some(port) = public_port {
                ensure!(port > 0, Error::<T>::InvalidPort);
            }
            for endpoint in rpc_endpoints {
                ensure!(!endpoint.is_empty(), Error::<T>::EmptyRpcEndpoint);
            }
            Ok(())
        }

        /// Require at least one miner and reject empty optional miner strings.
        /// Shared by the V1 and V2 store paths.
        fn validate_miners(miners: &MinerSpecsOf<T>) -> DispatchResult {
            ensure!(!miners.is_empty(), Error::<T>::NoMiners);
            for miner in miners {
                if let Some(label) = &miner.label {
                    ensure!(!label.is_empty(), Error::<T>::EmptyMinerLabel);
                }
                if let Some(backend) = &miner.backend {
                    ensure!(!backend.is_empty(), Error::<T>::EmptyMinerBackend);
                }
                if let Some(device_id) = &miner.device_id {
                    ensure!(!device_id.is_empty(), Error::<T>::EmptyMinerDeviceId);
                }
            }
            Ok(())
        }

        /// Reject empty required `system_info` strings and out-of-range GPU
        /// utilization. Length caps are already enforced by `BoundedVec`
        /// construction at decode time.
        fn validate_system_info(system_info: &SystemInfoOf<T>) -> DispatchResult {
            ensure!(!system_info.os.system.is_empty(), Error::<T>::EmptyOsSystem);
            ensure!(!system_info.cpu.brand.is_empty(), Error::<T>::EmptyCpuBrand);
            ensure!(!system_info.cpu.arch.is_empty(), Error::<T>::EmptyCpuArch);
            for gpu in &system_info.gpus {
                ensure!(!gpu.vendor.is_empty(), Error::<T>::EmptyGpuVendor);
                ensure!(!gpu.name.is_empty(), Error::<T>::EmptyGpuName);
                if let Some(util) = gpu.utilization_pct {
                    ensure!(util <= 100, Error::<T>::InvalidGpuUtilization);
                }
            }
            Ok(())
        }

        /// Reject empty required `runtime` version strings and an empty
        /// `docker_image` when present. Length caps are enforced by `BoundedVec`
        /// construction at decode time.
        fn validate_runtime(runtime: &RuntimeInfoOf<T>) -> DispatchResult {
            ensure!(!runtime.python.is_empty(), Error::<T>::EmptyPythonVersion);
            ensure!(
                !runtime.quip_version.is_empty(),
                Error::<T>::EmptyQuipVersion
            );
            if let Some(image) = &runtime.docker_image {
                ensure!(!image.is_empty(), Error::<T>::EmptyDockerImage);
            }
            Ok(())
        }

        fn validate_and_store_v1(
            input: NodeDescriptorV1InputOf<T>,
        ) -> Result<(NodeDescriptorOf<T>, u32), DispatchError> {
            Self::validate_node_identity(
                &input.node_id,
                &input.node_name,
                input.public_host.as_ref(),
                input.public_port,
                &input.rpc_endpoints,
            )?;
            Self::validate_miners(&input.miners)?;

            let payload_len = Self::descriptor_payload_len(
                &input.node_id,
                &input.node_name,
                input.public_host.as_ref(),
                &input.rpc_endpoints,
                &input.miners,
            );
            let deposit = Self::descriptor_deposit(payload_len);

            let stored = NodeDescriptor {
                schema_version: NODE_DESCRIPTOR_SCHEMA_V1,
                node_id: input.node_id,
                node_name: input.node_name,
                public_host: input.public_host,
                public_port: input.public_port,
                rpc_endpoints: input.rpc_endpoints,
                auto_mine: input.auto_mine,
                log_level: input.log_level,
                miners: input.miners,
                payload_hash: sp_core::H256::zero(),
                updated_at: frame_system::Pallet::<T>::block_number(),
                deposit,
                system_info: None,
                runtime: None,
            };
            Ok((stored, payload_len))
        }

        fn validate_and_store_v2(
            input: NodeDescriptorV2InputOf<T>,
        ) -> Result<(NodeDescriptorOf<T>, u32), DispatchError> {
            Self::validate_node_identity(
                &input.node_id,
                &input.node_name,
                input.public_host.as_ref(),
                input.public_port,
                &input.rpc_endpoints,
            )?;
            Self::validate_miners(&input.miners)?;

            let mut payload_len = Self::descriptor_payload_len(
                &input.node_id,
                &input.node_name,
                input.public_host.as_ref(),
                &input.rpc_endpoints,
                &input.miners,
            );

            let system_info = match input.system_info {
                Some(system_info) => {
                    Self::validate_system_info(&system_info)?;
                    payload_len =
                        payload_len.saturating_add(Self::system_info_payload_len(&system_info));
                    Some(system_info)
                }
                None => None,
            };

            let runtime = match input.runtime {
                Some(runtime) => {
                    Self::validate_runtime(&runtime)?;
                    payload_len = payload_len.saturating_add(Self::runtime_payload_len(&runtime));
                    Some(runtime)
                }
                None => None,
            };

            let deposit = Self::descriptor_deposit(payload_len);

            let stored = NodeDescriptor {
                schema_version: NODE_DESCRIPTOR_SCHEMA_V2,
                node_id: input.node_id,
                node_name: input.node_name,
                public_host: input.public_host,
                public_port: input.public_port,
                rpc_endpoints: input.rpc_endpoints,
                auto_mine: input.auto_mine,
                log_level: input.log_level,
                miners: input.miners,
                payload_hash: sp_core::H256::zero(),
                updated_at: frame_system::Pallet::<T>::block_number(),
                deposit,
                system_info,
                runtime,
            };
            Ok((stored, payload_len))
        }

        fn replace_descriptor(
            who: &T::AccountId,
            mut descriptor: NodeDescriptorOf<T>,
            payload_hash: sp_core::H256,
        ) -> DispatchResult {
            let previous = NodeDescriptors::<T>::get(who).map(|d| d.deposit);
            let previous_deposit = previous.unwrap_or_else(Zero::zero);
            let next_deposit = descriptor.deposit;

            if next_deposit > previous_deposit {
                T::Currency::reserve(who, next_deposit.saturating_sub(previous_deposit))?;
            } else if previous_deposit > next_deposit {
                // The surplus over the previous deposit is always reserved, so the
                // entire delta must be returnable; a remainder means accounting drift.
                let remaining =
                    T::Currency::unreserve(who, previous_deposit.saturating_sub(next_deposit));
                debug_assert!(remaining.is_zero());
            }

            descriptor.payload_hash = payload_hash;
            NodeDescriptors::<T>::insert(who, descriptor);
            Ok(())
        }

        // TODO: Likely better to make it a method on NodeDescriptorV1Input
        fn descriptor_payload_len(
            node_id: &NodeIdOf<T>,
            node_name: &NodeNameOf<T>,
            public_host: Option<&PublicHostOf<T>>,
            rpc_endpoints: &RpcEndpointsOf<T>,
            miners: &MinerSpecsOf<T>,
        ) -> u32 {
            let mut bytes = node_id.len().saturating_add(node_name.len());
            if let Some(host) = public_host {
                bytes = bytes.saturating_add(host.len());
            }
            for endpoint in rpc_endpoints {
                bytes = bytes.saturating_add(endpoint.len());
            }
            for miner in miners {
                if let Some(label) = &miner.label {
                    bytes = bytes.saturating_add(label.len());
                }
                if let Some(backend) = &miner.backend {
                    bytes = bytes.saturating_add(backend.len());
                }
                if let Some(device_id) = &miner.device_id {
                    bytes = bytes.saturating_add(device_id.len());
                }
            }
            bytes.saturated_into::<u32>()
        }

        /// Variable-length byte count of a `system_info` survey, added to the
        /// descriptor payload length so the deposit reflects the extra bytes.
        /// Fixed-size fields (cores, memory, index, utilization) are excluded,
        /// matching `descriptor_payload_len`.
        fn system_info_payload_len(system_info: &SystemInfoOf<T>) -> u32 {
            let mut bytes = system_info
                .os
                .system
                .len()
                .saturating_add(system_info.os.release.len())
                .saturating_add(system_info.os.machine.len())
                .saturating_add(system_info.cpu.brand.len())
                .saturating_add(system_info.cpu.arch.len());
            for gpu in &system_info.gpus {
                bytes = bytes
                    .saturating_add(gpu.vendor.len())
                    .saturating_add(gpu.name.len());
            }
            bytes.saturated_into::<u32>()
        }

        /// Variable-length byte count of a `runtime` block, added to the
        /// descriptor payload length. Fixed-size fields (protocol_version,
        /// in_docker) are excluded, matching `descriptor_payload_len`.
        fn runtime_payload_len(runtime: &RuntimeInfoOf<T>) -> u32 {
            let mut bytes = runtime
                .python
                .len()
                .saturating_add(runtime.quip_version.len());
            if let Some(image) = &runtime.docker_image {
                bytes = bytes.saturating_add(image.len());
            }
            bytes.saturated_into::<u32>()
        }

        fn descriptor_deposit(payload_len: u32) -> BalanceOf<T> {
            T::DescriptorDepositBase::get().saturating_add(
                T::DescriptorDepositPerByte::get()
                    .saturating_mul(payload_len.saturated_into::<BalanceOf<T>>()),
            )
        }
    }
}

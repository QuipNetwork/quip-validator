// This is free and unencumbered software released into the public domain.
//
// Anyone is free to copy, modify, publish, use, compile, sell, or
// distribute this software, either in source code form or as a compiled
// binary, for any purpose, commercial or non-commercial, and by any
// means.
//
// In jurisdictions that recognize copyright laws, the author or authors
// of this software dedicate any and all copyright interest in the
// software to the public domain. We make this dedication for the benefit
// of the public at large and to the detriment of our heirs and
// successors. We intend this dedication to be an overt act of
// relinquishment in perpetuity of all present and future rights to this
// software under copyright law.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
// EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
// IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
// OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
// ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
// OTHER DEALINGS IN THE SOFTWARE.
//
// For more information, please refer to <http://unlicense.org>

// Substrate and Polkadot dependencies
use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use frame_support::{
    derive_impl,
    dispatch::DispatchClass,
    parameter_types,
    traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, InstanceFilter, VariantCountOf},
    weights::{
        constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
        IdentityFee, Weight,
    },
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_runtime::{
    traits::{BlakeTwo256, ConvertInto, One, OpaqueKeys},
    FixedU128, Perbill,
};
use sp_version::RuntimeVersion;

use pallet_xqvm::WeightInfo as _;

// Local module imports
use super::{
    AccountId, Address, Babe, Balance, Balances, Block, BlockNumber, BoxedSessionKeys,
    EthExtraImpl, Hash, Nonce, OriginCaller, PalletInfo, Runtime, RuntimeCall, RuntimeEvent,
    RuntimeFreezeReason, RuntimeHoldReason, RuntimeOrigin, RuntimeTask, SessionKeys, Signature,
    System, Timestamp, EXISTENTIAL_DEPOSIT, MICRO_UNIT, MILLI_UNIT, SLOT_DURATION, UNIT, VERSION,
};
use crate::weights::{BlockExecutionWeight, ExtrinsicBaseWeight};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
/// Block length limits per dispatch class (QUI-2548); see `RuntimeBlockLength`.
pub const NORMAL_BLOCK_LENGTH: u32 = 3 * 1024 * 1024 + 512 * 1024;
pub const OPERATIONAL_BLOCK_LENGTH: u32 = 3 * 1024 * 1024 + 768 * 1024;
pub const MANDATORY_BLOCK_LENGTH: u32 = 5 * 1024 * 1024;
const AVERAGE_ON_INITIALIZE_RATIO: Perbill = Perbill::from_percent(10);

parameter_types! {
    pub const BlockHashCount: BlockNumber = 2400;
    pub const Version: RuntimeVersion = VERSION;

    /// We allow for 2 seconds of compute with a 6 second average block time.
    pub RuntimeBlockWeights: BlockWeights = {
        let max_block = Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX);
        let normal_max = NORMAL_DISPATCH_RATIO * max_block;

        BlockWeights::builder()
            .base_block(BlockExecutionWeight::get())
            .for_class(DispatchClass::all(), |weights| {
                weights.base_extrinsic = ExtrinsicBaseWeight::get();
            })
            .for_class(DispatchClass::Normal, |weights| {
                weights.max_total = Some(normal_max);
            })
            .for_class(DispatchClass::Operational, |weights| {
                weights.max_total = Some(max_block);
                weights.reserved = Some(max_block - normal_max);
            })
            .avg_block_initialization(AVERAGE_ON_INITIALIZE_RATIO)
            .build_or_panic()
    };
    /// Block length per dispatch class (QUI-2548, owner decision of
    /// 2026-11-25, `dec_20261125_205850_2d4af7ff`): Normal 3.5 MiB,
    /// Operational 3.75 MiB, Mandatory 5 MiB. `CheckWeight` admits an
    /// extrinsic only while the block's total encoded length stays within its
    /// class's limit, so no signed extrinsic -- Normal, or an Operational one
    /// such as `set_code` or a council `propose` that is included even when
    /// its dispatch fails -- carries a payload in the 8 MiB allocator order
    /// (over 4 MiB), which an importer could not hold next to the VM budget
    /// (see [`VM_BLOCK_RESIDENCY`]). Operational keeps 0.25 MiB above Normal in
    /// reserve; Mandatory (inherents only) keeps the previous 5 MiB. A runtime
    /// upgrade (`dispatch_as_root(set_code)` with the 1,587,318-B Wasm, a
    /// 1,589,028-B extrinsic) fits with 2.3 MB to spare.
    pub RuntimeBlockLength: BlockLength = BlockLength::builder()
        .max_length(MANDATORY_BLOCK_LENGTH)
        .modify_max_length_for_class(frame_support::dispatch::DispatchClass::Normal, |len| {
            *len = NORMAL_BLOCK_LENGTH;
        })
        .modify_max_length_for_class(
            frame_support::dispatch::DispatchClass::Operational,
            |len| {
                *len = OPERATIONAL_BLOCK_LENGTH;
            },
        )
        .build();
    pub const SS58Prefix: u8 = 42;
}

/// Stable error shared by all runtime-upgrade entry points.
pub const RUNTIME_IDENTITY_MISMATCH: &str = "Runtime spec_name does not match this network";

/// Emergency upgrades may bypass version ordering, but not network identity.
pub struct NetworkIdentityGuard;
impl frame_system::SetCode<Runtime> for NetworkIdentityGuard {
    fn set_code(code: Vec<u8>) -> frame_support::dispatch::DispatchResult {
        let version = sp_io::misc::runtime_version(&code)
            .and_then(|v| sp_version::RuntimeVersion::decode(&mut &v[..]).ok());
        // Match FRAME's exemption for the SDK kitchensink benchmark candidate.
        #[cfg(all(feature = "runtime-benchmarks", not(test)))]
        core::hint::black_box(version);
        #[cfg(not(all(feature = "runtime-benchmarks", not(test))))]
        if version.is_some_and(|version| version.spec_name != crate::VERSION.spec_name) {
            return Err(sp_runtime::DispatchError::Other(RUNTIME_IDENTITY_MISMATCH));
        }
        // Preserve the unchecked path's existing behavior when extraction fails.
        <() as frame_system::SetCode<Runtime>>::set_code(code)
    }
}

/// The default types are being injected by [`derive_impl`](`frame_support::derive_impl`) from
/// [`SoloChainDefaultConfig`](`struct@frame_system::config_preludes::SolochainDefaultConfig`),
/// but overridden as needed.
#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
    /// The block type for the runtime.
    type Block = Block;
    type BaseCallFilter = session_keys::SessionKeyCallFilter;
    /// Block & extrinsics weights: base values and limits.
    type BlockWeights = RuntimeBlockWeights;
    /// The maximum length of a block (in bytes).
    type BlockLength = RuntimeBlockLength;
    /// The identifier used to distinguish between accounts.
    type AccountId = AccountId;
    /// The type for storing how many extrinsics an account has signed.
    type Nonce = Nonce;
    /// The type for hashing blocks and tries.
    type Hash = Hash;
    /// Maximum number of block number to block hash mappings to keep (oldest pruned first).
    type BlockHashCount = BlockHashCount;
    /// The weight of database operations that the runtime can invoke.
    type DbWeight = RocksDbWeight;
    /// Version of the runtime.
    type Version = Version;
    /// The data to be stored in an account.
    type AccountData = pallet_balances::AccountData<Balance>;
    /// This is used as an identifier of the chain. 42 is the generic substrate prefix.
    type SS58Prefix = SS58Prefix;
    type MaxConsumers = frame_support::traits::ConstU32<16>;
    type OnSetCode = NetworkIdentityGuard;
    type PostInherents = election::RestartZeroWeightSnapshot;
}

parameter_types! {
    // BABE epochs are defined in slots. One epoch is one staking session (ShouldEndSession = Babe),
    // so six sessions give a 24h mainnet era or a 6h testnet era.
    pub const EpochDuration: u64 = crate::network_params::EPOCH_DURATION_SLOTS;
    pub const ExpectedBlockTime: u64 = SLOT_DURATION;
}

impl pallet_babe::Config for Runtime {
    type EpochDuration = EpochDuration;
    type ExpectedBlockTime = ExpectedBlockTime;
    type EpochChangeTrigger = pallet_babe::ExternalTrigger;
    type DisabledValidators = crate::Session;
    type WeightInfo = slashing::EquivocationWeights;
    type MaxAuthorities = ConstU32<64>;
    type MaxNominators = election::MaxBackersPerWinnerFinal;
    type KeyOwnerProof = sp_session::MembershipProof;
    type EquivocationReportSystem = pallet_babe::EquivocationReportSystem<
        Self,
        slashing::ReportOffences,
        slashing::ActiveBabeKeyOwner,
        slashing::ReportLongevity,
    >;
}

impl pallet_grandpa::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;

    type WeightInfo = slashing::EquivocationWeights;
    type MaxAuthorities = ConstU32<64>;
    type MaxNominators = election::MaxBackersPerWinnerFinal;
    type MaxSetIdSessionEntries = validator_onboarding::SetIdSessionEntries;

    type KeyOwnerProof = sp_session::MembershipProof;
    type EquivocationReportSystem = pallet_grandpa::EquivocationReportSystem<
        Self,
        slashing::ReportOffences,
        // The GRANDPA current-session key-reassignment analogue remains accepted.
        crate::Historical,
        slashing::ReportLongevity,
    >;
}

/// Staking queues validators; historical roots start with the first rotation.
impl pallet_session::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorId = <Self as frame_system::Config>::AccountId;
    type ValidatorIdOf = ConvertInto;
    type ShouldEndSession = Babe;
    type NextSessionRotation = Babe;
    type SessionManager = election::ElectionScheduler;
    type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
    type Keys = BoxedSessionKeys;
    type DisablingStrategy = pallet_session::disabling::UpToLimitWithReEnablingDisablingStrategy;
    type WeightInfo = session_keys::SessionWeights;
    type Currency = Balances;
    type KeyDeposit = ();
}

impl pallet_timestamp::Config for Runtime {
    /// A timestamp: milliseconds since the unix epoch.
    type Moment = u64;
    type OnTimestampSet = (launch_time::TimestampFloor, Babe);
    type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
    type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    // This limits only legacy named-reserve entries. Multisig and proxy use
    // the aggregate plain reserve and do not consult this bound; 64 is a
    // forward-looking cap for any future named-reserve users.
    type MaxReserves = ConstU32<64>;
    type ReserveIdentifier = [u8; 8];
    /// The type for recording an account's balance.
    type Balance = Balance;
    /// The ubiquitous event type.
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
    type AccountStore = System;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = VariantCountOf<RuntimeFreezeReason>;
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

parameter_types! {
    /// Chain-wide price for one storage item.
    pub const StorageDepositPerItem: Balance = 200 * MILLI_UNIT;
    /// Chain-wide price for one byte of storage.
    pub const StorageDepositPerByte: Balance = 10 * MICRO_UNIT;
}

// Keep the former Revive names as compatibility aliases for downstream runtime
// assertions. Revive consumes the shared chain storage price directly below.
pub type ReviveDepositPerItem = StorageDepositPerItem;
pub type ReviveDepositPerByte = StorageDepositPerByte;

parameter_types! {
    // One minimum multisig state uses a 32-byte call-hash key plus 57 value
    // bytes: Timepoint (8), Balance (16), depositor AccountId (32), and the
    // approvals Vec compact-length prefix (1). At the chain storage price,
    // this is 1 item + 89 bytes.
    pub const MultisigDepositBase: Balance =
        StorageDepositPerItem::get() + 89 * StorageDepositPerByte::get();
    // Each threshold approval adds one encoded 32-byte AccountId.
    pub const MultisigDepositFactor: Balance = 32 * StorageDepositPerByte::get();
    /// Weight grows with the sorted signatory set. One hundred supports
    /// institutional custody while remaining bounded; benchmark results may
    /// justify tightening this before mainnet.
    pub const MaxSignatories: u32 = 100;
}

impl pallet_multisig::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type DepositBase = MultisigDepositBase;
    type DepositFactor = MultisigDepositFactor;
    type MaxSignatories = MaxSignatories;
    type WeightInfo = pallet_multisig::weights::SubstrateWeight<Runtime>;
    type BlockNumberProvider = System;
}

impl pallet_utility::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type PalletsOrigin = OriginCaller;
    type WeightInfo = pallet_utility::weights::SubstrateWeight<Runtime>;
}

/// Custody proxy permissions deliberately have only a full-access option and
/// an asset-movement option. The latter admits the three signed balance
/// transfer calls and rejects every administrative, contract, and nested call.
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Encode,
    Decode,
    DecodeWithMemTracking,
    Debug,
    MaxEncodedLen,
    scale_info::TypeInfo,
)]
pub enum ProxyType {
    Any,
    TransferOnly,
}

impl Default for ProxyType {
    fn default() -> Self {
        Self::Any
    }
}

impl InstanceFilter<RuntimeCall> for ProxyType {
    fn filter(&self, call: &RuntimeCall) -> bool {
        match self {
            Self::Any => true,
            Self::TransferOnly => matches!(
                call,
                RuntimeCall::Balances(pallet_balances::Call::transfer_allow_death { .. })
                    | RuntimeCall::Balances(pallet_balances::Call::transfer_keep_alive { .. })
                    | RuntimeCall::Balances(pallet_balances::Call::transfer_all { .. })
            ),
        }
    }

    fn is_superset(&self, other: &Self) -> bool {
        self == &Self::Any || self == other
    }
}

parameter_types! {
    // Proxies is keyed by AccountId (32 bytes) and stores a Balance (16 bytes)
    // before its vector entries: 1 item + 48 bytes at the chain storage price.
    pub const ProxyDepositBase: Balance =
        StorageDepositPerItem::get() + 48 * StorageDepositPerByte::get();
    // Each ProxyDefinition encodes AccountId (32), ProxyType (1), and delay (4).
    pub const ProxyDepositFactor: Balance = 37 * StorageDepositPerByte::get();
    // Announcements has the same AccountId key and Balance base footprint.
    pub const AnnouncementDepositBase: Balance =
        StorageDepositPerItem::get() + 48 * StorageDepositPerByte::get();
    // Each announcement encodes real AccountId (32), call hash (32), and block (4).
    pub const AnnouncementDepositFactor: Balance = 68 * StorageDepositPerByte::get();
}

impl pallet_proxy::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type Currency = Balances;
    type ProxyType = ProxyType;
    type ProxyDepositBase = ProxyDepositBase;
    type ProxyDepositFactor = ProxyDepositFactor;
    type MaxProxies = ConstU32<32>;
    type WeightInfo = pallet_proxy::weights::SubstrateWeight<Runtime>;
    type MaxPending = ConstU32<32>;
    type CallHasher = BlakeTwo256;
    type AnnouncementDepositBase = AnnouncementDepositBase;
    type AnnouncementDepositFactor = AnnouncementDepositFactor;
    type BlockNumberProvider = System;
}

// `create_pure` intentionally stays available: a key-holding custodian can
// spawn an otherwise inaccessible vault account controlled only through the
// recorded proxy relationship, which is the standard pure-proxy custody flow.

parameter_types! {
    pub FeeMultiplier: Multiplier = Multiplier::one();
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = FungibleAdapter<Balances, ()>;
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightToFee = pallet_revive::evm::fees::BlockRatioFee<1, 1, Runtime, Balance>;
    type LengthToFee = IdentityFee<Balance>;
    type FeeMultiplierUpdate = ConstFeeMultiplier<FeeMultiplier>;
    type WeightInfo = pallet_transaction_payment::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const ReviveDepositPerChildTrieItem: Balance = 2 * MILLI_UNIT;
    pub ReviveCodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
    pub const ReviveMaxEthExtrinsicWeight: FixedU128 = FixedU128::from_rational(9, 10);
    /// One native 12-decimal plank equals 10^6 Ethereum 18-decimal units.
    pub const ReviveNativeToEthRatio: u32 = 1_000_000;
}

impl pallet_evm_chain_id::Config for Runtime {}

parameter_types! {
    /// The account that owns pallet-revive code and deposits deployed through
    /// Root (QUI-2549), e.g. a council motion via `dispatch_as_root`.
    ///
    /// It is the low 20 bytes of `keccak256("quip/revive-root-deployer")`,
    /// padded with `0xEE`. The `0xEE` padding makes it an "eth-derived"
    /// account, which pallet-revive treats as mapped
    /// (`AccountId32Mapper::is_mapped`): a keyless account could not sign
    /// `map_account`, and revive refuses an unmapped deployer
    /// (`AccountUnmapped`).
    ///
    /// Keyless: this chain's account ids are `Blake2b-256(domain || hybrid
    /// public key)`, so reaching this id from a key is a 256-bit preimage
    /// (matching the `0xEE` suffix alone is about 2^96 work); reaching it as
    /// an Ethereum address, or through `map_account`, is a 160-bit preimage.
    /// Controlling it would grant spending its balance and `remove_code` of
    /// code it owns, not deploy rights: those come from Root.
    ///
    /// **Governance must fund it before a Root deploy**; unfunded, the deploy
    /// fails on its storage deposit.
    pub ReviveRootDeployer: AccountId = {
        let hash = sp_core::hashing::keccak_256(b"quip/revive-root-deployer");
        let mut bytes = [0xEE_u8; 32];
        bytes[..20].copy_from_slice(&hash[12..]);
        AccountId::new(bytes)
    };
}

/// Who may upload or instantiate pallet-revive code (QUI-2549, owner decision
/// `dec_20261125_185005_b31add86`): Root, owning deposits as
/// [`ReviveRootDeployer`], or a Foundation member's own signed extrinsic, owning
/// them itself. Every deploy path reaches this check -- `upload_code`,
/// `instantiate`, `instantiate_with_code`, `eth_instantiate_with_code` (the
/// `eth_transact` create path, whose origin becomes the signer),
/// `eth_substrate_call` and `dispatch_as_fallback_account` (both dispatch their
/// inner call signed), and the dry-run APIs. Deploy rights are inherited by
/// anything that can dispatch as a member: a member's `Any`-type proxy, or a
/// multisig account made a member.
///
/// Every contract the Foundation deploys is callable by anyone, and the
/// budget has no revive term. A contract that forwards caller-supplied data or
/// calls a caller-chosen address recurses to `CALL_STACK_DEPTH`
/// (`AllowReentry`), and one that grows memory by a caller-controlled amount
/// reaches `EVM_MEMORY_BYTES` per frame. Either reopens the memory shape without
/// any `CREATE` (measured: a 19-byte self-forwarder with 128 KiB calldata,
/// 9.0 MiB; 1 MiB of memory plus recursion, 29.4 MiB). So each Foundation
/// deployment needs its worst-case memory reviewed against the budget, and a
/// contract that `CREATE`s/`CREATE2`s caller-supplied code must never be
/// deployed.
///
/// Benchmark builds keep `EnsureSigned`: pallet-revive's benchmarks dispatch as
/// `RawOrigin::Signed(whitelisted_caller())`. `runtime-benchmarks` is enabled
/// only by benchmark-stage (non-shipping) jobs: `benchmark-weights`, the
/// quantum-pow sweeps and `check-benchmark-preflight.sh`. The shipped Wasm
/// and the release image build without it, a normal-build test pins the
/// restriction, and the runtime refuses to compile with both
/// `runtime-benchmarks` and `on-chain-release-build`.
#[cfg(not(feature = "runtime-benchmarks"))]
pub type ReviveDeployOrigin = frame_support::traits::EitherOf<
    frame_system::EnsureRootWithSuccess<AccountId, ReviveRootDeployer>,
    frame_system::EnsureSignedBy<crate::FoundationMembership, AccountId>,
>;
/// Benchmark builds only: see the non-benchmark definition.
#[cfg(feature = "runtime-benchmarks")]
pub type ReviveDeployOrigin = frame_system::EnsureSigned<AccountId>;

impl pallet_revive::Config for Runtime {
    type Time = Timestamp;
    type Balance = Balance;
    type Currency = Balances;
    type OnBurn = ();
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type RuntimeOrigin = RuntimeOrigin;
    type RuntimeHoldReason = RuntimeHoldReason;
    type WeightInfo = pallet_revive::weights::SubstrateWeight<Runtime>;
    type Precompiles = ();
    type FindAuthor = pallet_session::FindAccountFromAuthorIndex<Runtime, Babe>;
    type DepositPerByte = StorageDepositPerByte;
    type DepositPerItem = StorageDepositPerItem;
    type DepositPerChildTrieItem = ReviveDepositPerChildTrieItem;
    type CodeHashLockupDepositPercent = ReviveCodeHashLockupDepositPercent;
    type AddressMapper = pallet_revive::AccountId32Mapper<Runtime>;
    type AllowEVMBytecode = ConstBool<true>;
    // QUI-2549: deployment is restricted to Root and Foundation members; see
    // `ReviveDeployOrigin` for every path it covers and the one it cannot
    // (a contract instantiating another contract).
    type UploadOrigin = ReviveDeployOrigin;
    type InstantiateOrigin = ReviveDeployOrigin;
    type RuntimeMemory = ConstU32<{ 128 * 1024 * 1024 }>;
    type PVFMemory = ConstU32<{ 512 * 1024 * 1024 }>;
    type ChainId = pallet_evm_chain_id::ChainId<Runtime>;
    type NativeToEthRatio = ReviveNativeToEthRatio;
    type FeeInfo = pallet_revive::evm::fees::Info<Address, Signature, EthExtraImpl>;
    type MaxEthExtrinsicWeight = ReviveMaxEthExtrinsicWeight;
    type DebugEnabled = ConstBool<false>;
    type GasScale = ConstU32<1_000>;
}

/// Configure the pallet-template in pallets/template.
impl pallet_template::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_template::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const ControllerAnnualEmission: Balance =
        pallet_emission_controller::CONTROLLER_ANNUAL_EMISSION_QUIP.saturating_mul(UNIT);
    pub const EmissionPotId: frame_support::PalletId = frame_support::PalletId(*b"qp/emiss");
}
impl pallet_emission_controller::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type Clock = Timestamp;
    type Unit = ConstU128<UNIT>;
    type AnnualEmission = ControllerAnnualEmission;
    type PotId = EmissionPotId;
    type MaxSubnets = ConstU32<32>;
    type WeightInfo = pallet_emission_controller::weights::SubstrateWeight<Runtime>;
}

impl pallet_faucet_ops::Config for Runtime {
    type MintOrigin = frame_support::traits::EitherOfDiverse<
        pallet_faucet_ops::EnsureFaucetAuthority<Runtime>,
        validator_onboarding::FoundationOrigin,
    >;
    type AuthorityOrigin = validator_onboarding::FoundationOrigin;
    type Emissions = crate::EmissionController;
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type WeightInfo = pallet_faucet_ops::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const MaxProgramSize: u32 = 65_536;
    /// Maximum basic blocks in a stored program, counted as the verifier's
    /// CFG builder counts them: the entry, every `TARGET`, and the position
    /// after every jump, `RANGE`, `ITER` and `NEXT`.
    ///
    /// This bounds the verifier's memory, which the byte limit does not.
    /// Measured natively, decode plus xqvm's full checker on a pure `TARGET` sled peaks
    /// at ~3 KiB per block: 14 MiB at 4,096 blocks and 200 MiB at the
    /// 65,520 blocks a `MaxProgramSize` program can hold. The reference
    /// machine's benchmark run exhausted the 128 MiB runtime heap at about
    /// 28,000 blocks, so the Wasm allocator costs roughly 2.4x the native
    /// figure. A failed allocation traps the runtime; it is not a dispatch
    /// error anyone can report or pay for.
    ///
    /// 2,048 blocks is ~7 MiB native, ~17 MiB in Wasm by that 2.4x estimate.
    /// `store_program` no longer builds that graph: it runs the structural gate
    /// (`pallet_xqvm::structural_gate`, decode plus the loop-nesting and
    /// jump-span passes). Measured on chain, a `store_program` of a
    /// `MaxProgramSize` program at 2,048 blocks peaks at 1.36 MiB of importer
    /// heap (1.340 MiB for the nested and flat shapes, 1.355 MiB for the jump
    /// chain; census `l_store_program_verify_peak`), inside the budget's 4 MiB
    /// non-VM term. Under the full checker it was 3.76 MiB for all three. (The
    /// old "one-eighth of the heap" matched the VM's old 16 MiB cap;
    /// `MaxVmMemory` is now 1 MiB.) It is also far past any real program -- the 196 xquad conformance vectors
    /// at the pinned revision have at most 8 basic blocks (median 1) -- so
    /// the ceiling binds abuse, not use. The bound is priced separately: `store_program` pre-charges
    /// its per-block component -- the structural gate's jump-span searches, one
    /// per jump, and jumps are bounded by blocks -- at this many blocks and
    /// refunds to the actual count.
    pub const MaxProgramBlocks: u32 = 2_048;
    /// Maximum static loop nesting in a stored program.
    ///
    /// This retains the structured-nesting admission policy. The pinned
    /// verifier checks exact heights without separate loop-body walks.
    /// QUI-2353 must calibrate register-heavy cyclic joins at the byte/block
    /// caps on the reference machine before restoring program admission.
    pub const MaxLoopDepth: u32 = 32;
    pub const MaxCallDataLen: u32 = 256;
    pub const MaxOutputSlots: u32 = 256;
    /// Weight charged per executed XQVM step, measured rather than assumed.
    ///
    /// Taken as the slope of the `execute_step` benchmark -- the marginal cost
    /// of one more step on the reference machine -- multiplied by a safety
    /// factor. Deriving it from the generated weights means regeneration keeps
    /// it honest; the previous hand-set 1 ns was roughly 13x below the
    /// measured cost of the *cheapest* opcode in the ISA.
    ///
    /// The safety factor covers the spread between one-step opcodes. A step
    /// is calibrated upstream as one `NOP` dispatch, while the benchmark
    /// exercises `NEXT`, which is dearer and still charges a single step; the
    /// slope therefore already sits above the unit cost, and the factor of 2
    /// leaves headroom for register and vector opcodes the benchmark does not
    /// reach.
    ///
    /// Since xqvm 0.4.0 it also covers opcodes whose cost scales with their
    /// operands, which a flat per-step price could not be made sound for
    /// before (QUI-1056). Those opcodes now charge additional steps for the
    /// work they are about to do -- `ENERGY` one per model term, the
    /// constraint expansions one per coefficient written, the grid scans one
    /// per cell -- so `WeightPerStep * steps` tracks work rather than
    /// instruction count. The unit counts are normative and specified in the
    /// toolchain's `spec/xqvm/METERING.md`; the pallet prices them, it does
    /// not restate them.
    pub XqvmWeightPerStep: Weight = {
        const STEP_WEIGHT_SAFETY_FACTOR: u64 = 2;
        let slope = pallet_xqvm::SubstrateWeight::<Runtime>::execute_step(1)
            .saturating_sub(pallet_xqvm::SubstrateWeight::<Runtime>::execute_step(0));
        slope.saturating_mul(STEP_WEIGHT_SAFETY_FACTOR)
    };

    /// Maximum bytes a single `execute` run may allocate inside the VM
    /// (W1-002). This is a **stateless per-run cap**: there is no block-scoped
    /// counter. The block-level bound follows from the cap plus the Wasm
    /// allocator's behaviour, argued below and measured by the census; it is not
    /// a running total.
    ///
    /// Why a per-run cap bounds the whole block. Importers run every extrinsic
    /// in one 128 MiB heap (`Core_execute_block`), and the freeing-bump
    /// allocator reuses a freed chunk only within its own power-of-two size
    /// order (`sc-allocator` `freeing_bump.rs`: `Order::from_size` +
    /// per-order free lists). VM runs are sequential and each frees all of its
    /// own allocations on drop, so the heap a block's runs cause in a given
    /// order is the *maximum* over runs of that order's live bytes, not the sum.
    /// Whole-block VM growth is therefore expected to stay within
    /// `Σ_orders (k · MaxVmMemory)`, independent of the number of runs, where
    /// `k` covers power-of-two rounding and the `EQUALITY`/`ATLEASTW` input-vec
    /// clones. The Σ term is not proved; it is measured, against xqvm 0.6.0.
    /// The adversarial mixed-family order sweep reaches 17.32·L at the shipped
    /// 1 MiB (census `c`; 17.54·L' at 256 KiB). It includes a linear model
    /// whose coefficient `BTreeMap` is written in its worst insertion order
    /// (`linear_anchored`: keys 0..5, then n-1 down to 5, leaving 5-entry
    /// leaves; claude_reviewer's repin R8), the same charge in more 256-B
    /// chunks than ascending order. The other holders that keep
    /// small buffers alive -- the 252 free registers and up to 256 `OUTPUT`
    /// clones in every order they fit, including size-2 models whose two
    /// `BTreeMap` leaves take two 256-B chunks for 96 B of charge -- and the
    /// value and loop stacks at their limits raise it to 19.30·L (census `j`'s
    /// L' sweep 18.39 / 18.33 / 19.30 at 256 KiB / 512 KiB / 1 MiB). Two
    /// uncharged copies raise it further: `EQUALITY` and `ATLEASTW` clone their
    /// index and coefficient vecs before any charge or type check (QUI-2546).
    /// With those clones in every order, a successful `ATLEASTW` with slack
    /// variables (whose success path makes further uncharged temporaries), and
    /// single runs that fill one order with several holder families at once
    /// (samples, OUTPUT clones, `MaxLoopDepth` ITER frames, a vec and its two
    /// clones), census `k` measures **20.99·L** at the shipped cap. Programs
    /// the full checker accepts reach the clones too, so this is not a cost of
    /// the structural gate. The pin is **24·L** ([`VM_SIGMA_ORDERS_PER_L`]),
    /// with stated headroom; its docs give the measurement, the analytic
    /// ceiling and the assumptions behind both.
    ///
    /// Two xqvm 0.6.0 changes brought it down from 28.64·L under 0.5.0:
    /// - the jump-span phase (W1-011) rejects at `store_program` a jump that
    ///   crosses a loop-body boundary, so a back-edge can no longer re-enter a
    ///   `RANGE`/`ITER` without its `NEXT`. The uncharged `LoopFrame` pile-up,
    ///   up to `LOOP_LIMIT` frames each holding a slice copy, is gone: a
    ///   program stored under that verifier holds at most `MaxLoopDepth` (32)
    ///   frames (the precondition below says which chains this covers);
    /// - `ENERGY` computes without cloning the sample (W1-013), so a sample no
    ///   longer doubles in its order (census `c` dropped by about 2·L).
    ///
    /// The pin is derived for a cap that is a POWER OF TWO, and the budget test
    /// still requires one: at any other cap the charged sample itself -- just
    /// over half a next_pow2(L) chunk, still <= L -- lands in the order above
    /// L's. Without the ENERGY clone that excess is now small (census `k`:
    /// 21.57·L at an emulated 768 KiB against 20.99 at 1 MiB; 30.24·L
    /// under 0.5.0), but the guard stays until Σ is re-derived for a specific
    /// non-power-of-two cap.
    ///
    /// `LVAL` is not a `k ≈ 2` term. It charges steps only, and it can clone a
    /// model into up to 255 registers per `ITER` frame. It is harmless today only
    /// because no on-chain path can put a model into a `VecXqmx`: `VECX` creates
    /// one empty, `VECPUSH` pushes integers only, `execute` calldata is integers
    /// only, and transform calldata is one `VecInt`. That invariant is the
    /// reason, and the calldata tripwire test below fails CI if a future ABI lets
    /// calldata carry a model.
    ///
    /// "Frees on drop" is the VM's own allocations. One term outlives the run
    /// but not the extrinsic: the compute-mempool accumulates a `transformed:
    /// Vec<Vec<i8>>` across the per-solution VMs of a single `submit_solution`,
    /// so it is retained for the extrinsic and freed when the extrinsic ends. Its
    /// payload is `QuantumMaxSolutions` (20) × `QuantumMaxNodes` (5000) bytes;
    /// its heap footprint under the allocator's chunk rule is 164,520 B
    /// (≈ 0.16 MiB) -- inside the fixed-run term below and small beside the VM
    /// caps. The `execute` path retains nothing comparable: a `ProgramExecuted`
    /// event's outputs reach state through `Events::append`, a host storage
    /// append, so they never reside in the Wasm heap this bound governs.
    ///
    /// Sizing (V-A step-0, measured through `sc_executor::WasmExecutor` with the
    /// node's default heap; harness `runtime/tests/vm_heap_census.rs`). The
    /// importer heap a block can be made to hold is:
    ///
    /// ```text
    ///   Σ_orders(k·L)   order sweep + holders + vec clones     20.99·L measured (k), 24·L pinned
    /// + fixed_run       worst case, L-independent (below)       0.45 MiB
    /// + block_residency valid block through finalize (census d)  62.97 MiB
    /// + non_vm_baseline maximal submit_proof decode             3.27 MiB
    /// ```
    ///
    /// `fixed_run` is the measured worst case of every per-run allocation the
    /// VM memory meter does not charge (census test `i_worst_case_fixed_run`):
    /// a `MaxProgramSize` (64 KiB) program, loaded and decoded, that the
    /// structural gate admits; the value stack pushed to its 8,192-entry
    /// limit; all 256 registers and output slots; `MaxLoopDepth` (32) nested
    /// loops; and a `TARGET`/`JUMP` chain filling the blocks `MaxProgramBlocks`
    /// allows, so decode's jump table and its scan temporaries are at their
    /// largest. The value stack is bounded by xqvm's `STACK_LIMIT` (8,192 `i64`
    /// entries), not by the memory meter, so it is accounted for here. The
    /// loop stack is bounded by `MaxLoopDepth` for a program stored under xqvm
    /// 0.6.0's jump-span phase (W1-011; see the precondition below); under
    /// 0.5.0 a back-edge could pile up `LOOP_LIMIT`
    /// (8,192) uncharged frames (933,504 B with `ITER` slice copies), which
    /// census `i` now asserts the verifier rejects.
    ///
    /// Importer heap net of an empty block, the larger of the two census
    /// programs: 0.453 MiB (474,784 B with the jump chain; 471,944 B without);
    /// the HALT-only baseline 0.025 MiB. Census `i` also runs two programs the
    /// structural gate admits and the full checker refused: every holder at its
    /// maximum and then the 8,193rd push (`VmStackOverflow`), 474,784 B, the
    /// same as the completed run, so a fault does not raise the high-water
    /// mark; and the stack filled from a 3-byte loop body, 149,464 B.
    ///
    /// At `L = 1 MiB`, on the measured terms: 20.99 + 0.45 + 62.97 + 3.27 ≈
    /// 87.7 MiB (31.5 % free). The budget test sums the PINNED terms, and that
    /// sum is the gate: 24 × 1 + 1 (fixed-run allowance) + 64 + 4 =
    /// **93 MiB**, 27.3 % free; with the 25 % floor (32 MiB),
    /// **125 MiB ≤ 128 MiB**, a margin of 3 MiB. For census `k`'s combined
    /// block, the summed measured terms (84.4) exceed its real high-water
    /// (75.0), because Σ vs empty and census d both contain the block-level
    /// share; this is a property of that block, not a bound.
    ///
    /// **What the pinned sum bounds.** QUI-2548 is closed by `BlockLength`
    /// (Normal 3.5 MiB, Operational 3.75 MiB): no admitted extrinsic carries a
    /// payload past the 4 MiB allocator order, and census d measures the
    /// Operational class (a signed `set_code`, included though it fails at
    /// dispatch) up to its limit. An unsigned `eth_transact`, whose decode and
    /// signer recovery hold further copies of its payload, is capped at
    /// [`crate::MAX_ETH_TRANSACT_PAYLOAD`] before the runtime decodes it (pool
    /// validation, block building and import; A10b, A10c), and census d
    /// measures it up to the cap, under the remark family's 61.97 MiB. The other extrinsic classes are small or already
    /// in a measured family: Mandatory is the timestamp inherent only;
    /// unsigned election pages are bounded by `MinerMaxLength` (128 KiB);
    /// equivocation reports carry fixed-size proofs; signed Normal calls with
    /// a free-length argument (`set_keys`'s proof, signed election pages) are
    /// the remark family. So the pinned sum is an upper bound on the importer
    /// heap for those classes -- VM runs (Σ and the fixed run), the largest
    /// non-VM dispatch, and the residency of every block the builder admits,
    /// Normal, Operational and `eth_transact` -- under the structural-gate
    /// precondition below. It has no revive term: pallet-revive contract
    /// execution is closed by origin instead (QUI-2549, see
    /// [`VM_BLOCK_RESIDENCY`]).
    ///
    /// 16 MiB (the
    /// original value) put the budget past 128 MiB: five `execute` calls built
    /// a block only its author could import.
    ///
    /// | cap   | Σ measured | Σ pin | pinned sum | free   | 25 % floor | worst real block    |
    /// |-------|------------|-------|------------|--------|------------|---------------------|
    /// | 1 MiB | 20.99·L    | 24    | 93 MiB     | 27.3 % | passes     | 75.012 MiB, 41.4 %  |
    /// | 2 MiB | 19.31·L *  | 24    | 117 MiB    | 8.6 %  | fails      | 76.936 MiB, 39.9 % †|
    /// | 4 MiB | -          | 24    | 165 MiB    | -      | fails      | -                   |
    ///
    /// (Under xqvm 0.6.0. The 2 MiB row was MEASURED on a throwaway build with
    /// both caps at 2 MiB, census `k` and `j` at that cap, not extrapolated,
    /// and census `k` re-measured there with the `linear_anchored` run:
    /// Σ 19.3093·L, the budget test passing with Σ 20, and the combined block
    /// importing at 76.936 MiB high-water. * That was before the
    /// `EQUALITY`/`ATLEASTW` clone holders, which 2 MiB was not re-measured
    /// with. At the Σ pin of 24 and the block-residency term of 64 MiB its
    /// pinned sum fails the floor (117 + 32 = 149 MiB > 128). † Its real block
    /// carried a remark below the 2 MiB payload cliff (see
    /// [`VM_BLOCK_RESIDENCY`]), so it understates. It is kept as evidence for any later raise; a different
    /// cap needs Σ re-measured there before it is pinned.)
    ///
    /// PRECONDITION (claude_reviewer's repin R8; !132 release blocker 5). The
    /// terms here -- the fixed run, Σ and the "at most `MaxLoopDepth` frames"
    /// claims -- hold only on a chain whose every `Programs` entry passes the
    /// structural gate (`pallet_xqvm::structural_gate`: xqvm 0.6.0's
    /// loop-nesting and jump-span phases). A fresh genesis satisfies it. The
    /// gate runs only at `store_program`; `execute` and the mempool transform
    /// decode and run what is stored without re-checking (pallet-xqvm's test
    /// `execute_does_not_re_verify_a_stored_program` pins this for `execute`),
    /// and the mempool's spec registration (`insert_job_spec` →
    /// `validate_programs`) checks only that each program exists and decodes.
    /// A program stored before 0.6.0 can still pile up loop frames under this
    /// runtime: 933,504 B of fixed run and about 26.7·L of Σ (measured at 1 MiB
    /// with the 0.5.0 pile-up runs under this runtime; 28.64·L under 0.5.0),
    /// past both pins. If such a chain is ever upgraded in place, run the
    /// structural gate over the stored programs and remove every one it refuses
    /// first.
    ///
    /// Why 1 MiB (owner decisions of 2026-11-22). Under xqvm 0.5.0 the cap went
    /// 2 MiB → 1.5 MiB when `ITER` pile-ups raised Σ from 20·L to 29·L, then
    /// 1.5 MiB → 1 MiB when 1.5 MiB, not a power of two, reached the next
    /// allocator order (29.52·L, 30.24·L with every holder class). xqvm 0.6.0
    /// (W1-011, W1-013) removes both effects for programs that pass its
    /// structural gate (the precondition above), and at the Σ pin of 20 that
    /// then applied, 2 MiB would have passed the floor again; at today's pin
    /// of 24 it does not (table above). The owner kept 1 MiB in favour of margin
    /// (decision of 2026-11-22, dec_20261122_225113_7840c1d4). The summed
    /// upper bound is the gate; a real block is only a second check. At 1 MiB
    /// that check reads: census `k`, the mixed sweep plus the holder runs plus
    /// the ~2 MiB remark and fillers, 75.012 MiB high-water (41.4 % free) with
    /// the worst remark on census d's plateau (61.012 MiB, 52.3 % free, with
    /// the old remark just below the 2 MiB payload cliff); the same VM runs
    /// plus the largest signed `set_code` that still fits the Operational
    /// limit (3,718,826 B), 73.746 MiB (42.4 % free); census `g`, without the
    /// holders and with that old remark, 60.97 MiB.
    ///
    /// Headroom for legitimate work, as measured rather than assumed. No
    /// legitimate on-chain `execute` workload exists today: the live testnet's
    /// `Programs` map is empty (and on main, e40cfba filters `store_program`).
    /// Two runs were executed and measured (census test
    /// `e_largest_legitimate_run`):
    /// - a synthetic model-building run, the seeded `linear` program building
    ///   a 4,096-variable model: 0.198 MiB of author heap;
    /// - the maximal mempool transform (20 × 5,000 spins through an identity
    ///   program on the real transform path), the largest run the runtime
    ///   performs: about 160 KB charged per VM, the `INPUT` copy (5,000 × 16 B)
    ///   plus the `OUTPUT` copy; its 40 KB calldata `Vec<i64>` sits outside the
    ///   charge.
    ///
    /// So 1 MiB is about 6.5× the maximal identity transform. Separately, an
    /// observation about the corpus rather than a heap measurement: the 196
    /// xquad conformance vectors at the pinned revision assemble to at most
    /// 16,403 B of bytecode (one stack-overflow stress vector; every other is
    /// at most 1,047 B, median 30 B). Bytecode size does not bound allocation
    /// (`PUSH n; BSMX` spends the full cap from a few bytes). As a heap
    /// measurement, run natively on every vector: each one that succeeds under
    /// the chain's budget charges at most 2,016 B of VM memory. Two
    /// `xqmx-grid` vectors (`allocator_size_at_the_maximum`,
    /// `model_growth_onto_the_maximum`) are designed SUCCESSES at their own
    /// 64 GiB budget; on chain the meter refuses them with
    /// `VmMemoryLimitExceeded` before allocating, which the on-chain replay
    /// accepts. Re-measure when a real model-building transform is proposed.
    ///
    /// It is not derived from `MaxStepLimit`, and must not be: the step budget
    /// bounds allocation only incidentally (a sample costs one step and eight
    /// bytes per element), so it alone would permit an allocation well past the
    /// heap. The pinned budget test below restates each measured term; its Σ term
    /// is pinned at 24·L, above census `k`'s 20.99·L at the shipped cap with
    /// every holder class (see [`VM_SIGMA_ORDERS_PER_L`] for the headroom).
    pub const MaxVmMemory: u64 = 1_048_576;

    /// Maximum bytes a single compute-mempool transform run may allocate inside
    /// the VM. A distinct constant from [`MaxVmMemory`] so the `execute` and
    /// transform caps can diverge, but set to the same 1 MiB today (the
    /// budget uses the larger of the two, so both must move together): the mempool
    /// builds a fresh `Vm` with the full cap per solution
    /// (`quantum-compute-mempool/src/xqvm.rs`), one instance at a time, so the
    /// same `Σ_orders(k·L)` argument bounds the per-run VM memory. A transform
    /// transports at most `QuantumMaxNodes` (5000) i64 spins, ~40 KiB, well
    /// under the cap. What the per-run cap does *not* cover is the per-extrinsic
    /// retained output: `submit_solution` keeps a `transformed: Vec<Vec<i8>>`
    /// live across all of a batch's per-solution VMs, bounded by
    /// `QuantumMaxSolutions` (20) × `QuantumMaxNodes` (5000) bytes of payload,
    /// 164,520 B of heap by the allocator's chunk rule, and freed when the
    /// extrinsic ends. See [`MaxVmMemory`]; the pinned budget
    /// test accounts for it inside the fixed-run term.
    pub const MaxTransformVmMemory: u64 = 1_048_576;

    /// Derived from block weight budget so a single execute call always
    /// fits in one block.  Uses 50 % of the normal dispatch budget to
    /// leave room for other extrinsics in the same block.
    ///
    /// Falls automatically as the per-step price rises: with the price
    /// measured rather than assumed, this is roughly an order of magnitude
    /// below the ~750M steps the hand-set constant used to permit. That is the
    /// point -- the old limit allowed a single extrinsic to run for over ten
    /// seconds against a two-second block budget.
    pub MaxStepLimit: u64 = {
        let normal = RuntimeBlockWeights::get()
            .get(frame_support::dispatch::DispatchClass::Normal)
            .max_total
            .unwrap_or(RuntimeBlockWeights::get().max_block);
        // Reserve half for other extrinsics.
        let budget = normal.ref_time() / 2;
        // Subtract the worst-case decode overhead, then divide by per-step cost.
        let base = pallet_xqvm::SubstrateWeight::<Runtime>::execute(MaxProgramSize::get(), MaxOutputSlots::get())
            .ref_time();
        let per_step = XqvmWeightPerStep::get().ref_time().max(1);
        budget.saturating_sub(base) / per_step
    };
}

/// Configure the XQVM pallet for on-chain bytecode execution.
impl pallet_xqvm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type RuntimeHoldReason = RuntimeHoldReason;
    // A stored program is priced at the chain storage price like Revive's
    // code blobs: one item for the program plus its bytes. At `MaxProgramSize`
    // the per-byte part is 0.655 UNIT, so a full 64 KiB program locks up
    // roughly 0.855 UNIT in total, returned when the program is removed.
    type DepositBase = StorageDepositPerItem;
    type DepositPerByte = StorageDepositPerByte;
    type MaxProgramSize = MaxProgramSize;
    type MaxProgramBlocks = MaxProgramBlocks;
    type MaxLoopDepth = MaxLoopDepth;
    type MaxCallDataLen = MaxCallDataLen;
    type MaxOutputSlots = MaxOutputSlots;
    type MaxStepLimit = MaxStepLimit;
    type MaxVmMemory = MaxVmMemory;
    type WeightPerStep = XqvmWeightPerStep;
    type WeightInfo = pallet_xqvm::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const QuantumMaxNodes: u32 = 5_000;
    pub const QuantumMaxEdges: u32 = 50_000;
    pub const QuantumMaxSolutions: u32 = 20;
    /// VM steps shared by every solution in one submission. Every submission
    /// reserves this up front, so it sets how many fit in a block.
    pub const QuantumMaxTransformSteps: u64 = 5_000_000;
    pub const QuantumMaxBidMiners: u32 = 16;
    pub const QuantumMaxOrdersPerProposer: u32 = 32;
    pub const QuantumMaxDeadlineBlocks: BlockNumber = 1_000;
    pub const QuantumMaxBlockWait: BlockNumber = 100;
    pub const QuantumMinReward: Balance = UNIT;
    /// Root of the compute-mempool's per-order reward escrow accounts (W1-006).
    pub const QuantumEscrowPalletId: frame_support::PalletId = frame_support::PalletId(*b"qp/escrw");
    pub const QuantumResultTtlBlocks: BlockNumber = 10_000;
    // Audit retention policy is 28 eras, independent of the unbonding period.
    pub const QuantumOrderRetentionBlocks: BlockNumber = 28 * 6 * crate::network_params::EPOCH_DURATION_SLOTS as u32;
    pub const QuantumPowMaxNodes: u32 = 5_000;
    pub const QuantumPowMaxEdges: u32 = 50_000;
    pub const QuantumPowMaxSolutions: u32 = 32;
    pub const QuantumPowMinNodes: u32 = 16;
    /// Decay rate unit. The threshold stays fixed for the first 100 chain
    /// blocks after a qblock, then eases continuously at 4.9375% of the
    /// remaining gap to the easy bound per 100 overdue blocks (subject to
    /// the one-energy-unit floor per epoch). The gate is set separately by
    /// `TARGET_PROOF_BLOCKS` in pallet-quantum-pow; retune both together.
    pub const QuantumPowEpochLength: BlockNumber = 100;
    pub const QuantumPowMinerDeposit: Balance = UNIT;
    pub const QuantumPowBlockReward: Balance = UNIT;
    /// Inclusive bounds for the council-settable qblock reward. The floor keeps
    /// a win worth more than the miner's deposit; the ceiling bounds what one
    /// subnet pot can pay out before the emission budget is re-planned.
    pub const QuantumPowMinBlockReward: Balance = UNIT / 10;
    pub const QuantumPowMaxBlockReward: Balance = 100 * UNIT;
    /// Round-length target for the difficulty controller, in blocks (~10 min at
    /// 6s blocks), and the council's allowed range. The floor sits above the
    /// fast-proof anchor (60 blocks) so the hardening band keeps an interior.
    pub const QuantumPowTargetProofBlocks: u32 = 100;
    pub const QuantumPowMinTargetProofBlocks: u32 = 80;
    pub const QuantumPowMaxTargetProofBlocks: u32 = 400;
    pub const QuantumPowMaxProofsPerBlock: u32 = 8;
    /// Upper bound on the cardinality of `allowed_h_values`, `allowed_j_values`,
    /// and `allowed_spin_values` per registered topology. Set well above the
    /// expected real-world maximum (the legacy ternary spec uses 3 for h; the
    /// Advantage2_system1 zero-field spin-glass spec uses 1 for h, 2 for j,
    /// 2 for spin) so future hardware-spec changes don't force a runtime
    /// upgrade.
    pub const QuantumPowMaxAllowedValues: u32 = 32;
    /// Energy-curve calibration: per-mille `c` values that define the
    /// `(max_energy, knee_energy, min_energy)` triple via
    /// `expected_gse` on the default topology and its h/J value
    /// specs. Defaults `(0.700, 0.725, 0.750)` keep the hard edge difficult
    /// without pushing the threshold into the known-impossible range.
    pub const QuantumPowCurveCEasyMilli: u32 = 700;
    pub const QuantumPowCurveCKneeMilli: u32 = 725;
    pub const QuantumPowCurveCHardMilli: u32 = 750;

    pub const MinerRegistryMaxNodeIdBytes: u32 = 64;
    pub const MinerRegistryMaxNodeNameBytes: u32 = 64;
    pub const MinerRegistryMaxPublicHostBytes: u32 = 253;
    pub const MinerRegistryMaxRpcEndpointBytes: u32 = 256;
    pub const MinerRegistryMaxRpcEndpoints: u32 = 8;
    pub const MinerRegistryMaxMinerSpecs: u32 = 16;
    pub const MinerRegistryMaxMinerLabelBytes: u32 = 64;
    pub const MinerRegistryMaxMinerBackendBytes: u32 = 32;
    pub const MinerRegistryMaxMinerDeviceIdBytes: u32 = 128;
    // schema-v2 `system_info` bounds, sized off measured payloads (worst-case
    // 8-GPU survey ~787 B), leaving generous headroom.
    pub const MinerRegistryMaxOsStringBytes: u32 = 64;
    pub const MinerRegistryMaxCpuBrandBytes: u32 = 96;
    pub const MinerRegistryMaxArchBytes: u32 = 16;
    pub const MinerRegistryMaxGpuVendorBytes: u32 = 16;
    pub const MinerRegistryMaxGpuNameBytes: u32 = 96;
    pub const MinerRegistryMaxGpus: u32 = 16;
    // schema-v2 `runtime` block: version strings run ~16 B; image refs with a
    // registry path + digest can reach ~120 B.
    pub const MinerRegistryMaxRuntimeVersionBytes: u32 = 48;
    pub const MinerRegistryMaxDockerImageBytes: u32 = 256;
    pub const MinerRegistryDescriptorDepositBase: Balance = MILLI_UNIT;
    pub const MinerRegistryDescriptorDepositPerByte: Balance = MICRO_UNIT;
    // W1-009 retention (O-D1): 5,040 qblocks = 28 days x 180 qblocks/day (one
    // qblock per 80 blocks x 6 s = 480 s). The coordinator reads only the
    // current candidate qblock; older history is kept for auditing only.
    pub const MinerRegistryParticipationRetention: u64 = 5_040;
    // Per-block `clear_prefix` limit for retention pruning; matches the
    // upper bound of the `prune_participation` benchmark component.
    pub const MinerRegistryMaxParticipationPrunePerBlock: u32 = 1_000;
}

/// The source owns bounded, content-addressed bytes admitted by pallet-xqvm's
/// size, basic-block and loop-depth guards. No program storage is duplicated.
pub struct QuantumProgramSource;
impl pallet_quantum_compute_mempool::xqvm::ProgramLookup<Hash> for QuantumProgramSource {
    fn program(hash: &Hash) -> Option<alloc::vec::Vec<u8>> {
        pallet_xqvm::Programs::<Runtime>::get(hash).map(|bytes| bytes.into_inner())
    }
}

parameter_types! {
    // Conservative reuse of the bounded execute base for every possible VM
    // instance, including lookup/decode, setup and proof. The adapter decodes
    // once, so repeated decode/read charges overestimate that part of the batch.
    // A transform VM gets exactly `TRANSFORM_OUTPUT_SLOTS` (1) output slots, so
    // that is what each instance is charged for -- not `MaxOutputSlots`, whose
    // worst case is a user `execute` and stays in the MaxStepLimit derivation.
    pub QuantumProgramLookupWeight: Weight =
        pallet_xqvm::SubstrateWeight::<Runtime>::execute(
            MaxProgramSize::get(),
            pallet_quantum_compute_mempool::xqvm::TRANSFORM_OUTPUT_SLOTS,
        )
        .saturating_mul(u64::from(QuantumMaxSolutions::get()));
    // Registration checks up to two programs; the call charges each bound.
    // Since QUI-2516 a check is a `Programs` lookup and a decode, nothing
    // more (`validate_programs`): no static verification, and no store,
    // hold or event. Priced as pallet-xqvm's `execute` at `MaxProgramSize`
    // with no output slots -- one `Programs` read and a decode of a maximal
    // program in its worst per-byte shape, plus a one-step VM run this check
    // does not make -- a conservative bound. It was `store_program`'s weight,
    // which also priced the verifier and the storage writes.
    pub QuantumProgramValidationWeight: Weight =
        pallet_xqvm::SubstrateWeight::<Runtime>::execute(MaxProgramSize::get(), 0);
}

// QUIP-26: rank-weighted TopN has the smallest tail share. Keep the
// production constants above ED, independently of the defensive payout skip.
const _: () = {
    let winners = pallet_quantum_compute_mempool::types::MAX_REWARD_WINNERS as u128;
    assert!(
        QuantumMinReward::get() / (winners * (winners + 1) / 2) >= EXISTENTIAL_DEPOSIT,
        "minimum compute reward permits a sub-ED winner share"
    );
};

impl pallet_quantum_compute_mempool::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MaxNodes = QuantumMaxNodes;
    type MaxEdges = QuantumMaxEdges;
    type MaxSolutions = QuantumMaxSolutions;
    type MaxBidMiners = QuantumMaxBidMiners;
    type MaxOrdersPerProposer = QuantumMaxOrdersPerProposer;
    type MaxDeadlineBlocks = QuantumMaxDeadlineBlocks;
    type MaxBlockWait = QuantumMaxBlockWait;
    type MinReward = QuantumMinReward;
    type ResultTtlBlocks = QuantumResultTtlBlocks;
    type OrderRetentionBlocks = QuantumOrderRetentionBlocks;
    type DefaultIsingSpecId = pallet_quantum_compute_mempool::CanonicalDefaultIsingSpecId<Runtime>;
    type ProgramSource = QuantumProgramSource;
    type MaxTransformStepsPerSubmission = QuantumMaxTransformSteps;
    type MaxVmMemory = MaxTransformVmMemory;
    type VmWeightPerStep = XqvmWeightPerStep;
    type VmProgramLookupWeight = QuantumProgramLookupWeight;
    type VmProgramValidationWeight = QuantumProgramValidationWeight;
    type VM = pallet_quantum_compute_mempool::xqvm::XqvmAdapter<Runtime>;
    type EscrowPalletId = QuantumEscrowPalletId;
    type WeightInfo = pallet_quantum_compute_mempool::weights::SubstrateWeight<Runtime>;
}

impl pallet_quantum_pow::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MaxNodes = QuantumPowMaxNodes;
    type MaxEdges = QuantumPowMaxEdges;
    type MaxSolutions = QuantumPowMaxSolutions;
    type MinNodes = QuantumPowMinNodes;
    type EpochLength = QuantumPowEpochLength;
    type MinerDeposit = QuantumPowMinerDeposit;
    type BlockReward = QuantumPowBlockReward;
    type MinBlockReward = QuantumPowMinBlockReward;
    type MaxBlockReward = QuantumPowMaxBlockReward;
    // QBlock reward and round-length changes are council business (2/3
    // motion), not Root.
    type AdminOrigin = validator_onboarding::FoundationOrRoot;
    type TargetProofBlocks = QuantumPowTargetProofBlocks;
    type MinTargetProofBlocks = QuantumPowMinTargetProofBlocks;
    type MaxTargetProofBlocks = QuantumPowMaxTargetProofBlocks;
    type RewardPayment = pallet_emission_controller::SubnetRewards<
        Runtime,
        { pallet_emission_controller::ISING_SUBNET },
    >;
    type MaxProofsPerBlock = QuantumPowMaxProofsPerBlock;
    type MaxAllowedValues = QuantumPowMaxAllowedValues;
    type CurveCEasyMilli = QuantumPowCurveCEasyMilli;
    type CurveCKneeMilli = QuantumPowCurveCKneeMilli;
    type CurveCHardMilli = QuantumPowCurveCHardMilli;
    type WeightInfo = pallet_quantum_pow::weights::SubstrateWeight<Runtime>;
}

pub struct RuntimeQBlockIds;

impl pallet_miner_registry::QBlockIdProvider for RuntimeQBlockIds {
    fn latest_qblock_id() -> Option<u64> {
        pallet_quantum_pow::Pallet::<Runtime>::latest_qblock_id()
    }
}

impl pallet_miner_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type QBlockIds = RuntimeQBlockIds;
    type MaxNodeIdBytes = MinerRegistryMaxNodeIdBytes;
    type MaxNodeNameBytes = MinerRegistryMaxNodeNameBytes;
    type MaxPublicHostBytes = MinerRegistryMaxPublicHostBytes;
    type MaxRpcEndpointBytes = MinerRegistryMaxRpcEndpointBytes;
    type MaxRpcEndpoints = MinerRegistryMaxRpcEndpoints;
    type MaxMinerSpecs = MinerRegistryMaxMinerSpecs;
    type MaxMinerLabelBytes = MinerRegistryMaxMinerLabelBytes;
    type MaxMinerBackendBytes = MinerRegistryMaxMinerBackendBytes;
    type MaxMinerDeviceIdBytes = MinerRegistryMaxMinerDeviceIdBytes;
    type MaxOsStringBytes = MinerRegistryMaxOsStringBytes;
    type MaxCpuBrandBytes = MinerRegistryMaxCpuBrandBytes;
    type MaxArchBytes = MinerRegistryMaxArchBytes;
    type MaxGpuVendorBytes = MinerRegistryMaxGpuVendorBytes;
    type MaxGpuNameBytes = MinerRegistryMaxGpuNameBytes;
    type MaxGpus = MinerRegistryMaxGpus;
    type MaxRuntimeVersionBytes = MinerRegistryMaxRuntimeVersionBytes;
    type MaxDockerImageBytes = MinerRegistryMaxDockerImageBytes;
    type DescriptorDepositBase = MinerRegistryDescriptorDepositBase;
    type DescriptorDepositPerByte = MinerRegistryDescriptorDepositPerByte;
    type ParticipationRetention = MinerRegistryParticipationRetention;
    type MaxParticipationPrunePerBlock = MinerRegistryMaxParticipationPrunePerBlock;
    type WeightInfo = pallet_miner_registry::weights::SubstrateWeight<Runtime>;
}

/// The importer-heap budget's allowance for the L-independent fixed per-run
/// term (see [`MaxVmMemory`]). Shared by the budget test and the census.
/// 1 MiB: covers the measured worst run (474,784 B) plus the mempool's
/// retained transform (164,520 B). It was 1.25 MiB under xqvm 0.5.0, whose
/// `ITER` loop-frame pile-up measured 933,504 B; 0.6.0 (W1-011) makes that
/// unstorable. Holds only while every stored program passes the structural
/// gate: see the precondition in [`MaxVmMemory`]'s docs. No runtime
/// parameter derives from it.
#[doc(hidden)]
pub const VM_FIXED_RUN_ALLOWANCE: u64 = 1_048_576;

/// The measured worst case of that term: census test `i_worst_case_fixed_run`,
/// the larger of its two programs (the jump chain filling `MaxProgramBlocks`,
/// under xqvm 0.6.0), importer heap net of an empty block. The census
/// asserts it measures no more than this, and the budget test builds on it.
///
/// Census `i` also measures two programs the structural gate admits and the
/// full checker refused: every holder at its maximum then a `VmStackOverflow`
/// (474,784 B, the same), and a stack-filling loop (149,464 B).
///
/// Like the allowance, it holds only while every stored program passes the
/// structural gate (the precondition in [`MaxVmMemory`]'s docs; `execute`
/// does not re-check).
///
/// Raise only with a measured census `i` run; a raise above
/// `VM_FIXED_RUN_ALLOWANCE - vm_transform_retained_heap()` requires
/// re-budgeting.
#[doc(hidden)]
pub const VM_FIXED_RUN_MEASURED: u64 = 474_784;

/// Whole-block VM growth across allocator orders, as a multiple of the per-run
/// cap L, under xqvm 0.6.0. The mixed-family order sweep alone measured
/// 17.32·L at the shipped 1 MiB (census `c_block_order_sweep_at_emulated_caps`,
/// with the worst-order `linear_anchored` model); with the register, `OUTPUT`
/// and stack holders, 19.30·L.
///
/// **The pin, 24, is empirical with stated headroom, not a tight measured
/// ceiling.**
///
/// Measured (census, `--release`, both runtimes), with every holder class:
/// register and OUTPUT holders, the stacks, the `EQUALITY`/`ATLEASTW` vec
/// clones in every order, `ATLEASTW`'s success path with slacks, and single
/// runs that fill one order with several families at once (`mix_clone`,
/// `models_mix`, `holders_iter`: samples, OUTPUT clones, `MaxLoopDepth` ITER
/// frames, a vec and its two clones). Every run is checked natively to be one
/// a chain at that real cap makes.
/// - **Census `k`, at the shipped 1 MiB cap, defines the pin: 20.99·L** vs
///   the empty block (20.9910). Its real combined block, with the worst remark
///   on census d's plateau, peaks at 75.012 MiB, 41.4 % of the heap free.
/// - The analytic per-order ceiling (claude_reviewer's R8 on A6c..A6f) is
///   **20.54·L at 1 MiB** (20.82 / 20.65·L' at 256 / 512 KiB), an upper
///   bound on the VM's per-order share: it sums, per allocator order, the
///   most chunks one legal run can hold there (the freeing-bump allocator
///   never splits, coalesces or moves chunks between orders, so each order
///   keeps the high-water mark of any run). With the fixed run's share
///   (0.45 MiB, ≈ 0.45·L) that is ≈ 20.99·L. Census `k` brackets the VM's
///   share rather than isolating it: 20.99 vs the empty block also counts
///   block-level allocations, and 19.22 net of the same-count control is a
///   lower bound. The measurement is consistent with the ceiling; how close
///   the two figures are is not evidence that either is tight.
/// - Census `j` is a scale check at emulated caps: 23.59 / 20.53 / 20.99·L'
///   at 256 KiB / 512 KiB / 1 MiB vs the empty block. At a small L' that
///   includes allocations that do not scale with L' -- the fixed run and
///   block-level overhead -- divided by L'. The step of about 0.56 MiB
///   (about 2·L') at 256 KiB, which any added extrinsic causes, a `halt` run
///   included, is a block-level allocation made when the block finalizes (at
///   256 KiB the 85th extrinsic crosses an allocator doubling; the same block
///   trapped at apply does not show it), not VM growth. Net of a same-count
///   `halt` control block,
///   a lower bound on the VM's share, `j` measures 16.99 / 17.16 / 19.26.
/// - **Gates.** STOP if census `k` at the shipped cap exceeds the pin vs the
///   empty block, or if any census `j`/`k` figure net of the same-count
///   control exceeds it (a lower bound, so conclusive). Census `j` vs the
///   empty block over the pin is reported and investigated, not a STOP.
///   **Never raise the pin to fit a measurement.**
/// - The ~3·L between the measured 20.99 and the pin is headroom for holder
///   shapes nobody has tried: each new shape so far raised the measurement.
///
/// The ceiling's assumptions; each is a RE-MEASURE TRIGGER:
/// 1. every xqvm buffer is at most twice its charge, except `BTreeMap` leaves;
/// 2. uncharged temporaries: at most two are live per run in any allocator
///    order above 4 KiB -- the `EQUALITY`/`ATLEAST`/`ATLEASTW` pre-charge
///    clones (QUI-2546). A successful `ATLEAST`/`ATLEASTW` with slack
///    variables holds up to five at once (the two clones, the `usize`
///    indices, the coefficient copy and a reallocation transient), but only
///    after its expansion charge, 24·N² + 8·N bytes for N = n + slacks, has
///    passed. So N ≤ √(L/24) (209 at 1 MiB), every such buffer is at most
///    16·N + 8 bytes, and for L ≤ 1 MiB they land in orders ≤ 4 KiB. The
///    ceiling counts them as three extra chunks in every such order:
///    +0.024·L at 1 MiB, +0.048·L' at 256 KiB. Charging or borrowing these
///    temporaries tightens this; changing the expansion charge re-derives it;
/// 3. a run's holder slots are 256 registers, 256 OUTPUT slots and
///    `MaxLoopDepth` (32) ITER frames, so raising `MaxOutputSlots` or
///    `MaxLoopDepth` requires a re-measure;
/// 4. no on-chain input can carry a model (the `LVAL` exclusion; the calldata
///    tripwire test below);
/// 5. the cap is a power of two (asserted below);
/// 6. the importer's allocator is the freeing-bump one this model describes
///    (power-of-two orders, an 8-byte header, no splitting or coalescing); a
///    polkadot-sdk repin that changes the executor's allocator, or a
///    runtime-side allocator, invalidates the per-order sum and re-runs the
///    census.
///
/// Any new xqvm opcode or allocation path -- any xqvm repin -- re-runs the
/// census.
///
/// The main uncharged source is `EQUALITY`/`ATLEAST`/`ATLEASTW`'s pre-charge
/// clones and success-path temporaries (QUI-2546; reachable by programs the
/// full checker accepts too). Once xqvm charges or borrows them, re-measure
/// and tighten the pin. The pinned budget at 24 is 93 MiB, 125 MiB with
/// `MIN_FREE`; the largest pin the 128 MiB heap allows with `MIN_FREE` is 27
/// (27 + 1 + 64 + 4 + 32), set by the block-residency term.
///
/// History: 20 until census covered the clones in every order, then 21, then
/// 24 with the mixed-family holders and the headroom above (A6). Under xqvm
/// 0.5.0 the `ITER` loop-frame pile-ups and the `ENERGY` clone made it
/// 28.64·L, pinned at 29; W1-011 and W1-013 remove
/// both. Derived for a power-of-two cap, and for a chain whose every stored
/// program passes the structural gate (the precondition in [`MaxVmMemory`]'s
/// docs: `execute` and the mempool transform do not re-check, so a program
/// stored under 0.5.0 still piles up, about 26.7·L of Σ measured at 1 MiB with
/// the 0.5.0 pile-up runs under this runtime; 28.64·L under 0.5.0).
#[doc(hidden)]
pub const VM_SIGMA_ORDERS_PER_L: u64 = 24;

/// Worst-case block residency in the importer heap: what a VALID block -- one
/// the builder admits, imported through finalize -- makes the importer hold,
/// measured **61.971 MiB** at the QUI-2548 block lengths (census
/// `d_encoded_block_residency_and_largest_extrinsic`, A10; 62.973 MiB under
/// the previous 5 MiB length, A6h), pinned at **64** (at least 1 MiB above).
///
/// Three components, from census d's shapes:
/// - **Decode and apply of a large extrinsic.** The worst block is one remark
///   with a payload just over 2 MiB plus fillers. The call is decoded twice --
///   by `LazyExtrinsic::decode`, then again by `do_apply_extrinsic`'s
///   re-decode -- and each decode grows two buffers in codec's
///   `decode_vec_with_len` (claude_reviewer's allocation trace,
///   `alloc-trace-cliff.log`): one ends at exactly the payload length, the
///   other doubles from 524,480 B. Step 1: a payload over 2 MiB puts the
///   exactly sized buffers in the 4 MiB order. Step 2: the doubling buffer's
///   rung 4 × 524,480 B = 2 MiB + 768 lands in the 8 MiB order; above 4 MiB +
///   1.5 KiB its next rung lands in the 16 MiB order. The opaque body and the
///   re-encoded extrinsic already sit in the 4 MiB order. Measured (A10): a
///   payload of 2 MiB − 1 holds 47.98 MiB; 2 MiB + 1 holds 53.98 (+6.0);
///   2 MiB + 768 holds 61.97 (+8.0), a plateau to about 2 MiB + 1.2 KiB, then
///   falling slowly as fewer fillers fit (55.96 MiB at the largest remark the
///   builder admits). The same block trapped at the last apply (no finalize)
///   holds 58.97 MiB of the 61.97.
/// - **The finalize re-read of `ExtrinsicData`.** `frame_system::finalize`
///   reads every extrinsic's stored bytes back to compute the extrinsics root.
/// - **The extrinsics-root encoding.** `ordered_root` `encode()`s those bytes
///   as one `Vec<Vec<u8>>`, through a buffer that starts from codec's
///   `[T]::size_hint` and grows by doubling; the freed rungs stay in their
///   orders. With the re-read, finalize adds 3.0 MiB to the worst block and up
///   to 23.0 MiB to a block of small fillers (27.3–35.0 MiB valid; 7.7–23.0
///   MiB trapped at the last apply).
///
/// Method: census d measures valid blocks only. A block that traps at apply or
/// at `final_checks` never finalizes and must never be this term's worst
/// case; census d keeps a copy of each shape trapped at the last apply (no
/// finalize) as a control. (Until A6h this term was 45 MiB, from a trapped block whose
/// remark payload sat about 1.7 KB under the 2 MiB cliff.) A cap below 2 MiB
/// on one extrinsic's payload would remove both steps; that is an owner
/// decision, and a runtime upgrade carries the Wasm (1,587,318 B today).
///
/// **Oversized Operational calls are closed by `BlockLength` (QUI-2548, owner
/// decision of 2026-11-25).** A signed Operational call -- `set_code`, or a
/// `Foundation::propose` wrapping any call -- that fails at dispatch is still
/// included and finalizes. Its payload is now bounded by the Operational limit
/// (3.75 MiB of block, header included), in the same 4 MiB allocator order as
/// the Normal class's largest remark, and census d measures the class: a
/// signed `set_code` swept across the 2 MiB cliffs holds exactly what a remark
/// of the same body holds, and the largest admitted (body 3,931,989 B) holds
/// 55.957 MiB. Census k's block of 119 VM runs plus the largest `set_code`
/// that still fits (3,718,826 B) imports at 73.746 MiB (42.4 % free). Under
/// the previous 5 MiB length that shape, with a 4,196,352-B `set_code` (the
/// 8 MiB order), was admitted by the builder and its import failed with
/// "Allocator ran out of space".
///
/// **`eth_transact` is capped before the runtime decodes it (A10b, A10c).** An
/// unsigned `eth_transact` holds more than a remark of the same body: the
/// RLP-decoded input, the signer recovery's re-encode, `into_call`'s copy and
/// the stored encoded transaction. Uncapped, a valid block with one 2 MiB +
/// 768 B `eth_transact` and fillers held 65.975 MiB (claude_reviewer's A10 R8),
/// above this term. The runtime now refuses a payload over
/// [`crate::MAX_ETH_TRANSACT_PAYLOAD`] (1,245,184 B) before decoding it, and
/// census d measures the family up to the cap with the same fillers: calls of
/// 256 KiB to the cap hold 36.75–49.25 MiB (1.0–4.0 MiB above a remark of the
/// same body); the largest PVM instantiate (reverted under A9, not deployed)
/// holds 49.50 MiB as a legacy transaction and 49.33 MiB as an EIP-1559 one
/// whose access list fills the rest of the cap.
///
/// **pallet-revive contract execution is closed by origin (QUI-2549):** only
/// Root and Foundation members may upload or instantiate code
/// ([`ReviveDeployOrigin`]), and a fresh genesis holds no contracts. The
/// budget has no revive term; the closure rests on measurement: under the
/// previous 5 MiB length, a block carrying a revive contract's recursion and a
/// signed `set_code` of 4 MiB or more was admitted by the builder and its
/// import failed with "Allocator ran out of space" (`eth_transact` is
/// unmeasured). **Opening deployment requires
/// a measured revive term plus limits.**
///
/// Every contract the Foundation deploys is callable by anyone, and the
/// budget has no revive term. A contract that forwards caller-supplied data or
/// calls a caller-chosen address recurses to `CALL_STACK_DEPTH`
/// (`AllowReentry`), and one that grows memory by a caller-controlled amount
/// reaches `EVM_MEMORY_BYTES` per frame. Either reopens the memory shape without
/// any `CREATE` (measured: a 19-byte self-forwarder with 128 KiB calldata,
/// 9.0 MiB; 1 MiB of memory plus recursion, 29.4 MiB). So each Foundation
/// deployment needs its worst-case memory reviewed against the budget, and a
/// contract that `CREATE`s/`CREATE2`s caller-supplied code must never be
/// deployed.
#[doc(hidden)]
pub const VM_BLOCK_RESIDENCY: u64 = 64 * 1024 * 1024;

/// Maximal non-VM dispatch, pinned at 4 MiB. The two largest measured: a
/// `submit_proof` decoding the full topology meta plus the packed proof,
/// 3.27 MiB (census `f_non_vm_baseline_submit_proof`), now the larger; and a
/// `store_program` decoding a `MaxProgramSize` program at `MaxProgramBlocks`
/// and running the structural gate over it, 1.36 MiB (census
/// `l_store_program_verify_peak`: 1.340 MiB for the nested and flat shapes,
/// 1.355 MiB for the jump chain). Under the full checker that `store_program`
/// was 3.76 MiB.
#[doc(hidden)]
pub const VM_NON_VM_BASELINE: u64 = 4 * 1024 * 1024;

/// The importer-heap budget from the pinned terms: Σ_orders(k·L) + the
/// fixed-run allowance + block residency + the non-VM baseline. The budget
/// test checks it against the heap; census `g` checks a real block against it.
#[doc(hidden)]
pub fn vm_importer_budget() -> u64 {
    VM_SIGMA_ORDERS_PER_L
        .saturating_mul(MaxVmMemory::get().max(MaxTransformVmMemory::get()))
        .saturating_add(VM_FIXED_RUN_ALLOWANCE)
        .saturating_add(VM_BLOCK_RESIDENCY)
        .saturating_add(VM_NON_VM_BASELINE)
}

/// Heap footprint of the compute mempool's per-extrinsic retained
/// `transformed: Vec<Vec<i8>>` (kimi W1-002 follow-up): it lives across the
/// per-solution VMs of one `submit_solution`, so it is retained for the
/// extrinsic, not a run, and is covered by the fixed-run allowance's slack.
/// Counted with the census `chunk_bytes` rule (freeing-bump chunk =
/// next_pow2(max(len, 8)) + 8): each inner `Vec<i8>` of `QuantumMaxNodes`
/// bytes, plus the outer Vec of `QuantumMaxSolutions` 24-byte headers.
#[doc(hidden)]
pub fn vm_transform_retained_heap() -> u64 {
    let chunk = |len: u64| len.max(8).next_power_of_two().saturating_add(8);
    let solutions = u64::from(QuantumMaxSolutions::get());
    solutions
        .saturating_mul(chunk(u64::from(QuantumMaxNodes::get())))
        .saturating_add(chunk(solutions.saturating_mul(24)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_compute_reward_covers_worst_ranked_tail_share() {
        let winners = pallet_quantum_compute_mempool::types::MAX_REWARD_WINNERS as u128;
        let tail = QuantumMinReward::get() / (winners * (winners + 1) / 2);
        assert_eq!(tail, 1_893_939_393);
        assert!(tail >= EXISTENTIAL_DEPOSIT);
    }

    /// W1-006: the escrow funding transfer creates the order's escrow account,
    /// so the smallest possible reward must reach the existential deposit; and
    /// distinct orders must get distinct escrow accounts.
    #[test]
    fn compute_reward_escrow_accounts_are_creatable_and_distinct() {
        assert!(QuantumMinReward::get() >= EXISTENTIAL_DEPOSIT);
        let escrow = pallet_quantum_compute_mempool::Pallet::<Runtime>::escrow_account;
        assert_ne!(escrow(0), escrow(1));
        assert_ne!(escrow(1), escrow(u64::MAX));
    }

    #[test]
    fn network_clock_and_lifecycle_use_the_approved_durations() {
        use frame_support::traits::Get;

        let (session_hours, era_hours, slack, longevity) =
            match crate::network_params::EPOCH_DURATION_SLOTS {
                2_400 => (4, 24, 1_200, 432_000),
                600 => (1, 6, 300, 108_000),
                other => panic!("unapproved epoch duration: {other}"),
            };
        assert_eq!(crate::SLOT_DURATION, 6_000);
        assert_eq!(EpochDuration::get(), session_hours * crate::HOURS as u64);
        assert_eq!(validator_onboarding::SessionsPerEra::get(), 6);
        assert_eq!(6 * EpochDuration::get(), era_hours * crate::HOURS as u64);
        assert_eq!(election::MinimumElectionSlack::get(), slack);
        assert_eq!(validator_onboarding::BondingDuration::get(), 30);
        assert_eq!(
            <<Runtime as pallet_staking::Config>::SlashDeferDuration as Get<u32>>::get(),
            27
        );
        assert_eq!(
            <<Runtime as pallet_staking::Config>::HistoryDepth as Get<u32>>::get(),
            84
        );
        assert_eq!(validator_onboarding::SetIdSessionEntries::get(), 181);
        assert_eq!(slashing::ReportLongevity::get(), longevity);
    }

    #[test]
    fn runtime_block_weights_use_benchmarked_overhead() {
        let weights = RuntimeBlockWeights::get();

        assert_eq!(weights.base_block, BlockExecutionWeight::get());
        for class in DispatchClass::all() {
            assert_eq!(
                weights.get(*class).base_extrinsic,
                ExtrinsicBaseWeight::get(),
                "unexpected base extrinsic weight for {class:?}",
            );
        }
    }

    #[test]
    fn runtime_block_weight_limits_preserve_sensible_defaults() {
        let weights = RuntimeBlockWeights::get();
        let max_block = Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX);
        let normal_max = NORMAL_DISPATCH_RATIO * max_block;

        assert_eq!(weights.max_block, max_block);
        assert_eq!(
            weights.get(DispatchClass::Normal).max_total,
            Some(normal_max)
        );
        assert_eq!(
            weights.get(DispatchClass::Operational).max_total,
            Some(max_block)
        );
        assert_eq!(
            weights.get(DispatchClass::Operational).reserved,
            Some(max_block - normal_max)
        );
        let initialization = AVERAGE_ON_INITIALIZE_RATIO * max_block;
        assert_eq!(
            weights.get(DispatchClass::Normal).max_extrinsic,
            Some(normal_max - initialization - ExtrinsicBaseWeight::get())
        );
        assert_eq!(
            weights.get(DispatchClass::Operational).max_extrinsic,
            Some(max_block - initialization - ExtrinsicBaseWeight::get())
        );
        assert_eq!(weights.get(DispatchClass::Mandatory).max_total, None);
    }

    #[test]
    fn runtime_block_weights_validate() {
        RuntimeBlockWeights::get()
            .validate()
            .expect("runtime block weights must be internally consistent");
    }
}

#[cfg(test)]
mod xqvm_weights {
    use super::*;

    /// The whole-block importer heap, derived from the per-run VM caps
    /// (W1-002), must stay under the executor's heap with margin.
    ///
    /// This pins the full V-A step-0 derivation, not just a per-run share. The
    /// failure it guards is not a rejected extrinsic: an allocation the heap
    /// cannot satisfy traps the whole `Core_execute_block`, which no dispatch
    /// error can report, so the bound has to hold for the *block*, not one run.
    /// Every term below is measured through `sc_executor::WasmExecutor` with the
    /// node's default heap by `runtime/tests/vm_heap_census.rs`; the values are
    /// restated here as the contract the constants must satisfy, and raising a
    /// cap past it fails this test rather than silently permitting an
    /// unimportable block.
    ///
    /// The budget covers the Operational class at its `BlockLength` limit
    /// (QUI-2548, closed by `BlockLength` and measured by census d and k) and
    /// has no revive term: pallet-revive contract execution is closed by origin
    /// instead (QUI-2549). See `VM_BLOCK_RESIDENCY`.
    #[test]
    fn vm_memory_budget_stays_under_the_runtime_heap() {
        /// `sc_executor::DEFAULT_HEAP_ALLOC_PAGES`, in bytes: 2048 64 KiB
        /// pages. Restated rather than imported -- it is a client-side
        /// constant and the runtime does not depend on the executor.
        const RUNTIME_HEAP_BYTES: u64 = 2048 * 64 * 1024; // 128 MiB

        // Whole-block VM growth across allocator orders, measured as a ratio of
        // the per-run cap L, under xqvm 0.6.0: the mixed-family order sweep,
        // the register, OUTPUT and stack holders, the EQUALITY/ATLEASTW clone
        // and success-path holders and the mixed-family holders reached
        // 20.99 x L at the shipped 1 MiB (census k, which defines the pin; the
        // sweep alone 17.32), pinned at 24 with stated headroom (see
        // VM_SIGMA_ORDERS_PER_L) -- for power-of-two caps, asserted below.
        // Rounding and the uncharged EQUALITY/ATLEASTW clones (QUI-2546) are
        // folded in; LVAL is excluded by the calldata invariant (see
        // `on_chain_calldata_cannot_carry_a_model`).
        const SIGMA_ORDERS_PER_L: u64 = VM_SIGMA_ORDERS_PER_L;
        // L-independent fixed per-run overhead: every allocation the VM memory
        // meter does not charge, at its worst case -- a maximal program loaded
        // and decoded, the value stack at xqvm's STACK_LIMIT (8,192 i64
        // entries; bounded by that count, not by the meter), the loop stack at
        // MaxLoopDepth frames (since xqvm 0.6.0's jump-span phase, W1-011, no
        // back-edge in a program stored under it can pile up more; a program
        // stored before it is outside this budget, see the precondition in
        // MaxVmMemory's docs), 256 registers and output slots, a jump
        // table filling MaxProgramBlocks. The allowance, the measured worst case and the
        // mempool's retained transform (164,520 B today) are the shared items
        // above; census `i` asserts the measurement against the same constant.
        // The assertion keeps the slack visible: if the measured worst case
        // plus the retained transform outgrew the allowance, it would need its
        // own budget line.
        const FIXED_RUN: u64 = VM_FIXED_RUN_ALLOWANCE;
        const FIXED_RUN_MEASURED: u64 = VM_FIXED_RUN_MEASURED;
        let transform_retained = vm_transform_retained_heap();
        assert!(
            transform_retained.saturating_add(FIXED_RUN_MEASURED) <= FIXED_RUN,
            "mempool retained transform {transform_retained} B + measured fixed run \
             {FIXED_RUN_MEASURED} B (worst case) exceeds the {FIXED_RUN} B fixed-run term; give the \
             retained Vec its own budget line",
        );
        // Worst-case block the importer holds in the same heap: a valid block
        // imported through finalize, its largest extrinsic's payload just over
        // 2 MiB -- measured 62.973 MiB (census
        // `d_encoded_block_residency_and_largest_extrinsic`, A6h).
        const BLOCK_RESIDENCY: u64 = VM_BLOCK_RESIDENCY;
        // Maximal non-VM dispatch: a `submit_proof` decoding the full
        // topology meta plus the packed proof, 3.27 MiB (census
        // `f_non_vm_baseline_submit_proof`); `store_program` running the
        // structural gate over a 64 KiB, 2,048-block program, 1.36 MiB (census
        // `l_store_program_verify_peak`; 3.76 MiB under the full checker).
        const NON_VM_BASELINE: u64 = VM_NON_VM_BASELINE;
        // Keep at least a quarter of the heap free (owner/architect gate).
        const MIN_FREE: u64 = RUNTIME_HEAP_BYTES / 4; // 32 MiB

        let vm_term =
            SIGMA_ORDERS_PER_L.saturating_mul(MaxVmMemory::get().max(MaxTransformVmMemory::get()));
        let budget = vm_term
            .saturating_add(FIXED_RUN)
            .saturating_add(BLOCK_RESIDENCY)
            .saturating_add(NON_VM_BASELINE);
        assert_eq!(
            budget,
            vm_importer_budget(),
            "the shared budget function sums the same terms"
        );
        // The Σ pin is derived for power-of-two caps. At a cap L that is not a
        // power of two, the charged sample itself -- just over half a
        // next_pow2(L) chunk, still <= L -- lands in the order above L's.
        // Under xqvm 0.5.0 its uncharged ENERGY clone doubled it there
        // (30.24·L at an emulated 768 KiB); 0.6.0 (W1-013) removes the clone,
        // and the excess is now small (20.17·L against 19.44 at 1 MiB),
        // but the guard stays until Σ is re-derived for such a cap. The guard itself rests on an
        // assumption about xqvm: every buffer it allocates is no larger than
        // its own charge, or than a charged source it copies, and is aligned
        // to at most 8 bytes. A buffer bigger than what was charged for it
        // would reach the next order at a power-of-two cap too.
        for (name, cap) in [
            ("MaxVmMemory", MaxVmMemory::get()),
            ("MaxTransformVmMemory", MaxTransformVmMemory::get()),
        ] {
            assert!(
                cap.is_power_of_two(),
                "{name} = {cap} B is not a power of two: such a cap reaches the next_pow2(L) \
                 allocator order, which the Σ pin was not derived for. Re-run census k (whose \
                 sweeps cover the next order) and re-derive VM_SIGMA_ORDERS_PER_L before \
                 changing the cap"
            );
        }

        // The pinned sum at the shipped caps, spelled out so a change to any
        // term shows here: 24 x 1 MiB + 1 MiB + 64 MiB + 4 MiB = 93 MiB, and
        // with MIN_FREE 125 MiB of the 128 MiB heap (a margin of 3 MiB).
        const MIB: u64 = 1024 * 1024;
        assert_eq!(
            budget,
            93 * MIB,
            "pinned importer budget at the shipped caps"
        );
        assert_eq!(budget.saturating_add(MIN_FREE), 125 * MIB);
        assert!(
            budget.saturating_add(MIN_FREE) <= RUNTIME_HEAP_BYTES,
            "importer budget {budget} bytes + {MIN_FREE} min-free exceeds the \
             {RUNTIME_HEAP_BYTES} byte heap (VM term {vm_term}); lower MaxVmMemory/\
             MaxTransformVmMemory or re-measure the terms",
        );
    }

    /// Tripwire for the `LVAL` exclusion above (W1-002 R8 F2): `LVAL` clones a
    /// model without charging VM memory, and is unreachable on chain only
    /// because no on-chain input can carry a model. If either calldata ABI ever
    /// can, this fails -- the `execute` half at compile time, the transform half
    /// at test time -- and `LVAL` has to be charged or re-bounded first.
    #[test]
    fn on_chain_calldata_cannot_carry_a_model() {
        // `execute` calldata is integers only: a richer type does not compile.
        fn execute_calldata(
            call: pallet_xqvm::Call<Runtime>,
        ) -> Option<frame_support::BoundedVec<i64, <Runtime as pallet_xqvm::Config>::MaxCallDataLen>>
        {
            match call {
                pallet_xqvm::Call::execute { calldata, .. } => Some(calldata),
                _ => None,
            }
        }
        let _ = execute_calldata;
        // Transform calldata is exactly one integer vector.
        let calldata = pallet_quantum_compute_mempool::xqvm::transform_calldata(vec![1, -1, 1]);
        assert!(
            matches!(calldata.as_slice(), [xqvm::RegVal::VecInt(spins)] if spins == &vec![1, -1, 1]),
            "transform calldata ABI changed: {calldata:?}"
        );
    }

    /// The transform VM weight and the mempool adapter must agree on the
    /// number of output slots a transform VM gets: the weight charges
    /// `TRANSFORM_OUTPUT_SLOTS`, and the adapter must give the VM exactly that
    /// many. The weight half is checked arithmetically; the adapter half by
    /// running a transform that writes the last slot (accepted) and one that
    /// writes one past it (rejected).
    #[test]
    fn transform_weight_charges_the_adapters_output_slots() {
        use pallet_quantum_compute_mempool::xqvm::{
            QuantumVm, XqvmAdapter, TRANSFORM_OUTPUT_SLOTS,
        };
        use sp_runtime::traits::{BlakeTwo256, Hash, TrailingZeroInput};

        assert_eq!(
            QuantumProgramLookupWeight::get(),
            pallet_xqvm::SubstrateWeight::<Runtime>::execute(
                MaxProgramSize::get(),
                TRANSFORM_OUTPUT_SLOTS
            )
            .saturating_mul(u64::from(QuantumMaxSolutions::get())),
        );

        sp_io::TestExternalities::default().execute_with(|| {
            let solver = <AccountId as codec::Decode>::decode(&mut TrailingZeroInput::zeroes())
                .expect("zero account decodes");
            for (slot, accepted) in [
                (TRANSFORM_OUTPUT_SLOTS - 1, true),
                (TRANSFORM_OUTPUT_SLOTS, false),
            ] {
                // r0 = calldata[0] (the spin VecInt); OUTPUT it to `slot`.
                let mut b = xqvm::InstructionBuilder::new();
                b.emit_push(0).emit(xqvm::Instruction::Input {
                    reg: xqvm::Register(0),
                });
                b.emit_push(i64::from(slot))
                    .emit(xqvm::Instruction::Output {
                        reg: xqvm::Register(0),
                    });
                b.emit_halt();
                let bytes = b.build().expect("builds").encode();
                let hash = BlakeTwo256::hash(&bytes);
                pallet_xqvm::Programs::<Runtime>::insert(
                    hash,
                    frame_support::BoundedVec::<u8, MaxProgramSize>::try_from(bytes).expect("fits"),
                );
                let outcome = XqvmAdapter::<Runtime>::transform_solutions(
                    &BlakeTwo256::hash(b"slot-spec"),
                    None,
                    Some(&hash),
                    &solver,
                    vec![vec![1, -1]],
                );
                assert_eq!(
                    outcome.result.is_ok(),
                    accepted,
                    "output slot {slot}: {:?}",
                    outcome.result
                );
            }
        });
    }

    /// A single `execute` at the maximum step limit must fit inside the budget
    /// the limit was derived from.
    ///
    /// This is the invariant the old hand-set `WeightPerStep` violated: it
    /// admitted ~750M steps priced at 0.75 s that took over ten seconds to run,
    /// against a two-second block. Both constants are now derived from the
    /// generated weights, so this holds by construction -- the test is here to
    /// catch a regeneration or a refactor that quietly breaks the derivation.
    #[test]
    fn max_step_limit_fits_the_reserved_budget() {
        let normal = RuntimeBlockWeights::get()
            .get(DispatchClass::Normal)
            .max_total
            .unwrap_or(RuntimeBlockWeights::get().max_block)
            .ref_time();
        let reserved = normal / 2;

        let worst_case = pallet_xqvm::SubstrateWeight::<Runtime>::execute(
            MaxProgramSize::get(),
            MaxOutputSlots::get(),
        )
        .ref_time()
        .saturating_add(
            XqvmWeightPerStep::get()
                .ref_time()
                .saturating_mul(MaxStepLimit::get()),
        );

        assert!(
            worst_case <= reserved,
            "worst-case execute is {worst_case} ps against {reserved} ps reserved",
        );
    }

    /// The per-step price must stay above the measured marginal cost of a step.
    ///
    /// Derived from the same benchmark rather than a copied number, so it
    /// tracks regeneration instead of going stale.
    #[test]
    fn per_step_price_exceeds_measured_cost() {
        let measured = pallet_xqvm::SubstrateWeight::<Runtime>::execute_step(1)
            .saturating_sub(pallet_xqvm::SubstrateWeight::<Runtime>::execute_step(0))
            .ref_time();

        assert!(
            XqvmWeightPerStep::get().ref_time() > measured,
            "priced {} ps/step is not above the measured {measured} ps/step",
            XqvmWeightPerStep::get().ref_time(),
        );
    }
}

pub mod election;
#[cfg(test)]
mod governance_matrix;
pub mod launch_time;
pub mod staking_rewards;
pub mod validator_onboarding;

pub mod session_keys;
pub mod slashing;
pub mod vesting;

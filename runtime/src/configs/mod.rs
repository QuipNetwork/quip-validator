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
use frame_support::{
    derive_impl,
    dispatch::DispatchClass,
    parameter_types,
    traits::{ConstBool, ConstU128, ConstU32, ConstU64, ConstU8, Get, VariantCountOf},
    weights::{
        constants::{RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
        IdentityFee, Weight,
    },
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::{ConstFeeMultiplier, FungibleAdapter, Multiplier};
use sp_core::crypto::Ss58Codec;
use sp_runtime::{
    traits::{ConvertInto, One, OpaqueKeys},
    FixedU128, Perbill,
};
use sp_version::RuntimeVersion;

use pallet_xqvm::WeightInfo as _;

// Local module imports
use super::{
    AccountId, Address, Babe, Balance, Balances, Block, BlockNumber, EthExtraImpl, Hash, Nonce,
    PalletInfo, Runtime, RuntimeCall, RuntimeEvent, RuntimeFreezeReason, RuntimeHoldReason,
    RuntimeOrigin, RuntimeTask, SessionKeys, Signature, System, Timestamp, EXISTENTIAL_DEPOSIT,
    MICRO_UNIT, MILLI_UNIT, SLOT_DURATION, UNIT, VERSION,
};
use crate::weights::{BlockExecutionWeight, ExtrinsicBaseWeight};

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
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
    // Replacement for the now-deprecated `BlockLength::max_with_normal_ratio` —
    // reconstruct the same shape via the builder: max = 5 MiB for all dispatch
    // classes, but the Normal class is scaled down by `NORMAL_DISPATCH_RATIO`.
    pub RuntimeBlockLength: BlockLength = BlockLength::builder()
        .max_length(5 * 1024 * 1024)
        .modify_max_length_for_class(frame_support::dispatch::DispatchClass::Normal, |len| {
            *len = NORMAL_DISPATCH_RATIO * (5u32 * 1024 * 1024);
        })
        .build();
    pub const SS58Prefix: u8 = 42;
}

/// All migrations of the runtime, aside from the ones declared in the pallets.
///
/// This can be a tuple of types, each implementing `OnRuntimeUpgrade`.
#[allow(unused_parens)]
type SingleBlockMigrations = ();

/// The default types are being injected by [`derive_impl`](`frame_support::derive_impl`) from
/// [`SoloChainDefaultConfig`](`struct@frame_system::config_preludes::SolochainDefaultConfig`),
/// but overridden as needed.
#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig)]
impl frame_system::Config for Runtime {
    /// The block type for the runtime.
    type Block = Block;
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
    type SingleBlockMigrations = SingleBlockMigrations;
}

parameter_types! {
    // BABE epochs are defined in slots. Keep them short enough for local development.
    pub const EpochDuration: u64 = 10 * super::MINUTES as u64;
    pub const ExpectedBlockTime: u64 = SLOT_DURATION;
}

impl pallet_babe::Config for Runtime {
    type EpochDuration = EpochDuration;
    type ExpectedBlockTime = ExpectedBlockTime;
    type EpochChangeTrigger = pallet_babe::SameAuthoritiesForever;
    type DisabledValidators = ();
    type WeightInfo = ();
    type MaxAuthorities = ConstU32<32>;
    type MaxNominators = ConstU32<0>;
    type KeyOwnerProof = sp_core::Void;
    type EquivocationReportSystem = ();
}

impl pallet_grandpa::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;

    type WeightInfo = ();
    type MaxAuthorities = ConstU32<32>;
    type MaxNominators = ConstU32<0>;
    type MaxSetIdSessionEntries = ConstU64<0>;

    type KeyOwnerProof = sp_core::Void;
    type EquivocationReportSystem = ();
}

/// Session keys (BABE + GRANDPA) are registered at genesis and never rotated by
/// the runtime — `SessionManager = ()` returns `None` on `new_session`, so the
/// pallet retains the genesis validator set forever. The session API exists so
/// that explorers and the polkadot.js client can surface
/// `api.query.session.validators` and so that hybrid session keys can be
/// rotated via the standard `author_rotateKeys` RPC flow once that work lands.
impl pallet_session::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type ValidatorId = <Self as frame_system::Config>::AccountId;
    type ValidatorIdOf = ConvertInto;
    type ShouldEndSession = Babe;
    type NextSessionRotation = Babe;
    type SessionManager = ();
    type SessionHandler = <SessionKeys as OpaqueKeys>::KeyTypeIdProviders;
    type Keys = SessionKeys;
    type DisablingStrategy = ();
    type WeightInfo = pallet_session::weights::SubstrateWeight<Runtime>;
    type Currency = Balances;
    type KeyDeposit = ();
}

impl pallet_timestamp::Config for Runtime {
    /// A timestamp: milliseconds since the unix epoch.
    type Moment = u64;
    type OnTimestampSet = Babe;
    type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
    type WeightInfo = ();
}

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
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
    pub const ReviveDepositPerByte: Balance = 10 * MICRO_UNIT;
    pub const ReviveDepositPerItem: Balance = 200 * MILLI_UNIT;
    pub const ReviveDepositPerChildTrieItem: Balance = 2 * MILLI_UNIT;
    pub ReviveCodeHashLockupDepositPercent: Perbill = Perbill::from_percent(30);
    pub const ReviveMaxEthExtrinsicWeight: FixedU128 = FixedU128::from_rational(9, 10);
    /// One native 12-decimal plank equals 10^6 Ethereum 18-decimal units.
    pub const ReviveNativeToEthRatio: u32 = 1_000_000;
}

impl pallet_evm_chain_id::Config for Runtime {}

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
    type DepositPerByte = ReviveDepositPerByte;
    type DepositPerItem = ReviveDepositPerItem;
    type DepositPerChildTrieItem = ReviveDepositPerChildTrieItem;
    type CodeHashLockupDepositPercent = ReviveCodeHashLockupDepositPercent;
    type AddressMapper = pallet_revive::AccountId32Mapper<Runtime>;
    type AllowEVMBytecode = ConstBool<true>;
    type UploadOrigin = frame_system::EnsureSigned<AccountId>;
    type InstantiateOrigin = frame_system::EnsureSigned<AccountId>;
    type RuntimeMemory = ConstU32<{ 128 * 1024 * 1024 }>;
    type PVFMemory = ConstU32<{ 512 * 1024 * 1024 }>;
    type ChainId = pallet_evm_chain_id::ChainId<Runtime>;
    type NativeToEthRatio = ReviveNativeToEthRatio;
    type FeeInfo = pallet_revive::evm::fees::Info<Address, Signature, EthExtraImpl>;
    type MaxEthExtrinsicWeight = ReviveMaxEthExtrinsicWeight;
    type DebugEnabled = ConstBool<false>;
    type GasScale = ConstU32<1_000>;
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = pallet_sudo::weights::SubstrateWeight<Runtime>;
}

/// Configure the pallet-template in pallets/template.
impl pallet_template::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = pallet_template::weights::SubstrateWeight<Runtime>;
}

impl pallet_faucet_ops::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type WeightInfo = pallet_faucet_ops::weights::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const MaxProgramSize: u32 = 65_536;
    pub const MaxCallDataLen: u32 = 256;
    pub const MaxOutputSlots: u32 = 256;
    pub const XqvmWeightPerStep: Weight = Weight::from_parts(1_000, 0);

    /// Derived from block weight budget so a single execute call always
    /// fits in one block.  Uses 50 % of the normal dispatch budget to
    /// leave room for other extrinsics in the same block.
    pub MaxStepLimit: u64 = {
        let normal = RuntimeBlockWeights::get()
            .get(frame_support::dispatch::DispatchClass::Normal)
            .max_total
            .unwrap_or(RuntimeBlockWeights::get().max_block);
        // Reserve half for other extrinsics.
        let budget = normal.ref_time() / 2;
        // Subtract execute_base overhead, then divide by per-step cost.
        let base = pallet_xqvm::SubstrateWeight::<Runtime>::execute_base()
            .ref_time();
        let per_step = XqvmWeightPerStep::get().ref_time();
        budget.saturating_sub(base) / per_step
    };
}

/// Configure the XQVM pallet for on-chain bytecode execution.
impl pallet_xqvm::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type MaxProgramSize = MaxProgramSize;
    type MaxCallDataLen = MaxCallDataLen;
    type MaxOutputSlots = MaxOutputSlots;
    type MaxStepLimit = MaxStepLimit;
    type WeightPerStep = XqvmWeightPerStep;
    type WeightInfo = pallet_xqvm::SubstrateWeight<Runtime>;
}

parameter_types! {
    pub const QuantumMaxNodes: u32 = 5_000;
    pub const QuantumMaxEdges: u32 = 50_000;
    pub const QuantumMaxSolutions: u32 = 20;
    pub const QuantumMaxBidMiners: u32 = 16;
    pub const QuantumMaxOrdersPerProposer: u32 = 32;
    pub const QuantumMaxDeadlineBlocks: BlockNumber = 1_000;
    pub const QuantumMaxBlockWait: BlockNumber = 100;
    pub const QuantumMinReward: Balance = UNIT;
    pub const QuantumResultTtlBlocks: BlockNumber = 10_000;
    pub const QuantumPowMaxNodes: u32 = 5_000;
    pub const QuantumPowMaxEdges: u32 = 50_000;
    pub const QuantumPowMaxSolutions: u32 = 32;
    pub const QuantumPowMinNodes: u32 = 16;
    /// Decay interval: difficulty eases 2.5% per full epoch elapsed since
    /// the last qblock (PoW-won block). 100 chain blocks is deliberately
    /// co-located with `TARGET_PROOF_BLOCKS` (pallet-quantum-pow
    /// `difficulty.rs`): the first decay step, the gentle-hardening plateau
    /// (5%±4%), and the easing rate ramp all begin at the same 100-block
    /// (600s) boundary, so a qblock round is either "sub-epoch, adjusts
    /// from the stored difficulty" or "decayed, adjusts gently from the
    /// decay-eased base" — never a mix. They are separate constants on
    /// purpose (decay cadence vs rate-band anchor); retuning this value
    /// does NOT move the rate bands, so revisit both together.
    pub const QuantumPowEpochLength: BlockNumber = 100;
    /// Retarget over twenty target intervals.
    ///
    /// Qblock arrivals are ~Poisson, so shot noise is `sqrt(expected)`. The
    /// clamp fires whenever `|N - E| >= E/4`: at `E = 10` that is
    /// `sigma/E = 0.32`, and ~43% of on-target windows still slam the bar by the
    /// full quarter-span. The proportional band dominates only once
    /// `sigma/E <= 0.25`, i.e. `E >= 16`; twenty gives margin.
    pub const QuantumPowRetargetWindowEpochs: u32 = 20;
    /// Handicap slope, per-mille of the curve bar per sigma of frustration.
    /// Tunable by upgrade: riff-morph's calibration puts it between 4 and 29
    /// with correlations under 0.3, and 6 is the low end of that scatter.
    pub const QuantumPowHandicapPerSigmaPermille: i64 =
        pallet_quantum_pow::difficulty::DEFAULT_HANDICAP_PER_SIGMA_PERMILLE;
    /// Largest single-window difficulty move. NARROWING this without widening
    /// `QuantumPowRetargetWindowEpochs` fails `integrity_test`, which derives
    /// the minimum window from it.
    pub const QuantumPowRetargetMaxStepPermille: i64 =
        pallet_quantum_pow::difficulty::DEFAULT_RETARGET_MAX_STEP_PERMILLE;
    pub const QuantumPowMinerDeposit: Balance = UNIT;
    pub const QuantumPowBlockReward: Balance = UNIT;
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
    /// Induced width at or below which a topology's core is exactly solvable.
    /// Matches the riff toolkit's `EXACT_CEILING`; raising it re-classifies
    /// every registered topology at once as classical solvers improve.
    pub const QuantumPowExactSolveCeiling: u32 = 20;

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
}

/// Account attributed as the builder for migration-inserted default job specs.
///
/// Genesis presets can record each preset's actual root account directly, but
/// runtime migrations need one chain-independent account baked into the
/// runtime. Operator 1 is the current quip-testnet sudo holder.
pub const QUANTUM_DEFAULT_JOB_SPEC_BUILDER_SS58: &str =
    "5GZMoWFMoNGLZKT1tduLMQQQC7dBQo4MHkYqriCdDATXqaYi";

pub struct QuantumDefaultJobSpecBuilder;

impl Get<AccountId> for QuantumDefaultJobSpecBuilder {
    fn get() -> AccountId {
        AccountId::from_ss58check(QUANTUM_DEFAULT_JOB_SPEC_BUILDER_SS58)
            .expect("default job spec builder SS58 address is valid")
    }
}

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
    type DefaultIsingSpecId = pallet_quantum_compute_mempool::CanonicalDefaultIsingSpecId<Runtime>;
    type DefaultJobSpecBuilder = QuantumDefaultJobSpecBuilder;
    type VM = pallet_quantum_compute_mempool::xqvm::NoOpVm;
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
    type RetargetWindowEpochs = QuantumPowRetargetWindowEpochs;
    type MinerDeposit = QuantumPowMinerDeposit;
    type BlockReward = QuantumPowBlockReward;
    type MaxProofsPerBlock = QuantumPowMaxProofsPerBlock;
    type MaxAllowedValues = QuantumPowMaxAllowedValues;
    type CurveCEasyMilli = QuantumPowCurveCEasyMilli;
    type CurveCKneeMilli = QuantumPowCurveCKneeMilli;
    type CurveCHardMilli = QuantumPowCurveCHardMilli;
    type ExactSolveCeiling = QuantumPowExactSolveCeiling;
    type HandicapPerSigmaPermille = QuantumPowHandicapPerSigmaPermille;
    type RetargetMaxStepPermille = QuantumPowRetargetMaxStepPermille;
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
    type WeightInfo = pallet_miner_registry::weights::SubstrateWeight<Runtime>;
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Catches typos and silent address drift in the hardcoded SS58 string.
    /// Without this, a malformed constant would only surface as a panic inside
    /// `on_runtime_upgrade` on a live chain — a chain-bricking failure mode.
    #[test]
    fn default_job_spec_builder_ss58_decodes() {
        let parsed = AccountId::from_ss58check(QUANTUM_DEFAULT_JOB_SPEC_BUILDER_SS58)
            .expect("hardcoded SS58 must decode");
        assert_eq!(parsed, QuantumDefaultJobSpecBuilder::get());
    }
}

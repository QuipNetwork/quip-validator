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

// External crates imports
use alloc::{vec, vec::Vec};
use frame_support::{
    genesis_builder_helper::{build_state, get_preset},
    weights::Weight,
};
use pallet_grandpa::AuthorityId as GrandpaId;
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_runtime::{
    traits::{Block as BlockT, NumberFor},
    transaction_validity::{TransactionSource, TransactionValidity},
    ApplyExtrinsicResult,
};
use sp_session::OpaqueGeneratedSessionKeys;
use sp_version::RuntimeVersion;

// Local module imports
use super::{
    AccountId, Babe, Balance, Block, BlockNumber, Executive, Grandpa, Hash, InherentDataExt,
    MinerRegistry, Nonce, QuantumComputeMempool, QuantumPow, Runtime, RuntimeCall,
    RuntimeGenesisConfig, SessionKeys, System, TransactionPayment, BABE_GENESIS_EPOCH_CONFIG,
    VERSION,
};
use pallet_quantum_pow::RegisteredTopologies;

type QuantumPowNodes = frame_support::BoundedVec<u32, super::configs::QuantumPowMaxNodes>;
type QuantumPowEdges = frame_support::BoundedVec<(u32, u32), super::configs::QuantumPowMaxEdges>;
type QuantumPowAllowedValues = frame_support::BoundedVec<
    quantum_validation::MilliValue,
    super::configs::QuantumPowMaxAllowedValues,
>;
type QuantumMempoolNodes = frame_support::BoundedVec<u32, super::configs::QuantumMaxNodes>;
type QuantumMempoolEdges = frame_support::BoundedVec<(u32, u32), super::configs::QuantumMaxEdges>;
type QuantumMempoolFields = frame_support::BoundedVec<i32, super::configs::QuantumMaxNodes>;
type QuantumMempoolCouplings = frame_support::BoundedVec<i32, super::configs::QuantumMaxEdges>;
type QuantumMempoolMinerAccounts =
    frame_support::BoundedVec<AccountId, super::configs::QuantumMaxBidMiners>;
type QuantumMempoolMinerTypes = frame_support::BoundedVec<
    pallet_quantum_compute_mempool::types::MinerType,
    frame_support::traits::ConstU32<8>,
>;

pallet_revive::impl_runtime_apis_plus_revive_traits!(
    Runtime,
    Revive,
    Executive,
    super::EthExtraImpl,

    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion {
            VERSION
        }

        fn execute_block(block: <Block as BlockT>::LazyBlock) {
            Executive::execute_block(block);
        }

        fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
            Executive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }

        fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
            Runtime::metadata_at_version(version)
        }

        fn metadata_versions() -> Vec<u32> {
            Runtime::metadata_versions()
        }
    }

    impl frame_support::view_functions::runtime_api::RuntimeViewFunction<Block> for Runtime {
        fn execute_view_function(id: frame_support::view_functions::ViewFunctionId, input: Vec<u8>) -> Result<Vec<u8>, frame_support::view_functions::ViewFunctionDispatchError> {
            Runtime::execute_view_function(id, input)
        }
    }

    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
            Executive::apply_extrinsic(extrinsic)
        }

        fn finalize_block() -> <Block as BlockT>::Header {
            Executive::finalize_block()
        }

        fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
            data.create_extrinsics()
        }

        fn check_inherents(
            block: <Block as BlockT>::LazyBlock,
            data: sp_inherents::InherentData,
        ) -> sp_inherents::CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: <Block as BlockT>::Extrinsic,
            block_hash: <Block as BlockT>::Hash,
        ) -> TransactionValidity {
            Executive::validate_transaction(source, tx, block_hash)
        }
    }

    impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
        fn offchain_worker(header: &<Block as BlockT>::Header) {
            Executive::offchain_worker(header)
        }
    }

    impl sp_consensus_babe::BabeApi<Block> for Runtime {
        fn configuration() -> sp_consensus_babe::BabeConfiguration {
            let epoch_config = Babe::epoch_config().unwrap_or(BABE_GENESIS_EPOCH_CONFIG);

            sp_consensus_babe::BabeConfiguration {
                slot_duration: Babe::slot_duration(),
                epoch_length: super::configs::EpochDuration::get(),
                c: epoch_config.c,
                authorities: Babe::authorities().to_vec(),
                randomness: Babe::randomness(),
                allowed_slots: epoch_config.allowed_slots,
            }
        }

        fn current_epoch_start() -> sp_consensus_babe::Slot {
            Babe::current_epoch_start()
        }

        fn current_epoch() -> sp_consensus_babe::Epoch {
            Babe::current_epoch()
        }

        fn next_epoch() -> sp_consensus_babe::Epoch {
            Babe::next_epoch()
        }

        fn generate_key_ownership_proof(
            _slot: sp_consensus_babe::Slot,
            _authority_id: sp_consensus_babe::AuthorityId,
        ) -> Option<sp_consensus_babe::OpaqueKeyOwnershipProof> {
            None
        }

        fn submit_report_equivocation_unsigned_extrinsic(
            _equivocation_proof: sp_consensus_babe::EquivocationProof<<Block as BlockT>::Header>,
            _key_owner_proof: sp_consensus_babe::OpaqueKeyOwnershipProof,
        ) -> Option<()> {
            None
        }
    }

    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(owner: Vec<u8>, seed: Option<Vec<u8>>) -> OpaqueGeneratedSessionKeys {
            SessionKeys::generate(owner.as_slice(), seed).into()
        }

        fn decode_session_keys(
            encoded: Vec<u8>,
        ) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
            SessionKeys::decode_into_raw_public_keys(&encoded)
        }
    }

    impl sp_consensus_grandpa::GrandpaApi<Block> for Runtime {
        fn grandpa_authorities() -> sp_consensus_grandpa::AuthorityList {
            Grandpa::grandpa_authorities()
        }

        fn current_set_id() -> sp_consensus_grandpa::SetId {
            Grandpa::current_set_id()
        }

        fn submit_report_equivocation_unsigned_extrinsic(
            _equivocation_proof: sp_consensus_grandpa::EquivocationProof<
                <Block as BlockT>::Hash,
                NumberFor<Block>,
            >,
            _key_owner_proof: sp_consensus_grandpa::OpaqueKeyOwnershipProof,
        ) -> Option<()> {
            None
        }

        fn generate_key_ownership_proof(
            _set_id: sp_consensus_grandpa::SetId,
            _authority_id: GrandpaId,
        ) -> Option<sp_consensus_grandpa::OpaqueKeyOwnershipProof> {
            // NOTE: this is the only implementation possible since we've
            // defined our key owner proof type as a bottom type (i.e. a type
            // with no values).
            None
        }
    }

    impl pallet_quantum_pow::QuantumPowApi<Block, BlockNumber, AccountId, Balance, QuantumPowNodes, QuantumPowEdges, QuantumPowAllowedValues> for Runtime {
        fn mining_snapshot(
            topology_hash: Option<sp_core::H256>,
        ) -> Option<pallet_quantum_pow::types::MiningSnapshot<QuantumPowNodes, QuantumPowEdges, QuantumPowAllowedValues>> {
            QuantumPow::mining_snapshot(topology_hash)
        }

        fn topology_meta(
            hash: sp_core::H256,
        ) -> Option<pallet_quantum_pow::types::TopologyMeta<QuantumPowNodes, QuantumPowEdges, QuantumPowAllowedValues, BlockNumber>> {
            RegisteredTopologies::<Runtime>::get(hash)
        }

        fn winning_solution(
            block_number: BlockNumber,
        ) -> Option<pallet_quantum_pow::types::QBlockWithNonce<AccountId, Balance, BlockNumber>> {
            QuantumPow::qblock_with_nonce(block_number)
        }

        fn latest_qblock_id() -> Option<u64> {
            QuantumPow::latest_qblock_id()
        }

        fn qblock_id_by_block(block_number: BlockNumber) -> Option<u64> {
            QuantumPow::qblock_id_by_block(block_number)
        }

        fn qblock_by_id(
            qblock_id: u64,
        ) -> Option<pallet_quantum_pow::types::QBlockWithNonce<AccountId, Balance, BlockNumber>> {
            QuantumPow::qblock_with_nonce_by_id(qblock_id)
        }

        fn qblock_by_block(
            block_number: BlockNumber,
        ) -> Option<pallet_quantum_pow::types::QBlockWithNonce<AccountId, Balance, BlockNumber>> {
            QuantumPow::qblock_with_nonce(block_number)
        }

        fn current_difficulty() -> pallet_quantum_pow::types::DifficultyConfig {
            QuantumPow::default_topology()
                .map(|h| QuantumPow::current_difficulty_for(h, System::block_number()))
                .unwrap_or_default()
        }

        fn current_hardness() -> pallet_quantum_pow::types::DifficultyConfig {
            QuantumPow::default_topology()
                .map(|h| QuantumPow::current_difficulty_for(h, System::block_number()))
                .unwrap_or_default()
        }

        fn difficulty_for(
            topology_hash: sp_core::H256,
        ) -> Option<pallet_quantum_pow::types::DifficultyConfig> {
            QuantumPow::difficulty_for_api(topology_hash)
        }

        fn mineable_topologies() -> alloc::vec::Vec<sp_core::H256> {
            QuantumPow::mineable_topologies()
        }
    }

    impl pallet_quantum_compute_mempool::QuantumComputeMempoolApi<
        Block,
        AccountId,
        Balance,
        BlockNumber,
        Hash,
        QuantumMempoolNodes,
        QuantumMempoolEdges,
        QuantumMempoolFields,
        QuantumMempoolCouplings,
        QuantumMempoolMinerAccounts,
        QuantumMempoolMinerTypes,
    > for Runtime {
        fn open_order_ids(start_after: Option<u64>, limit: u32) -> Vec<u64> {
            QuantumComputeMempool::open_order_ids(start_after, limit)
        }

        fn job_order(
            order_id: u64,
        ) -> Option<pallet_quantum_compute_mempool::types::JobOrder<
            AccountId,
            Balance,
            BlockNumber,
            Hash,
            pallet_quantum_compute_mempool::types::IsingParams<
                QuantumMempoolNodes,
                QuantumMempoolEdges,
                QuantumMempoolFields,
                QuantumMempoolCouplings,
            >,
            pallet_quantum_compute_mempool::types::JobMode<
                QuantumMempoolMinerAccounts,
                QuantumMempoolMinerTypes,
            >,
        >> {
            QuantumComputeMempool::job_order(order_id)
        }

        fn order_result(
            order_id: u64,
        ) -> Option<
            pallet_quantum_compute_mempool::types::StoredResult<AccountId, Balance, BlockNumber>,
        > {
            QuantumComputeMempool::result_for_order(order_id)
        }

        fn order_top_solvers(
            order_id: u64,
        ) -> Vec<pallet_quantum_compute_mempool::types::RankedSolver<AccountId>> {
            QuantumComputeMempool::order_top_solvers(order_id)
        }
    }

    impl pallet_miner_registry::MinerRegistryApi<Block, AccountId, BlockNumber> for Runtime {
        fn participants_by_qblock(
            qblock_id: u64,
            start_after: Option<AccountId>,
            limit: u32,
        ) -> Vec<(AccountId, pallet_miner_registry::ParticipationRecord<BlockNumber>)> {
            MinerRegistry::participants_by_qblock(qblock_id, start_after, limit)
        }

        fn participant_count_by_qblock(qblock_id: u64) -> u32 {
            MinerRegistry::participant_count_by_qblock(qblock_id)
        }
    }

    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
        fn account_nonce(account: AccountId) -> Nonce {
            System::account_nonce(account)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
        fn query_info(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_info(uxt, len)
        }
        fn query_fee_details(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_fee_details(uxt, len)
        }
        fn query_weight_to_fee(weight: Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
        for Runtime
    {
        fn query_call_info(
            call: RuntimeCall,
            len: u32,
        ) -> pallet_transaction_payment::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_call_info(call, len)
        }
        fn query_call_fee_details(
            call: RuntimeCall,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_call_fee_details(call, len)
        }
        fn query_weight_to_fee(weight: Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    impl frame_benchmarking::Benchmark<Block> for Runtime {
        fn benchmark_metadata(extra: bool) -> (
            Vec<frame_benchmarking::BenchmarkList>,
            Vec<frame_support::traits::StorageInfo>,
        ) {
            use frame_benchmarking::{baseline, BenchmarkList};
            use frame_support::traits::StorageInfoTrait;
            use frame_system_benchmarking::Pallet as SystemBench;
            use frame_system_benchmarking::extensions::Pallet as SystemExtensionsBench;
            use baseline::Pallet as BaselineBench;
            use super::*;

            let mut list = Vec::<BenchmarkList>::new();
            list_benchmarks!(list, extra);

            let storage_info = AllPalletsWithSystem::storage_info();

            (list, storage_info)
        }

        #[allow(non_local_definitions)]
        fn dispatch_benchmark(
            config: frame_benchmarking::BenchmarkConfig
        ) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, alloc::string::String> {
            use frame_benchmarking::{baseline, BenchmarkBatch};
            use sp_storage::TrackedStorageKey;
            use frame_system_benchmarking::Pallet as SystemBench;
            use frame_system_benchmarking::extensions::Pallet as SystemExtensionsBench;
            use baseline::Pallet as BaselineBench;
            use super::*;

            impl frame_system_benchmarking::Config for Runtime {}
            impl baseline::Config for Runtime {}

            use frame_support::traits::WhitelistedStorageKeys;
            let whitelist: Vec<TrackedStorageKey> = AllPalletsWithSystem::whitelisted_storage_keys();

            let mut batches = Vec::<BenchmarkBatch>::new();
            let params = (&config, &whitelist);
            add_benchmarks!(params, batches);

            Ok(batches)
        }
    }

    #[cfg(feature = "try-runtime")]
    impl frame_try_runtime::TryRuntime<Block> for Runtime {
        fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
            // NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
            // have a backtrace here. If any of the pre/post migration checks fail, we shall stop
            // right here and right now.
            let weight = Executive::try_runtime_upgrade(checks).unwrap();
            (weight, super::configs::RuntimeBlockWeights::get().max_block)
        }

        fn execute_block(
            block: <Block as BlockT>::LazyBlock,
            state_root_check: bool,
            signature_check: bool,
            select: frame_try_runtime::TryStateSelect
        ) -> Weight {
            // NOTE: intentional unwrap: we don't want to propagate the error backwards, and want to
            // have a backtrace here.
            Executive::try_execute_block(block, state_root_check, signature_check, select).expect("execute-block failed")
        }
    }

    impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
        fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
            build_state::<RuntimeGenesisConfig>(config)
        }

        fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
            get_preset::<RuntimeGenesisConfig>(id, crate::genesis_config_presets::get_preset)
        }

        fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
            crate::genesis_config_presets::preset_names()
        }
    }
);

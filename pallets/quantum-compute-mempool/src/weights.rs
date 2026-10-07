//! Public weight composition. Generated measurements live in `benchmark_weights`.
//!
//! Settlement reference timing is still a release gate; see the settlement
//! section in `docs/pallet-benchmarking-plan.md`. The proof envelope below is
//! maintained independently because the pinned SDK takes a maximum across
//! storage prefixes instead of adding their simultaneous proof contributions.

use crate::benchmark_weights::{self, WeightInfo as BenchmarkWeightInfo};
use core::marker::PhantomData;
use frame_support::{
    traits::{Get, StorageInfoTrait},
    weights::Weight,
};

/// Conservative settlement proof floor under the documented SDK trie model.
pub const CLAIM_REWARD_PROOF_FLOOR: u64 = 1024 * 1024;

// SDK model: largest u32 map-cardinality bucket (eight radix-16 layers),
// plus two global trie layers, each charged separately to EVERY touched key.
// This is an estimation convention, not an enforced cardinality cap or a
// theorem about every adversarial trie shape. See the repository docs.
const TRIE_OVERHEAD_PER_KEY: u64 = (8 + 2) * 15 * 33;

fn storage_key_envelope<S: StorageInfoTrait>() -> u64 {
    let info = S::storage_info();
    assert_eq!(info.len(), 1, "expected a single settlement storage prefix");
    u64::from(
        info[0]
            .max_size
            .expect("settlement storage must be bounded"),
    )
    .saturating_add(TRIE_OVERHEAD_PER_KEY)
}

/// Derive the complete envelope from this runtime's encoded types and limits.
/// Used by the pallet integrity hook, never by dispatch or weight evaluation.
pub fn claim_reward_proof_envelope<T: crate::Config>() -> u64 {
    let winners = u64::from(crate::types::MAX_REWARD_WINNERS);
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(storage_key_envelope::<crate::JobSpecs<T>>())
        .saturating_add(
            storage_key_envelope::<crate::OrderTopSolvers<T>>()
                .max(storage_key_envelope::<crate::OrderFrontRunner<T>>()),
        )
        .saturating_add(
            storage_key_envelope::<frame_system::Account<T>>()
                .saturating_mul(winners.saturating_add(1)),
        )
        .saturating_add(storage_key_envelope::<crate::Solvers<T>>().saturating_mul(winners))
        // Include write-only keys conservatively, as well as whitelisted issuance.
        .saturating_add(storage_key_envelope::<crate::OpenOrders<T>>())
        .saturating_add(storage_key_envelope::<pallet_balances::TotalIssuance<T>>())
}

/// Fail runtime integrity validation if the fixed floor or normal call limit
/// no longer covers settlement. Currency/VM changes also require rebenchmarking.
pub fn check_claim_reward_weight<T: crate::Config>() {
    assert_eq!(
        crate::types::MAX_REWARD_WINNERS,
        32,
        "settlement winner limit changed: remeasure DB counts and timing"
    );
    assert!(
        claim_reward_proof_envelope::<T>() <= CLAIM_REWARD_PROOF_FLOOR,
        "settlement proof floor no longer covers configured storage bounds"
    );
    let weight = <T as crate::Config>::WeightInfo::claim_reward();
    assert!(
        weight.proof_size() >= CLAIM_REWARD_PROOF_FLOOR,
        "settlement weight provider bypasses the proof floor"
    );
    let limits = T::BlockWeights::get();
    let normal = limits.get(frame_support::dispatch::DispatchClass::Normal);
    assert!(
        weight.all_lte(normal.max_extrinsic.unwrap_or(limits.max_block)),
        "settlement weight exceeds normal max_extrinsic"
    );
    assert!(
        weight.all_lte(normal.max_total.unwrap_or(limits.max_block)),
        "settlement weight exceeds normal block allowance"
    );
}

fn with_claim_reward_proof_floor(weight: Weight) -> Weight {
    weight.max(Weight::from_parts(0, CLAIM_REWARD_PROOF_FLOOR))
}

pub trait WeightInfo {
    fn register_solver() -> Weight;
    fn deregister_solver() -> Weight;
    fn register_job_spec() -> Weight;
    fn propose_job(n: u32, e: u32, b: u32, t: u32) -> Weight;
    fn submit_solution(n: u32, e: u32, s: u32) -> Weight;
    fn claim_reward() -> Weight;
    fn reclaim_order() -> Weight;
    fn purge_result() -> Weight;
    fn finalise_expired_orders(k: u32) -> Weight;
}

pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
    fn register_solver() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::register_solver()
    }
    fn deregister_solver() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::deregister_solver()
    }
    fn register_job_spec() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::register_job_spec()
    }
    fn propose_job(n: u32, e: u32, b: u32, t: u32) -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::propose_job(n, e, b, t)
    }
    fn submit_solution(n: u32, e: u32, s: u32) -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::submit_solution(n, e, s)
    }
    fn claim_reward() -> Weight {
        with_claim_reward_proof_floor(
            <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::claim_reward(),
        )
    }
    fn reclaim_order() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::reclaim_order()
    }
    fn purge_result() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::purge_result()
    }
    fn finalise_expired_orders(k: u32) -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::finalise_expired_orders(k)
    }
}
impl WeightInfo for () {
    fn register_solver() -> Weight {
        <() as BenchmarkWeightInfo>::register_solver()
    }
    fn deregister_solver() -> Weight {
        <() as BenchmarkWeightInfo>::deregister_solver()
    }
    fn register_job_spec() -> Weight {
        <() as BenchmarkWeightInfo>::register_job_spec()
    }
    fn propose_job(n: u32, e: u32, b: u32, t: u32) -> Weight {
        <() as BenchmarkWeightInfo>::propose_job(n, e, b, t)
    }
    fn submit_solution(n: u32, e: u32, s: u32) -> Weight {
        <() as BenchmarkWeightInfo>::submit_solution(n, e, s)
    }
    fn claim_reward() -> Weight {
        with_claim_reward_proof_floor(<() as BenchmarkWeightInfo>::claim_reward())
    }
    fn reclaim_order() -> Weight {
        <() as BenchmarkWeightInfo>::reclaim_order()
    }
    fn purge_result() -> Weight {
        <() as BenchmarkWeightInfo>::purge_result()
    }
    fn finalise_expired_orders(k: u32) -> Weight {
        <() as BenchmarkWeightInfo>::finalise_expired_orders(k)
    }
}

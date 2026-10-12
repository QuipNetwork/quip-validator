//! Public weight composition. Generated measurements live in `benchmark_weights`.
//!
//! Reference measurements include ordinary/dust settlement and first/replacement
//! submissions; see `docs/pallet-benchmarking-plan.md`. The proof envelope below is
//! maintained independently because the pinned SDK takes a maximum across
//! storage prefixes instead of adding their simultaneous proof contributions.

use crate::benchmark_weights::{self, WeightInfo as BenchmarkWeightInfo};
use codec::MaxEncodedLen;
use core::marker::PhantomData;
use frame_support::{
    traits::{Get, StorageInfoTrait},
    weights::Weight,
};

/// Conservative settlement proof floor under the documented SDK trie model.
pub const CLAIM_REWARD_PROOF_FLOOR: u64 = 1024 * 1024;
pub const RECLAIM_ORDER_PROOF_FLOOR: u64 = 1024 * 1024;
pub const PROPOSE_JOB_PROOF_FLOOR: u64 = 1024 * 1024;
pub const SUBMIT_SOLUTION_PROOF_FLOOR: u64 = 1024 * 1024;

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
        .saturating_add(storage_key_envelope::<crate::ProposerOrders<T>>())
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

/// Refund touches only the order, proposer index/account, open index and issuance.
pub fn reclaim_order_proof_envelope<T: crate::Config>() -> u64 {
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(storage_key_envelope::<crate::ProposerOrders<T>>())
        .saturating_add(storage_key_envelope::<frame_system::Account<T>>())
        .saturating_add(storage_key_envelope::<crate::OpenOrders<T>>())
        .saturating_add(storage_key_envelope::<pallet_balances::TotalIssuance<T>>())
}

/// Conservative union of proposal keys, including the optional requirement.
pub fn propose_job_proof_envelope<T: crate::Config>() -> u64 {
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(storage_key_envelope::<crate::JobSpecs<T>>())
        .saturating_add(storage_key_envelope::<crate::ProposerOrders<T>>())
        .saturating_add(storage_key_envelope::<frame_system::Account<T>>())
        .saturating_add(storage_key_envelope::<crate::NextOrderId<T>>())
        .saturating_add(storage_key_envelope::<crate::OpenOrders<T>>())
        .saturating_add(storage_key_envelope::<crate::OrdersExpiringAt<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderPayeeRequirements<T>>())
        .saturating_add(
            frame_system::pallet_prelude::BlockNumberFor::<T>::max_encoded_len() as u64
                + TRIE_OVERHEAD_PER_KEY,
        )
}

/// Both submission APIs touch these bounded keys; count either ranking and
/// both expiry entries, even though a replacement need not reschedule expiry.
pub fn submit_solution_proof_envelope<T: crate::Config>() -> u64 {
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(storage_key_envelope::<crate::JobSpecs<T>>())
        .saturating_add(storage_key_envelope::<crate::Solvers<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderSolutions<T>>())
        .saturating_add(
            storage_key_envelope::<crate::OrderTopSolvers<T>>()
                .max(storage_key_envelope::<crate::OrderFrontRunner<T>>()),
        )
        .saturating_add(storage_key_envelope::<crate::OrdersExpiringAt<T>>().saturating_mul(2))
        .saturating_add(storage_key_envelope::<crate::OrderPayeeRequirements<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderPayees<T>>())
        .saturating_add(
            frame_system::pallet_prelude::BlockNumberFor::<T>::max_encoded_len() as u64
                + TRIE_OVERHEAD_PER_KEY,
        )
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
    assert!(
        reclaim_order_proof_envelope::<T>() <= RECLAIM_ORDER_PROOF_FLOOR,
        "refund proof floor no longer covers configured storage bounds"
    );
    assert!(
        submit_solution_proof_envelope::<T>() <= SUBMIT_SOLUTION_PROOF_FLOOR,
        "submission proof floor no longer covers storage bounds"
    );
    assert!(
        propose_job_proof_envelope::<T>() <= PROPOSE_JOB_PROOF_FLOOR,
        "proposal proof floor no longer covers storage bounds"
    );
    let limits = T::BlockWeights::get();
    let normal = limits.get(frame_support::dispatch::DispatchClass::Normal);
    for (weight, floor) in [
        (
            <T as crate::Config>::WeightInfo::propose_job(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxBidMiners::get(),
                crate::MinerTypesOf::bound() as u32,
            ),
            PROPOSE_JOB_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::propose_job_with_payee_requirement(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxBidMiners::get(),
                crate::MinerTypesOf::bound() as u32,
            ),
            PROPOSE_JOB_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::claim_reward(),
            CLAIM_REWARD_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::reclaim_order(),
            RECLAIM_ORDER_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::submit_solution(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxSolutions::get(),
            ),
            SUBMIT_SOLUTION_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::submit_solution_with_payee(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxSolutions::get(),
            ),
            SUBMIT_SOLUTION_PROOF_FLOOR,
        ),
    ] {
        assert!(
            weight.proof_size() >= floor,
            "weight provider bypasses proof floor"
        );
        assert!(
            weight.all_lte(normal.max_extrinsic.unwrap_or(limits.max_block)),
            "call weight exceeds normal max_extrinsic"
        );
        assert!(
            weight.all_lte(normal.max_total.unwrap_or(limits.max_block)),
            "call weight exceeds normal block allowance"
        );
    }
}

fn with_claim_reward_proof_floor(weight: Weight) -> Weight {
    weight.max(Weight::from_parts(0, CLAIM_REWARD_PROOF_FLOOR))
}

fn with_reclaim_order_proof_floor(weight: Weight) -> Weight {
    weight.max(Weight::from_parts(0, RECLAIM_ORDER_PROOF_FLOOR))
}

fn with_propose_job_proof_floor(weight: Weight) -> Weight {
    weight.max(Weight::from_parts(0, PROPOSE_JOB_PROOF_FLOOR))
}

fn with_submit_solution_proof_floor(weight: Weight) -> Weight {
    weight.max(Weight::from_parts(0, SUBMIT_SOLUTION_PROOF_FLOOR))
}

// Each generated value already includes its provider's DB costs. Take the
// componentwise maximum first, then the maintained simultaneous-proof floor.
fn claim_reward_weight<W: BenchmarkWeightInfo>() -> Weight {
    with_claim_reward_proof_floor(W::claim_reward().max(W::claim_reward_dust()))
}

fn submit_solution_weight<W: BenchmarkWeightInfo>(n: u32, e: u32, s: u32) -> Weight {
    with_submit_solution_proof_floor(
        W::submit_solution(n, e, s)
            .max(W::submit_solution_with_payee(n, e, s))
            .max(W::submit_solution_replacement(n, e, s))
            .max(W::submit_solution_clear_payee(n, e, s)),
    )
}

pub trait WeightInfo {
    fn propose_job_with_payee_requirement(n: u32, e: u32, b: u32, t: u32) -> Weight;
    fn submit_solution_with_payee(n: u32, e: u32, s: u32) -> Weight;
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
    fn propose_job_with_payee_requirement(n: u32, e: u32, b: u32, t: u32) -> Weight {
        with_propose_job_proof_floor(<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::propose_job_with_payee_requirement(n, e, b, t))
    }
    fn submit_solution_with_payee(n: u32, e: u32, s: u32) -> Weight {
        submit_solution_weight::<benchmark_weights::SubstrateWeight<T>>(n, e, s)
    }

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
        with_propose_job_proof_floor(
            <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::propose_job(n, e, b, t),
        )
    }
    fn submit_solution(n: u32, e: u32, s: u32) -> Weight {
        submit_solution_weight::<benchmark_weights::SubstrateWeight<T>>(n, e, s)
    }
    fn claim_reward() -> Weight {
        claim_reward_weight::<benchmark_weights::SubstrateWeight<T>>()
    }
    fn reclaim_order() -> Weight {
        with_reclaim_order_proof_floor(
            <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::reclaim_order(),
        )
    }
    fn purge_result() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::purge_result()
    }
    fn finalise_expired_orders(k: u32) -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::finalise_expired_orders(k)
    }
}
impl WeightInfo for () {
    fn propose_job_with_payee_requirement(n: u32, e: u32, b: u32, t: u32) -> Weight {
        with_propose_job_proof_floor(
            <() as BenchmarkWeightInfo>::propose_job_with_payee_requirement(n, e, b, t),
        )
    }
    fn submit_solution_with_payee(n: u32, e: u32, s: u32) -> Weight {
        submit_solution_weight::<()>(n, e, s)
    }

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
        with_propose_job_proof_floor(<() as BenchmarkWeightInfo>::propose_job(n, e, b, t))
    }
    fn submit_solution(n: u32, e: u32, s: u32) -> Weight {
        submit_solution_weight::<()>(n, e, s)
    }
    fn claim_reward() -> Weight {
        claim_reward_weight::<()>()
    }
    fn reclaim_order() -> Weight {
        with_reclaim_order_proof_floor(<() as BenchmarkWeightInfo>::reclaim_order())
    }
    fn purge_result() -> Weight {
        <() as BenchmarkWeightInfo>::purge_result()
    }
    fn finalise_expired_orders(k: u32) -> Weight {
        <() as BenchmarkWeightInfo>::finalise_expired_orders(k)
    }
}

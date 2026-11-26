//! Public weight composition. Generated measurements live in `benchmark_weights`.
//!
//! Reference measurements include ordinary/dust settlement and first/replacement
//! submissions. The proof envelope below is
//! maintained independently because the pinned SDK takes a maximum across
//! storage prefixes instead of adding their simultaneous proof contributions.

use crate::benchmark_weights::{self, WeightInfo as BenchmarkWeightInfo};
use codec::MaxEncodedLen;
use core::marker::PhantomData;
use frame_support::{
    traits::{Get, StorageInfoTrait},
    weights::{constants::RocksDbWeight, RuntimeDbWeight, Weight},
};

/// Conservative unmeasured allowance for deleting all 32 maximum-size payloads.
/// Keep until the reference runner measures the enlarged expiry/closure paths.
pub const SOLUTION_PRUNING_PROOF_FLOOR: u64 = 4 * 1024 * 1024;
pub fn solution_pruning_weight<T: crate::Config>() -> Weight {
    solution_pruning_weight_with(T::DbWeight::get())
}

/// `solution_pruning_weight` for an explicit database weight.
fn solution_pruning_weight_with(db: RuntimeDbWeight) -> Weight {
    let cap = u64::from(crate::types::MAX_SOLVERS_PER_OPEN_ORDER);
    // OrderSolutions and the OrderAnswers index (W1-012), each at most `cap`
    // entries per order and cleared in the same step, and the order's
    // NextSubmissionSeq counter (W3-001).
    Weight::from_parts(100_000_000, SOLUTION_PRUNING_PROOF_FLOOR)
        .saturating_add(db.reads_writes(2 * cap + 2, 2 * cap + 1))
}

/// Unmeasured registration reserve/refund allowance pending reference weights.
pub fn solver_deposit_weight<T: crate::Config>() -> Weight {
    Weight::from_parts(100_000_000, 16_000).saturating_add(T::DbWeight::get().reads_writes(4, 3))
}

/// Bound all auxiliary order records in addition to solution payload pruning.
pub const CLOSED_ORDER_PRUNING_PROOF_FLOOR: u64 = 2 * 1024 * 1024;
pub fn schedule_order_pruning_weight<T: crate::Config>() -> Weight {
    Weight::from_parts(50_000_000, 10_000).saturating_add(T::DbWeight::get().reads_writes(2, 2))
}

/// Lazy finalisation in `claim_reward`/`reclaim_order` (W3-006, design §B4 v2
/// item 3): one measured `finalise_order` at maximum size (`CallbackWithPoll`,
/// all winners) plus removing the order's `OrdersExpiringAt` key.
pub fn lazy_finalisation_weight<T: crate::Config>() -> Weight {
    <<T as crate::Config>::WeightInfo as WeightInfo>::finalise_expired_orders(1)
        .saturating_add(T::DbWeight::get().writes(1))
        // W3-001: finalisation forfeits the order's outstanding commitments.
        .saturating_add(
            <<T as crate::Config>::WeightInfo as WeightInfo>::forfeit_commitments(
                <T as crate::Config>::MaxCommitmentsPerOrder::get(),
            ),
        )
}

/// Conservative unmeasured cleanup bound, including empty-bucket traversal.
pub fn closed_order_pruning_weight<T: crate::Config>(orders: u32, buckets: u64) -> Weight {
    closed_order_pruning_weight_with(T::DbWeight::get(), orders, buckets)
}

/// `closed_order_pruning_weight` for an explicit database weight.
fn closed_order_pruning_weight_with(db: RuntimeDbWeight, orders: u32, buckets: u64) -> Weight {
    let orders = u64::from(orders);
    Weight::from_parts(50_000_000, 5_000 + 5_000 * buckets)
        .saturating_add(
            Weight::from_parts(100_000_000, CLOSED_ORDER_PRUNING_PROOF_FLOOR)
                .saturating_mul(orders),
        )
        .saturating_add(db.reads_writes(2 + buckets + 35 * orders, 1 + 40 * orders))
        .saturating_add(solution_pruning_weight_with(db).saturating_mul(orders))
}

/// The sweep's provider (A12): the measured `prune_closed_orders(k)`, floored
/// componentwise by the hand bound at the maximum bucket visits. The benchmark
/// varies orders only, with one bucket visit, so the bucket dimension is
/// priced by the hand floor; flooring the provider itself keeps any caller
/// that charges the provider alone covered (`on_initialize` also floors it at
/// the shape actually visited).
fn prune_closed_orders_weight(generated: Weight, db: RuntimeDbWeight, orders: u32) -> Weight {
    generated.max(closed_order_pruning_weight_with(
        db,
        orders,
        u64::from(crate::types::MAX_PRUNED_ORDERS_PER_BLOCK),
    ))
}

fn closed_order_pruning_proof_envelope<T: crate::Config>() -> u64 {
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(
            storage_key_envelope::<crate::OrderPayees<T>>()
                .saturating_mul(u64::from(crate::types::MAX_SOLVERS_PER_OPEN_ORDER)),
        )
        .saturating_add(storage_key_envelope::<crate::OrderPayeeRequirements<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderFrontRunner<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderTopSolvers<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderResults<T>>())
        .saturating_add(storage_key_envelope::<crate::OpenOrders<T>>())
        .saturating_add(storage_key_envelope::<crate::OrdersPrunableAt<T>>())
}

/// Conservative settlement proof floor under the documented SDK trie model.
pub const CLAIM_REWARD_PROOF_FLOOR: u64 = 1024 * 1024;
pub const RECLAIM_ORDER_PROOF_FLOOR: u64 = 1024 * 1024;
pub const PROPOSE_JOB_PROOF_FLOOR: u64 = 1024 * 1024;
pub const SUBMIT_SOLUTION_PROOF_FLOOR: u64 = 1024 * 1024;
/// A reveal is a submission plus the commitment and, on an eviction, the
/// evicted solver's records and a rebuild over the answer index (W3-001):
/// 1.25 MiB.
pub const REVEAL_SOLUTION_PROOF_FLOOR: u64 = 1024 * 1024 + 256 * 1024;

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
        // Winners, the proposer and the order's escrow account (W1-006).
        .saturating_add(
            storage_key_envelope::<frame_system::Account<T>>()
                .saturating_mul(winners.saturating_add(2)),
        )
        .saturating_add(storage_key_envelope::<crate::Solvers<T>>().saturating_mul(winners))
        // Include write-only keys conservatively, as well as whitelisted issuance.
        .saturating_add(storage_key_envelope::<crate::OpenOrders<T>>())
        .saturating_add(storage_key_envelope::<pallet_balances::TotalIssuance<T>>())
}

/// Refund touches only the order, proposer index, the escrow and proposer
/// accounts (W1-006), open index and issuance.
pub fn reclaim_order_proof_envelope<T: crate::Config>() -> u64 {
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(storage_key_envelope::<crate::ProposerOrders<T>>())
        .saturating_add(storage_key_envelope::<frame_system::Account<T>>().saturating_mul(2))
        .saturating_add(storage_key_envelope::<crate::OpenOrders<T>>())
        .saturating_add(storage_key_envelope::<pallet_balances::TotalIssuance<T>>())
}

/// Conservative union of proposal keys, including the optional requirement.
pub fn propose_job_proof_envelope<T: crate::Config>() -> u64 {
    storage_key_envelope::<crate::JobOrders<T>>()
        .saturating_add(storage_key_envelope::<crate::JobSpecs<T>>())
        .saturating_add(storage_key_envelope::<crate::ProposerOrders<T>>())
        // The proposer and the order's escrow account (W1-006).
        .saturating_add(storage_key_envelope::<frame_system::Account<T>>().saturating_mul(2))
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
        // The answer index: the duplicate read, and on improvement the old
        // entry removed and the new one inserted (W1-012).
        .saturating_add(storage_key_envelope::<crate::OrderAnswers<T>>().saturating_mul(2))
        .saturating_add(
            storage_key_envelope::<crate::OrderTopSolvers<T>>()
                .max(storage_key_envelope::<crate::OrderFrontRunner<T>>()),
        )
        .saturating_add(storage_key_envelope::<crate::OrdersExpiringAt<T>>().saturating_mul(2))
        .saturating_add(storage_key_envelope::<crate::OrderPayeeRequirements<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderPayees<T>>())
        // The order's arrival counter, read and written once (W3-001).
        .saturating_add(storage_key_envelope::<crate::NextSubmissionSeq<T>>())
        .saturating_add(
            frame_system::pallet_prelude::BlockNumberFor::<T>::max_encoded_len() as u64
                + TRIE_OVERHEAD_PER_KEY,
        )
}

/// A reveal (W3-001): a submission's keys, plus the commitment entry and the
/// order's commitment count (consumed), the solver's account (the deposit
/// unreserved) and, on an eviction (B5e), the evicted solver's solution and
/// payee (removed: write-only keys at `max_size`, like the rest of this
/// model) and the ranking rebuild's reads of the rest of the answer index
/// (the submission already counts two of its entries).
pub fn reveal_solution_proof_envelope<T: crate::Config>() -> u64 {
    submit_solution_proof_envelope::<T>()
        .saturating_add(storage_key_envelope::<crate::AnswerCommitments<T>>())
        .saturating_add(storage_key_envelope::<crate::CommitmentCount<T>>())
        .saturating_add(storage_key_envelope::<frame_system::Account<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderSolutions<T>>())
        .saturating_add(storage_key_envelope::<crate::OrderPayees<T>>())
        .saturating_add(
            storage_key_envelope::<crate::OrderAnswers<T>>().saturating_mul(u64::from(
                crate::types::MAX_SOLVERS_PER_OPEN_ORDER.saturating_sub(1),
            )),
        )
}

/// Fail runtime integrity validation if the fixed floor or normal call limit
/// no longer covers settlement. Currency/VM changes also require rebenchmarking.
pub fn check_claim_reward_weight<T: crate::Config>() {
    assert!(
        closed_order_pruning_proof_envelope::<T>() <= CLOSED_ORDER_PRUNING_PROOF_FLOOR,
        "closed order pruning proof bound no longer covers configured records"
    );
    assert!(
        storage_key_envelope::<crate::OrderSolutions<T>>()
            .saturating_add(storage_key_envelope::<crate::OrderAnswers<T>>())
            .saturating_mul(u64::from(crate::types::MAX_SOLVERS_PER_OPEN_ORDER))
            // The arrival counter, cleared with the solutions (W3-001).
            .saturating_add(storage_key_envelope::<crate::NextSubmissionSeq<T>>())
            <= SOLUTION_PRUNING_PROOF_FLOOR,
        "solution pruning proof floor no longer covers the payload cap"
    );
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
        reveal_solution_proof_envelope::<T>() <= REVEAL_SOLUTION_PROOF_FLOOR,
        "reveal proof floor no longer covers storage bounds"
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
            <T as crate::Config>::WeightInfo::claim_reward()
                .saturating_add(solution_pruning_weight::<T>())
                .saturating_add(schedule_order_pruning_weight::<T>()),
            CLAIM_REWARD_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::reclaim_order()
                .saturating_add(solution_pruning_weight::<T>())
                .saturating_add(schedule_order_pruning_weight::<T>()),
            RECLAIM_ORDER_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::submit_solution(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxSolutions::get(),
            )
            .saturating_add(crate::Pallet::<T>::transform_weight(
                T::MaxTransformStepsPerSubmission::get(),
                true,
            )),
            SUBMIT_SOLUTION_PROOF_FLOOR,
        ),
        (
            <T as crate::Config>::WeightInfo::submit_solution_with_payee(
                T::MaxNodes::get(),
                T::MaxEdges::get(),
                T::MaxSolutions::get(),
            )
            .saturating_add(crate::Pallet::<T>::transform_weight(
                T::MaxTransformStepsPerSubmission::get(),
                true,
            )),
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

fn with_reveal_solution_proof_floor(weight: Weight) -> Weight {
    weight.max(Weight::from_parts(0, REVEAL_SOLUTION_PROOF_FLOOR))
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
    fn prune_closed_orders(k: u32) -> Weight;
    fn auto_close_unsolved(k: u32) -> Weight;
    fn commit_answer() -> Weight;
    fn forfeit_commitments(c: u32) -> Weight;
    fn reveal_solution(n: u32, e: u32, s: u32) -> Weight;
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
    fn prune_closed_orders(k: u32) -> Weight {
        prune_closed_orders_weight(
            <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::prune_closed_orders(k),
            T::DbWeight::get(),
            k,
        )
    }
    fn auto_close_unsolved(k: u32) -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::auto_close_unsolved(k)
    }
    fn commit_answer() -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::commit_answer()
    }
    fn forfeit_commitments(c: u32) -> Weight {
        <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::forfeit_commitments(c)
    }
    fn reveal_solution(n: u32, e: u32, s: u32) -> Weight {
        with_reveal_solution_proof_floor(
            <benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::reveal_solution(
                n, e, s,
            ),
        )
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
    fn prune_closed_orders(k: u32) -> Weight {
        prune_closed_orders_weight(
            <() as BenchmarkWeightInfo>::prune_closed_orders(k),
            RocksDbWeight::get(),
            k,
        )
    }
    fn auto_close_unsolved(k: u32) -> Weight {
        <() as BenchmarkWeightInfo>::auto_close_unsolved(k)
    }
    fn commit_answer() -> Weight {
        <() as BenchmarkWeightInfo>::commit_answer()
    }
    fn forfeit_commitments(c: u32) -> Weight {
        <() as BenchmarkWeightInfo>::forfeit_commitments(c)
    }
    fn reveal_solution(n: u32, e: u32, s: u32) -> Weight {
        with_reveal_solution_proof_floor(<() as BenchmarkWeightInfo>::reveal_solution(n, e, s))
    }
}

use alloc::vec;
use alloc::vec::Vec;

use crate::types::{FrontRunner, RankedSolver};

/// Update a ranked top-N set with a new candidate.
pub fn update_ranked_solvers<AccountId: Clone + Eq, BlockNumber: Copy + Ord>(
    current: &[RankedSolver<AccountId, BlockNumber>],
    candidate: RankedSolver<AccountId, BlockNumber>,
    limit: usize,
) -> Vec<RankedSolver<AccountId, BlockNumber>> {
    let mut next = current.to_vec();

    if let Some(existing) = next
        .iter_mut()
        .find(|entry| entry.solver == candidate.solver)
    {
        if candidate.energy_milli < existing.energy_milli {
            *existing = candidate;
        }
    } else {
        next.push(candidate);
    }

    // W3-001: energy, then effective time, then arrival (N2).
    next.sort_by_key(|entry| entry.rank_key());
    next.truncate(limit);
    next
}

/// Compute single-best payouts.
pub fn single_best_payouts<AccountId: Clone, BlockNumber>(
    front_runner: &FrontRunner<AccountId, BlockNumber>,
    reward: u128,
) -> Vec<(AccountId, u128, i64)> {
    vec![(
        front_runner.solver.clone(),
        reward,
        front_runner.energy_milli,
    )]
}

/// Compute equal-split payouts, assigning any remainder to the best-ranked solver.
pub fn top_n_equal_payouts<AccountId: Clone, BlockNumber>(
    ranked: &[RankedSolver<AccountId, BlockNumber>],
    reward: u128,
) -> Vec<(AccountId, u128, i64)> {
    if ranked.is_empty() {
        return Vec::new();
    }

    let len = ranked.len() as u128;
    let base = reward / len;
    let remainder = reward % len;

    ranked
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let extra = if index == 0 { remainder } else { 0 };
            (entry.solver.clone(), base + extra, entry.energy_milli)
        })
        .collect()
}

/// Compute rank-weighted payouts.
///
/// Rank 0 is the best-ranked solver. With `k` ranked solvers, rank `r` has
/// weight `k - r`, so the leader has weight `k` and the last solver weight 1.
/// Energies only decide the order; their sign and magnitude never change the
/// split. Any rounding remainder goes to rank 0.
pub fn top_n_weighted_payouts<AccountId: Clone, BlockNumber>(
    ranked: &[RankedSolver<AccountId, BlockNumber>],
    reward: u128,
) -> Vec<(AccountId, u128, i64)> {
    if ranked.is_empty() {
        return Vec::new();
    }

    let count = ranked.len() as u128;
    // Sum of the weights k, k-1, ..., 1.
    let total_weight = count.saturating_mul(count.saturating_add(1)) / 2;

    let mut payouts = Vec::with_capacity(ranked.len());
    let mut distributed = 0_u128;

    for (index, entry) in ranked.iter().enumerate() {
        let weight = count.saturating_sub(index as u128);
        // Split the division so the product never overflows: the quotient
        // part is exact, and the remainder part multiplies a value below
        // `total_weight` by a weight of at most MAX_REWARD_WINNERS.
        let payout = (reward / total_weight)
            .saturating_mul(weight)
            .saturating_add((reward % total_weight).saturating_mul(weight) / total_weight);
        distributed = distributed.saturating_add(payout);
        payouts.push((entry.solver.clone(), payout, entry.energy_milli));
    }

    let remainder = reward.saturating_sub(distributed);
    if let Some((_, payout, _)) = payouts.first_mut() {
        *payout = payout.saturating_add(remainder);
    }

    payouts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked(energies: &[i64]) -> Vec<RankedSolver<u64, u64>> {
        energies
            .iter()
            .enumerate()
            .map(|(index, energy)| RankedSolver {
                solver: index as u64 + 1,
                energy_milli: *energy,
                answer: sp_core::H256::repeat_byte(index as u8),
                effective_at: 0,
                seq: index as u32,
            })
            .collect()
    }

    fn entry(
        solver: u64,
        energy_milli: i64,
        effective_at: u64,
        seq: u32,
    ) -> RankedSolver<u64, u64> {
        RankedSolver {
            solver,
            energy_milli,
            answer: sp_core::H256::repeat_byte(solver as u8),
            effective_at,
            seq,
        }
    }

    /// W3-001 B5b: ties in energy go to the earlier effective time, then the
    /// earlier arrival -- whatever order the entries arrive in.
    #[test]
    fn ranking_orders_by_energy_then_effective_time_then_arrival() {
        let current = vec![entry(1, 5, 10, 0), entry(2, 5, 10, 2)];
        let earlier = update_ranked_solvers(&current, entry(3, 5, 3, 3), 3);
        assert_eq!(
            earlier.iter().map(|e| e.solver).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
        let between = update_ranked_solvers(&current, entry(3, 5, 10, 1), 3);
        assert_eq!(
            between.iter().map(|e| e.solver).collect::<Vec<_>>(),
            vec![1, 3, 2]
        );
        let lower = update_ranked_solvers(&current, entry(3, 4, 99, 9), 2);
        assert_eq!(
            lower.iter().map(|e| e.solver).collect::<Vec<_>>(),
            vec![3, 1]
        );
    }

    fn amounts(payouts: &[(u64, u128, i64)]) -> Vec<u128> {
        payouts.iter().map(|payout| payout.1).collect()
    }

    #[test]
    fn weighted_all_negative_pays_leader_most() {
        // aglais order 240: weights 2 and 1 over 3, remainder to rank 0.
        let payouts = top_n_weighted_payouts(&ranked(&[-1_500, -500]), 100);
        assert_eq!(amounts(&payouts), vec![67, 33]);
    }

    #[test]
    fn weighted_mixed_sign_never_pays_worse_rank_more() {
        // aglais order 241 split 25/75 under magnitude weighting.
        let payouts = top_n_weighted_payouts(&ranked(&[-1_000, 3_000]), 100);
        assert_eq!(amounts(&payouts), vec![67, 33]);
    }

    #[test]
    fn weighted_all_positive_follows_rank() {
        // weights 3, 2, 1 over 6.
        let payouts = top_n_weighted_payouts(&ranked(&[100, 200, 300]), 120);
        assert_eq!(amounts(&payouts), vec![60, 40, 20]);
    }

    #[test]
    fn weighted_is_monotone_in_rank_and_sums_to_reward() {
        let payouts = top_n_weighted_payouts(&ranked(&[-5, -5, 0, 7, 9]), 1_000);
        let amounts = amounts(&payouts);
        assert_eq!(amounts, vec![335, 266, 200, 133, 66]);
        assert!(amounts.windows(2).all(|pair| pair[0] >= pair[1]));
        assert_eq!(amounts.iter().sum::<u128>(), 1_000);
    }

    #[test]
    fn weighted_single_solver_takes_all() {
        let payouts = top_n_weighted_payouts(&ranked(&[2_000]), 100);
        assert_eq!(amounts(&payouts), vec![100]);
    }

    #[test]
    fn weighted_empty_ranked_pays_nothing() {
        assert!(top_n_weighted_payouts::<u64, u64>(&[], 100).is_empty());
    }

    #[test]
    fn weighted_zero_energies_follow_rank() {
        let payouts = top_n_weighted_payouts(&ranked(&[0, 0]), 90);
        assert_eq!(amounts(&payouts), vec![60, 30]);
    }

    #[test]
    fn weighted_large_reward_keeps_rank_shares() {
        // u128::MAX = 6 * q + 3, so weights 3, 2, 1 give 3q + 1, 2q + 1, q
        // before the remainder of 1 goes to rank 0.
        let reward = u128::MAX;
        let q = reward / 6;
        let payouts = top_n_weighted_payouts(&ranked(&[-3, -2, -1]), reward);
        let amounts = amounts(&payouts);
        assert_eq!(amounts, vec![3 * q + 2, 2 * q + 1, q]);
        assert_eq!(amounts[0] + amounts[1] + amounts[2], reward);
    }
}

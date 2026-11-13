use alloc::vec;
use alloc::vec::Vec;

use crate::types::{FrontRunner, RankedSolver};

/// Update a ranked top-N set with a new candidate.
pub fn update_ranked_solvers<AccountId: Clone + Eq>(
    current: &[RankedSolver<AccountId>],
    candidate: RankedSolver<AccountId>,
    limit: usize,
) -> Vec<RankedSolver<AccountId>> {
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

    next.sort_by_key(|entry| entry.energy_milli);
    next.truncate(limit);
    next
}

/// Compute single-best payouts.
pub fn single_best_payouts<AccountId: Clone>(
    front_runner: &FrontRunner<AccountId>,
    reward: u128,
) -> Vec<(AccountId, u128, i64)> {
    vec![(
        front_runner.solver.clone(),
        reward,
        front_runner.energy_milli,
    )]
}

/// Compute equal-split payouts, assigning any remainder to the best-ranked solver.
pub fn top_n_equal_payouts<AccountId: Clone>(
    ranked: &[RankedSolver<AccountId>],
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
pub fn top_n_weighted_payouts<AccountId: Clone>(
    ranked: &[RankedSolver<AccountId>],
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

    fn ranked(energies: &[i64]) -> Vec<RankedSolver<u64>> {
        energies
            .iter()
            .enumerate()
            .map(|(index, energy)| RankedSolver {
                solver: index as u64 + 1,
                energy_milli: *energy,
            })
            .collect()
    }

    fn amounts(payouts: &[(u64, u128, i64)]) -> Vec<u128> {
        payouts.iter().map(|payout| payout.1).collect()
    }

    #[test]
    fn weighted_payouts_follow_rank_and_split_the_whole_reward() {
        let large = u128::MAX;
        let q = large / 6;
        // (name, energies by rank, reward, expected payouts)
        let cases: [(&str, &[i64], u128, Vec<u128>); 8] = [
            // aglais order 240: weights 2 and 1 over 3, remainder to rank 0.
            (
                "all negative pays the leader most",
                &[-1_500, -500],
                100,
                vec![67, 33],
            ),
            // aglais order 241 split 25/75 under magnitude weighting.
            (
                "mixed sign never pays a worse rank more",
                &[-1_000, 3_000],
                100,
                vec![67, 33],
            ),
            // weights 3, 2, 1 over 6.
            (
                "all positive follows rank",
                &[100, 200, 300],
                120,
                vec![60, 40, 20],
            ),
            (
                "ties and mixed signs",
                &[-5, -5, 0, 7, 9],
                1_000,
                vec![335, 266, 200, 133, 66],
            ),
            ("single solver takes all", &[2_000], 100, vec![100]),
            ("empty ranking pays nothing", &[], 100, vec![]),
            ("zero energies follow rank", &[0, 0], 90, vec![60, 30]),
            // u128::MAX = 6 * q + 3, so weights 3, 2, 1 give 3q + 1, 2q + 1, q
            // before the remainder of 1 goes to rank 0.
            (
                "large reward keeps rank shares",
                &[-3, -2, -1],
                large,
                vec![3 * q + 2, 2 * q + 1, q],
            ),
        ];
        for (name, energies, reward, expected) in cases {
            let payouts = amounts(&top_n_weighted_payouts(&ranked(energies), reward));
            assert_eq!(payouts, expected, "{name}");
            // Property: payouts never increase with rank and use the whole reward.
            assert!(
                payouts.windows(2).all(|pair| pair[0] >= pair[1]),
                "{name}: payouts must be monotone in rank"
            );
            if !payouts.is_empty() {
                assert_eq!(payouts.iter().sum::<u128>(), reward, "{name}: sum");
            }
        }
    }
}

//! Mempool-specific, best-anchored diverse selection. PoW selection is unchanged.

use alloc::{vec, vec::Vec};
use quantum_validation::{symmetric_hamming, ValidationError};

/// Inputs have already passed energy validation; `best` is their first minimum.
/// Keep that vector, then greedily maximise minimum distance to the selected set.
/// Equal distances retain input order. At most O(s² * n) spin comparisons.
pub(crate) fn select_from_best(
    solutions: &[Vec<i8>],
    best: usize,
    target: usize,
) -> Result<Vec<usize>, ValidationError> {
    let mut selected = vec![best];
    let mut chosen = vec![false; solutions.len()];
    chosen[best] = true;
    let mut distances = vec![u32::MAX; solutions.len()];
    while selected.len() < target.max(1).min(solutions.len()) {
        let last = *selected.last().expect("best is always selected");
        let mut next = None;
        for candidate in 0..solutions.len() {
            if chosen[candidate] {
                continue;
            }
            distances[candidate] = distances[candidate]
                .min(symmetric_hamming(&solutions[candidate], &solutions[last])?);
            if next.is_none_or(|index| distances[candidate] > distances[index]) {
                next = Some(candidate);
            }
        }
        let next = next.expect("an unselected candidate remains");
        selected.push(next);
        chosen[next] = true;
    }
    Ok(selected)
}

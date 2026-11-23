//! Canonical form of an answer row (W1-012).
//!
//! Two rows are the same answer when they differ only by a global spin flip or
//! in isolated spins -- spins with no field and no non-zero coupling, whose
//! value cannot change the energy. The canonical row makes such rows equal, so
//! a hash of it identifies the answer.

use alloc::vec;
use alloc::vec::Vec;

use crate::errors::ValidationError;
use crate::fixed::MilliValue;
use crate::validation::{ensure_valid_spins, TopologyIndex};

/// The canonical form of answer rows for one topology, with its fields `h`
/// and couplings `j` (`j[k]` belongs to the index's `k`-th edge). The
/// isolation mask is computed once, so canonicalising each further row is
/// O(n) -- the form W1-004 needs to dedupe a whole proof.
///
/// 1. a spin is *isolated* when its field is 0 and every coupling it takes part
///    in is 0;
/// 2. if the first non-isolated spin is negative, the whole row is negated
///    (flip normalisation, owner O-B3);
/// 3. every isolated spin is set to `+1`.
///
/// Flip-equivalent rows and rows differing only in isolated spins therefore
/// have the same canonical form.
///
/// Isolation is syntactic: a spin is coupled when any edge it is listed on
/// carries a non-zero coupling. A spin with no field whose only couplings are
/// a self-loop (an edge `(u, u)`, whose energy term is a constant) or
/// duplicate edges on one pair that cancel cannot change the energy, yet this
/// mask counts it as non-isolated, so rows differing in it are different
/// answers here. The topology is not rejected for either shape: accepted by
/// the owner on 2026-11-23 (dec_20261123_074855_b6618ed7), together with
/// diversity being measured on raw rows (see `calculate_diversity`).
pub struct AnswerCanonicalizer {
    isolated: Vec<bool>,
    /// The first non-isolated position, whose sign decides the flip.
    first_coupled: Option<usize>,
}

impl AnswerCanonicalizer {
    /// Build the isolation mask for `topology`, `h` and `j`.
    pub fn new(
        h: &[MilliValue],
        j: &[MilliValue],
        topology: &TopologyIndex,
    ) -> Result<Self, ValidationError> {
        let n = topology.len();
        if h.len() != n {
            return Err(ValidationError::FieldLengthMismatch {
                expected: n,
                actual: h.len(),
            });
        }
        if j.len() != topology.edge_count() {
            return Err(ValidationError::EdgeWeightLengthMismatch {
                edges: topology.edge_count(),
                weights: j.len(),
            });
        }
        let mut coupled = vec![false; n];
        for (&(u, v), &coupling) in topology.edge_positions().iter().zip(j.iter()) {
            if coupling != 0 {
                coupled[u] = true;
                coupled[v] = true;
            }
        }
        let isolated: Vec<bool> = (0..n)
            .map(|position| h[position] == 0 && !coupled[position])
            .collect();
        let first_coupled = isolated.iter().position(|&is_isolated| !is_isolated);
        Ok(Self {
            isolated,
            first_coupled,
        })
    }

    /// The canonical form of `row`.
    pub fn canonical(&self, row: &[i8]) -> Result<Vec<i8>, ValidationError> {
        if row.len() != self.isolated.len() {
            return Err(ValidationError::SolutionLengthMismatch {
                expected: self.isolated.len(),
                actual: row.len(),
            });
        }
        ensure_valid_spins(row)?;
        let negate = self.first_coupled.is_some_and(|position| row[position] < 0);
        Ok(row
            .iter()
            .zip(self.isolated.iter())
            .map(|(&spin, &is_isolated)| {
                if is_isolated {
                    1
                } else if negate {
                    -spin
                } else {
                    spin
                }
            })
            .collect())
    }
}

/// The canonical form of one `row`; see [`AnswerCanonicalizer`]. Checks the
/// row length first, then `h` and `j`, then the spins.
pub fn canonical_answer_row(
    row: &[i8],
    h: &[MilliValue],
    j: &[MilliValue],
    topology: &TopologyIndex,
) -> Result<Vec<i8>, ValidationError> {
    let n = topology.len();
    if row.len() != n {
        return Err(ValidationError::SolutionLengthMismatch {
            expected: n,
            actual: row.len(),
        });
    }
    AnswerCanonicalizer::new(h, j, topology)?.canonical(row)
}

#[cfg(test)]
mod tests {
    use super::canonical_answer_row;
    use crate::validation::TopologyIndex;

    // Nodes 10, 20, 30, 40: 10-20 coupled, 30 has a field only, 40 isolated.
    fn topology() -> (TopologyIndex, [i32; 4], [i32; 2]) {
        let index = TopologyIndex::new(&[10, 20, 30, 40], &[(10, 20), (30, 40)]).unwrap();
        (index, [0, 0, 500, 0], [-1_000, 0])
    }

    #[test]
    fn flips_and_isolated_spins_share_one_canonical_row() {
        let (index, h, j) = topology();
        let a = canonical_answer_row(&[1, -1, 1, 1], &h, &j, &index).unwrap();
        // Global flip.
        assert_eq!(
            canonical_answer_row(&[-1, 1, -1, -1], &h, &j, &index).unwrap(),
            a
        );
        // Isolated spin (node 40) differs.
        assert_eq!(
            canonical_answer_row(&[1, -1, 1, -1], &h, &j, &index).unwrap(),
            a
        );
        // Flip plus isolated difference.
        assert_eq!(
            canonical_answer_row(&[-1, 1, -1, 1], &h, &j, &index).unwrap(),
            a
        );
        assert_eq!(a, [1, -1, 1, 1]);
    }

    #[test]
    fn distinct_answers_stay_distinct() {
        let (index, h, j) = topology();
        let a = canonical_answer_row(&[1, -1, 1, 1], &h, &j, &index).unwrap();
        // Node 30 has a field, so it is not isolated: changing it is a new answer.
        assert_ne!(
            canonical_answer_row(&[1, -1, -1, 1], &h, &j, &index).unwrap(),
            a
        );
        // A coupled spin changing is a new answer.
        assert_ne!(
            canonical_answer_row(&[1, 1, 1, 1], &h, &j, &index).unwrap(),
            a
        );
    }

    #[test]
    fn the_flip_is_decided_by_the_first_non_isolated_spin() {
        // Node 10 (position 0) is isolated: no field, no non-zero coupling.
        // The flip must follow position 1, the first non-isolated spin, never
        // raw position 0 -- whose value says nothing about the answer.
        let index = TopologyIndex::new(&[10, 20, 30], &[(10, 20), (20, 30)]).unwrap();
        let (h, j) = ([0, 0, 0], [0, -1_000]);
        // The coupled part flipped, the isolated spin not: the same answer.
        let a = canonical_answer_row(&[1, -1, 1], &h, &j, &index).unwrap();
        let b = canonical_answer_row(&[1, 1, -1], &h, &j, &index).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, [1, 1, -1]);
        // And with the isolated spin negative as well.
        assert_eq!(
            canonical_answer_row(&[-1, -1, 1], &h, &j, &index).unwrap(),
            a
        );
    }

    #[test]
    fn rejects_malformed_rows() {
        let (index, h, j) = topology();
        assert!(canonical_answer_row(&[1, 1, 1], &h, &j, &index).is_err());
        assert!(canonical_answer_row(&[1, 1, 1, 2], &h, &j, &index).is_err());
        assert!(canonical_answer_row(&[1, 1, 1, 1], &h[..3], &j, &index).is_err());
    }
}

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

/// The canonical form of `row` for the topology `topology` with fields `h`
/// and couplings `j` (`j[k]` belongs to the index's `k`-th edge):
///
/// 1. a spin is *isolated* when its field is 0 and every coupling it takes part
///    in is 0;
/// 2. if the first non-isolated spin is negative, the whole row is negated
///    (flip normalisation, owner O-B3);
/// 3. every isolated spin is set to `+1`.
///
/// Flip-equivalent rows and rows differing only in isolated spins therefore
/// have the same canonical form.
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
    ensure_valid_spins(row)?;

    let mut coupled = vec![false; n];
    for (&(u, v), &coupling) in topology.edge_positions().iter().zip(j.iter()) {
        if coupling != 0 {
            coupled[u] = true;
            coupled[v] = true;
        }
    }
    let isolated = |position: usize| h[position] == 0 && !coupled[position];
    let negate = (0..n)
        .find(|&position| !isolated(position))
        .is_some_and(|position| row[position] < 0);
    Ok(row
        .iter()
        .enumerate()
        .map(|(position, &spin)| {
            if isolated(position) {
                1
            } else if negate {
                -spin
            } else {
                spin
            }
        })
        .collect())
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
    fn rejects_malformed_rows() {
        let (index, h, j) = topology();
        assert!(canonical_answer_row(&[1, 1, 1], &h, &j, &index).is_err());
        assert!(canonical_answer_row(&[1, 1, 1, 2], &h, &j, &index).is_err());
        assert!(canonical_answer_row(&[1, 1, 1, 1], &h[..3], &j, &index).is_err());
    }
}

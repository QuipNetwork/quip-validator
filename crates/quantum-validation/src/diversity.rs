//! Diversity and symmetric Hamming distance helpers for Ising solution sets.

use alloc::collections::btree_map::{BTreeMap, Entry};
use alloc::vec;
use alloc::vec::Vec;

use crate::errors::ValidationError;
use crate::fixed::{round_div_u64, MilliDiversity, MilliEnergy, MILLI_SCALE};
use crate::validation::{ensure_valid_spins, validate_solution_set};

/// Compute symmetric Hamming distance between two spin configurations.
///
/// The distance is symmetric under global spin flip, so `a` and `-a` have
/// distance `0`. This matches the Python reference behavior and the Ising
/// model's sign symmetry.
pub fn symmetric_hamming(a: &[i8], b: &[i8]) -> Result<u32, ValidationError> {
    if a.len() != b.len() {
        return Err(ValidationError::SolutionLengthMismatch {
            expected: a.len(),
            actual: b.len(),
        });
    }

    ensure_valid_spins(a)?;
    ensure_valid_spins(b)?;

    Ok(symmetric_hamming_validated(a, b))
}

fn symmetric_hamming_validated(a: &[i8], b: &[i8]) -> u32 {
    let mut direct = 0_u32;
    let mut inverted = 0_u32;

    for (&lhs, &rhs) in a.iter().zip(b.iter()) {
        if lhs != rhs {
            direct += 1;
        }
        if lhs != -rhs {
            inverted += 1;
        }
    }

    direct.min(inverted)
}

/// Indices of one representative per class of copies (W1-004).
///
/// Two rows are copies when they are identical or global spin flips of each
/// other -- symmetric Hamming distance `0` -- regardless of their energies
/// (owner decision O-B3: with `h != 0` a flipped row is often still
/// energy-valid, so counting it would pad every answer for free). Each class
/// keeps its lowest-energy member; ties go to the first occurrence. The
/// returned indices refer to `rows` and are in increasing order.
///
/// `energies` must be parallel to `rows`; a length mismatch is a caller bug
/// and is reported as [`ValidationError::FieldLengthMismatch`].
///
/// Classes are found by flip-normalizing each row (negated when its first spin
/// is negative) and grouping equal keys, which is exactly the relation
/// "symmetric Hamming distance is 0", in O(S·n·log S) for S rows of n spins.
pub fn distinct_solutions<T: AsRef<[i8]>>(
    rows: &[T],
    energies: &[MilliEnergy],
) -> Result<Vec<usize>, ValidationError> {
    if rows.len() != energies.len() {
        return Err(ValidationError::FieldLengthMismatch {
            expected: rows.len(),
            actual: energies.len(),
        });
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    ensure_valid_solution_inputs(rows)?;

    let mut kept: BTreeMap<Vec<i8>, usize> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let row = row.as_ref();
        let key: Vec<i8> = if row.first().is_some_and(|&spin| spin < 0) {
            row.iter().map(|&spin| -spin).collect()
        } else {
            row.to_vec()
        };
        match kept.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(index);
            }
            Entry::Occupied(mut slot) => {
                if energies[index] < energies[*slot.get()] {
                    slot.insert(index);
                }
            }
        }
    }
    let mut indices: Vec<usize> = kept.into_values().collect();
    indices.sort_unstable();
    Ok(indices)
}

/// Compute average pairwise diversity in milli precision.
///
/// The result is the average normalized symmetric Hamming distance between all
/// pairs of solutions, scaled by [`MILLI_SCALE`].
///
/// Examples:
///
/// - `0` means no diversity
/// - `333` means `0.333`
/// - `1000` means maximal diversity
pub fn calculate_diversity<T: AsRef<[i8]>>(
    solutions: &[T],
) -> Result<MilliDiversity, ValidationError> {
    if solutions.len() < 2 {
        return Ok(0);
    }

    let expected_len = ensure_valid_solution_inputs(solutions)?;

    let mut total_distance = 0_u64;
    let mut pair_count = 0_u64;

    for i in 0..solutions.len() {
        for j in (i + 1)..solutions.len() {
            total_distance += u64::from(symmetric_hamming_validated(
                solutions[i].as_ref(),
                solutions[j].as_ref(),
            ));
            pair_count += 1;
        }
    }

    let numerator = total_distance
        .checked_mul(MILLI_SCALE as u64)
        .ok_or(ValidationError::ArithmeticOverflow)?;
    let denominator = pair_count
        .checked_mul(expected_len as u64)
        .ok_or(ValidationError::ArithmeticOverflow)?;

    Ok(round_div_u64(numerator, denominator) as MilliDiversity)
}

/// Select a subset of solutions that maximizes pairwise separation.
///
/// The current implementation uses a simple farthest-point strategy:
///
/// 1. start from the most distant pair
/// 2. repeatedly add the candidate whose minimum distance to the selected set
///    is maximal
///
/// The returned indices refer to positions in the input slice.
pub fn select_diverse<T: AsRef<[i8]>>(
    solutions: &[T],
    target_count: usize,
) -> Result<Vec<usize>, ValidationError> {
    if solutions.is_empty() || target_count == 0 {
        return Ok(Vec::new());
    }

    ensure_valid_solution_inputs(solutions)?;

    if solutions.len() <= target_count {
        return Ok((0..solutions.len()).collect());
    }

    if target_count == 1 {
        return Ok(vec![0]);
    }

    // Compute every O(n)-spin Hamming distance once. The previous greedy
    // implementation recomputed distances inside each selection round,
    // turning the worst case into O(s³·n). Reusing this matrix keeps the
    // dimension model at O(s²·n), with O(s²) scalar selection work.
    let distances = pairwise_distances(solutions);
    let mut selected = most_distant_pair(&distances);
    let mut is_selected = vec![false; solutions.len()];
    is_selected[selected[0]] = true;
    is_selected[selected[1]] = true;

    let mut min_distance = vec![u32::MAX; solutions.len()];
    for candidate in 0..solutions.len() {
        min_distance[candidate] =
            distances[candidate][selected[0]].min(distances[candidate][selected[1]]);
    }

    while selected.len() < target_count {
        let mut best_index = None;
        let mut best_min_distance = 0_u32;

        for candidate in 0..solutions.len() {
            if is_selected[candidate] {
                continue;
            }

            if best_index.is_none() || min_distance[candidate] > best_min_distance {
                best_index = Some(candidate);
                best_min_distance = min_distance[candidate];
            }
        }

        if let Some(index) = best_index {
            selected.push(index);
            is_selected[index] = true;
            for candidate in 0..solutions.len() {
                if !is_selected[candidate] {
                    min_distance[candidate] =
                        min_distance[candidate].min(distances[candidate][index]);
                }
            }
        } else {
            break;
        }
    }

    Ok(selected)
}

fn pairwise_distances<T: AsRef<[i8]>>(solutions: &[T]) -> Vec<Vec<u32>> {
    let mut distances = vec![vec![0; solutions.len()]; solutions.len()];
    for i in 0..solutions.len() {
        for j in (i + 1)..solutions.len() {
            let distance =
                symmetric_hamming_validated(solutions[i].as_ref(), solutions[j].as_ref());
            distances[i][j] = distance;
            distances[j][i] = distance;
        }
    }
    distances
}

fn most_distant_pair(distances: &[Vec<u32>]) -> Vec<usize> {
    let mut best = (0_usize, 1_usize);
    let mut best_distance = distances[0][1];

    for (i, row) in distances.iter().enumerate() {
        for (j, &distance) in row.iter().enumerate().skip(i + 1) {
            if distance > best_distance {
                best = (i, j);
                best_distance = distance;
            }
        }
    }

    vec![best.0, best.1]
}

fn ensure_valid_solution_inputs<T: AsRef<[i8]>>(solutions: &[T]) -> Result<usize, ValidationError> {
    let expected_len = solutions[0].as_ref().len();
    if !validate_solution_set(solutions, expected_len) {
        for solution in solutions {
            let spins = solution.as_ref();
            if spins.len() != expected_len {
                return Err(ValidationError::SolutionLengthMismatch {
                    expected: expected_len,
                    actual: spins.len(),
                });
            }
            ensure_valid_spins(spins)?;
        }
    }

    Ok(expected_len)
}

#[cfg(test)]
mod tests {
    use super::{calculate_diversity, distinct_solutions, select_diverse, symmetric_hamming};
    use crate::errors::ValidationError;
    use alloc::vec::Vec;

    #[test]
    fn distinct_solutions_merges_identical_and_flipped_rows() {
        let a = [1, -1, 1, 1];
        let b = [1, 1, -1, -1];
        let rows = [a, b, a, [-1, 1, -1, -1], b];
        // Equal energies: ties go to the first occurrence.
        assert_eq!(distinct_solutions(&rows, &[5, 5, 5, 5, 5]).unwrap(), [0, 1]);
    }

    #[test]
    fn distinct_solutions_keeps_the_lowest_energy_member_of_a_flip_class() {
        // With h != 0 a row and its flip have different energies; the flip of
        // `a` is the better one here and must be the kept member.
        let a = [1, -1, 1, 1];
        let flipped = [-1, 1, -1, -1];
        let other = [1, 1, 1, 1];
        let kept = distinct_solutions(&[a, other, flipped], &[-10, -5, -30]).unwrap();
        assert_eq!(kept, [1, 2]);
    }

    #[test]
    fn distinct_solutions_semantics_do_not_depend_on_row_order() {
        let rows: [[i8; 3]; 5] = [[1, 1, 1], [-1, -1, -1], [1, -1, 1], [-1, 1, -1], [1, 1, -1]];
        let energies = [-3, -4, -1, -2, -6];
        // The semantic result: each class's kept (row, energy), up to order.
        let semantic = |rows: &[[i8; 3]], energies: &[i64]| {
            let mut kept: Vec<([i8; 3], i64)> = distinct_solutions(rows, energies)
                .unwrap()
                .into_iter()
                .map(|i| (rows[i], energies[i]))
                .collect();
            kept.sort();
            kept
        };
        let forward = semantic(&rows, &energies);
        assert_eq!(forward.len(), 3);
        let mut rev_rows = rows;
        rev_rows.reverse();
        let mut rev_energies = energies;
        rev_energies.reverse();
        assert_eq!(semantic(&rev_rows, &rev_energies), forward);
        let kept_rows: Vec<[i8; 3]> = forward.iter().map(|(row, _)| *row).collect();
        assert_eq!(
            calculate_diversity(&kept_rows).unwrap(),
            calculate_diversity(
                &semantic(&rev_rows, &rev_energies)
                    .iter()
                    .map(|(row, _)| *row)
                    .collect::<Vec<_>>()
            )
            .unwrap()
        );
    }

    #[test]
    fn distinct_solutions_rejects_mismatched_energies() {
        assert_eq!(
            distinct_solutions(&[[1, 1]], &[]),
            Err(ValidationError::FieldLengthMismatch {
                expected: 1,
                actual: 0
            })
        );
        assert_eq!(
            distinct_solutions::<[i8; 2]>(&[], &[]).unwrap(),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn symmetric_hamming_respects_global_spin_flip() {
        let a = [1, -1, 1, -1];
        let b = [-1, 1, -1, 1];

        assert_eq!(symmetric_hamming(&a, &b).unwrap(), 0);
    }

    #[test]
    fn diversity_is_zero_for_identical_solutions() {
        let solutions = [[1, -1], [1, -1]];

        assert_eq!(calculate_diversity(&solutions).unwrap(), 0);
    }

    #[test]
    fn select_diverse_prefers_extremes_first() {
        let solutions = [[1, 1], [1, -1], [-1, 1], [-1, -1]];
        let selected = select_diverse(&solutions, 2).unwrap();

        assert_eq!(selected.len(), 2);
        assert_ne!(selected[0], selected[1]);
    }

    #[test]
    fn select_diverse_honors_single_solution_target() {
        let solutions = [[1, 1], [1, -1], [-1, 1]];

        assert_eq!(select_diverse(&solutions, 1).unwrap(), vec![0]);
    }
}

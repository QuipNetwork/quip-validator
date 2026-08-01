//! Exact Ising energy computation in fixed-point milli precision.

use crate::errors::ValidationError;
use crate::fixed::{MilliEnergy, MilliValue, MILLI_SCALE};
use crate::puzzle_spec::AllowedValueSpec;
use crate::validation::{ensure_valid_spins, TopologyIndex};

/// Empirical SA alignment-efficiency factor for the field term. Calibrated
/// against the v0.1 Python reference; applies only to the h contribution.
const DEFAULT_H_ALPHA: f64 = 0.88;

/// Compute the Ising energy of a spin configuration.
///
/// The function expects:
///
/// - `solution.len() == nodes.len()`
/// - `h.len() == nodes.len()`
/// - `edges.len() == j.len()`
/// - all spin values are in `{-1, +1}`
/// - every edge endpoint is present in `nodes`
///
/// The `nodes` slice defines the variable ordering used to interpret the
/// `solution` slice.
///
/// All values are fixed-point milli precision. For example:
///
/// - `500` means `0.5`
/// - `-1250` means `-1.25`
///
/// Returns a [`ValidationError`] if the structural inputs are malformed or if
/// the fixed-point computation overflows.
pub fn energy_of_solution(
    solution: &[i8],
    h: &[MilliValue],
    edges: &[(u32, u32)],
    j: &[MilliValue],
    nodes: &[u32],
) -> Result<MilliEnergy, ValidationError> {
    validate_shape_lengths(solution, h, edges, j, nodes.len())?;
    let topology = TopologyIndex::new(nodes, edges)?;
    energy_of_solution_indexed(solution, h, edges, j, &topology)
}

/// Compute Ising energy using a topology index validated once by the caller.
///
/// This is equivalent to [`energy_of_solution`], but avoids rebuilding the
/// node-id index when scoring multiple solutions for the same topology.
pub fn energy_of_solution_indexed(
    solution: &[i8],
    h: &[MilliValue],
    edges: &[(u32, u32)],
    j: &[MilliValue],
    topology: &TopologyIndex,
) -> Result<MilliEnergy, ValidationError> {
    validate_shape_lengths(solution, h, edges, j, topology.len())?;
    ensure_valid_spins(solution)?;

    let mut energy = 0_i64;

    for (position, &field) in h.iter().enumerate() {
        energy = energy
            .checked_add(i64::from(field) * i64::from(solution[position]))
            .ok_or(ValidationError::ArithmeticOverflow)?;
    }

    for (&(u, v), &coupling) in edges.iter().zip(j.iter()) {
        let u_pos = topology
            .position(u)
            .ok_or(ValidationError::UnknownNodeInEdge { node: u })?;
        let v_pos = topology
            .position(v)
            .ok_or(ValidationError::UnknownNodeInEdge { node: v })?;
        energy = energy
            .checked_add(
                i64::from(coupling) * i64::from(solution[u_pos]) * i64::from(solution[v_pos]),
            )
            .ok_or(ValidationError::ArithmeticOverflow)?;
    }

    Ok(energy)
}

/// Estimate the expected ground-state energy for the h/J distributions a
/// registered topology actually samples, at a chosen empirical `c`.
///
/// The estimate is `-(c·⟨|J|⟩·√d·n + c·α·⟨|h|⟩·n/√d)` in milli precision,
/// where `d = 2·num_edges/num_nodes` is the average degree and `⟨|h|⟩`,
/// `⟨|J|⟩` are the mean magnitudes (unit scale, 1.0 == 1000 milli) of the
/// field and coupling distributions under each spec's uniform sampling.
///
/// Deriving the magnitudes from the specs keeps the difficulty curve aligned
/// with what puzzles actually sample: a zero-field spec (`Set([0])`) has
/// `⟨|h|⟩ = 0`, so the field term drops out entirely rather than crediting
/// energy no puzzle can produce. With the legacy ternary-h `{-1, 0, +1}`
/// (`⟨|h|⟩ = 2/3`) and binary-J `{-1, +1}` (`⟨|J|⟩ = 1`) specs at `c = 0.75`,
/// this reproduces `shared.energy_utils.expected_solution_energy` from the
/// v0.1 Python reference.
///
/// The `c` factor encodes SA efficiency at a given computational effort —
/// lower values (e.g. 0.7) correspond to less effort and a higher
/// (less-negative) energy, higher values (e.g. 0.8) to more effort and a
/// lower (more-negative) energy.
///
/// Returns `0` for an empty topology, and
/// [`ValidationError::EmptyAllowedValues`] when either spec is empty or has
/// inverted bounds (impossible for a topology that passed registration).
pub fn expected_gse(
    num_nodes: u32,
    num_edges: u32,
    c: f64,
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
) -> Result<MilliEnergy, ValidationError> {
    let h_mean_abs = mean_abs_unit(allowed_h)?;
    let j_mean_abs = mean_abs_unit(allowed_j)?;

    if num_nodes == 0 || num_edges == 0 {
        return Ok(0);
    }

    let n = f64::from(num_nodes);
    let m = f64::from(num_edges);
    let avg_degree = (2.0 * m) / n;
    let sqrt_avg_degree = libm::sqrt(avg_degree);

    let j_contribution = -c * j_mean_abs * sqrt_avg_degree * n;
    let h_contribution = -c * DEFAULT_H_ALPHA * h_mean_abs * n / sqrt_avg_degree;

    Ok(libm::round((j_contribution + h_contribution) * (MILLI_SCALE as f64)) as MilliEnergy)
}

/// Mean |value| of a spec on the unit scale (1.0 == [`MILLI_SCALE`] milli),
/// under the spec's own uniform sampling distribution.
fn mean_abs_unit(spec: &AllowedValueSpec<&[MilliValue]>) -> Result<f64, ValidationError> {
    match *spec {
        AllowedValueSpec::Set(values) => {
            if values.is_empty() {
                return Err(ValidationError::EmptyAllowedValues);
            }
            let mut sum_abs_milli = 0.0_f64;
            for &value in values {
                sum_abs_milli += f64::from(value.unsigned_abs());
            }
            Ok(sum_abs_milli / (values.len() as f64 * MILLI_SCALE as f64))
        }
        // IntegerRange samples whole integers in [min, max] and scales by
        // MILLI_SCALE, so the integers themselves are already unit scale.
        AllowedValueSpec::IntegerRange { min, max } => {
            discrete_mean_abs(i64::from(min), i64::from(max))
        }
        // ContinuousRange samples integer milli steps in [min, max] (see
        // `AllowedValueSpec::sample`), so the discrete mean over milli
        // values divided by MILLI_SCALE is exact, not an approximation.
        AllowedValueSpec::ContinuousRange { min, max } => {
            Ok(discrete_mean_abs(i64::from(min), i64::from(max))? / MILLI_SCALE as f64)
        }
    }
}

/// Mean of |i| over the uniform integer distribution on `[min, max]`.
fn discrete_mean_abs(min: i64, max: i64) -> Result<f64, ValidationError> {
    if max < min {
        return Err(ValidationError::EmptyAllowedValues);
    }
    if min >= 0 {
        return Ok((min + max) as f64 / 2.0);
    }
    if max <= 0 {
        return Ok(-((min + max) as f64) / 2.0);
    }
    // Straddles zero: Σ|i| = T(-min) + T(max) with T(k) = k(k+1)/2. i128
    // keeps the triangular numbers exact for the full i32-derived range.
    let triangle = |k: i128| k * (k + 1) / 2;
    let sum_abs = triangle(i128::from(-min)) + triangle(i128::from(max));
    let span = i128::from(max) - i128::from(min) + 1;
    Ok(sum_abs as f64 / span as f64)
}

fn validate_shape_lengths(
    solution: &[i8],
    h: &[MilliValue],
    edges: &[(u32, u32)],
    j: &[MilliValue],
    node_count: usize,
) -> Result<(), ValidationError> {
    if node_count == 0 {
        return Err(ValidationError::EmptyNodes);
    }

    if solution.len() != node_count {
        return Err(ValidationError::SolutionLengthMismatch {
            expected: node_count,
            actual: solution.len(),
        });
    }

    if h.len() != node_count {
        return Err(ValidationError::FieldLengthMismatch {
            expected: node_count,
            actual: h.len(),
        });
    }

    if edges.len() != j.len() {
        return Err(ValidationError::EdgeWeightLengthMismatch {
            edges: edges.len(),
            weights: j.len(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{energy_of_solution, energy_of_solution_indexed, expected_gse};
    use crate::errors::ValidationError;
    use crate::fixed::MilliValue;
    use crate::puzzle_spec::AllowedValueSpec;
    use crate::validation::TopologyIndex;

    fn set_spec(values: &[MilliValue]) -> AllowedValueSpec<&[MilliValue]> {
        AllowedValueSpec::Set(values)
    }

    #[test]
    fn computes_energy_for_simple_problem() {
        let nodes = [0, 1];
        let edges = [(0, 1)];
        let h = [500, -1_000];
        let j = [250];
        let solution = [1, -1];

        let energy = energy_of_solution(&solution, &h, &edges, &j, &nodes).unwrap();

        assert_eq!(energy, 1_250);
    }

    #[test]
    fn indexed_energy_matches_the_standalone_path() {
        let nodes = [10, 20, 30];
        let edges = [(10, 20), (20, 30), (30, 10)];
        let h = [500, -1_000, 250];
        let j = [250, -500, 750];
        let solution = [1, -1, 1];
        let topology = TopologyIndex::new(&nodes, &edges).unwrap();

        assert_eq!(
            energy_of_solution_indexed(&solution, &h, &edges, &j, &topology).unwrap(),
            energy_of_solution(&solution, &h, &edges, &j, &nodes).unwrap(),
        );
    }

    #[test]
    fn rejects_invalid_spin_values() {
        let nodes = [0, 1];
        let edges = [(0, 1)];
        let h = [0, 0];
        let j = [1_000];
        let solution = [1, 0];

        let error = energy_of_solution(&solution, &h, &edges, &j, &nodes).unwrap_err();

        assert_eq!(
            error,
            ValidationError::InvalidSpinValue { index: 1, value: 0 }
        );
    }

    #[test]
    fn rejects_unknown_node_in_edge() {
        let nodes = [0, 1];
        let edges = [(0, 2)];
        let h = [0, 0];
        let j = [1_000];
        let solution = [1, -1];

        let error = energy_of_solution(&solution, &h, &edges, &j, &nodes).unwrap_err();

        assert_eq!(error, ValidationError::UnknownNodeInEdge { node: 2 });
    }

    #[test]
    fn expected_gse_ternary_field_matches_reference() {
        // Legacy ternary h ∈ {-1, 0, +1} (⟨|h|⟩ = 2/3) + binary J ∈ {-1, +1}
        // (⟨|J|⟩ = 1) at c = 0.75, n=1024, m=2048 (avg degree 4, √4 = 2):
        //   J term = -0.75 · 1     · 2 · 1024       = -1536.0
        //   h term = -0.75 · 0.88 · (2/3) · 1024/2  =  -225.28
        //   total                                   = -1761.28 → -1_761_280 milli
        // This is the value the v0.1 Python reference produces, so the Python
        // parity fixtures stay valid.
        let h = set_spec(&[-1000, 0, 1000]);
        let j = set_spec(&[-1000, 1000]);
        assert_eq!(expected_gse(1024, 2048, 0.75, &h, &j).unwrap(), -1_761_280);
    }

    #[test]
    fn expected_gse_zero_field_drops_h_term() {
        // h = {0} → pure ±J spin glass: E ≈ -c·⟨|J|⟩·√(2m/n)·n, no field
        // term. n=1024, m=2048 → avg degree 4, √4 = 2:
        // -0.75 · 1.0 · 2 · 1024 = -1536.0 → -1_536_000 milli. Exactly the
        // ternary value above minus its -225_280 field term.
        let h = set_spec(&[0]);
        let j = set_spec(&[-1000, 1000]);
        assert_eq!(expected_gse(1024, 2048, 0.75, &h, &j).unwrap(), -1_536_000);
    }

    #[test]
    fn expected_gse_is_more_negative_for_larger_c() {
        let h = set_spec(&[-1000, 0, 1000]);
        let j = set_spec(&[-1000, 1000]);
        let easy = expected_gse(1024, 2048, 0.70, &h, &j).unwrap();
        let hard = expected_gse(1024, 2048, 0.80, &h, &j).unwrap();
        assert!(
            hard < easy,
            "hard (c=0.80) must be more negative than easy (c=0.70): hard={hard}, easy={easy}",
        );
    }

    #[test]
    fn expected_gse_integer_range_matches_equivalent_set() {
        // IntegerRange {-2..=2} samples {-2,-1,0,1,2} scaled by MILLI_SCALE,
        // exactly the same distribution as the explicit milli set.
        let h_range: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: -2, max: 2 };
        let h_set = set_spec(&[-2000, -1000, 0, 1000, 2000]);
        let j = set_spec(&[-1000, 1000]);
        assert_eq!(
            expected_gse(1024, 2048, 0.75, &h_range, &j).unwrap(),
            expected_gse(1024, 2048, 0.75, &h_set, &j).unwrap(),
        );
    }

    #[test]
    fn expected_gse_positive_continuous_range_matches_equivalent_mean() {
        // ContinuousRange [500, 1500] milli is all-positive with mean |v| =
        // 1000 milli — the same mean magnitude as the binary ±1000 set.
        let h_range: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::ContinuousRange {
            min: 500,
            max: 1500,
        };
        let h_set = set_spec(&[-1000, 1000]);
        let j = set_spec(&[-1000, 1000]);
        assert_eq!(
            expected_gse(1024, 2048, 0.75, &h_range, &j).unwrap(),
            expected_gse(1024, 2048, 0.75, &h_set, &j).unwrap(),
        );
    }

    #[test]
    fn expected_gse_rejects_empty_or_inverted_specs() {
        let valid = set_spec(&[-1000, 1000]);
        let empty = set_spec(&[]);
        let inverted: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: 3, max: -3 };
        assert_eq!(
            expected_gse(1024, 2048, 0.75, &empty, &valid).unwrap_err(),
            ValidationError::EmptyAllowedValues,
        );
        assert_eq!(
            expected_gse(1024, 2048, 0.75, &valid, &inverted).unwrap_err(),
            ValidationError::EmptyAllowedValues,
        );
    }

    #[test]
    fn expected_gse_is_zero_for_empty_topology() {
        let h = set_spec(&[0]);
        let j = set_spec(&[-1000, 1000]);
        assert_eq!(expected_gse(0, 100, 0.75, &h, &j).unwrap(), 0);
        assert_eq!(expected_gse(100, 0, 0.75, &h, &j).unwrap(), 0);
    }
}

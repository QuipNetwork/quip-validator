//! Structural validation helpers for spins and solution sets.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::errors::ValidationError;
use crate::fixed::{round_div_u64, MilliDiversity, MilliEnergy, MilliValue, MILLI_SCALE};

/// Return `true` if every value is a valid Ising spin.
///
/// Valid spins are restricted to `-1` and `+1`.
pub fn validate_spins(spins: &[i8]) -> bool {
    spins.iter().all(|&spin| matches!(spin, -1 | 1))
}

/// Return `true` if every solution has the expected length and valid spins.
///
/// This is a cheap boolean helper intended for callers that only need a pass/fail
/// result. Callers that need detailed failure information should use the higher
/// level math APIs that return [`ValidationError`] directly.
pub fn validate_solution_set<T: AsRef<[i8]>>(solutions: &[T], expected_len: usize) -> bool {
    solutions.iter().all(|solution| {
        let spins = solution.as_ref();
        spins.len() == expected_len && validate_spins(spins)
    })
}

pub(crate) fn ensure_valid_spins(spins: &[i8]) -> Result<(), ValidationError> {
    for (index, spin) in spins.iter().enumerate() {
        if !matches!(spin, -1 | 1) {
            return Err(ValidationError::InvalidSpinValue {
                index,
                value: *spin,
            });
        }
    }
    Ok(())
}

/// Validated node-id to solution-position index for an Ising topology.
///
/// Building this once turns repeated edge endpoint lookups from linear scans
/// over `nodes` into logarithmic map lookups. Callers scoring multiple
/// solutions should reuse the same index for the whole solution set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopologyIndex {
    positions: BTreeMap<u32, usize>,
}

impl TopologyIndex {
    /// Validate node uniqueness and edge endpoints, then build the index.
    pub fn new(nodes: &[u32], edges: &[(u32, u32)]) -> Result<Self, ValidationError> {
        let mut positions = BTreeMap::new();
        for (position, &node) in nodes.iter().enumerate() {
            if positions.insert(node, position).is_some() {
                return Err(ValidationError::DuplicateNode { node });
            }
        }

        for &(u, v) in edges {
            if !positions.contains_key(&u) {
                return Err(ValidationError::UnknownNodeInEdge { node: u });
            }
            if !positions.contains_key(&v) {
                return Err(ValidationError::UnknownNodeInEdge { node: v });
            }
        }

        Ok(Self { positions })
    }

    pub(crate) fn len(&self) -> usize {
        self.positions.len()
    }

    pub(crate) fn position(&self, node: u32) -> Option<usize> {
        self.positions.get(&node).copied()
    }
}

/// Report returned by [`validate_solution`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolutionValidation {
    /// Whether the solution and its topology are valid.
    pub valid: bool,
    /// Human-readable validation errors, mirroring the Python reference style.
    pub errors: Vec<String>,
    /// Computed Ising energy in milli precision.
    pub energy_milli: MilliEnergy,
    /// Fraction of satisfied couplings, scaled by [`MILLI_SCALE`].
    pub satisfaction_rate_milli: MilliDiversity,
}

/// Validate that Ising parameters are structurally consistent with the topology.
///
/// This mirrors the Python helper `_validate_topology_consistency`, while using
/// the Rust crate's canonical slice-based representation (`nodes`, `edges`,
/// `h`, `j`).
///
/// When `allowed_h_values` or `allowed_j_values` are provided, all field or
/// coupling values must be members of those sets.
pub fn validate_topology_consistency(
    nodes: &[u32],
    edges: &[(u32, u32)],
    h: &[MilliValue],
    j: &[MilliValue],
    allowed_h_values: Option<&[MilliValue]>,
    allowed_j_values: Option<&[MilliValue]>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut known_nodes = BTreeSet::new();

    for &node in nodes {
        if !known_nodes.insert(node) {
            errors.push(format!("Duplicate node id: {node}"));
        }
    }

    if h.len() != nodes.len() {
        errors.push(format!(
            "Wrong h parameter count: {} != {}",
            h.len(),
            nodes.len()
        ));
    }

    if j.len() != edges.len() {
        errors.push(format!(
            "Wrong J parameter count: {} != {}",
            j.len(),
            edges.len()
        ));
    }

    if let Some(allowed) = allowed_h_values {
        let allowed_display = display_milli_list(allowed);
        for (&node_id, &value) in nodes.iter().zip(h.iter()) {
            if !allowed.contains(&value) {
                errors.push(format!(
                    "Invalid h[{node_id}] = {}, expected one of {}",
                    display_milli_value(value),
                    allowed_display
                ));
            }
        }
    }

    let allowed_j_display = allowed_j_values.map(display_milli_list);
    for (index, &(u, v)) in edges.iter().enumerate() {
        if !known_nodes.contains(&u) || !known_nodes.contains(&v) {
            errors.push(format!("J parameter for invalid edge: ({u}, {v})"));
        }

        if let (Some(allowed), Some(allowed_display)) =
            (allowed_j_values, allowed_j_display.as_ref())
        {
            if let Some(&value) = j.get(index) {
                if !allowed.contains(&value) {
                    let expectation = if is_binary_j_set(allowed) {
                        "±1.0".to_string()
                    } else {
                        format!("one of {allowed_display}")
                    };
                    errors.push(format!(
                        "Invalid J value J[({}, {})] = {} (expected {})",
                        u,
                        v,
                        display_milli_value(value),
                        expectation
                    ));
                }
            }
        }
    }

    errors
}

/// Validate a single Ising solution and compute summary metrics.
///
/// This function mirrors the Python `validate_solution(...)` flow:
///
/// 1. check length
/// 2. check spin alphabet `{-1, +1}`
/// 3. validate topology consistency
/// 4. compute energy
/// 5. compute coupling satisfaction rate
pub fn validate_solution(
    spins: &[i8],
    nodes: &[u32],
    edges: &[(u32, u32)],
    h: &[MilliValue],
    j: &[MilliValue],
    allowed_h_values: Option<&[MilliValue]>,
    allowed_j_values: Option<&[MilliValue]>,
) -> SolutionValidation {
    let mut result = SolutionValidation {
        valid: true,
        errors: Vec::new(),
        energy_milli: 0,
        satisfaction_rate_milli: 0,
    };

    if spins.len() != nodes.len() {
        result.valid = false;
        result.errors.push(format!(
            "Wrong solution length: {} != {}",
            spins.len(),
            nodes.len()
        ));
        return result;
    }

    let invalid_values: Vec<i8> = spins
        .iter()
        .copied()
        .filter(|spin| !matches!(spin, -1 | 1))
        .collect();
    if !invalid_values.is_empty() {
        result.valid = false;
        result.errors.push(format!(
            "Invalid spin values: {} (must be -1 or +1)",
            display_i8_set(&invalid_values)
        ));
        return result;
    }

    let topology_errors =
        validate_topology_consistency(nodes, edges, h, j, allowed_h_values, allowed_j_values);
    if !topology_errors.is_empty() {
        result.valid = false;
        result.errors = topology_errors;
        return result;
    }

    result.energy_milli = crate::energy::energy_of_solution(spins, h, edges, j, nodes)
        .expect("topology validated above");

    let mut satisfied_couplings = 0_u64;
    for (&(u, v), &coupling) in edges.iter().zip(j.iter()) {
        let pos_i = nodes
            .iter()
            .position(|&node| node == u)
            .expect("validated edge endpoint");
        let pos_j = nodes
            .iter()
            .position(|&node| node == v)
            .expect("validated edge endpoint");

        let coupling_energy =
            i64::from(coupling) * i64::from(spins[pos_i]) * i64::from(spins[pos_j]);
        if coupling_energy < 0 {
            satisfied_couplings += 1;
        }
    }

    if !j.is_empty() {
        result.satisfaction_rate_milli =
            round_div_u64(satisfied_couplings * (MILLI_SCALE as u64), j.len() as u64)
                as MilliDiversity;
    }

    result
}

fn display_milli_value(value: MilliValue) -> String {
    let abs = i64::from(value).abs();
    let sign = if value < 0 { "-" } else { "" };
    let whole = abs / 1000;
    let frac = abs % 1000;

    if frac == 0 {
        format!("{sign}{whole}.0")
    } else if frac % 100 == 0 {
        format!("{sign}{whole}.{}", frac / 100)
    } else if frac % 10 == 0 {
        format!("{sign}{whole}.{:02}", frac / 10)
    } else {
        format!("{sign}{whole}.{frac:03}")
    }
}

fn display_milli_list(values: &[MilliValue]) -> String {
    let values = values
        .iter()
        .map(|&value| display_milli_value(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn display_i8_set(values: &[i8]) -> String {
    let mut unique = values.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let values = unique
        .iter()
        .map(i8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{values}}}")
}

fn is_binary_j_set(values: &[MilliValue]) -> bool {
    values.len() == 2 && values.contains(&-1_000) && values.contains(&1_000)
}

#[cfg(test)]
mod tests {
    use super::{validate_solution, validate_topology_consistency, SolutionValidation};

    #[test]
    fn topology_consistency_accepts_valid_problem() {
        let errors = validate_topology_consistency(
            &[0, 1],
            &[(0, 1)],
            &[1_000, 0],
            &[-1_000],
            Some(&[-1_000, 0, 1_000]),
            Some(&[-1_000, 1_000]),
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn validate_solution_reports_valid_metrics() {
        let validation = validate_solution(
            &[1, -1],
            &[0, 1],
            &[(0, 1)],
            &[500, -1_000],
            &[250],
            None,
            None,
        );

        assert_eq!(
            validation,
            SolutionValidation {
                valid: true,
                errors: Vec::new(),
                energy_milli: 1_250,
                satisfaction_rate_milli: 1_000,
            }
        );
    }
}

//! Structural validation helpers for spins and solution sets.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::errors::ValidationError;
use crate::fixed::{round_div_u64, MilliDiversity, MilliEnergy, MilliValue, MILLI_SCALE};
use crate::hardness::NodeIndex;

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

pub(crate) fn ensure_valid_topology<'a>(
    nodes: &'a [u32],
    edges: &[(u32, u32)],
) -> Result<NodeIndex<'a>, ValidationError> {
    // Returned, not dropped: `energy_of_solution` needs the same index for its
    // edge loop and would otherwise build a second over the same slice.
    let node_index = NodeIndex::new(nodes);
    ensure_valid_topology_with_index(&node_index, nodes, edges)?;
    Ok(node_index)
}

/// [`ensure_valid_topology`] over an index the caller already built.
///
/// Checks unchanged — same errors, same ORDER. Only the index construction
/// moves out, so `generate_ising_model` can share one with the frustration pass
/// instead of each building its own over the same `topology.nodes`.
pub(crate) fn ensure_valid_topology_with_index(
    node_index: &NodeIndex<'_>,
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Result<(), ValidationError> {
    // Was `nodes[..position].contains(&node)` per node — O(n^2) — plus
    // `nodes.contains(&u)` per edge endpoint — O(m*n) — on the `submit_proof`
    // consensus path, since `generate_ising_model` calls this on every proof.
    //
    // Both mattered measurably: benchmarking against the real runtime showed the
    // O(m*n) term inflating the per-edge slope, and once the sibling O(n*m) in
    // `energy_of_solution` was removed the per-node slope went UP — the
    // signature of a superlinear term it had been masking.
    //
    // `NodeIndex` is O(1) on the contiguous ids the chain canonicalises to and
    // O(log n) otherwise. Duplicate detection reuses it: an id is a duplicate
    // exactly when its resolved position is not where it actually sits.
    for (position, &node) in nodes.iter().enumerate() {
        if node_index.position(node) != Some(position) {
            return Err(ValidationError::DuplicateNode { node });
        }
    }

    for &(u, v) in edges {
        if node_index.position(u).is_none() {
            return Err(ValidationError::UnknownNodeInEdge { node: u });
        }
        if node_index.position(v).is_none() {
            return Err(ValidationError::UnknownNodeInEdge { node: v });
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
    // Computed once, outside the sink, exactly as before — not per defect.
    // `scan_topology_consistency` never formats anything itself.
    let allowed_h_display = allowed_h_values.map(display_milli_list);
    let allowed_j_display = allowed_j_values.map(display_milli_list);

    let mut errors = Vec::new();
    scan_topology_consistency(
        nodes,
        edges,
        h,
        j,
        allowed_h_values,
        allowed_j_values,
        |d| {
            errors.push(match d {
                Defect::DuplicateNode { node } => format!("Duplicate node id: {node}"),
                Defect::FieldCount { actual, expected } => {
                    format!("Wrong h parameter count: {actual} != {expected}")
                }
                Defect::CouplingCount { actual, expected } => {
                    format!("Wrong J parameter count: {actual} != {expected}")
                }
                Defect::FieldNotAllowed { node_id, value } => format!(
                    "Invalid h[{node_id}] = {}, expected one of {}",
                    display_milli_value(value),
                    allowed_h_display.as_deref().unwrap_or("[]")
                ),
                Defect::UnknownEdgeEndpoint { u, v } => {
                    format!("J parameter for invalid edge: ({u}, {v})")
                }
                Defect::SelfLoop { u, v } => format!("Self-loop at node {u}: edge ({u}, {v})"),
                Defect::CouplingNotAllowed { u, v, value } => {
                    let expectation = match allowed_j_values {
                        Some(allowed) if is_binary_j_set(allowed) => "±1.0".to_string(),
                        _ => format!("one of {}", allowed_j_display.as_deref().unwrap_or("[]")),
                    };
                    format!(
                        "Invalid J value J[({}, {})] = {} (expected {})",
                        u,
                        v,
                        display_milli_value(value),
                        expectation
                    )
                }
            });
            // Never short-circuit: this spelling accumulates every defect.
            true
        },
    );

    errors
}

/// Whether [`validate_topology_consistency`] would find nothing to report,
/// without building the report.
///
/// Exactly `validate_topology_consistency(..).is_empty()`, sharing that
/// function's scan rather than reimplementing it, so the two cannot drift. Both
/// on-chain callers only ever asked the boolean question.
///
/// The saving is on the ADVERSARIAL path, not the happy one: a clean graph
/// formats nothing and never allocates. `propose_job` is signed, flat-weighted
/// and takes user-supplied graphs up to `MaxEdges`, so 5_000 duplicate node ids
/// plus 50_000 unknown-endpoint edges make the string version build 55_000 heap
/// `String`s the caller throws away on its way to one `InvalidTopology`. This
/// stops at the first defect and allocates nothing at all.
pub fn topology_consistency_ok(
    nodes: &[u32],
    edges: &[(u32, u32)],
    h: &[MilliValue],
    j: &[MilliValue],
    allowed_h_values: Option<&[MilliValue]>,
    allowed_j_values: Option<&[MilliValue]>,
) -> bool {
    scan_topology_consistency(
        nodes,
        edges,
        h,
        j,
        allowed_h_values,
        allowed_j_values,
        // The first defect is the answer; stop the scan.
        |_| false,
    )
}

/// A structural defect, carrying only the data a message needs — no formatting.
/// Not `pub`: shared vocabulary between the two spellings, not an API.
enum Defect {
    DuplicateNode { node: u32 },
    FieldCount { actual: usize, expected: usize },
    CouplingCount { actual: usize, expected: usize },
    FieldNotAllowed { node_id: u32, value: MilliValue },
    UnknownEdgeEndpoint { u: u32, v: u32 },
    SelfLoop { u: u32, v: u32 },
    CouplingNotAllowed { u: u32, v: u32, value: MilliValue },
}

/// The single checker behind [`validate_topology_consistency`] and
/// [`topology_consistency_ok`].
///
/// `sink` returns `false` to stop the scan; returns `true` iff no defect was
/// reported. One checker rather than two: this is a consensus surface, where a
/// fast path drifting from the reporting path would mean an accepted graph the
/// diagnostics call malformed.
///
/// ORDER OF EMISSION is the whole contract, so the traversal is unchanged:
/// duplicate nodes, h count, J count, h membership, then one pass over edges
/// emitting unknown-endpoint, self-loop and J membership in that order per edge.
fn scan_topology_consistency(
    nodes: &[u32],
    edges: &[(u32, u32)],
    h: &[MilliValue],
    j: &[MilliValue],
    allowed_h_values: Option<&[MilliValue]>,
    allowed_j_values: Option<&[MilliValue]>,
    mut sink: impl FnMut(Defect) -> bool,
) -> bool {
    // `NodeIndex`, not the O(n^2) `nodes[..position].contains(&node)`. The
    // sibling `ensure_valid_topology` had the identical pair of scans removed;
    // this one kept them, and it is reached from
    // `pallet_quantum_compute_mempool::propose_job` — SIGNED, flat-weighted,
    // user-supplied graphs up to `MaxEdges`. Worst case at n = 5_000 /
    // m = 50_000: ~12.5M (the node scan is n(n-1)/2, not n^2) + ~500M
    // comparisons, for a flat fee.
    //
    // Error text, COUNT and ORDER are all unchanged, because `NodeIndex`
    // resolves the LEFTMOST occurrence, matching the linear scan this replaced.
    // `binary_search_by_key` would return an arbitrary match among equal keys
    // and flip which id gets reported on inputs like `[0, 1, 1, 0]`. See the
    // note on `NodeIndex::position`.
    let node_index = NodeIndex::new(nodes);
    for (position, &node) in nodes.iter().enumerate() {
        if node_index.position(node) != Some(position) && !sink(Defect::DuplicateNode { node }) {
            return false;
        }
    }

    if h.len() != nodes.len()
        && !sink(Defect::FieldCount {
            actual: h.len(),
            expected: nodes.len(),
        })
    {
        return false;
    }

    if j.len() != edges.len()
        && !sink(Defect::CouplingCount {
            actual: j.len(),
            expected: edges.len(),
        })
    {
        return false;
    }

    if let Some(allowed) = allowed_h_values {
        for (&node_id, &value) in nodes.iter().zip(h.iter()) {
            if !allowed.contains(&value) && !sink(Defect::FieldNotAllowed { node_id, value }) {
                return false;
            }
        }
    }

    for (index, &(u, v)) in edges.iter().enumerate() {
        if (node_index.position(u).is_none() || node_index.position(v).is_none())
            && !sink(Defect::UnknownEdgeEndpoint { u, v })
        {
            return false;
        }

        // NOTE: no duplicate-edge scan. `edges[..index].contains(..)` is
        // `O(m²)` — 1.25e9 comparisons at `propose_job`'s 50,000-edge ceiling,
        // for a fixed fee. Parallel edges are harmless anyway: they sum into an
        // effective coupling. `register_topology` sorts its edges, so a cheap
        // adjacent-pair check belongs there if it is ever wanted.
        //
        // A self-loop contributes `J·σ·σ = J` whatever the spin — a constant no
        // configuration can influence — yet `energy_bound_milli` counts it,
        // inflating the anchor and loosening every bar priced against it. It
        // also makes `verify_planar_embedding` refuse unconditionally, so a
        // genuinely planar zero-field topology carrying one could never be
        // retired by the fraud proof.
        if u == v && !sink(Defect::SelfLoop { u, v }) {
            return false;
        }

        if let Some(allowed) = allowed_j_values {
            if let Some(&value) = j.get(index) {
                if !allowed.contains(&value) && !sink(Defect::CouplingNotAllowed { u, v, value }) {
                    return false;
                }
            }
        }
    }

    true
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
    use super::{
        ensure_valid_topology, topology_consistency_ok, validate_solution,
        validate_topology_consistency, SolutionValidation,
    };
    use crate::errors::ValidationError;
    use crate::fixed::MilliValue;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    /// The exact strings, in the exact order, on an input carrying TWO kinds
    /// of defect at once.
    ///
    /// Nothing pinned this before: the other tests are positive, or single-error
    /// fixtures on nodes `[0, 1]` with no duplicate and no unknown endpoint. So
    /// the accumulate-and-order behaviour was untested and the duplicate branch
    /// produced no observed string anywhere — while commit 653340b changed
    /// `NodeIndex` to leftmost-match SPECIFICALLY to keep this order.
    ///
    /// `[5, 9, 5, 9]` takes the `NodeIndex::Table` arm; leftmost-match flags
    /// positions 2 and 3, in index order: `5` then `9`. An arbitrary winner
    /// among the equal keys would flag positions 0 and 1 and the report would
    /// STILL say `5` then `9` — so the `NodeIndex` table test, which compares
    /// flagged POSITIONS against the linear scan, is the other half of this pin.
    #[test]
    fn topology_consistency_reports_duplicates_then_bad_edges_in_order() {
        let errors = validate_topology_consistency(
            &[5, 9, 5, 9],
            &[(5, 9), (5, 42)],
            // Lengths correct on purpose, so no count error interleaves.
            &[0, 0, 0, 0],
            &[0, 0],
            None,
            None,
        );

        assert_eq!(
            errors,
            vec![
                String::from("Duplicate node id: 5"),
                String::from("Duplicate node id: 9"),
                String::from("J parameter for invalid edge: (5, 42)"),
            ]
        );
    }

    /// `ValidationError::DuplicateNode` is matched in the mempool's error
    /// mapper but was never produced by any test.
    #[test]
    fn ensure_valid_topology_rejects_a_duplicate_node() {
        assert_eq!(
            ensure_valid_topology(&[5, 9, 5, 9], &[(5, 9)]).unwrap_err(),
            ValidationError::DuplicateNode { node: 5 },
        );
        // The edge scan runs only after the node scan passes, so a topology with
        // both defects reports the duplicate — the order the mapper sees.
        assert_eq!(
            ensure_valid_topology(&[5, 5], &[(5, 42)]).unwrap_err(),
            ValidationError::DuplicateNode { node: 5 },
        );
        assert_eq!(
            ensure_valid_topology(&[5, 9], &[(5, 42)]).unwrap_err(),
            ValidationError::UnknownNodeInEdge { node: 42 },
        );
        assert!(ensure_valid_topology(&[5, 9], &[(5, 9)]).is_ok());
    }

    /// A self-loop is refused at REGISTRATION, not left for the witness.
    ///
    /// `validate_topology_consistency` has an explicit self-loop branch and
    /// `register_topology` gates on `.is_empty()`. `WitnessError::SelfLoop`
    /// stays reachable only for topologies stored before that branch existed —
    /// `canonical_graph` preserves self-loops and v6 carries them forward — a
    /// stored-state path no new registration can reach, which is why it is
    /// classified as not submitter-fixable.
    #[test]
    fn a_self_loop_is_refused_at_registration_not_left_for_the_planarity_witness() {
        let errors = validate_topology_consistency(&[0, 1], &[(0, 0)], &[0, 0], &[0], None, None);
        assert_eq!(
            errors,
            vec![String::from("Self-loop at node 0: edge (0, 0)")],
            "a self-loop must be a registration-time refusal",
        );

        // The witness path still names it, for topologies already stored.
        assert_eq!(
            crate::hardness::verify_planar_embedding(&[0, 1], &[(0, 0)], &[vec![0u32], Vec::new()]),
            Err(crate::hardness::WitnessError::SelfLoop),
        );
    }

    /// `topology_consistency_ok` must be exactly
    /// `validate_topology_consistency(..).is_empty()`. They share one scan, so
    /// this guards against the sharing being undone, not against drift between
    /// two hand-written checkers.
    #[test]
    fn the_boolean_fast_path_agrees_with_the_reporting_path() {
        let binary: &[MilliValue] = &[-1_000, 1_000];
        let ternary: &[MilliValue] = &[-1_000, 0, 1_000];

        type Case<'a> = (
            &'a [u32],
            &'a [(u32, u32)],
            &'a [MilliValue],
            &'a [MilliValue],
            Option<&'a [MilliValue]>,
            Option<&'a [MilliValue]>,
        );
        let cases: &[Case] = &[
            // Clean.
            (
                &[0, 1],
                &[(0, 1)],
                &[1_000, 0],
                &[-1_000],
                Some(ternary),
                Some(binary),
            ),
            (&[], &[], &[], &[], None, None),
            (
                &[5, 9, 17],
                &[(5, 17)],
                &[0, 0, 0],
                &[1_000],
                None,
                Some(binary),
            ),
            // Duplicates.
            (&[5, 9, 5, 9], &[(5, 9)], &[0, 0, 0, 0], &[0], None, None),
            (&[0, 1, 1, 0], &[], &[0, 0, 0, 0], &[], None, None),
            // Count mismatches.
            (&[0, 1], &[(0, 1)], &[0], &[0], None, None),
            (&[0, 1], &[(0, 1)], &[0, 0], &[], None, None),
            // Membership.
            (
                &[0, 1],
                &[(0, 1)],
                &[7, 0],
                &[-1_000],
                Some(ternary),
                Some(binary),
            ),
            (
                &[0, 1],
                &[(0, 1)],
                &[0, 0],
                &[7],
                Some(ternary),
                Some(binary),
            ),
            (
                &[0, 1],
                &[(0, 1)],
                &[0, 0],
                &[7],
                Some(ternary),
                Some(ternary),
            ),
            // Unknown endpoint and self-loop.
            (&[5, 9], &[(5, 42)], &[0, 0], &[0], None, None),
            (&[0, 1], &[(0, 0)], &[0, 0], &[0], None, None),
            // Everything at once.
            (
                &[5, 5],
                &[(5, 42), (5, 5)],
                &[0],
                &[7],
                Some(binary),
                Some(binary),
            ),
        ];

        for &(nodes, edges, h, j, ah, aj) in cases {
            let errors: Vec<String> = validate_topology_consistency(nodes, edges, h, j, ah, aj);
            assert_eq!(
                topology_consistency_ok(nodes, edges, h, j, ah, aj),
                errors.is_empty(),
                "{nodes:?} / {edges:?}: fast path disagrees with {errors:?}",
            );
        }
    }

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

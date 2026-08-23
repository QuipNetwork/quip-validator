//! Per-instance hardness invariants for the PoUW energy bar.
//!
//! The miner picks its salt, so it picks its instance. The chain prices that
//! rather than preventing it — the energy bar scales with instance hardness.
//! See `pallet_quantum_pow::difficulty::instance_bar_milli`.
//!
//! For a fixed graph and fixed `|J|` an instance is determined by its coupling
//! signs, and every gauge `J_ij ↦ J_ij·s_i·s_j` relabels spins without changing
//! the physics. The cycle frustration pattern is the only invariant of that
//! action, so [`frustration_index_milli`] is the instance's one gauge-invariant
//! degree of freedom, not merely a proxy. Both anchors are exact: at `0` the
//! instance is gauge-equivalent to all-ferromagnetic with ground state exactly
//! `−Σ|J|` ([`energy_bound_milli`]); at the spec's expected frustration the
//! mean-field [`crate::energy::expected_gse`] applies.
//!
//! [`Regime`] is the orthogonal, weight-independent key: a property of the
//! graph alone, so no salt can move it.

/// Put a graph into the canonical form the topology hash names: nodes sorted
/// ascending, every edge oriented `(min, max)`, then the edge list sorted.
///
/// Edge POSITION is load-bearing: `generate_ising_model` maps `j[k]` to
/// `edges[k]`, so a different edge order is a different instance under the same
/// hash. In the plain crate so off-chain tools can reproduce it — a
/// `prove_topology_planar` rotation built against pre-canonicalization edges is
/// rejected with no way to tell why.
///
/// One definition because there were three (`hash_topology`,
/// `register_topology`, the v6 migration) and two disagreed: the hash
/// canonicalized, storage did not, so the hash stopped naming the instance the
/// chain generated.
pub fn canonical_graph(nodes: &[u32], edges: &[(u32, u32)]) -> (Vec<u32>, Vec<(u32, u32)>) {
    let mut nodes = nodes.to_vec();
    nodes.sort_unstable();
    let mut edges: Vec<(u32, u32)> = edges
        .iter()
        .map(|&(u, v)| if u <= v { (u, v) } else { (v, u) })
        .collect();
    edges.sort_unstable();
    (nodes, edges)
}

/// How many sigma below its expectation this draw's frustration landed,
/// unclamped. Positive means less frustrated than typical, i.e. easier.
/// Returns `0` when there is no expectation or no cycle to frustrate.
///
/// Sigma is that of a mean over `cycles` Bernoulli(p) draws with
/// `p = expected/1000`, carried in MICRO (sigma x 1000) rather than milli.
/// Dividing before the square root and then flooring the root loses precision
/// twice: at Z(12,4)'s 41,065 cycles the true sigma is 2.467 milli, the two
/// floors report 2, and the +/-3 sigma band becomes reachable in roughly a
/// fifth of the salt draws it should take. Scaling up before the root keeps
/// sigma to ~0.1%.
///
/// Lives here, not in the pallet that rejects on it: it reads no storage and no
/// `Config`, and a miner must evaluate it BEFORE spending solve effort —
/// `submit_proof` refuses an instance outside the band and the error tells them
/// to re-salt. From the pallet, reproducing that off-chain meant linking the
/// whole of FRAME. Both inputs come from [`frustration_index`].
pub fn frustration_sigmas(
    frustration_milli: u32,
    expected_frustration_milli: u32,
    frustration_cycles: u64,
) -> i64 {
    if expected_frustration_milli == 0 || frustration_cycles == 0 {
        return 0;
    }
    let p = i128::from(expected_frustration_milli.min(1000));
    let cycles = i128::from(frustration_cycles).max(1);
    let variance_micro = (p * (1000 - p) * 1_000_000) / cycles;
    let sigma_micro = (variance_micro.max(0) as u128).isqrt().max(1) as i128;
    let deviation = i128::from(expected_frustration_milli) - i128::from(frustration_milli);
    // Clamped, not wrapped: bounded in practice, guaranteed here.
    (deviation * 1000 / sigma_micro).clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// Cap on how far the handicap may move the bar, in standard deviations.
///
/// Consensus-critical, not a tuning knob: `submit_proof` refuses an instance
/// whose frustration lands outside `+/-` this. It, not the per-sigma slope, is
/// the load-bearing limit on salt-shopping. Here rather than in the pallet for
/// the same reason as [`frustration_sigmas`] — a miner needs the predicate and
/// its bound together to predict a refusal without running a node.
pub const HANDICAP_MAX_SIGMA: i64 = 3;

/// Expected frustration, in milli, for a coupling spec that pins it — or
/// `None` when the spec implies nothing and the operator's declaration stands.
///
/// A sign-symmetric spec makes each fundamental cycle an independent fair coin,
/// so the index is exactly half; a graph with no cycles takes zero. Both
/// anchors are exact, which is why the value is DERIVED and overwrites the
/// declaration rather than merely being checked against it: rejecting a wrong
/// declaration still leaves an operator to get it right.
///
/// Being wrong here is unrecoverable. This is the mean the sigma band is
/// measured against, so an error of one percentage point shifts every draw's
/// z-score by roughly `0.01 * sqrt(cycles) / 0.5`. At a few thousand cycles
/// that is well past the +/-3 sigma gate, and EVERY honest `submit_proof` is
/// refused `InstanceOutsideFrustrationBand` forever.
pub fn expected_frustration_for(
    coupling_spec: &AllowedValueSpec<&[MilliValue]>,
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Option<u32> {
    if !coupling_spec.is_sign_symmetric() {
        return None;
    }
    Some(if cycle_rank(nodes, edges) == 0 {
        0
    } else {
        // Each fundamental cycle is an independent fair coin.
        500
    })
}

/// A "caterpillar of cliques" — TEST/BENCHMARK FIXTURE, gated out of the
/// runtime blob. `n` vertices, each adjacent to the next `window`, so
/// consecutive windows of `window + 1` vertices are mutually adjacent.
///
/// WORST CASE for [`induced_width_at_most`]: eliminating in vertex order fills
/// a full `window`-sized bag at every step, so the early abort never fires —
/// the shape `prove_topology_exact`'s weight is priced against.
///
/// `window` is a parameter, not fixed at `ExactSolveCeiling`: emitting
/// `ceiling` edges per vertex exhausts a bounded edge budget partway and leaves
/// the tail ISOLATED, flattening the per-node slope the sweep exists to fit.
/// The benchmark narrows the window; the characterization runs unbounded.
///
/// Shared with the pallet's `benchmarking.rs`, which CALIBRATES against it:
/// `PROVE_EXACT_K1_NODE` comes from the characterization while the benchmark
/// asserts it saturates every bag. The copies had diverged on the window, so
/// the deployed constant was priced against an unmeasured graph.
#[cfg(any(feature = "std", feature = "runtime-benchmarks"))]
pub fn caterpillar_of_cliques(n: usize, window: usize, max_edges: usize) -> Vec<(u32, u32)> {
    let mut edges = Vec::new();
    for v in 0..n {
        for w in (v + 1)..(v + 1 + window).min(n) {
            if edges.len() >= max_edges {
                return edges;
            }
            edges.push((v as u32, w as u32));
        }
    }
    edges
}

/// Why a tractability witness was refused.
///
/// These are permissionless, fee-paying calls whose entire product is a
/// verdict, so one `None` charged a submitter to learn nothing. The distinction
/// that matters: "your witness is malformed" — retry — versus "your witness is
/// well-formed but its claim is false" — do not retry.
///
/// Per the v6 canonicalization note the likeliest cause of a rejected rotation
/// is one generated against PRE-canonicalization edge order, which
/// [`WitnessError::EdgeNotIncident`] / [`WitnessError::RotationLength`] name
/// outright and an `Option` cannot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WitnessError {
    /// The order/rotation does not have one entry per node.
    OrderLength,
    /// A named node is not in the topology.
    ///
    /// From `induced_width_at_most` this IS the submitter's order and is
    /// fixable. `verify_planar_embedding` reports the stored topology's own bad
    /// edge as [`WitnessError::TopologyEdgeUnknownNode`] instead, so the two
    /// cannot be confused.
    UnknownNode,
    /// The stored TOPOLOGY has an edge naming a node outside its node list.
    ///
    /// A defect in what was registered, not in the witness. Split out from
    /// `UnknownNode`, which told a submitter their correct rotation named a bad
    /// node — so they would retry, pay again, and get the same answer forever.
    TopologyEdgeUnknownNode,
    /// A node is named more than once, so the order is not a permutation.
    DuplicateNode,
    /// An edge index is past the end of the edge list.
    EdgeIndexOutOfRange,
    /// An edge appears in a vertex's rotation but is not incident to it, or
    /// appears twice at the same end.
    EdgeNotIncident,
    /// The stored TOPOLOGY has a self-loop, which has no two-sided traversal.
    ///
    /// Not fixable by the submitter: `canonical_graph` preserves self-loops and
    /// v6 carries them forward, so a topology registered before the self-loop
    /// check existed still has one. Retrying with a different rotation can never
    /// help, which is why it is classified as a false claim rather than
    /// malformed.
    SelfLoop,
    /// A vertex's rotation length does not match its degree.
    RotationLength,
    /// WELL-FORMED, but the claim is false: an elimination bag exceeded the
    /// ceiling. Retrying with a different order may still succeed; retrying
    /// with the SAME order will not.
    WidthAboveCeiling,
    /// WELL-FORMED, but the claim is false: Euler's formula puts the
    /// embedding above genus zero, so it is not planar.
    NotPlanar,
}

impl WitnessError {
    /// Whether re-encoding and resubmitting the witness can possibly help.
    ///
    /// Named for the question a fee-paying submitter asks. The older
    /// `is_malformed` had no honest answer for [`WitnessError::SelfLoop`] and
    /// [`WitnessError::TopologyEdgeUnknownNode`], which are neither a malformed
    /// witness nor a false claim: the STORED topology is the defective thing.
    ///
    /// No in-tree caller — the partition that ships on chain is
    /// `pallet_quantum_pow`'s `witness_error` mapper, exhaustive over the same
    /// variants. The two are asserted independently and can disagree, so the
    /// test below enumerates every variant rather than using `matches!`.
    pub fn is_submitter_fixable(self) -> bool {
        // Exhaustive on purpose: as `!matches!(.., WidthAboveCeiling |
        // NotPlanar)` an eleventh variant would silently default to "fixable"
        // here while the pallet's exhaustive mapper refused to compile.
        match self {
            // Not fixable: the witness is fine, the claim or the stored
            // topology is not.
            Self::WidthAboveCeiling
            | Self::NotPlanar
            | Self::SelfLoop
            | Self::TopologyEdgeUnknownNode => false,
            Self::OrderLength
            | Self::UnknownNode
            | Self::DuplicateNode
            | Self::EdgeIndexOutOfRange
            | Self::EdgeNotIncident
            | Self::RotationLength => true,
        }
    }
}

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use crate::fixed::MilliValue;
use crate::puzzle_spec::AllowedValueSpec;

/// Structural solvability regime of a registered topology. Classified
/// off-chain by the riff toolkit (irreducible core after the full exact
/// reduction stack) and registered by governance; weight-independent, so it
/// is a property of the topology rather than of any sampled instance.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub enum Regime {
    /// Reduces to a core below the exact-solve ceiling: the ground state is
    /// tractable, so clearing a slack bar on it is not useful work.
    Exact,
    /// The irreducible core stays above the ceiling; only heuristic solving
    /// applies.
    Heuristic,
}

/// Number of independent cycles in the graph — `m - n + components`, counted
/// with the same union-find the frustration index uses.
///
/// Zero means a forest, which has no cycle to frustrate and therefore no
/// expected frustration to declare.
pub fn cycle_rank(nodes: &[u32], edges: &[(u32, u32)]) -> u64 {
    let index = NodeIndex::new(nodes);
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn find(parent: &mut [usize], v: usize) -> usize {
        let mut root = v;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = v;
        while parent[cur] != cur {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    let mut cycles = 0u64;
    for &(u, v) in edges {
        let (Some(a), Some(b)) = (index.position(u), index.position(v)) else {
            continue;
        };
        if a == b {
            cycles += 1;
            continue;
        }
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra == rb {
            cycles += 1;
        } else {
            parent[ra] = rb;
        }
    }
    cycles
}

/// The smallest `Σ|h| + Σ|J|` any draw from these specs can produce, in milli
/// — so `-loosest_energy_bound_milli` is the *highest* (easiest) anchor the
/// topology can ever hand an instance.
///
/// Registration uses it to refuse specs that admit an all-zero draw: a bound of
/// `0` means every configuration has energy `0`, so any bar clears for no work.
///
/// Deliberately *not* compared against the energy curve. The related concern —
/// the anchor above `curve.max_milli`, so the `max(curve_bar, anchor)` clamp
/// disconnects the retarget — needs the curve weighed against the spec
/// distribution, since the loosest single draw over-rejects healthy topologies.
pub fn loosest_energy_bound_milli(
    node_count: u64,
    edge_count: u64,
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
) -> u64 {
    node_count
        .saturating_mul(allowed_h.min_abs_milli())
        .saturating_add(edge_count.saturating_mul(allowed_j.min_abs_milli()))
}

/// `Σ|h| + Σ|J|`, in milli: the magnitude of the lowest energy any
/// configuration could reach, since `E = Σ h·σ + Σ J·σ·σ` and no term can
/// contribute more than its own magnitude. Negated, it is the tightest bar the
/// chain can ever demand. Saturates rather than wrapping.
///
/// For the zero-field instances production topologies sample this is `Σ|J|`,
/// and then *exact* rather than a bound: an unfrustrated instance is
/// gauge-equivalent to all-ferromagnetic, so `−Σ|J|` is precisely its ground
/// state. With fields present, field and coupling preferences can conflict, so
/// it stays a lower bound — dropping the field term would put the bar *above*
/// what a field-bearing instance can reach.
pub fn energy_bound_milli(h: &[MilliValue], j: &[MilliValue]) -> u64 {
    h.iter().chain(j.iter()).fold(0u64, |acc, &w| {
        acc.saturating_add(u64::from(w.unsigned_abs()))
    })
}

/// Position lookup for a topology's node ids. `pub(crate)` for
/// `energy_of_solution` and `validate_topology_consistency`, both of which it
/// lifts off a linear scan that made them superlinear on the `submit_proof`
/// consensus path — the measurements are at each call site.
///
/// Registered topologies are conventionally the contiguous range `0..n`, so the
/// identity is tried first and a sorted table is built only for sparse ids; the
/// common case stays allocation-free.
///
/// An `enum`, not a struct of two `bool`s plus a `Vec`: that form had four
/// representable states for three real paths, and the flag/`Vec` coupling was
/// set once in the constructor then re-transcribed as an if/else-if chain in
/// `position`, so the chain's ORDER was load-bearing and the `Vec` could be
/// empty while `position` still searched it. That is how the ascending path
/// shipped broken in a working copy: every lookup on the path production
/// actually takes returned `None`. Three variants make that unrepresentable.
#[derive(Debug)]
pub(crate) enum NodeIndex<'a> {
    /// `nodes[i] == i` for all `i`: the id IS the position, no storage needed.
    Contiguous { len: usize },
    /// Strictly ascending with gaps — `binary_search` on the caller's own
    /// slice, no table built. What `canonical_graph` produces; see `new`.
    Ascending(&'a [u32]),
    /// `(id, position)` sorted on the whole tuple; lookup is leftmost-match.
    /// The fallback for unsorted or duplicate-bearing ids.
    Table(Vec<(u32, u32)>),
}

impl<'a> NodeIndex<'a> {
    pub(crate) fn new(nodes: &'a [u32]) -> Self {
        // Both scans are vacuously true on `[]`, so it lands on
        // `Contiguous { len: 0 }` and resolves nothing; `[7]` lands `Ascending`.
        if nodes.iter().enumerate().all(|(i, &id)| id as usize == i) {
            return Self::Contiguous { len: nodes.len() };
        }
        // Ascending-with-gaps is what actually ships: `canonical_graph` SORTS
        // node ids but does not renumber them to `0..n`, and a hardware working
        // graph (Pegasus/Zephyr, dead qubits removed) has gaps. Without this
        // branch every registered topology took the allocating path — a `Vec`
        // plus an `O(n log n)` sort per construction, several times per proof.
        // Strict `<` also proves distinctness, which the duplicate check in
        // `ensure_valid_topology` relies on.
        if nodes.windows(2).all(|w| w[0] < w[1]) {
            return Self::Ascending(nodes);
        }
        let mut sorted: Vec<(u32, u32)> = nodes
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i as u32))
            .collect();
        sorted.sort_unstable();
        Self::Table(sorted)
    }

    /// How many nodes the index was built over, so the `_with_index` cores do
    /// not need the `nodes` slice alongside. Not named `len`: it is not a
    /// container measurement (and would trip `clippy::len_without_is_empty`).
    pub(crate) fn node_count(&self) -> usize {
        match self {
            Self::Contiguous { len } => *len,
            Self::Ascending(nodes) => nodes.len(),
            Self::Table(sorted) => sorted.len(),
        }
    }

    pub(crate) fn position(&self, id: u32) -> Option<usize> {
        match self {
            Self::Contiguous { len } => ((id as usize) < *len).then_some(id as usize),
            // Already sorted, so its own index IS the position. No table.
            Self::Ascending(nodes) => nodes.binary_search(&id).ok(),
            Self::Table(sorted) => {
                // LEFTMOST match, not `binary_search_by_key`.
                //
                // `slice::binary_search*` documents that with multiple equal
                // keys "any one of the matches could be returned" — unspecified.
                // The linear scan this replaced always resolved the FIRST
                // occurrence, and `ensure_valid_topology` /
                // `validate_topology_consistency` derive duplicate detection
                // from `position(id) != Some(i)`, so an arbitrary winner changes
                // WHICH node id is reported. Verified: on `[0, 1, 1, 0]` the two
                // spellings disagree. No on-chain caller inspects more than
                // `is_empty()` today, but a toolchain bump could flip the
                // reported id with no source change. `sorted` is sorted on the
                // whole `(id, position)` tuple, so the leftmost equal key is the
                // lowest original position: exactly the old semantics.
                let i = sorted.partition_point(|&(key, _)| key < id);
                (i < sorted.len() && sorted[i].0 == id).then(|| sorted[i].1 as usize)
            }
        }
    }
}

/// Verify an elimination order and return the induced width it achieves, or an
/// error if the order is not a permutation of `nodes` or its width exceeds
/// `ceiling`.
///
/// Treewidth is expensive to *compute* — a min-fill search over a
/// hardware-scale graph runs for hours — but an elimination order is a witness
/// that can be *checked* cheaply, and checking yields an upper bound, the
/// direction that certifies tractability. Replaying the order costs
/// `O(n·ceiling²)` because the check aborts the moment a bag exceeds the
/// ceiling: only the small-width verdict is interesting.
///
/// Self-loops and edges with an endpoint outside `nodes` are ignored; neither
/// contributes to a bag.
pub fn induced_width_at_most(
    nodes: &[u32],
    edges: &[(u32, u32)],
    order: &[u32],
    ceiling: u32,
) -> Result<u32, WitnessError> {
    let n = nodes.len();
    if order.len() != n {
        return Err(WitnessError::OrderLength);
    }
    let index = NodeIndex::new(nodes);

    // `rank[position]` is when that node is eliminated. Filling it is also
    // the permutation check: every node must be named exactly once.
    let mut rank = vec![u32::MAX; n];
    for (step, &id) in order.iter().enumerate() {
        let position = index.position(id).ok_or(WitnessError::UnknownNode)?;
        if rank[position] != u32::MAX {
            return Err(WitnessError::DuplicateNode);
        }
        rank[position] = step as u32;
    }
    if n == 0 {
        return Ok(0);
    }

    // Adjacency keyed by elimination rank, so elimination is just `0..n`.
    let mut adjacency: Vec<BTreeSet<u32>> = vec![BTreeSet::new(); n];
    for &(u, v) in edges.iter() {
        let (Some(u_pos), Some(v_pos)) = (index.position(u), index.position(v)) else {
            continue;
        };
        if u_pos == v_pos {
            continue;
        }
        let (u_rank, v_rank) = (rank[u_pos], rank[v_pos]);
        adjacency[u_rank as usize].insert(v_rank);
        adjacency[v_rank as usize].insert(u_rank);
    }

    let mut width = 0u32;
    let mut bag: Vec<u32> = Vec::new();
    for step in 0..n {
        // The bag is the not-yet-eliminated neighbourhood.
        bag.clear();
        bag.extend(
            adjacency[step]
                .iter()
                .copied()
                .filter(|&other| other as usize > step),
        );
        let size = bag.len() as u32;
        if size > ceiling {
            return Err(WitnessError::WidthAboveCeiling);
        }
        if size > width {
            width = size;
        }
        // Eliminating a vertex makes its bag a clique.
        for i in 0..bag.len() {
            for j in (i + 1)..bag.len() {
                let (a, b) = (bag[i], bag[j]);
                adjacency[a as usize].insert(b);
                adjacency[b as usize].insert(a);
            }
        }
        // The eliminated vertex is never revisited.
        adjacency[step].clear();
    }
    Ok(width)
}

/// Gauge-invariant frustration of a realized instance, in milli (`0..=1000`):
/// the fraction of fundamental cycles that cannot be satisfied.
///
/// Each bond has a type `t = −sign(J)` (ferromagnetic `+1` when `J < 0`,
/// antiferromagnetic `−1` when `J > 0` under this crate's `E = Σ J·σ·σ`); a
/// cycle is frustrated iff the product of types around it is negative. Every
/// cycle vertex is touched by two cycle bonds, so that product survives any
/// gauge `J_ij ↦ J_ij·s_i·s_j` — it sees through however the instance was
/// planted, which is the whole point.
///
/// Parity union-find over node positions: `parity[v]` carries the XOR of bond
/// types from `v` to its component root, so a cycle-closing edge is frustrated
/// iff `parity[u] ^ parity[v] ^ bit(t)` is set. `O(m·α(n))` with three flat
/// arrays and no per-edge allocation — the per-proof path is hot, and the
/// earlier map-based formulation cost ~1800× the sign-count pass it replaced.
///
/// `nodes` defines the vertex set (same ordering contract as
/// [`crate::energy::energy_of_solution`]). Endpoints absent from `nodes`,
/// self-loops and zero couplings are skipped; a forest returns `0`. Parallel
/// edges each close their own 2-cycle, so an opposing pair counts as
/// frustrated; registered hardware topologies are simple graphs.
pub fn frustration_index_milli(nodes: &[u32], edges: &[(u32, u32)], j: &[MilliValue]) -> u32 {
    frustration_index(nodes, edges, j).0
}

/// [`frustration_index_milli`] plus the number of fundamental cycles it
/// averaged over.
///
/// The count lets a caller scale a per-instance handicap in units of the
/// index's own standard deviation: the index is a mean over `cycles`
/// roughly-independent Bernoulli draws, so `σ ≈ √(p(1000−p)/cycles)` in milli.
/// Without it a handicap cannot tell sampling noise from signal — and at
/// hardware scale (tens of thousands of cycles, σ a couple of milli) that
/// distinction is the whole mechanism.
pub fn frustration_index(nodes: &[u32], edges: &[(u32, u32)], j: &[MilliValue]) -> (u32, u64) {
    frustration_index_with_index(&NodeIndex::new(nodes), edges, j)
}

/// [`frustration_index`] over an index the caller already built.
///
/// `submit_proof` builds a `NodeIndex` over the same immutable
/// `topology.nodes` for `generate_ising_model` and then again here. The owning
/// wrapper above stays for callers with only a slice.
pub(crate) fn frustration_index_with_index(
    index: &NodeIndex<'_>,
    edges: &[(u32, u32)],
    j: &[MilliValue],
) -> (u32, u64) {
    let n = index.node_count();
    if n == 0 {
        return (0, 0);
    }

    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut rank: Vec<u8> = vec![0; n];
    let mut parity: Vec<u8> = vec![0; n];

    /// Root of `v` with the accumulated parity from `v` to that root,
    /// path-compressing as it unwinds.
    fn find(parent: &mut [u32], parity: &mut [u8], v: usize) -> (usize, u8) {
        let mut root = v;
        let mut acc = 0u8;
        while parent[root] as usize != root {
            acc ^= parity[root];
            root = parent[root] as usize;
        }
        // Second pass: repoint every node on the path straight at the root,
        // rewriting its parity to be root-relative.
        let mut node = v;
        let mut node_acc = acc;
        while parent[node] as usize != node {
            let next = parent[node] as usize;
            let next_acc = node_acc ^ parity[node];
            parent[node] = root as u32;
            parity[node] = node_acc;
            node = next;
            node_acc = next_acc;
        }
        (root, acc)
    }

    let (mut cycles, mut frustrated) = (0u64, 0u64);
    for (&(u, v), &w) in edges.iter().zip(j.iter()) {
        let (Some(u_pos), Some(v_pos)) = (index.position(u), index.position(v)) else {
            continue;
        };
        if u_pos == v_pos || w == 0 {
            continue;
        }
        // bit 1 == antiferromagnetic (t = -1), i.e. J > 0.
        let bit = u8::from(w > 0);
        let (u_root, u_parity) = find(&mut parent, &mut parity, u_pos);
        let (v_root, v_parity) = find(&mut parent, &mut parity, v_pos);
        if u_root == v_root {
            cycles += 1;
            frustrated += u64::from(u_parity ^ v_parity ^ bit);
            continue;
        }
        // Union by rank, choosing the new edge's parity so that the relation
        // `parity[u] ^ parity[v] == bit` holds across the merged component.
        let link = u_parity ^ v_parity ^ bit;
        if rank[u_root] < rank[v_root] {
            parent[u_root] = v_root as u32;
            parity[u_root] = link;
        } else {
            parent[v_root] = u_root as u32;
            parity[v_root] = link;
            if rank[u_root] == rank[v_root] {
                rank[u_root] += 1;
            }
        }
    }

    if cycles == 0 {
        return (0, 0);
    }
    (((frustrated * 1000 + cycles / 2) / cycles) as u32, cycles)
}

/// Verify a combinatorial embedding (rotation system) and return the number of
/// faces it traces, or an error if it is not a planar embedding of the graph.
///
/// **Why a witness rather than a test.** A zero-field Ising ground state is
/// max-cut, and planar max-cut is polynomial-time regardless of width: a 40×40
/// grid has treewidth 40 — far above any exactness ceiling — yet is exactly
/// solvable, so certifying on width alone is unsound. The verifier cannot run a
/// planarity algorithm itself, but a planar *embedding* is checkable in `O(m)`:
/// trace the faces the rotation induces and confirm Euler's formula. Same
/// upper-bound-witness shape as [`induced_width_at_most`].
///
/// `rotation` gives, per vertex position in `nodes`, the cyclic order of its
/// incident edge indices — exactly that vertex's edges, each once. Tracing
/// embeds each component on its own sphere, so the condition is
/// `n − m + f = 2c`, not the `1 + c` of a shared plane drawing where the outer
/// faces merge; an isolated vertex contributes one face and no darts.
///
/// Self-loops and edges with an endpoint outside `nodes` are REJECTED, not
/// skipped: an embedding claims something about the whole graph, so a
/// silently-dropped edge would let a non-planar graph masquerade as the planar
/// subgraph that remains.
pub fn verify_planar_embedding<R: AsRef<[u32]>>(
    nodes: &[u32],
    edges: &[(u32, u32)],
    rotation: &[R],
) -> Result<u64, WitnessError> {
    let n = nodes.len();
    if rotation.len() != n {
        return Err(WitnessError::OrderLength);
    }
    let index = NodeIndex::new(nodes);

    // Endpoint positions per edge; any malformed edge rejects the claim.
    let mut endpoints: Vec<(usize, usize)> = Vec::with_capacity(edges.len());
    for &(u, v) in edges.iter() {
        // The TOPOLOGY's own edge list, not the submitted rotation.
        let (u_pos, v_pos) = (
            index
                .position(u)
                .ok_or(WitnessError::TopologyEdgeUnknownNode)?,
            index
                .position(v)
                .ok_or(WitnessError::TopologyEdgeUnknownNode)?,
        );
        if u_pos == v_pos {
            return Err(WitnessError::SelfLoop);
        }
        endpoints.push((u_pos, v_pos));
    }
    let m = endpoints.len();

    // `slot[edge][end]` is where that edge sits in its endpoint's rotation.
    // Filling it also checks the rotation lists a vertex's incident edges
    // exactly once each.
    let mut degree = vec![0usize; n];
    for &(u_pos, v_pos) in &endpoints {
        degree[u_pos] += 1;
        degree[v_pos] += 1;
    }
    let mut slot: Vec<[usize; 2]> = vec![[usize::MAX; 2]; m];
    for (v, order) in rotation.iter().map(AsRef::as_ref).enumerate() {
        if order.len() != degree[v] {
            return Err(WitnessError::RotationLength);
        }
        for (position, &edge) in order.iter().enumerate() {
            let edge = edge as usize;
            if edge >= m {
                return Err(WitnessError::EdgeIndexOutOfRange);
            }
            let (u_pos, v_pos) = endpoints[edge];
            let end = if u_pos == v {
                0
            } else if v_pos == v {
                1
            } else {
                return Err(WitnessError::EdgeNotIncident);
            };
            if slot[edge][end] != usize::MAX {
                return Err(WitnessError::EdgeNotIncident);
            }
            slot[edge][end] = position;
        }
    }
    if slot
        .iter()
        .any(|s| s[0] == usize::MAX || s[1] == usize::MAX)
    {
        return Err(WitnessError::EdgeNotIncident);
    }

    // Trace faces as orbits of "arrive along a dart, leave by the next edge
    // in the rotation at the head". Dart `2e + s` runs from endpoint `s` of
    // edge `e` to the other.
    let mut visited = vec![false; 2 * m];
    // Each isolated vertex is its own component, contributing one face and
    // no darts for the trace below to find.
    let mut faces = degree.iter().filter(|&&d| d == 0).count() as u64;
    for start in 0..(2 * m) {
        if visited[start] {
            continue;
        }
        faces += 1;
        let mut dart = start;
        while !visited[dart] {
            visited[dart] = true;
            let edge = dart / 2;
            let side = dart % 2;
            let (u_pos, v_pos) = endpoints[edge];
            let head = if side == 0 { v_pos } else { u_pos };
            let head_end = if side == 0 { 1 } else { 0 };
            let order = rotation[head].as_ref();
            let next_position = (slot[edge][head_end] + 1) % order.len();
            let next_edge = order[next_position] as usize;
            // Leave `head` along `next_edge`: the dart whose tail is `head`.
            let (next_u, _) = endpoints[next_edge];
            dart = if next_u == head {
                2 * next_edge
            } else {
                2 * next_edge + 1
            };
        }
    }

    // Components, so Euler's formula can be applied to a disconnected graph.
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut v: usize) -> usize {
        while parent[v] != v {
            parent[v] = parent[parent[v]];
            v = parent[v];
        }
        v
    }
    for &(u_pos, v_pos) in &endpoints {
        let (a, b) = (root(&mut parent, u_pos), root(&mut parent, v_pos));
        if a != b {
            parent[a] = b;
        }
    }
    let components = (0..n).filter(|&v| root(&mut parent, v) == v).count() as u64;

    // n − m + f = 2c holds exactly when every component embeds at genus 0.
    let lhs = n as i128 - m as i128 + faces as i128;
    if lhs == 2 * components as i128 {
        Ok(faces)
    } else {
        Err(WitnessError::NotPlanar)
    }
}

#[cfg(test)]
mod tests {
    use super::WitnessError;

    /// All three `NodeIndex` paths must resolve identically — including the
    /// ascending-with-gaps path production actually takes, the one that shipped
    /// broken in a working copy (see the type doc), caught then only because
    /// five pallet tests happened to register `[5, 6]`-style graphs.
    #[test]
    fn every_node_index_path_resolves_the_same_positions() {
        use super::NodeIndex;

        // contiguous 0..n
        let contiguous = [0u32, 1, 2, 3];
        // ascending with gaps — what `canonical_graph` produces
        let ascending = [5u32, 9, 17, 100];
        // unsorted — the fallback table
        let unsorted = [17u32, 5, 100, 9];

        for (nodes, label) in [
            (&contiguous[..], "contiguous"),
            (&ascending[..], "ascending"),
            (&unsorted[..], "unsorted"),
        ] {
            let index = NodeIndex::new(nodes);
            for (want, &id) in nodes.iter().enumerate() {
                assert_eq!(
                    index.position(id),
                    Some(want),
                    "{label}: id {id} should resolve to {want}"
                );
            }
            // An id that is not present resolves to nothing on every path.
            assert_eq!(index.position(4_242), None, "{label}: absent id");
        }

        // DUPLICATES. `ensure_valid_topology` and
        // `validate_topology_consistency` derive "is this a duplicate" from
        // `position(id) != Some(i)`, so WHICH occurrence wins decides which node
        // id gets reported — see the leftmost-match note on `position`. Pinned
        // against the linear scan it replaced, directly.
        for dup in [
            &[5u32, 5][..],
            &[5, 5, 5][..],
            &[9, 5, 9][..],
            &[5, 9, 5, 9][..],
            &[0, 1, 1, 0][..],
            &[3, 3, 1, 1, 2][..],
        ] {
            let index = NodeIndex::new(dup);
            let flagged: alloc::vec::Vec<u32> = dup
                .iter()
                .enumerate()
                .filter(|(p, &n)| index.position(n) != Some(*p))
                .map(|(_, &n)| n)
                .collect();
            // Exactly what `nodes[..position].contains(&node)` produced.
            let want: alloc::vec::Vec<u32> = dup
                .iter()
                .enumerate()
                .filter(|(p, &n)| dup[..*p].contains(&n))
                .map(|(_, &n)| n)
                .collect();
            assert_eq!(
                flagged, want,
                "{dup:?}: must flag exactly the occurrences the old linear scan did"
            );
        }

        // `windows(2)` is vacuously true on both of these.
        assert_eq!(NodeIndex::new(&[7u32]).position(7), Some(0));
        assert_eq!(NodeIndex::new(&[7u32]).position(0), None);
        assert_eq!(NodeIndex::new(&[]).position(0), None);
    }

    /// Every arm of `NodeIndex`, against `Vec::iter().position()` — the linear
    /// scan it replaced, and the definition of leftmost-match.
    ///
    /// Table-driven because the failure guarded against is a WHOLE ARM being
    /// wrong, which a test exercising only `[0, 1, 2, 3]` cannot see. The
    /// expected variant is asserted alongside the positions — otherwise the
    /// suite would still pass if the constructor collapsed everything onto one
    /// arm.
    #[test]
    fn node_index_matches_a_linear_scan_on_every_arm() {
        use super::NodeIndex;
        use alloc::vec::Vec;

        /// Which arm the constructor must pick, without exposing the payload.
        #[derive(Debug, PartialEq, Eq)]
        enum Arm {
            Contiguous,
            Ascending,
            Table,
        }
        fn arm_of(index: &NodeIndex<'_>) -> Arm {
            match index {
                NodeIndex::Contiguous { .. } => Arm::Contiguous,
                NodeIndex::Ascending(_) => Arm::Ascending,
                NodeIndex::Table(_) => Arm::Table,
            }
        }

        let cases: &[(&[u32], Arm)] = &[
            // Empty: both scans vacuously true, so `Contiguous { len: 0 }`.
            (&[], Arm::Contiguous),
            (&[0], Arm::Contiguous),
            (&[0, 1, 2, 3], Arm::Contiguous),
            // Single element that is not `0` is ascending, not contiguous.
            (&[7], Arm::Ascending),
            // Ascending with gaps — `canonical_graph` output, dead qubits.
            (&[5, 6], Arm::Ascending),
            (&[0, 2, 5], Arm::Ascending),
            (&[5, 9, 17, 100], Arm::Ascending),
            // Unsorted.
            (&[3, 1, 2], Arm::Table),
            (&[17, 5, 100, 9], Arm::Table),
            // Duplicates. `[0, 1, 1, 0]` is where `binary_search_by_key`
            // disagreed with the linear scan.
            (&[0, 1, 1, 0], Arm::Table),
            (&[5, 9, 5, 9], Arm::Table),
            (&[5, 5], Arm::Table),
            (&[5, 5, 5], Arm::Table),
            (&[9, 5, 9], Arm::Table),
            (&[3, 3, 1, 1, 2], Arm::Table),
        ];

        for (nodes, want_arm) in cases {
            let index = NodeIndex::new(nodes);
            assert_eq!(arm_of(&index), *want_arm, "{nodes:?}: wrong arm");
            assert_eq!(index.node_count(), nodes.len(), "{nodes:?}: node_count");

            // Every present id plus absent ones at both ends of the range.
            let mut probes: Vec<u32> = nodes.to_vec();
            probes.extend([0, 1, 2, 3, 4, 6, 42, 4_242, u32::MAX]);
            for id in probes {
                assert_eq!(
                    index.position(id),
                    nodes.iter().position(|&n| n == id),
                    "{nodes:?}: position({id}) must equal the linear scan",
                );
            }
        }
    }

    #[test]
    fn the_fixable_partition_is_what_a_submitter_can_act_on() {
        use super::WitnessError as W;
        // Fixable by the submitter: re-encode and resubmit.
        for e in [
            W::OrderLength,
            W::UnknownNode,
            W::DuplicateNode,
            W::EdgeIndexOutOfRange,
            W::EdgeNotIncident,
            W::RotationLength,
        ] {
            assert!(e.is_submitter_fixable(), "{e:?} is the submitter's to fix");
        }
        // NOT fixable: either the claim is false, or the stored topology is
        // the defective thing. Retrying burns another fee for the same answer.
        for e in [
            W::WidthAboveCeiling,
            W::NotPlanar,
            W::SelfLoop,
            W::TopologyEdgeUnknownNode,
        ] {
            assert!(
                !e.is_submitter_fixable(),
                "{e:?} cannot be fixed by resubmitting"
            );
        }
    }

    #[test]
    fn frustration_sigmas_measures_deviation_in_sigma() {
        use super::frustration_sigmas;

        // No expectation to compare against, or nothing to frustrate.
        assert_eq!(frustration_sigmas(500, 0, 10_000), 0);
        assert_eq!(frustration_sigmas(500, 500, 0), 0);
        // Exactly typical is zero sigma.
        assert_eq!(frustration_sigmas(500, 500, 10_000), 0);

        // POSITIVE means less frustrated than expected, i.e. easier. Backwards,
        // the handicap inverts from an equalizer into an amplifier.
        assert!(
            frustration_sigmas(400, 500, 10_000) > 0,
            "less frustrated => positive"
        );
        assert!(
            frustration_sigmas(600, 500, 10_000) < 0,
            "more frustrated => negative"
        );

        // Sigma shrinks as cycles grow, so the SAME milli deviation is more
        // sigma on a bigger graph.
        let few = frustration_sigmas(400, 500, 100);
        let many = frustration_sigmas(400, 500, 40_000);
        assert!(
            many > few,
            "more cycles => tighter sigma => larger z ({many} vs {few})"
        );

        // The precision note above: at ~41k cycles the true sigma is 2.467
        // milli, so a 100-milli deviation is ~40 sigma. Flooring twice would
        // report ~50 and make the +/-3 band far too easy to reach.
        let z = frustration_sigmas(400, 500, 41_065);
        assert!((38..=42).contains(&z), "expected ~40 sigma, got {z}");
    }

    #[test]
    fn expected_frustration_is_derived_only_when_the_spec_pins_it() {
        use super::expected_frustration_for;
        use crate::puzzle_spec::AllowedValueSpec;

        let triangle_nodes = [0u32, 1, 2];
        let triangle_edges = [(0u32, 1u32), (1, 2), (0, 2)];
        let path_edges = [(0u32, 1u32), (1, 2)];

        // Sign-symmetric: each fundamental cycle is a fair coin, so half.
        let symmetric: [crate::MilliValue; 2] = [-1000, 1000];
        let spec = AllowedValueSpec::Set(&symmetric[..]);
        assert_eq!(
            expected_frustration_for(&spec, &triangle_nodes, &triangle_edges),
            Some(500)
        );
        // A forest has nothing to frustrate.
        assert_eq!(
            expected_frustration_for(&spec, &triangle_nodes, &path_edges),
            Some(0)
        );

        // Not sign-symmetric: the spec pins nothing, so the declaration stands.
        let lopsided: [crate::MilliValue; 2] = [-1000, 2000];
        let spec = AllowedValueSpec::Set(&lopsided[..]);
        assert_eq!(
            expected_frustration_for(&spec, &triangle_nodes, &triangle_edges),
            None
        );
    }

    /// The canonical order is CONSENSUS-VISIBLE: `hash_topology` hashes it, so
    /// two spellings of it are two names for the same graph. That happened once
    /// — the hash canonicalized, storage did not — and cost a storage migration
    /// plus a `transaction_version` bump. Pinned so a future edit fails here.
    #[test]
    fn canonical_graph_pins_the_consensus_visible_order() {
        use super::canonical_graph;

        // Nodes sort ascending; edges orient (min, max) THEN sort as pairs.
        let (n, e) = canonical_graph(&[3, 1, 2], &[(3, 1), (2, 1)]);
        assert_eq!(n, vec![1, 2, 3]);
        assert_eq!(e, vec![(1, 2), (1, 3)]);

        // Fixed point on canonical input — the migration re-canonicalizes every
        // stored topology and relies on this.
        let (n2, e2) = canonical_graph(&n, &e);
        assert_eq!((n2, e2), (n, e));

        // Self-loops survive as `(u, u)`, and duplicate edges are NOT
        // deduplicated: two couplings between the same pair are two
        // couplings, and `generate_ising_model` maps `j[k]` to `edges[k]`.
        let (_, e3) = canonical_graph(&[0, 1], &[(1, 0), (0, 0), (0, 1)]);
        assert_eq!(e3, vec![(0, 0), (0, 1), (0, 1)]);

        // Sorting is by first element then second, not by any other key.
        let (_, e4) = canonical_graph(&[0, 1, 2], &[(2, 0), (1, 2), (0, 1)]);
        assert_eq!(e4, vec![(0, 1), (0, 2), (1, 2)]);
    }

    use super::{
        energy_bound_milli, frustration_index_milli, induced_width_at_most, verify_planar_embedding,
    };

    #[test]
    fn energy_bound_sums_both_field_and_coupling_magnitudes() {
        assert_eq!(energy_bound_milli(&[], &[]), 0);
        assert_eq!(energy_bound_milli(&[], &[1_000, -1_000, 500]), 2_500);
        // The field term keeps the bound below a field-bearing instance's
        // reachable energy: |h| alone can carry it past -Sum|J|.
        assert_eq!(energy_bound_milli(&[-1_000, 1_000], &[1_000]), 3_000);
        assert_eq!(energy_bound_milli(&[i32::MIN], &[i32::MIN]), 4_294_967_296);
    }

    #[test]
    fn all_antiferromagnetic_triangle_is_fully_frustrated() {
        // Three J > 0 bonds in an odd cycle: the canonical frustrated triangle.
        let nodes = [0, 1, 2];
        let edges = [(0, 1), (1, 2), (0, 2)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[1_000, 1_000, 1_000]),
            1_000
        );
    }

    #[test]
    fn all_ferromagnetic_triangle_is_unfrustrated() {
        let nodes = [0, 1, 2];
        let edges = [(0, 1), (1, 2), (0, 2)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[-1_000, -1_000, -1_000]),
            0
        );
    }

    #[test]
    fn even_antiferromagnetic_cycle_is_unfrustrated() {
        // C4, all-AFM: an even number of AFM bonds per cycle is 2-colorable.
        let nodes = [0, 1, 2, 3];
        let edges = [(0, 1), (1, 2), (2, 3), (0, 3)];
        assert_eq!(frustration_index_milli(&nodes, &edges, &[1_000; 4]), 0);
    }

    #[test]
    fn single_flipped_bond_frustrates_even_cycle() {
        let nodes = [0, 1, 2, 3];
        let edges = [(0, 1), (1, 2), (2, 3), (0, 3)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[1_000, 1_000, 1_000, -1_000]),
            1_000
        );
    }

    #[test]
    fn forest_has_no_cycles_to_frustrate() {
        let nodes = [0, 1, 2, 3];
        let edges = [(0, 1), (1, 2), (1, 3)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[1_000, -1_000, 1_000]),
            0
        );
    }

    #[test]
    fn frustration_is_gauge_invariant() {
        // Gauging must not move the index. Bowtie of two triangles sharing
        // vertex 2, flipping spins {1, 3}: the all-AFM triangle is frustrated,
        // the all-FM one is not, so a gauge rewriting four of the six couplings
        // must still report 1 of 2.
        let nodes = [0, 1, 2, 3, 4];
        let edges = [(0, 1), (1, 2), (0, 2), (2, 3), (3, 4), (2, 4)];
        let j = [1_000, 1_000, 1_000, -1_000, -1_000, -1_000];
        let base = frustration_index_milli(&nodes, &edges, &j);
        let s = [1i64, -1, 1, -1, 1];
        let gauged: Vec<i32> = edges
            .iter()
            .zip(j.iter())
            .map(|(&(u, v), &w)| (i64::from(w) * s[u as usize] * s[v as usize]) as i32)
            .collect();
        assert_eq!(frustration_index_milli(&nodes, &edges, &gauged), base);
        assert_ne!(gauged.as_slice(), j.as_slice());
        assert_eq!(base, 500);
    }

    #[test]
    fn sparse_node_ids_resolve_through_the_sorted_fallback() {
        // Same frustrated triangle, relabelled off the contiguous range.
        let nodes = [70, 11, 42];
        let edges = [(70, 11), (11, 42), (70, 42)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[1_000, 1_000, 1_000]),
            1_000
        );
    }

    #[test]
    fn unknown_endpoints_self_loops_and_zero_couplings_are_skipped() {
        let nodes = [0, 1, 2];
        let edges = [(0, 1), (1, 2), (0, 2), (0, 9), (1, 1)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[1_000, 1_000, 1_000, 1_000, 1_000]),
            1_000
        );
        // A zero coupling has no bond type, so its cycle is not counted.
        let edges = [(0, 1), (1, 2), (0, 2)];
        assert_eq!(
            frustration_index_milli(&nodes, &edges, &[1_000, 1_000, 0]),
            0
        );
    }

    #[test]
    fn parallel_edges_close_two_cycles() {
        // An opposing parallel pair cannot be satisfied together.
        let nodes = [0, 1];
        assert_eq!(
            frustration_index_milli(&nodes, &[(0, 1), (0, 1)], &[1_000, -1_000]),
            1_000
        );
        // A matching pair can.
        assert_eq!(
            frustration_index_milli(&nodes, &[(0, 1), (0, 1)], &[1_000, 1_000]),
            0
        );
    }

    #[test]
    fn disconnected_components_pool_their_cycles() {
        // Two disjoint triangles, one frustrated and one not: 1 of 2 cycles.
        let nodes = [0, 1, 2, 3, 4, 5];
        let edges = [(0, 1), (1, 2), (0, 2), (3, 4), (4, 5), (3, 5)];
        let j = [1_000, 1_000, 1_000, -1_000, -1_000, -1_000];
        assert_eq!(frustration_index_milli(&nodes, &edges, &j), 500);
    }

    // ─────────────── elimination-order witness verification ───────────────

    #[test]
    fn a_tree_eliminates_at_width_one() {
        // Leaves first: every bag is the single parent.
        let nodes = [0, 1, 2, 3];
        let edges = [(0, 1), (1, 2), (1, 3)];
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 2, 3, 1], 20),
            Ok(1)
        );
    }

    #[test]
    fn a_clique_needs_its_full_width() {
        // K4 has induced width 3 under every order, so a ceiling of 2 refuses.
        let nodes = [0, 1, 2, 3];
        let edges = [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1, 2, 3], 20),
            Ok(3)
        );
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1, 2, 3], 3),
            Ok(3)
        );
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1, 2, 3], 2),
            Err(WitnessError::WidthAboveCeiling)
        );
    }

    #[test]
    fn fill_edges_are_counted() {
        // A 4-cycle has max degree 2, but eliminating a vertex fills in its two
        // neighbours. Order 0,2 (opposite corners) first forces the fill out.
        let nodes = [0, 1, 2, 3];
        let edges = [(0, 1), (1, 2), (2, 3), (0, 3)];
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 2, 1, 3], 20),
            Ok(2)
        );
        // Refusing at ceiling 1 proves the fill edge was actually added: a
        // missing fill would let the last two vertices come out at width 0.
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 2, 1, 3], 1),
            Err(WitnessError::WidthAboveCeiling)
        );
    }

    #[test]
    fn order_must_be_a_permutation_of_the_nodes() {
        let nodes = [0, 1, 2];
        let edges = [(0, 1), (1, 2)];
        // Too short.
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1], 20),
            Err(WitnessError::OrderLength)
        );
        // Repeats a node (and so omits another).
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1, 1], 20),
            Err(WitnessError::DuplicateNode)
        );
        // Names a node the topology does not have.
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1, 9], 20),
            Err(WitnessError::UnknownNode)
        );
    }

    #[test]
    fn width_depends_on_the_order_so_a_witness_is_worth_checking() {
        // A star: hub first pays its full degree, leaves first pays 1. Both are
        // valid orders — which is why the prover supplies one rather than the
        // chain searching.
        let nodes = [0, 1, 2, 3, 4];
        let edges = [(0, 1), (0, 2), (0, 3), (0, 4)];
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[0, 1, 2, 3, 4], 20),
            Ok(4)
        );
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[1, 2, 3, 4, 0], 20),
            Ok(1)
        );
    }

    #[test]
    fn sparse_ids_self_loops_and_foreign_edges_are_handled() {
        let nodes = [70, 11, 42];
        let edges = [(70, 11), (11, 42), (11, 11), (70, 9)];
        assert_eq!(
            induced_width_at_most(&nodes, &edges, &[70, 42, 11], 20),
            Ok(1)
        );
    }

    // ───────────────── planar-embedding witness verification ─────────────

    /// K4 is planar: drawn with one vertex inside the triangle, it has 4
    /// faces and Euler holds.
    #[test]
    fn a_planar_embedding_traces_the_right_number_of_faces() {
        let nodes = [0, 1, 2, 3];
        // edges: 0-1(0) 1-2(1) 2-0(2) 0-3(3) 1-3(4) 2-3(5)
        let edges = [(0, 1), (1, 2), (2, 0), (0, 3), (1, 3), (2, 3)];
        // Rotations for the standard planar drawing of K4.
        let rotation = alloc::vec![
            alloc::vec![0u32, 3, 2],
            alloc::vec![1u32, 4, 0],
            alloc::vec![2u32, 5, 1],
            alloc::vec![3u32, 4, 5],
        ];
        // n - m + f = 4 - 6 + f = 2*1 => f = 4.
        assert_eq!(verify_planar_embedding(&nodes, &edges, &rotation), Ok(4));
    }

    /// K5 is not planar, so no rotation system can satisfy Euler — the check
    /// rejects whatever is offered.
    #[test]
    fn no_rotation_system_can_embed_k5() {
        let nodes = [0, 1, 2, 3, 4];
        let mut edges = alloc::vec::Vec::new();
        for a in 0..5u32 {
            for b in (a + 1)..5 {
                edges.push((a, b));
            }
        }
        // Incident edges in index order — well-formed, just not planar.
        let rotation: alloc::vec::Vec<alloc::vec::Vec<u32>> = (0..5u32)
            .map(|v| {
                edges
                    .iter()
                    .enumerate()
                    .filter(|(_, &(a, b))| a == v || b == v)
                    .map(|(i, _)| i as u32)
                    .collect()
            })
            .collect();
        assert_eq!(
            verify_planar_embedding(&nodes, &edges, &rotation),
            Err(WitnessError::NotPlanar)
        );
    }

    /// A malformed rotation is rejected rather than silently reinterpreted:
    /// omitting an edge would let a non-planar graph pass as the planar
    /// subgraph that remains.
    #[test]
    fn a_rotation_must_list_every_incident_edge_exactly_once() {
        let nodes = [0, 1, 2];
        let edges = [(0, 1), (1, 2), (2, 0)];
        let good = alloc::vec![
            alloc::vec![0u32, 2],
            alloc::vec![1u32, 0],
            alloc::vec![2u32, 1],
        ];
        assert!(verify_planar_embedding(&nodes, &edges, &good).is_ok());

        // Drops an edge from vertex 0.
        let short = alloc::vec![
            alloc::vec![0u32],
            alloc::vec![1u32, 0],
            alloc::vec![2u32, 1],
        ];
        assert_eq!(
            verify_planar_embedding(&nodes, &edges, &short),
            Err(WitnessError::RotationLength)
        );

        // Names an edge vertex 0 is not on.
        let wrong = alloc::vec![
            alloc::vec![0u32, 1],
            alloc::vec![1u32, 0],
            alloc::vec![2u32, 1],
        ];
        assert_eq!(
            verify_planar_embedding(&nodes, &edges, &wrong),
            Err(WitnessError::EdgeNotIncident)
        );

        // Repeats one instead of listing both.
        let repeated = alloc::vec![
            alloc::vec![0u32, 0],
            alloc::vec![1u32, 0],
            alloc::vec![2u32, 1],
        ];
        assert_eq!(
            verify_planar_embedding(&nodes, &edges, &repeated),
            Err(WitnessError::EdgeNotIncident)
        );
    }

    /// Euler's formula is applied per component, so a disconnected planar
    /// graph still verifies.
    #[test]
    fn disconnected_planar_graphs_verify() {
        // Two disjoint triangles.
        let nodes = [0, 1, 2, 3, 4, 5];
        let edges = [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)];
        let rotation = alloc::vec![
            alloc::vec![0u32, 2],
            alloc::vec![1u32, 0],
            alloc::vec![2u32, 1],
            alloc::vec![3u32, 5],
            alloc::vec![4u32, 3],
            alloc::vec![5u32, 4],
        ];
        // Traced per component: each triangle keeps its own outer face, so
        // n - m + f = 6 - 6 + f = 2*2 => f = 4.
        assert_eq!(verify_planar_embedding(&nodes, &edges, &rotation), Ok(4));
    }

    #[test]
    fn self_loops_and_foreign_endpoints_reject_the_claim() {
        let nodes = [0, 1];
        assert_eq!(
            verify_planar_embedding(
                &nodes,
                &[(0, 0)],
                &alloc::vec![alloc::vec![0u32], alloc::vec![]]
            ),
            Err(WitnessError::SelfLoop)
        );
        assert_eq!(
            verify_planar_embedding(
                &nodes,
                &[(0, 9)],
                &alloc::vec![alloc::vec![0u32], alloc::vec![]]
            ),
            // The TOPOLOGY's edge names a node outside its node list: a defect
            // in what was registered, not in the submitted rotation.
            Err(WitnessError::TopologyEdgeUnknownNode)
        );
    }

    #[test]
    fn empty_topology_has_zero_width() {
        assert_eq!(induced_width_at_most(&[], &[], &[], 20), Ok(0));
    }
}

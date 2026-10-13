//! Benchmarking setup for pallet-quantum-pow.

use super::*;

#[allow(unused)]
use crate::Pallet as QuantumPow;
use alloc::{vec, vec::Vec};
use frame_benchmarking::v2::*;
use frame_support::{
    traits::{Currency, Get},
    BoundedVec,
};
use frame_system::RawOrigin;
use quantum_validation::{
    derive_nonce, packed::pack_solution, AllowedValueSpec, MilliValue, MILLI_SCALE,
};
use sp_runtime::traits::{SaturatedConversion, Saturating};

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: frame_support::traits::Get<u32>,
{
    items
        .try_into()
        .ok()
        .expect("benchmark input fits within bounds")
}

fn benchmark_funding<T: Config>() -> BalanceOf<T> {
    T::MinerDeposit::get().saturating_mul(10u32.saturated_into())
}

fn fund_account<T: Config>(who: &T::AccountId, amount: BalanceOf<T>) {
    let _ = T::Currency::make_free_balance_be(who, amount);
}

fn easy_difficulty() -> types::DifficultyConfig {
    types::DifficultyConfig {
        max_energy_milli: i64::MAX,
    }
}

const SCALE: MilliValue = MILLI_SCALE as MilliValue;

fn allowed_spin_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, SCALE]))
}

fn allowed_h_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, 0, SCALE]))
}

fn allowed_j_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, SCALE]))
}

/// A coupling spec that is NOT sign-symmetric. Deliberate.
///
/// `register_topology` DERIVES `expected_frustration_milli` for a sign-symmetric
/// spec, overwriting the declared `0` with `500` and putting `submit_proof`
/// behind the +/-3 sigma band. Sigma shrinks as cycles grow, so on a large swept
/// fixture the band is a few milli wide and whether a given `n` lands inside it
/// depends on the draw — the benchmark would fail at some sweep points only. An
/// asymmetric spec pins nothing: the declared `0` stands and
/// `frustration_sigmas` returns `0` unconditionally.
fn asymmetric_j_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, 2 * SCALE]))
}

/// An `n`-node graph carrying `e` edges: a spanning path `0-1-...-(n-1)`
/// plus chords, so `e - (n - 1)` edges close a fundamental cycle.
///
/// The point is the CYCLES. `SUBMIT_PROOF_K_EDGE` prices
/// `frustration_index`, whose cost sits in the parity union-find's
/// cycle-closing branch. The original fixture was a fixed 2-node, 1-edge,
/// ACYCLIC graph, so that branch never ran while the constant derived from it
/// shipped.
///
/// `n` and `e` are SEPARATE sweep components on purpose. Tying `e = 2n - 3`
/// makes them collinear, and the weight formula has independent `k1·n`, `k2·e`
/// and `k6·e` terms — a collinear sweep fits `k1 + 2·k2 + 2·k6` as one lump and
/// cannot recover the constant this benchmark exists to back. Chords are emitted
/// in a fixed order, so the shape is deterministic at every `(n, e)`.
fn cyclic_topology<T: Config>(n: u32, e: u32) -> (NodesOf<T>, EdgesOf<T>, sp_core::H256) {
    let node_ids: Vec<u32> = (0..n).collect();
    let mut edge_list: Vec<(u32, u32)> = (0..n.saturating_sub(1)).map(|i| (i, i + 1)).collect();
    // Chords, deterministic order, until the edge budget is met. Skips
    // adjacent pairs (already on the path) so no edge is duplicated.
    'outer: for span in 2..n.max(2) {
        for u in 0..n.saturating_sub(span) {
            if edge_list.len() as u32 >= e {
                break 'outer;
            }
            edge_list.push((u, u + span));
        }
    }
    let nodes = bounded::<_, T::MaxNodes>(node_ids);
    let edges = bounded::<_, T::MaxEdges>(edge_list);
    let topology_hash = crate::topology::hash_topology(
        &nodes,
        &edges,
        &allowed_h_set::<T>().as_slice(),
        &asymmetric_j_set::<T>().as_slice(),
        &allowed_spin_set::<T>().as_slice(),
    );
    (nodes, edges, topology_hash)
}

fn register_cyclic_topology_for<T: Config>(
    n: u32,
    e: u32,
) -> (NodesOf<T>, EdgesOf<T>, sp_core::H256) {
    let (nodes, edges, topology_hash) = cyclic_topology::<T>(n, e);
    assert!(QuantumPow::<T>::register_topology(
        RawOrigin::Root.into(),
        nodes.clone(),
        edges.clone(),
        allowed_h_set::<T>(),
        asymmetric_j_set::<T>(),
        allowed_spin_set::<T>(),
        types::TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // Declared zero and NOT overwritten, because the spec is not
            // sign-symmetric. See `asymmetric_j_set`.
            expected_frustration_milli: 0,
        },
    )
    .is_ok());
    (nodes, edges, topology_hash)
}

/// A valid proof for an `n`-node topology: all spins at `+1`.
fn valid_proof_for<T: Config>(
    miner: &T::AccountId,
    topology_hash: sp_core::H256,
    n: u32,
) -> QuantumProofOf<T> {
    frame_system::Pallet::<T>::set_block_number(1u32.into());
    let salt = [7u8; 32];
    // Read the SAME source `submit_proof` reads — the `LastProofBlockHash`
    // cache, NOT `frame_system::block_hash`. They coincide only on pristine
    // state, so the wrong one gives `InvalidNonce` as soon as a benchmark runs
    // twice in one externalities, which is what a `Linear` component does.
    let last_proof_block_hash_bytes = LastProofBlockHash::<T>::get().0;
    let miner_bytes = QuantumPow::<T>::account_to_bytes(miner);
    let nonce = derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &salt);

    let spin_spec = allowed_spin_set::<T>();
    let spins: Vec<quantum_validation::MilliValue> = vec![SCALE; n as usize];
    let packed = pack_solution(&spins, &spin_spec.as_slice()).expect("binary spin pack succeeds");
    let solutions: PackedSpinBytesOf<T> = bounded::<u8, T::MaxNodes>(packed);

    types::QuantumProof {
        topology_hash,
        nonce,
        salt,
        solutions,
        device_access_time_us: 0,
    }
}

/// Smallest `n` whose COMPLETE graph carries at least `max_edges` edges: the
/// least `n` with `n(n - 1) / 2 >= max_edges`.
///
/// Closed form of the bisection it replaces:
/// `n(n-1)/2 >= e  <=>  n >= (1 + sqrt(1 + 8e)) / 2`, so the answer is
/// `ceil((1 + sqrt(1 + 8e)) / 2)`.
///
/// That `sqrt` is REAL. `ceil((1 + isqrt(1 + 8e)) / 2)` is WRONG: at `e = 4`,
/// `isqrt(33) = 5` puts `(1 + 5) / 2` exactly on `3`, the ceiling is a no-op,
/// and `3·2/2 = 3 < 4`. Truncation costs at most one, so take the seed as a
/// FLOOR and correct upward against the defining inequality. Do not
/// reintroduce plain `ceil`.
///
/// A `const fn` ON PURPOSE. The bisection ran in the benchmark BODY, so the
/// DECLARED range still started below the floor and every sample under it
/// measured identical work — a flat prefix at rising declared `n`, dragging the
/// fitted per-node slope DOWN, an undercharge on a consensus weight. Being
/// const-evaluable is what lets the floor live in the `Linear<..>` RANGE, where
/// FRAME's regression sees it.
///
/// `tests::complete_cover_nodes_matches_bisection` pins agreement over
/// `0..300_000`, not just at endpoints.
const fn complete_cover_nodes(max_edges: u32) -> u32 {
    let target = max_edges as u64;
    let radicand = 1u64.saturating_add(8u64.saturating_mul(target));
    // floor((1 + isqrt(1 + 8e)) / 2): never above the answer, at most one below.
    let mut n = (1u64.saturating_add(radicand.isqrt())) / 2;
    while n * n.saturating_sub(1) / 2 < target {
        n += 1;
    }
    n as u32
}

/// Lower end of any node sweep that registers a topology and then wants an edge
/// count independent of it. Two floors at once:
///
/// - `MinNodes`, below which `register_topology` refuses outright (16 on the
///   runtime, 2 in the mock);
/// - `complete_cover_nodes(MaxEdges)`, above which the simple-graph edge cap
///   `min(n(n-1)/2, MaxEdges)` stops being quadratic in `n` and becomes the
///   constant `MaxEdges` — which is what makes `e` INDEPENDENT of `n`.
///
/// Clamped into `[3, MaxNodes]` so the emitted range can never invert.
fn min_swept_nodes<T: Config>() -> u32 {
    let cover = complete_cover_nodes(T::MaxEdges::get()).max(3);
    T::MinNodes::get().max(cover).min(T::MaxNodes::get())
}

/// Lower end of the edge sweep, `MaxNodes + 4`.
///
/// The fixture floors `e` at `n + 4` (five cycles), and `frame-benchmarking-cli`
/// pins every OTHER component at its maximum while sweeping one — so
/// `n == MaxNodes` throughout. Anything lower is a flat prefix: rising declared
/// `e`, identical cost.
fn min_swept_edges<T: Config>() -> u32 {
    T::MaxNodes::get().saturating_add(4).min(T::MaxEdges::get())
}

/// Lower end of `prove_topology_exact`'s node sweep, `ExactSolveCeiling + 1`.
///
/// The caterpillar needs a window of `ceiling + 1` mutually-adjacent vertices to
/// saturate a bag, so the body floors `node_count` there. Anything lower is a
/// flat prefix, biasing the fitted node slope DOWN on an extrinsic quadratic in
/// the ceiling.
///
/// Clamped to `MaxNodes`, which the MOCK hits: 16 against a ceiling of 20
/// collapses the range to `Linear<16, 16>`. Not a regression — the mock sweep
/// was already one point, pinned at 16 by the body clamp and merely declared as
/// thirteen; this makes it visible. Only `impl_benchmark_test_suite!` runs
/// against the mock, without fitting, so no shipped weight comes from it.
/// Widening the mock past the ceiling is tracked separately.
fn min_exact_nodes<T: Config>() -> u32 {
    T::ExactSolveCeiling::get()
        .saturating_add(1)
        .min(T::MaxNodes::get())
}

fn allowed_set_with_len<T: Config>(
    len: u32,
    offset: MilliValue,
) -> AllowedValueSpec<AllowedValueSetOf<T>> {
    assert!(len >= 1);
    assert!(len <= T::MaxAllowedValues::get());
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(
        (0..len)
            .map(|index| {
                offset
                    .checked_add(index as MilliValue)
                    .expect("benchmark allowed value fits")
            })
            .collect(),
    ))
}

fn allowed_sets_with_total<T: Config>(
    total: u32,
) -> (
    AllowedValueSpec<AllowedValueSetOf<T>>,
    AllowedValueSpec<AllowedValueSetOf<T>>,
    AllowedValueSpec<AllowedValueSetOf<T>>,
) {
    let max = T::MaxAllowedValues::get();
    assert!(total >= 3);
    assert!(total <= max.saturating_mul(3));

    let mut lengths = [1_u32; 3];
    for index in 0..total.saturating_sub(3) {
        let slot = index as usize % lengths.len();
        if lengths[slot] < max {
            lengths[slot] += 1;
        } else {
            let next = (slot + 1) % lengths.len();
            if lengths[next] < max {
                lengths[next] += 1;
            } else {
                lengths[(slot + 2) % lengths.len()] += 1;
            }
        }
    }

    // Offsets keep the swept specs valid under registration's refusals:
    // magnitudes stay near SCALE so the curve calibration holds, and the J
    // set never contains zero, so no all-zero instance is reachable.
    (
        allowed_set_with_len::<T>(lengths[0], -SCALE),
        allowed_set_with_len::<T>(lengths[1], SCALE),
        allowed_set_with_len::<T>(lengths[2], -SCALE),
    )
}

fn topology_with_dimensions<T: Config>(
    offset: u32,
    node_count: u32,
    edge_count: u32,
    allowed_h: &AllowedValueSpec<AllowedValueSetOf<T>>,
    allowed_j: &AllowedValueSpec<AllowedValueSetOf<T>>,
    allowed_spin: &AllowedValueSpec<AllowedValueSetOf<T>>,
) -> (NodesOf<T>, EdgesOf<T>, sp_core::H256) {
    assert!(node_count >= T::MinNodes::get().max(2));
    assert!(node_count <= T::MaxNodes::get());
    assert!(edge_count >= 1);
    assert!(edge_count <= T::MaxEdges::get());

    let node_ids: Vec<u32> = (0..node_count)
        .map(|index| offset.checked_add(index).expect("benchmark node id fits"))
        .collect();
    let edge_ids: Vec<(u32, u32)> = (0..edge_count)
        .map(|index| {
            let source = index % node_count;
            (
                offset
                    .checked_add(source)
                    .expect("benchmark source id fits"),
                offset
                    .checked_add((source + 1) % node_count)
                    .expect("benchmark target id fits"),
            )
        })
        .collect();
    let nodes = bounded::<_, T::MaxNodes>(node_ids);
    let edges = bounded::<_, T::MaxEdges>(edge_ids);
    let topology_hash = crate::topology::hash_topology(
        &nodes,
        &edges,
        &allowed_h.as_slice(),
        &allowed_j.as_slice(),
        &allowed_spin.as_slice(),
    );
    (nodes, edges, topology_hash)
}

fn topology_with_offset<T: Config>(offset: u32) -> (NodesOf<T>, EdgesOf<T>, sp_core::H256) {
    let node_count = T::MinNodes::get().max(2);
    topology_with_dimensions::<T>(
        offset,
        node_count,
        node_count.saturating_sub(1).max(1),
        &allowed_h_set::<T>(),
        &allowed_j_set::<T>(),
        &allowed_spin_set::<T>(),
    )
}

fn sample_topology<T: Config>() -> (NodesOf<T>, EdgesOf<T>, sp_core::H256) {
    topology_with_offset::<T>(0)
}

fn register_miner_for<T: Config>(who: &T::AccountId) {
    // Fund RELATIVE to the configured deposit, not from a literal. A flat
    // 1_000_000 covers the mock's `MinerDeposit = 100` but is six orders of
    // magnitude short of the runtime's `UNIT = 1e12`, so `register_miner` fails
    // and every `submit_proof` benchmark aborts against the real runtime.
    // `impl_benchmark_test_suite!` cannot catch that: it runs in the mock.
    let deposit: u128 = T::MinerDeposit::get().saturated_into();
    fund_account::<T>(
        who,
        deposit
            .saturating_mul(1_000)
            .max(1_000_000)
            .saturated_into(),
    );
    assert!(QuantumPow::<T>::register_miner(RawOrigin::Signed(who.clone()).into()).is_ok());
}

fn register_topology_for<T: Config>() -> (NodesOf<T>, EdgesOf<T>, sp_core::H256) {
    let (nodes, edges, topology_hash) = sample_topology::<T>();
    assert!(QuantumPow::<T>::register_topology(
        RawOrigin::Root.into(),
        nodes.clone(),
        edges.clone(),
        allowed_h_set::<T>(),
        allowed_j_set::<T>(),
        allowed_spin_set::<T>(),
        types::TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // The benchmark topology is acyclic, so no handicap applies.
            expected_frustration_milli: 0,
        },
    )
    .is_ok());
    (nodes, edges, topology_hash)
}

/// Registers a second, distinct (non-default) connected topology, returning
/// its hash. Used to drive whitelist benchmarks that need a topology other
/// than the auto-whitelisted first registration.
fn register_extra_topology_for<T: Config>(offset: u32) -> sp_core::H256 {
    let (nodes, edges, topology_hash) = topology_with_offset::<T>(offset);
    assert!(QuantumPow::<T>::register_topology(
        RawOrigin::Root.into(),
        nodes,
        edges,
        allowed_h_set::<T>(),
        allowed_j_set::<T>(),
        allowed_spin_set::<T>(),
        types::TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // The benchmark topology is acyclic, so no handicap applies.
            expected_frustration_milli: 0,
        },
    )
    .is_ok());
    topology_hash
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn register_miner() {
        let caller: T::AccountId = whitelisted_caller();
        fund_account::<T>(&caller, benchmark_funding::<T>());

        #[extrinsic_call]
        QuantumPow::register_miner(RawOrigin::Signed(caller.clone()));

        assert!(Miners::<T>::contains_key(caller));
    }

    #[benchmark]
    fn deregister_miner() {
        let caller: T::AccountId = whitelisted_caller();
        register_miner_for::<T>(&caller);

        #[extrinsic_call]
        QuantumPow::deregister_miner(RawOrigin::Signed(caller.clone()));

        assert!(!Miners::<T>::contains_key(caller));
    }

    #[benchmark]
    fn register_topology(
        n: Linear<{ T::MinNodes::get() }, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        a: Linear<3, { T::MaxAllowedValues::get().saturating_mul(3) }>,
    ) {
        let (allowed_h, allowed_j, allowed_spin) = allowed_sets_with_total::<T>(a);
        // Floor the edge count at a path graph. Registration refuses a curve
        // calibrated past the topology's own typical weight, and a near-
        // edgeless many-node topology trips exactly that guard: the curve's
        // low-degree field term outgrows `-(Sum|h| + Sum|J|)`. Only the
        // low-e/high-n corner moves, so each component's fitted slope keeps
        // its full sweep range wherever the point is actually registrable.
        let e = e.max(n.saturating_sub(1)).max(1);
        let (topology_nodes, topology_edges, topology_hash) =
            topology_with_dimensions::<T>(0, n, e, &allowed_h, &allowed_j, &allowed_spin);

        #[extrinsic_call]
        QuantumPow::register_topology(
            RawOrigin::Root,
            topology_nodes,
            topology_edges,
            allowed_h,
            allowed_j,
            allowed_spin,
            types::TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // The benchmark topology is acyclic, so no handicap applies.
                expected_frustration_milli: 0,
            },
        );

        assert!(RegisteredTopologies::<T>::contains_key(topology_hash));
    }

    #[benchmark]
    fn set_default_topology() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        // register_topology does not seed MineableTopologies; only
        // add_mineable_topology and the v3 migration do.
        MineableTopologies::<T>::insert(topology_hash, ());
        // Clear the first-registration seeding so the call exercises the
        // repointing write, not a no-op.
        DefaultTopology::<T>::kill();

        #[extrinsic_call]
        QuantumPow::set_default_topology(RawOrigin::Root, topology_hash);

        assert_eq!(DefaultTopology::<T>::get(), Some(topology_hash));
    }

    #[benchmark]
    fn set_difficulty() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        let difficulty = easy_difficulty();

        #[extrinsic_call]
        QuantumPow::set_difficulty(RawOrigin::Root, topology_hash, difficulty);

        assert_eq!(Difficulties::<T>::get(topology_hash), Some(difficulty));
    }

    #[benchmark]
    fn set_topology_curve() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        let curve_c = crate::difficulty::CurveC {
            easy_milli: 600,
            knee_milli: 700,
            hard_milli: 800,
        };

        #[extrinsic_call]
        QuantumPow::set_topology_curve(RawOrigin::Root, topology_hash, curve_c);

        assert_eq!(TopologyCurveC::<T>::get(topology_hash), Some(curve_c));
    }

    /// Swept over node AND edge count on a CYCLIC fixture.
    ///
    /// `SUBMIT_PROOF_K_EDGE` prices the `frustration_index` and
    /// `energy_bound_milli` passes, and the previous fixed 2-node, 1-edge
    /// acyclic fixture never executed the parity union-find's cycle-closing
    /// branch. Here `e` is swept INDEPENDENTLY of `n` with a floor of `n + 4`,
    /// so every sweep point carries at least five fundamental cycles. (`2n - 3`
    /// tied the two together; that is why `e` became its own component.)
    ///
    /// Both floors live in the DECLARED RANGES, not the body: FRAME regresses
    /// measured time against the DECLARED component value, so a body floor
    /// yields identical cost at rising declared values — a flat prefix that
    /// pulls the fitted slope DOWN, an undercharge on a consensus weight.
    /// Before this, `n: Linear<4, MaxNodes>` was raised in the body to
    /// `MinNodes` and then to the complete-cover floor (9 in the mock, 317 on
    /// the runtime), flattening ~6 of 13 mock points and ~3 of 50 runtime
    /// points; `e: Linear<4, MaxEdges>` was raised to `n + 4` with `n` pinned at
    /// `MaxNodes` (FRAME holds every other component at its maximum while
    /// sweeping one), flattening everything below `MaxNodes + 4`.
    #[benchmark]
    fn submit_proof(
        n: Linear<{ min_swept_nodes::<T>() }, { T::MaxNodes::get() }>,
        e: Linear<{ min_swept_edges::<T>() }, { T::MaxEdges::get() }>,
    ) {
        let caller: T::AccountId = whitelisted_caller();
        register_miner_for::<T>(&caller);
        // Clamp `e` into what an `n`-node simple graph can carry, FLOORED at
        // `n + 4`, not `n - 1`. At `n - 1` the graph is a spanning tree and the
        // cycle-closing branch never runs. At exactly `n` there is ONE cycle and
        // whether the draw frustrates it is a coin flip, so `submit_proof`
        // rejects `InstanceIsGaugeTrivial` on half the nonces. Five cycles puts
        // an all-satisfied draw at 2^-5.
        //
        // `n` starts at `min_swept_nodes`: at once `MinNodes` (16 runtime, 2
        // mock) and the point where `n(n-1)/2` clears `MaxEdges`. The second
        // keeps the components INDEPENDENT — below it the cap
        // `min(n(n-1)/2, MaxEdges)` is quadratic in `n` (below n ~ 317 at
        // MaxEdges = 50_000), so sweeping `n` with `e` held high silently sweeps
        // `e` too, the same contamination as the collinear `e = 2n - 3`.
        //
        // These clamps are redundant with the declared ranges and kept only as a
        // guard: `max_simple == max_edges` is asserted below, so a configuration
        // that puts them back in play fails loudly rather than quietly refitting
        // a contaminated slope.
        let n = n.max(min_swept_nodes::<T>());
        let max_edges = T::MaxEdges::get();
        let max_simple = (u64::from(n) * (u64::from(n) - 1) / 2).min(u64::from(max_edges)) as u32;
        let e = e.max(n.saturating_add(4)).min(max_simple);
        let (_nodes, _edges, topology_hash) = register_cyclic_topology_for::<T>(n, e);
        MineableTopologies::<T>::insert(topology_hash, ());
        Difficulties::<T>::insert(topology_hash, easy_difficulty());
        let proof = valid_proof_for::<T>(&caller, topology_hash, n);

        #[extrinsic_call]
        QuantumPow::submit_proof(RawOrigin::Signed(caller.clone()), proof);

        let miner = Miners::<T>::get(caller).expect("miner remains registered");
        assert_eq!(miner.proofs_submitted, 1);
        assert_eq!(BlockProofCount::<T>::get(), 1);
        assert!(
            BlockBestProof::<T>::get().is_some(),
            "the proof was accepted"
        );
        // The graph must actually carry cycles, or the cycle-closing branch
        // never runs and the per-edge constant measures the wrong work.
        // Asserted on the CONSTRUCTION, not the realized frustration, which
        // depends on the draw. `e > n - 1` (one cycle) is NOT enough: a single
        // cycle is the coin flip that makes `InstanceIsGaugeTrivial` a property
        // of the nonce draw.
        assert!(
            e >= n.saturating_add(4),
            "fewer than five cycles; the draw can be gauge-trivial"
        );
        // The load-bearing one. `max_simple == max_edges` says the `n` floor did
        // its job and the edge cap is no longer quadratic in `n` — i.e. `e` is
        // an independent component. An off-by-one in the floor silently
        // restores the quadratic cap with no other signal anywhere.
        assert_eq!(
            max_simple, max_edges,
            "the n floor failed, so e is not independent of n"
        );
    }

    /// The permissionless exactness challenge, at its worst case: an order
    /// whose every bag sits exactly at `ExactSolveCeiling`, so the early
    /// abort never fires and the replay pays the full `nodes * ceiling^2`.
    ///
    /// Swept, not run at one point: `WeightInfo::prove_topology_exact` is linear
    /// in BOTH nodes and edges, and a single point confirms only one total.
    /// Edges move with `n` here (a caterpillar has `~n * ceiling` edges), so the
    /// two terms are fitted together, which matches how they are consumed.
    #[benchmark]
    fn prove_topology_exact(n: Linear<{ min_exact_nodes::<T>() }, { T::MaxNodes::get() }>) {
        let caller: T::AccountId = whitelisted_caller();
        let ceiling = T::ExactSolveCeiling::get() as usize;
        // A caterpillar of overlapping cliques: consecutive windows of
        // `ceiling + 1` mutually-adjacent vertices saturate every bag without
        // ever exceeding the ceiling.
        let node_count = (n as usize)
            .max(ceiling + 1)
            .min(T::MaxNodes::get() as usize);
        let nodes: Vec<u32> = (0..node_count as u32).collect();
        // Window width chosen so the whole caterpillar fits the edge budget.
        // Emitting `ceiling` edges per vertex in vertex order exhausts
        // `MaxEdges` partway along and leaves every later vertex ISOLATED: the
        // replay does no bag work for the tail and the node slope comes out too
        // shallow. Narrow the window so every vertex carries its share.
        let max_edges = T::MaxEdges::get() as usize;
        let window = ceiling.min(max_edges / node_count.max(1)).max(1);
        // Shared with `verifier_cost::elimination_witness_cost`, which is
        // where PROVE_EXACT_K1_NODE is measured. Two copies of this shape had
        // already drifted on the window.
        let edge_list = quantum_validation::caterpillar_of_cliques(node_count, window, max_edges);
        let nodes_bv = bounded::<_, T::MaxNodes>(nodes.clone());
        let edges_bv = bounded::<_, T::MaxEdges>(edge_list);
        let topology_hash = crate::topology::hash_topology(
            &nodes_bv,
            &edges_bv,
            &allowed_h_set::<T>().as_slice(),
            &allowed_j_set::<T>().as_slice(),
            &allowed_spin_set::<T>().as_slice(),
        );
        assert!(QuantumPow::<T>::register_topology(
            RawOrigin::Root.into(),
            nodes_bv,
            edges_bv,
            allowed_h_set::<T>(),
            allowed_j_set::<T>(),
            allowed_spin_set::<T>(),
            types::TopologyHardness {
                // Both above the ceiling, so the challenge has a claim to
                // falsify and the topology is not already exact.
                residual_difficulty: u32::MAX,
                core_width: u32::MAX,
                expected_frustration_milli: 0,
            },
        )
        .is_ok());

        #[extrinsic_call]
        QuantumPow::prove_topology_exact(
            RawOrigin::Signed(caller),
            topology_hash,
            bounded::<_, T::MaxNodes>(nodes),
        );

        // Assert the benchmark HIT its worst case, not merely that the call
        // succeeded. `residual_difficulty <= ceiling` holds by construction, so
        // it cannot fail and would certify a weight from a degenerate graph
        // eliminating at width 1. Equality means the caterpillar really did
        // saturate every bag — the `nodes * ceiling^2` shape being priced.
        //
        // Compared against what the graph can REACH, not the ceiling outright:
        // a caterpillar of `node_count` vertices cannot exceed `node_count - 1`,
        // so a config with `MaxNodes < ExactSolveCeiling + 1` would fail an
        // equality against the ceiling for a non-degeneracy reason. Bounded by
        // three real things: the replay's abort ceiling, the vertices available
        // to form a bag, and the window the edge budget allowed.
        let attainable = T::ExactSolveCeiling::get()
            .min(node_count.saturating_sub(1) as u32)
            .min(window as u32);
        assert_eq!(
            TopologyHardnessOf::<T>::get(topology_hash)
                .expect("record registered")
                .residual_difficulty,
            attainable,
            "benchmark must saturate every bag it can or it prices the wrong work"
        );
    }

    #[benchmark]
    fn set_topology_hardness() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();

        #[extrinsic_call]
        QuantumPow::set_topology_hardness(
            RawOrigin::Root,
            topology_hash,
            types::TopologyHardness {
                residual_difficulty: 1,
                core_width: 1,
                expected_frustration_milli: 500,
            },
        );

        assert_eq!(
            TopologyHardnessOf::<T>::get(topology_hash)
                .expect("record set")
                .residual_difficulty,
            1
        );
    }

    #[benchmark]
    fn add_mineable_topology() {
        // First registration becomes the auto-whitelisted default. Adding a
        // distinct, non-default topology exercises the worst case: the
        // single-active-topology guard scans the (default-only) whitelist
        // before inserting.
        let _ = register_topology_for::<T>();
        let topology_hash = register_extra_topology_for::<T>(10_000);

        #[extrinsic_call]
        QuantumPow::add_mineable_topology(RawOrigin::Root, topology_hash);

        assert!(MineableTopologies::<T>::contains_key(topology_hash));
    }

    #[benchmark]
    fn remove_mineable_topology() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        MineableTopologies::<T>::insert(topology_hash, ());
        // Clear the first-registration default so the remove is not blocked
        // by the default-topology guard.
        DefaultTopology::<T>::kill();

        #[extrinsic_call]
        QuantumPow::remove_mineable_topology(RawOrigin::Root, topology_hash);

        assert!(!MineableTopologies::<T>::contains_key(topology_hash));
    }

    /// `prove_topology_planar` had NO benchmark. `PROVE_PLANAR_K1_EDGE` was
    /// measured only by `quantum-validation`'s `planar_witness_cost`
    /// characterization test, and `PROVE_PLANAR_K2_NODE` never at all. The
    /// extrinsic is permissionless, so an unbacked weight is the block-stuffing
    /// exposure the exact witness's own history records.
    ///
    /// Fixture is an `n`-cycle: `n` nodes, `n` edges, genus zero
    /// (`n - n + 2 = 2`), every vertex of degree two — so ANY order of a
    /// vertex's two incident edges is a valid cyclic order and the rotation is
    /// trivial to build at every sweep point. The h spec is zero-only because
    /// the extrinsic refuses a field-bearing topology.
    ///
    /// KNOWN LIMITATION: `m == n`, so the sweep is COLLINEAR and fits the SUM of
    /// `PROVE_PLANAR_K1_EDGE` and `PROVE_PLANAR_K2_NODE`, not each. They cannot
    /// simply be swept independently: a planar simple graph admits at most
    /// `3n - 6` edges, and a valid ROTATION SYSTEM for an arbitrary `(n, m)`
    /// planar graph requires computing the embedding, which is the thing being
    /// verified. A triangulated strip would give `m ≈ 2n - 3`, a second
    /// non-parallel point — still not a free parameter. Until then treat the two
    /// as one lumped per-dart term; the face trace is per-dart (`2m`) anyway.
    /// Tracked separately.
    #[benchmark]
    fn prove_topology_planar(n: Linear<{ T::MinNodes::get() }, { T::MaxNodes::get() }>) {
        let caller: T::AccountId = whitelisted_caller();
        // `MinNodes` is DECLARED, not clamped in the body. Registration refuses
        // anything smaller, so `Linear<4, MaxNodes>` plus a body
        // `n.max(MinNodes)` made the first twelve runtime points measure an
        // identical 16-node cycle at rising declared `n` — a flat prefix that
        // drags the per-node slope down. No complete-cover floor here: an
        // `n`-cycle carries exactly `n` edges, never the simple-graph cap. The
        // clamp is a guard only.
        let n = n.max(T::MinNodes::get());
        let node_count = (n as usize).min(T::MaxEdges::get() as usize).max(3);
        let nodes: Vec<u32> = (0..node_count as u32).collect();
        // Edge `i` is `(i, i+1)`; the last closes the cycle.
        let mut edge_list: Vec<(u32, u32)> =
            (0..node_count as u32 - 1).map(|i| (i, i + 1)).collect();
        edge_list.push((0, node_count as u32 - 1));

        let nodes_bv = bounded::<_, T::MaxNodes>(nodes);
        let edges_bv = bounded::<_, T::MaxEdges>(edge_list);
        let zero_h = AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![0]));
        let topology_hash = crate::topology::hash_topology(
            &nodes_bv,
            &edges_bv,
            &zero_h.as_slice(),
            &allowed_j_set::<T>().as_slice(),
            &allowed_spin_set::<T>().as_slice(),
        );
        assert!(QuantumPow::<T>::register_topology(
            RawOrigin::Root.into(),
            nodes_bv,
            edges_bv,
            zero_h,
            allowed_j_set::<T>(),
            allowed_spin_set::<T>(),
            types::TopologyHardness {
                // Above the ceiling, so the challenge has something to claim.
                residual_difficulty: 64,
                core_width: 64,
                expected_frustration_milli: 500,
            },
        )
        .is_ok());

        // Rotation built AFTER registration: `register_topology` canonicalizes
        // the edge list and the rotation indexes edge POSITIONS in the stored
        // order. Building it against submission order is the mistake the v6
        // migration note warns about.
        let stored = RegisteredTopologies::<T>::get(topology_hash).expect("just registered");
        let mut incident: Vec<Vec<u32>> = vec![Vec::new(); node_count];
        for (idx, &(u, v)) in stored.edges.iter().enumerate() {
            incident[u as usize].push(idx as u32);
            incident[v as usize].push(idx as u32);
        }
        let rotation: BoundedVec<BoundedVec<u32, T::MaxEdges>, T::MaxNodes> =
            bounded::<_, T::MaxNodes>(
                incident
                    .into_iter()
                    .map(bounded::<_, T::MaxEdges>)
                    .collect::<Vec<_>>(),
            );

        #[extrinsic_call]
        QuantumPow::prove_topology_planar(RawOrigin::Signed(caller), topology_hash, rotation);

        // The witness must have been ACCEPTED, or the sweep prices a
        // rejection path and the per-edge constant means nothing.
        let hardness = TopologyHardnessOf::<T>::get(topology_hash).expect("still classified");
        assert!(
            hardness.core_width <= T::ExactSolveCeiling::get(),
            "the planar witness was not accepted, so this run does not price \
             the tracing work"
        );
    }

    impl_benchmark_test_suite!(QuantumPow, crate::mock::new_test_ext(), crate::mock::Test);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bisection `complete_cover_nodes` replaced, verbatim apart from a `hi`
    /// bracket widened past any `MaxNodes`, so the two compare as pure functions
    /// of `max_edges` rather than through a clamp.
    fn bisect_complete_cover_nodes(max_edges: u32) -> u32 {
        let mut lo = 3u64;
        let mut hi = 1_000_000u64;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if mid * (mid - 1) / 2 >= u64::from(max_edges) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo as u32
    }

    /// Agreement across the whole range, not at the endpoints.
    ///
    /// Endpoints are what the closed form is least likely to get wrong; every
    /// interesting disagreement is interior, at a perfect square of `1 + 8e` or
    /// one either side. `0..300_000` covers any plausible `MaxEdges` (the
    /// runtime's is 50_000) with an order of magnitude to spare.
    #[test]
    fn complete_cover_nodes_matches_bisection() {
        for e in 0u32..300_000 {
            assert_eq!(
                complete_cover_nodes(e).max(3),
                bisect_complete_cover_nodes(e),
                "closed form disagrees with the bisection at max_edges = {e}"
            );
        }
    }

    /// The DEFINING property, independent of the bisection: `n` covers `e` and
    /// `n - 1` does not, so `n` really is the least such node count.
    #[test]
    fn complete_cover_nodes_is_the_least_covering_n() {
        for e in 0u32..300_000 {
            let n = u64::from(complete_cover_nodes(e).max(3));
            assert!(
                n * (n - 1) / 2 >= u64::from(e),
                "n = {n} does not cover max_edges = {e}"
            );
            if n > 3 {
                let below = n - 1;
                assert!(
                    below * (below - 1) / 2 < u64::from(e),
                    "n = {n} is not minimal for max_edges = {e}"
                );
            }
        }
    }

    /// The ranges the benchmarks emit under the mock, spelled out so a change to
    /// `MaxNodes`/`MaxEdges`/`MinNodes` that inverts or empties a sweep fails
    /// here rather than inside a benchmark run.
    #[test]
    fn mock_sweep_ranges_are_usable() {
        use crate::mock::Test;

        let max_nodes = <Test as Config>::MaxNodes::get();
        let max_edges = <Test as Config>::MaxEdges::get();
        assert_eq!((max_nodes, max_edges), (16, 32), "mock constants moved");
        // MinNodes is 2, the complete-cover floor for MaxEdges = 32 is 9.
        assert_eq!(min_swept_nodes::<Test>(), 9);
        assert_eq!(min_swept_edges::<Test>(), 20);
        assert!(
            min_swept_nodes::<Test>() < max_nodes,
            "node sweep collapsed"
        );
        assert!(
            min_swept_edges::<Test>() < max_edges,
            "edge sweep collapsed"
        );
        // The independence the `n` floor buys, at the bottom of the node sweep:
        // the simple-graph cap is already the constant MaxEdges.
        let n = u64::from(min_swept_nodes::<Test>());
        assert_eq!(
            (n * (n - 1) / 2).min(u64::from(max_edges)) as u32,
            max_edges
        );
    }

    /// `prove_topology_exact`'s declared range must match what its body does.
    ///
    /// The body floors `node_count` at `ceiling + 1` and caps it at `MaxNodes`.
    /// `Linear<8, MaxNodes>` against that was entirely flat in the mock
    /// (`min(21, 16) == 16` at every step) and ~4 flat points of 50 on the
    /// runtime. FRAME regresses against the DECLARED value, so a flat prefix is
    /// an undercharge.
    #[test]
    fn the_exact_sweep_declares_the_floor_its_body_enforces() {
        use crate::mock::Test;

        let ceiling = <Test as Config>::ExactSolveCeiling::get();
        let max_nodes = <Test as Config>::MaxNodes::get();
        assert_eq!((ceiling, max_nodes), (20, 16), "mock constants moved");
        // Degenerate here, and honestly so: the ceiling exceeds MaxNodes.
        assert_eq!(min_exact_nodes::<Test>(), 16);

        // The declared floor must equal what the body computes at the bottom of
        // the range. That equality is the whole fix.
        for n in [min_exact_nodes::<Test>(), max_nodes] {
            let body = (n as usize)
                .max(ceiling as usize + 1)
                .min(max_nodes as usize);
            assert_eq!(
                body, n as usize,
                "declared n={n} is not what the body uses ({body}); the sweep \
                 is flat there and the fitted node slope comes out too shallow"
            );
        }
    }
}

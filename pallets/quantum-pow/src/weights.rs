#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use core::marker::PhantomData;
use frame_support::{traits::Get, weights::{constants::RocksDbWeight, Weight}};

use crate::benchmark_weights::{self, WeightInfo as BenchmarkWeightInfo};

pub trait WeightInfo {
	fn register_miner() -> Weight;
	fn deregister_miner() -> Weight;
	/// Weight for `register_topology` over `(nodes, edges, allowed_values)`.
	///
	/// Root-only, but dimensioned anyway: `validate_topology_consistency` is
	/// `O(n^2)` in the node scan plus `O(m*n)` resolving edge endpoints, so a
	/// flat constant under-charged by orders of magnitude at `MaxNodes`-scale
	/// topologies. The reference-machine benchmark sweeps all three
	/// dimensions.
	fn register_topology(n: u32, e: u32, a: u32) -> Weight;
	fn set_default_topology() -> Weight;
	fn set_difficulty() -> Weight;
	fn set_topology_curve() -> Weight;
	/// Weight for `submit_proof`: `W(n, e) = BASE + Kₙ·n + Kₑ·e`. There is no
	/// `solutions` dimension — a proof carries exactly one configuration by
	/// type. See the constants below for the measurements behind Kₙ/Kₑ.
	fn submit_proof(nodes: u32, edges: u32) -> Weight;
	/// Weight for `prove_topology_exact`, which replays an elimination-order
	/// witness. Aborts as soon as a bag exceeds the ceiling, so the work is
	/// `O(nodes * ceiling^2)` set operations plus one pass over the edges —
	/// permissionless, so it must be charged, not assumed.
	fn prove_topology_exact(nodes: u32, edges: u32) -> Weight;
	/// Weight for `prove_topology_planar`: one pass to index the rotation
	/// system and one face trace, both `O(m)`.
	fn prove_topology_planar(nodes: u32, edges: u32) -> Weight;
	/// Weight for `set_topology_hardness`: one registered-topology read, one
	/// existing-record read for the no-widening check, one write.
	fn set_topology_hardness() -> Weight;
	fn add_mineable_topology() -> Weight;
	fn remove_mineable_topology() -> Weight;
}

// Measured and validated 2026-07-31. The constants below are hand-derived upper bounds from
// complexity analysis, not frame-benchmarking output, and benchmarking says they are adequate
// -- they exceed the measured median at every sampled point with 1.2x-2.0x margin. Not
// re-derived, because the measurement found nothing to correct. Re-deriving them is riff-3rp;
// unlike the six constants they replaced, both are now identifiable, so a sweep can fit them.
//
// frame-benchmarking-cli, release node, wasm-execution: Compiled, i7-10710U. Directly
// measured at pinned (n, e) corners (25-30 repeats each, medians over 700+ samples at the
// worst case), NOT read off a fitted model -- see the warning below:
//
//     (n, e)            median    max     charged   charged/median
//     (5_000, 50_000)   5.44 ms   11.94   10.64 ms      1.96x
//     (2_500, 25_000)   3.92 ms    9.78    5.33 ms      1.36x
//     (317, 5_004)      0.88 ms    4.57    1.07 ms      1.22x
//
// DO NOT TRUST THE FITTED MODEL HERE; MEASURE POINTS. A full 50-step sweep fits
// `4_528_343_520 + 251_829 * e` and drops the `n` slope entirely. That model predicts
// 17.12 ms at (5_000, 50_000), where direct measurement gives 5.44 ms -- it over-predicts
// its own worst case by 3.1x. Two reasons it misleads: frame-benchmarking-cli pins every
// other component at its maximum while sweeping one, and `e` is swept over
// Linear<5_004, 50_000>, so the intercept is an extrapolation to a point outside the data.
//
// The model is also misspecified, which is why single sweeps disagree. Sweeping each
// dimension with the other pinned gives Kn = 45_411 ps/node and Ke = 95_300 ps/edge (both
// R^2 ~ 0.98), but solving for BASE from the two intercepts yields 56_124_085 ps one way and
// 84_125_548 ps the other -- a 33% spread between two good fits, i.e. the shape is wrong,
// not the data. Measuring Ke at both pinned n values shows why:
//
//     Ke(n =   317) =  95_300 ps/edge
//     Ke(n = 5_000) = 251_829 ps/edge      2.64x apart while n moves 15.8x
//
// There is a real sublinear n-e interaction that `BASE + Kn*n + Ke*e` cannot express.
// Sweeping n with e pinned at maximum returns R^2 = 0.037 -- n has essentially no effect
// once the edge work dominates. So the per-node term is not separately identifiable from
// this extrinsic at production bounds, and pretending otherwise would be fitting noise.
//
// If you change these constants, validate the same way -- pin (n, e) at corners with
// --low/--high and compare against measured medians -- rather than transferring a fitted
// intercept. Note `--low == --high` panics the median-slopes analyser (`index out of
// bounds`, analysis.rs:286) because a zero-variance component has no slope; pass
// --no-median-slopes and read --json-file.

// QIP-03 dimension-scaled submit_proof weight:
//
// W(n, e) = BASE + Kn*n + Ke*e
//
// - BASE = 50_000_000 -- extrinsic overhead (charged DB weight is added on top), sized to
//   the 166 us steady-state floor recorded at the smallest admissible topology
// - Kn = 6_000   per node: topology traversal, Ising h-term sampling, spin validation
// - Ke = 500_000 per edge: topology traversal, Ising J-term sampling, the energy
//   calculation, and the two per-proof instance passes (`frustration_index`, a parity
//   union-find over the couplings, and `energy_bound_milli`, one linear sum). Sized at
//   ~2x the fastest steady-state per-edge cost the sweep fixture recorded; the earlier
//   4x-margined native micro-measurement (~40 ns/edge) missed the sampling and decode
//   work that the end-to-end sweep prices in.
//
// Worst case at production bounds (n = 5_000, e = 50_000):
// 50M + 30M + 25_000M = 25_080M ref_time ~ 25 ms (1 ref_time unit = 1 ps), plus ~625M of
// charged DB weight (9 reads + 4 writes at RocksDb costs). Eight proofs
// (MaxProofsPerBlock) ~ 200 ms of the 2 s block ref_time budget.
//
// Two terms, not six. This was BASE + k1*n + k2*e + k3*s*n + k4*s*e + k5*s^2*n. With `s`
// forced to 1 the six constants collapsed to two identifiable sums, so no benchmark could
// ever separate them. Kn is still that collapse (k1 + k3 + k5 = 6_000). Ke and BASE are
// NOT: the collapsed Ke (k2 + k4 + k6 = 212_000, i.e. 0.212 us/edge) was measured by the
// `run-quantum-pow-sweeps.sh` fixed points to UNDERCHARGE the real WASM verifier — the
// steady-state floor alone was 0.24 us/edge at e = 50_000 and 0.36 us/edge at e = 5_004
// (arm64 native host; the x86 reference machine only runs the WASM slower). The recorded
// fixture in `testdata/submit-proof-sweeps.tsv` gates against those floors, and
// 0.212 us/edge failed it. Recalibrated to 0.5 us/edge — roughly 2x the fastest observed
// steady per-edge cost — and BASE to 50 us, covering the 166 us floor measured at the
// smallest admissible topology (317 nodes / 321 edges) with the per-edge term.
//
// The four solution-scaled terms were not decoration: they priced a rejected
// multi-configuration SCALE decode (up to `MaxSolutions` = 32) on the unpaid pool-validation
// path. Deleting them alone would have reopened that undercharge; the type change removed the
// decode instead. See `QuantumProof::solutions`.
//
// If multi-configuration submission ever returns, the solution-scaled terms come back
// together with a sweep that can fit them -- not before.
const SUBMIT_PROOF_BASE_REF_TIME: u64 = 50_000_000;
const SUBMIT_PROOF_K_NODE: u64 = 6_000;
const SUBMIT_PROOF_K_EDGE: u64 = 500_000;

// Distinct storage items on the submit_proof accept path. Reads:
// RegisteredTopologies (overlay-cached), Miners, BlockProofCount,
// MineableTopologies, LastProofBlockHash, Difficulties, LastProofBlock,
// TopologyCurveC, BlockBestProof. Writes: Miners, BlockBestProof,
// BlockProofCount; 4 is a conservative carry-over from the flat weight.
const SUBMIT_PROOF_READS: u64 = 9;
const SUBMIT_PROOF_WRITES: u64 = 4;

// Explicit per-allowed-value charge retained on top of the benchmarked
// register_topology weight, so the allowed-set dimension keeps a visible
// slope even when the regression's fitted slope sits under its noise floor.
const REGISTER_TOPOLOGY_ALLOWED_VALUE: u64 = 1_000_000;

/// Per-node cost of replaying an elimination order: up to `ceiling^2 / 2`
/// ordered-set operations per node, with the runtime ceiling folded in.
///
/// MEASURED. `quantum-validation`'s `elimination_witness_cost` drives a
/// ceiling-saturating order (every bag at `ExactSolveCeiling = 20`, so the
/// early abort never fires) over n = 4800: **19.87 us/node** natively in
/// release, and the permissionless call carries a 4x WASM margin over that. An
/// earlier hand-guessed 0.4 us/node under-charged by ~50x — ~1.9 ms of ref_time
/// for ~95 ms of work, a block-stuffing vector rather than a rounding error.
const PROVE_EXACT_K1_NODE: u64 = 80_000_000;
/// Per-edge cost of seeding the rank-keyed adjacency.
const PROVE_EXACT_K2_EDGE: u64 = 2_000;

/// Reads: RegisteredTopologies, TopologyHardnessOf, DefaultTopology,
/// MineableTopologies. Writes (worst case): TopologyHardnessOf,
/// MineableTopologies.
const PROVE_EXACT_READS: u64 = 4;
const PROVE_EXACT_WRITES: u64 = 2;

/// Per-edge cost of tracing a rotation system: two darts per edge, each
/// visited exactly once, plus building the slot index.
///
/// MEASURED. `quantum-validation`'s `planar_witness_cost` traces a 60x80 grid
/// (n = 4800, m = 9460) and reports **0.319 us/edge** natively in release on
/// the REJECTING path — which still pays the full index build and face trace,
/// so it is the worst case. The charge carries a ~6x WASM margin.
const PROVE_PLANAR_K1_EDGE: u64 = 2_000_000;

/// Per-node cost: the rotation-list length check and the component
/// union-find, both linear in nodes.
const PROVE_PLANAR_K2_NODE: u64 = 400_000;

fn prove_topology_planar_dimension_weight(nodes: u32, edges: u32) -> Weight {
	// The node term takes the NODE constant. It once reused `PROVE_EXACT_K2_EDGE`
	// — a per-edge figure applied to the node count, which bounds nothing.
	Weight::from_parts(10_000_000, 0)
		.saturating_add(Weight::from_parts(PROVE_PLANAR_K2_NODE, 0).saturating_mul(nodes.into()))
		.saturating_add(Weight::from_parts(PROVE_PLANAR_K1_EDGE, 0).saturating_mul(edges.into()))
}

/// Dimension-dependent portion of `prove_topology_exact`'s weight, shared by
/// both `WeightInfo` impls so the formula cannot diverge between the runtime
/// and the mock.
fn prove_topology_exact_dimension_weight(nodes: u32, edges: u32) -> Weight {
	Weight::from_parts(10_000_000, 0)
		.saturating_add(Weight::from_parts(PROVE_EXACT_K1_NODE, 0).saturating_mul(nodes.into()))
		.saturating_add(Weight::from_parts(PROVE_EXACT_K2_EDGE, 0).saturating_mul(edges.into()))
}

/// Dimension-dependent portion of `submit_proof`'s weight (BASE plus both
/// dimension terms), shared by both `WeightInfo` impls so the formula cannot diverge
/// between the runtime and the mock/native fallback (`()`).
fn submit_proof_dimension_weight(nodes: u32, edges: u32) -> Weight {
	let node_cost = Weight::from_parts(SUBMIT_PROOF_K_NODE, 0).saturating_mul(nodes.into());
	let edge_cost = Weight::from_parts(SUBMIT_PROOF_K_EDGE, 0).saturating_mul(edges.into());

	Weight::from_parts(SUBMIT_PROOF_BASE_REF_TIME, 0)
		.saturating_add(node_cost)
		.saturating_add(edge_cost)
}

pub struct SubstrateWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for SubstrateWeight<T> {
	fn register_miner() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::register_miner()
	}

	fn deregister_miner() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::deregister_miner()
	}

	fn register_topology(n: u32, e: u32, a: u32) -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::register_topology(
			n,
			e,
			a,
		)
		// Explicitly retain the allowed-set dimension even when its small
		// local slope falls below the ordinary regression's noise floor.
		.saturating_add(
			Weight::from_parts(REGISTER_TOPOLOGY_ALLOWED_VALUE, 0).saturating_mul(a.into()),
		)
	}

	fn set_default_topology() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::set_default_topology()
	}

	fn set_difficulty() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::set_difficulty()
	}

	fn set_topology_curve() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::set_topology_curve()
	}

	fn submit_proof(nodes: u32, edges: u32) -> Weight {
		// See submit_proof_dimension_weight for the QIP-03 formula and constants.
		submit_proof_dimension_weight(nodes, edges)
			.saturating_add(T::DbWeight::get().reads(SUBMIT_PROOF_READS))
			.saturating_add(T::DbWeight::get().writes(SUBMIT_PROOF_WRITES))
	}

	fn prove_topology_exact(nodes: u32, edges: u32) -> Weight {
		prove_topology_exact_dimension_weight(nodes, edges)
			.saturating_add(T::DbWeight::get().reads(PROVE_EXACT_READS))
			.saturating_add(T::DbWeight::get().writes(PROVE_EXACT_WRITES))
	}

	fn prove_topology_planar(nodes: u32, edges: u32) -> Weight {
		prove_topology_planar_dimension_weight(nodes, edges)
			.saturating_add(T::DbWeight::get().reads(PROVE_EXACT_READS))
			.saturating_add(T::DbWeight::get().writes(PROVE_EXACT_WRITES))
	}

	fn set_topology_hardness() -> Weight {
		Weight::from_parts(10_000_000, 0)
			.saturating_add(T::DbWeight::get().reads(2_u64))
			.saturating_add(T::DbWeight::get().writes(1_u64))
	}

	fn add_mineable_topology() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::add_mineable_topology()
	}

	fn remove_mineable_topology() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::remove_mineable_topology()
	}
}

impl WeightInfo for () {
	fn register_miner() -> Weight {
		<() as BenchmarkWeightInfo>::register_miner()
	}

	fn deregister_miner() -> Weight {
		<() as BenchmarkWeightInfo>::deregister_miner()
	}

	fn register_topology(n: u32, e: u32, a: u32) -> Weight {
		<() as BenchmarkWeightInfo>::register_topology(n, e, a)
			.saturating_add(
				Weight::from_parts(REGISTER_TOPOLOGY_ALLOWED_VALUE, 0).saturating_mul(a.into()),
			)
	}

	fn set_default_topology() -> Weight {
		<() as BenchmarkWeightInfo>::set_default_topology()
	}

	fn set_difficulty() -> Weight {
		<() as BenchmarkWeightInfo>::set_difficulty()
	}

	fn set_topology_curve() -> Weight {
		<() as BenchmarkWeightInfo>::set_topology_curve()
	}

	fn submit_proof(nodes: u32, edges: u32) -> Weight {
		// Same formula as SubstrateWeight, with RocksDbWeight DB costs.
		submit_proof_dimension_weight(nodes, edges)
			.saturating_add(RocksDbWeight::get().reads(SUBMIT_PROOF_READS))
			.saturating_add(RocksDbWeight::get().writes(SUBMIT_PROOF_WRITES))
	}

	fn prove_topology_exact(nodes: u32, edges: u32) -> Weight {
		prove_topology_exact_dimension_weight(nodes, edges)
			.saturating_add(RocksDbWeight::get().reads(PROVE_EXACT_READS))
			.saturating_add(RocksDbWeight::get().writes(PROVE_EXACT_WRITES))
	}

	fn prove_topology_planar(nodes: u32, edges: u32) -> Weight {
		prove_topology_planar_dimension_weight(nodes, edges)
			.saturating_add(RocksDbWeight::get().reads(PROVE_EXACT_READS))
			.saturating_add(RocksDbWeight::get().writes(PROVE_EXACT_WRITES))
	}

	fn set_topology_hardness() -> Weight {
		Weight::from_parts(10_000_000, 0)
			.saturating_add(RocksDbWeight::get().reads(2_u64))
			.saturating_add(RocksDbWeight::get().writes(1_u64))
	}

	fn add_mineable_topology() -> Weight {
		<() as BenchmarkWeightInfo>::add_mineable_topology()
	}

	fn remove_mineable_topology() -> Weight {
		<() as BenchmarkWeightInfo>::remove_mineable_topology()
	}
}

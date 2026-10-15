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

// QIP-03 dimension-scaled submit_proof weight:
//
// W(n, e) = BASE + Kn*n + Ke*e
//
// - BASE = 50_000_000 -- extrinsic overhead (charged DB weight is added on top).
// - Kn = 6_000   per node: topology traversal, Ising h-term sampling, spin validation.
// - Ke = 90_000  per edge: topology traversal, Ising J-term sampling, the energy
//   calculation, and the two per-proof instance passes (`frustration_index`, a parity
//   union-find over the couplings, and `energy_bound_milli`, one linear sum).
//
// Fitted to the reference-machine sweep in `testdata/submit-proof-sweeps.tsv`
// (`benchmark-quantum-pow-sweeps` on node02, see its provenance lines). Charged weight,
// DB included, over each recorded MAXIMUM:
//
//     point        (n, e)            max       charged   ratio
//     minimum      (317, 321)        0.10 ms   0.76 ms   7.4x   (base/DB dominated)
//     nodes        (5_000, 5_004)    0.64 ms   1.21 ms   1.9x   (base/DB dominated)
//     edges        (317, 50_000)     3.76 ms   5.23 ms   1.39x
//     worst_case   (5_000, 50_000)   4.06 ms   5.26 ms   1.30x
//
// Worst case at production bounds: 50M + 30M + 4_500M = 4.58 ms of ref_time (1 unit = 1
// ps) plus ~0.68 ms of charged DB weight. Eight proofs (MaxProofsPerBlock) ~ 42 ms, about
// 2% of the 2 s block ref_time budget.
//
// `weight_covers_recorded_sweeps_with_ten_percent_floor` requires >= 1.1x at every point
// and <= 1.5x where Kn/Ke dominate (edges, worst_case); the CI job re-checks the 10%
// floor on every reference run. The earlier Ke = 500_000 was sized on a laptop arm64
// fixture (~0.18 us/edge); the reference machine measures ~0.07 us/edge, so it
// overcharged ~6x.
//
// Provenance. The model was BASE + k1*n + k2*e + k3*s*n + k4*s*e + k5*s^2*n + k6*n*e
// with up to `MaxSolutions` configurations per proof. A proof now carries exactly one
// configuration by type (`QuantumProof::solutions`), so the solution-scaled terms price
// nothing; they also priced a rejected multi-configuration SCALE decode on the unpaid
// pool-validation path, which the type change removed rather than repriced.
//
// Do not transfer a fitted frame-benchmarking intercept here. Sweeping one component
// pins the others at their maximum and extrapolates the intercept outside the data, and
// there is a sublinear n-e interaction `BASE + Kn*n + Ke*e` cannot express. Validate by
// pinning (n, e) at corners and comparing against measured maxima.
const SUBMIT_PROOF_BASE_REF_TIME: u64 = 50_000_000;
const SUBMIT_PROOF_K_NODE: u64 = 6_000;
const SUBMIT_PROOF_K_EDGE: u64 = 90_000;

// Distinct storage items on the submit_proof accept path. Reads:
// TopologyDims (weight closure), RegisteredTopologies (overlay-cached), Miners,
// BlockProofCount, MineableTopologies, TopologyHardnessOf, LastProofBlockHash,
// RecentRoundSeeds, CountedProofKeys, PendingUncles, SharedDifficultyOf,
// LastProofBlock, DifficultySetAt, TopologyCurveC, BlockBestProof. Writes:
// Miners, BlockBestProof, PendingUncles, BlockProofCount; 5 is a conservative
// carry-over from the flat weight.
const SUBMIT_PROOF_READS: u64 = 15;
const SUBMIT_PROOF_WRITES: u64 = 5;

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

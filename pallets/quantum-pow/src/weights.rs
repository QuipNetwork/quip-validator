#![cfg_attr(rustfmt, rustfmt_skip)]
#![allow(unused_parens)]
#![allow(unused_imports)]

use core::marker::PhantomData;
use frame_support::{traits::Get, weights::{constants::RocksDbWeight, Weight}};

use crate::benchmark_weights::{self, WeightInfo as BenchmarkWeightInfo};

pub trait WeightInfo {
	fn register_miner() -> Weight;
	fn deregister_miner() -> Weight;
	fn register_topology(n: u32, e: u32, a: u32) -> Weight;
	fn set_default_topology() -> Weight;
	fn set_difficulty() -> Weight;
	fn set_topology_curve() -> Weight;
	fn set_topology_energy_band() -> Weight;
	/// Calculate weight for `submit_proof` from its nonlinear dimensions.
	///
	/// `n` is the registered topology's node count, `e` its edge count, and
	/// `s` the number of submitted solutions.
	fn submit_proof(n: u32, e: u32, s: u32) -> Weight;
	/// The admission cost that `register_topology` adds for a family
	/// descriptor, measured on the slowest family at the largest admitted
	/// lattice.
	fn register_family_topology() -> Weight;
	/// A family proof of `s` witnesses on an ice-class topology, measured
	/// at the largest admitted lattice. See [`FamilyWeightClass`].
	fn submit_family_proof_ice(s: u32) -> Weight;
	/// A family proof of `s` witnesses on a routing-class topology.
	fn submit_family_proof_routing(s: u32) -> Weight;
	/// A family proof on a square-class topology, at the nonce path that
	/// measured slowest. Independent of the witness count.
	fn submit_family_proof_square() -> Weight;
	fn add_mineable_topology() -> Weight;
	fn remove_mineable_topology() -> Weight;
}

/// The weight class of a family, keyed by its id.
///
/// The verifier cost of a family proof is set by regeneration, which differs
/// by an order of magnitude between the families (`docs/family-activation.md`
/// has the measurements). Each class is benchmarked on its own worst-case
/// fixture. A family outside the three classes is charged the maximum of all
/// three, so an allowlist entry can never be cheaper than its measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FamilyWeightClass {
	/// The ice families: ice-glass, twisted-ice-dimer, charged-ice, ice-fields.
	Ice,
	/// Two-commodity routing.
	Routing,
	/// The square-lattice families: polariton-static, flow-static,
	/// defects-static.
	Square,
	/// A family id that no benchmark fixture covers.
	Unknown,
}

impl FamilyWeightClass {
	pub fn of(family_id: &[u8]) -> Self {
		match family_id {
			b"quip/ice-glass" | b"quip/twisted-ice-dimer" | b"quip/charged-ice"
			| b"quip/ice-fields" => Self::Ice,
			b"quip/two-commodity-routing" => Self::Routing,
			b"quip/polariton-static" | b"quip/flow-static" | b"quip/defects-static" => {
				Self::Square
			}
			_ => Self::Unknown,
		}
	}
}

/// The pre-dispatch weight of a family proof of `solutions` witnesses.
pub fn family_proof_weight<W: WeightInfo>(family_id: &[u8], solutions: u32) -> Weight {
	match FamilyWeightClass::of(family_id) {
		FamilyWeightClass::Ice => W::submit_family_proof_ice(solutions),
		FamilyWeightClass::Routing => W::submit_family_proof_routing(solutions),
		FamilyWeightClass::Square => W::submit_family_proof_square(),
		FamilyWeightClass::Unknown => W::submit_family_proof_ice(solutions)
			.max(W::submit_family_proof_routing(solutions))
			.max(W::submit_family_proof_square()),
	}
}

// A FRAME benchmark fixes one salt, so its regression sees the regeneration
// cost of one nonce. That cost varies with the nonce: across 24 nonces on the
// routing fixture the slowest regeneration took 2.1 times the mean, and 1.5
// times on the ice fixture (native, developer machine, 2026-11-11; see
// `docs/family-activation.md`). The family weights are scaled by this margin
// so the charge covers nonces the benchmark did not draw. Revisit it when a
// sweep over nonces on the reference machine narrows the spread.
const FAMILY_NONCE_VARIANCE_PERCENT: u64 = 150;

// The square class cannot carry the full margin: its measured weight is
// 902 ms, and 150% on top exceeds the normal-class per-extrinsic limit of
// about 1.30 s (2 s block, 75% normal ratio, minus the 10% on-initialize
// reserve and the extrinsic base weight), so no square proof would fit in a
// block. 40% keeps the charge near 1.26 s. The benchmark already measures the
// costliest path, a skipped polariton nonce on the largest admitted side, and
// the polariton spread observed natively was 1.5 times the mean.
// The runtime test `every_allowed_family_proof_fits_the_normal_extrinsic_limit`
// keeps every allowed family under the limit.
const SQUARE_NONCE_VARIANCE_PERCENT: u64 = 40;

fn with_family_nonce_margin(weight: Weight, percent: u64) -> Weight {
	weight.saturating_add(weight.saturating_mul(percent).saturating_div(100))
}

// The ordinary FRAME regression is intentionally not used as submit_proof's
// final workload model: three additive Linear<> terms cannot represent the
// measured solution×node, solution×edge, and solution²×node work. The
// generated module supplies the benchmarked fixed overhead, proof size, and
// storage accounting. The model shape was calibrated against the six-point
// node02 sweep from GitLab job 15552403591 at commit 1dfd86e. Jobs 15618730781
// and 15638128172 are independent holdouts used to size the safety buffer. The
// coefficients retain at least a 15% envelope over every recorded maximum,
// and at most 1.5x at the coefficient-dominated points;
// fresh node02 sweeps enforce a separate 10% operational floor so normal host
// variation does not consume the entire calibration margin. The base
// regenerated on 2026-09-22 measured about 3% lower than the 2026-08-04 run,
// which took the solution_edges point under 20%; k4 moved from 18,500 to
// 18,800 to restore the envelope (about 21% there, 20.7% at worst_case).
//
// W(n,e,s) = measured_base
//          + k1*n + k2*e + k3*s*n + k4*s*e + k5*s²*n + k6*n*e
//
// The n*e term is a conservative empirical proxy for the combined topology
// cost observed by that sweep. It does not imply literal O(n*e) verifier
// complexity: TopologyIndex construction and lookups use BTreeMap and are
// structurally closer to n*log(n) + e*log(n).
//
// K4 was raised from 18_500 to 19_100 on 2026-08-28. The envelope above is
// measured against `measured_base`, which comes from the generated module and
// therefore moves with every reference-machine regeneration. That run came in
// about 2% cheaper across this pallet, and the binding point -- solution_edges
// against the slowest recorded sweep, job 15552403591 -- had been clearing 20%
// by 0.03 points. A 2% move in the base was enough to breach it.
//
// The old value was not wrong; it was calibrated with no room for the ordinary
// variation of the thing it is measured against. The three recorded maxima for
// that one point span 20.0% to 26.5% of margin at identical dimensions, so the
// host noise alone is wider than the cushion the coefficient carried. K4 is the
// solution*edge lever and solution_edges is the point that isolates that
// dimension, which is why the correction lands there.
//
// The binding point is now worst_case at 21.09%, so a regeneration would have
// to come in more than a percent cheaper again to breach 20%. Restoring a
// larger cushion means revisiting K6, which dominates worst_case; that is a
// wider pricing decision than closing this breach and has not been made here.
const SUBMIT_PROOF_K1_NODE: u64 = 1_200;
const SUBMIT_PROOF_K2_EDGE: u64 = 2_400;
const SUBMIT_PROOF_K3_SOLUTION_NODE: u64 = 6_000;
const SUBMIT_PROOF_K4_SOLUTION_EDGE: u64 = 19_100;
const SUBMIT_PROOF_K5_SOLUTION_SQ_NODE: u64 = 1_200;
const SUBMIT_PROOF_K6_NODE_EDGE: u64 = 535;
const REGISTER_TOPOLOGY_ALLOWED_VALUE: u64 = 1_000_000;

fn submit_proof_dimension_weight(nodes: u32, edges: u32, solutions: u32) -> Weight {
	let node_cost = Weight::from_parts(SUBMIT_PROOF_K1_NODE, 0).saturating_mul(nodes.into());
	let edge_cost = Weight::from_parts(SUBMIT_PROOF_K2_EDGE, 0).saturating_mul(edges.into());
	let solution_node_cost = Weight::from_parts(SUBMIT_PROOF_K3_SOLUTION_NODE, 0)
		.saturating_mul(solutions.into())
		.saturating_mul(nodes.into());
	let solution_edge_cost = Weight::from_parts(SUBMIT_PROOF_K4_SOLUTION_EDGE, 0)
		.saturating_mul(solutions.into())
		.saturating_mul(edges.into());
	let solution_squared_cost = Weight::from_parts(SUBMIT_PROOF_K5_SOLUTION_SQ_NODE, 0)
		.saturating_mul(solutions.into())
		.saturating_mul(solutions.into())
		.saturating_mul(nodes.into());
	let node_edge_cost = Weight::from_parts(SUBMIT_PROOF_K6_NODE_EDGE, 0)
		.saturating_mul(nodes.into())
		.saturating_mul(edges.into());

	node_cost
		.saturating_add(edge_cost)
		.saturating_add(solution_node_cost)
		.saturating_add(solution_edge_cost)
		.saturating_add(solution_squared_cost)
		.saturating_add(node_edge_cost)
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

	fn set_topology_energy_band() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::set_topology_energy_band()
	}

	fn submit_proof(n: u32, e: u32, s: u32) -> Weight {
		// Zeroing the generated additive components retains only the measured
		// base, proof-size estimate, and dispatch-body DB counts.
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::submit_proof(0, 0, 0)
			.saturating_add(submit_proof_dimension_weight(n, e, s))
			// The benchmark records the body lookup. Weight evaluation performs
			// one additional pre-dispatch RegisteredTopologies read.
			.saturating_add(T::DbWeight::get().reads(1_u64))
	}

	fn register_family_topology() -> Weight {
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::register_family_topology()
	}

	fn submit_family_proof_ice(s: u32) -> Weight {
		with_family_nonce_margin(
			<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::submit_family_proof_ice(s),
			FAMILY_NONCE_VARIANCE_PERCENT,
		)
		// Weight evaluation performs one pre-dispatch RegisteredTopologies read.
		.saturating_add(T::DbWeight::get().reads(1_u64))
	}

	fn submit_family_proof_routing(s: u32) -> Weight {
		with_family_nonce_margin(
			<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::submit_family_proof_routing(s),
			FAMILY_NONCE_VARIANCE_PERCENT,
		)
		.saturating_add(T::DbWeight::get().reads(1_u64))
	}

	fn submit_family_proof_square() -> Weight {
		with_family_nonce_margin(
			<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::submit_family_proof_square(),
			SQUARE_NONCE_VARIANCE_PERCENT,
		)
		.saturating_add(T::DbWeight::get().reads(1_u64))
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

	fn set_topology_energy_band() -> Weight {
		<() as BenchmarkWeightInfo>::set_topology_energy_band()
	}

	fn submit_proof(n: u32, e: u32, s: u32) -> Weight {
		<() as BenchmarkWeightInfo>::submit_proof(0, 0, 0)
			.saturating_add(submit_proof_dimension_weight(n, e, s))
			.saturating_add(RocksDbWeight::get().reads(1_u64))
	}

	fn register_family_topology() -> Weight {
		<() as BenchmarkWeightInfo>::register_family_topology()
	}

	fn submit_family_proof_ice(s: u32) -> Weight {
		with_family_nonce_margin(
			<() as BenchmarkWeightInfo>::submit_family_proof_ice(s),
			FAMILY_NONCE_VARIANCE_PERCENT,
		)
			.saturating_add(RocksDbWeight::get().reads(1_u64))
	}

	fn submit_family_proof_routing(s: u32) -> Weight {
		with_family_nonce_margin(
			<() as BenchmarkWeightInfo>::submit_family_proof_routing(s),
			FAMILY_NONCE_VARIANCE_PERCENT,
		)
			.saturating_add(RocksDbWeight::get().reads(1_u64))
	}

	fn submit_family_proof_square() -> Weight {
		with_family_nonce_margin(
			<() as BenchmarkWeightInfo>::submit_family_proof_square(),
			SQUARE_NONCE_VARIANCE_PERCENT,
		)
			.saturating_add(RocksDbWeight::get().reads(1_u64))
	}

	fn add_mineable_topology() -> Weight {
		<() as BenchmarkWeightInfo>::add_mineable_topology()
	}

	fn remove_mineable_topology() -> Weight {
		<() as BenchmarkWeightInfo>::remove_mineable_topology()
	}
}

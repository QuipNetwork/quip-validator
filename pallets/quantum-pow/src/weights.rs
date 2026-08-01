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
	/// Calculate weight for `submit_proof` from its nonlinear dimensions.
	///
	/// `n` is the registered topology's node count, `e` its edge count, and
	/// `s` the number of submitted solutions.
	fn submit_proof(n: u32, e: u32, s: u32) -> Weight;
	fn add_mineable_topology() -> Weight;
	fn remove_mineable_topology() -> Weight;
}

// The ordinary FRAME regression is intentionally not used as submit_proof's
// final workload model: three additive Linear<> terms cannot represent the
// measured solution×node, solution×edge, and solution²×node work. The
// generated module supplies the benchmarked fixed overhead, proof size, and
// storage accounting. The model shape was calibrated against the six-point
// node02 sweep from GitLab job 15552403591 at commit 1dfd86e. Jobs 15618730781
// and 15638128172 are independent holdouts used to size the safety buffer. The
// coefficients retain at least a 20% envelope over every recorded maximum;
// fresh node02 sweeps enforce a separate 10% operational floor so normal host
// variation does not consume the entire calibration margin.
//
// W(n,e,s) = measured_base
//          + k1*n + k2*e + k3*s*n + k4*s*e + k5*s²*n + k6*n*e
//
// The n*e term is a conservative empirical proxy for the combined topology
// cost observed by that sweep. It does not imply literal O(n*e) verifier
// complexity: TopologyIndex construction and lookups use BTreeMap and are
// structurally closer to n*log(n) + e*log(n).
const SUBMIT_PROOF_K1_NODE: u64 = 1_200;
const SUBMIT_PROOF_K2_EDGE: u64 = 2_400;
const SUBMIT_PROOF_K3_SOLUTION_NODE: u64 = 6_000;
const SUBMIT_PROOF_K4_SOLUTION_EDGE: u64 = 18_500;
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

	fn submit_proof(n: u32, e: u32, s: u32) -> Weight {
		// Zeroing the generated additive components retains only the measured
		// base, proof-size estimate, and dispatch-body DB counts.
		<benchmark_weights::SubstrateWeight<T> as BenchmarkWeightInfo>::submit_proof(0, 0, 0)
			.saturating_add(submit_proof_dimension_weight(n, e, s))
			// The benchmark records the body lookup. Weight evaluation performs
			// one additional pre-dispatch RegisteredTopologies read.
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

	fn submit_proof(n: u32, e: u32, s: u32) -> Weight {
		<() as BenchmarkWeightInfo>::submit_proof(0, 0, 0)
			.saturating_add(submit_proof_dimension_weight(n, e, s))
			.saturating_add(RocksDbWeight::get().reads(1_u64))
	}

	fn add_mineable_topology() -> Weight {
		<() as BenchmarkWeightInfo>::add_mineable_topology()
	}

	fn remove_mineable_topology() -> Weight {
		<() as BenchmarkWeightInfo>::remove_mineable_topology()
	}
}

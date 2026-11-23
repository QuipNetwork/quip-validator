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
        min_solutions: 1,
        max_energy_milli: i64::MAX,
        min_diversity_milli: 0,
    }
}

const SCALE: MilliValue = MILLI_SCALE as MilliValue;

fn allowed_spin_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, SCALE]))
}

/// No 0: with fewer edges than nodes (`submit_proof`'s `e` below `n`), a node
/// outside every edge that drew a 0 field would be isolated, and rows differing
/// only there are one answer (W1-004 keys by the W1-012 canonical form).
/// Non-zero fields keep `valid_proof_for`'s rows pairwise distinct answers, the
/// dedupe's worst case, for every `e`.
fn allowed_h_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, SCALE]))
}

fn allowed_j_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, SCALE]))
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

    (
        allowed_set_with_len::<T>(lengths[0], -100_000),
        allowed_set_with_len::<T>(lengths[1], 0),
        allowed_set_with_len::<T>(lengths[2], 100_000),
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
    fund_account::<T>(who, benchmark_funding::<T>());
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
    )
    .is_ok());
    topology_hash
}

/// How many distinct answers `valid_proof_for` can hold: `n - 1` free spins give
/// at most `2^(n - 1)` flip classes.
fn achievable_distinct(node_count: u32, solution_count: u32) -> u32 {
    let free = node_count.saturating_sub(1);
    if free >= 32 {
        solution_count
    } else {
        solution_count.min(1u32 << free)
    }
}

fn valid_proof_for<T: Config>(
    miner: &T::AccountId,
    topology_hash: sp_core::H256,
    solution_count: u32,
) -> QuantumProofOf<T> {
    frame_system::Pallet::<T>::set_block_number(1u32.into());

    let salt = [7u8; 32];
    let last_proof_block_hash_bytes = LastProofBlockHash::<T>::get().0;
    let miner_bytes = QuantumPow::<T>::account_to_bytes(miner);
    let nonce = derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &salt);

    let topology =
        RegisteredTopologies::<T>::get(topology_hash).expect("benchmark topology is registered");
    let spin_spec = allowed_spin_set::<T>();
    let solutions: PackedSolutionsOf<T> = bounded::<_, T::MaxSolutions>(
        (0..solution_count)
            .map(|solution_index| {
                // Pairwise distinct answers (W1-004), the dedupe's worst case:
                // spin 0 is +1 in every row, so no row is a global flip of
                // another, and spin i >= 1 is negated when bit i - 1 of the
                // row index is set. (The previous parity pattern made every
                // row a copy or flip of the first.)
                let spins: Vec<MilliValue> = (0..topology.nodes.len() as u32)
                    .map(|node_index| {
                        let flipped = node_index >= 1
                            && node_index <= 32
                            && (solution_index >> (node_index - 1)) & 1 == 1;
                        if flipped {
                            -SCALE
                        } else {
                            SCALE
                        }
                    })
                    .collect();
                let packed = pack_solution(&spins, &spin_spec.as_slice())
                    .expect("binary spin pack succeeds");
                bounded::<u8, T::MaxNodes>(packed)
            })
            .collect(),
    );

    types::QuantumProof {
        topology_hash,
        nonce,
        salt,
        solutions,
        device_access_time_us: 0,
    }
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
        );

        assert!(RegisteredTopologies::<T>::contains_key(topology_hash));
    }

    #[benchmark]
    fn set_default_topology() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        // Ensure the topology is on the mineable whitelist (register_topology
        // does not seed MineableTopologies; only add_mineable_topology and the
        // v3 migration do).
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

    #[benchmark]
    fn submit_proof(
        n: Linear<{ T::MinNodes::get() }, { T::MaxNodes::get() }>,
        e: Linear<1, { T::MaxEdges::get() }>,
        s: Linear<1, { T::MaxSolutions::get() }>,
    ) {
        let caller: T::AccountId = whitelisted_caller();
        register_miner_for::<T>(&caller);
        let allowed_h = allowed_h_set::<T>();
        let allowed_j = allowed_j_set::<T>();
        let allowed_spin = allowed_spin_set::<T>();
        let (topology_nodes, topology_edges, topology_hash) =
            topology_with_dimensions::<T>(0, n, e, &allowed_h, &allowed_j, &allowed_spin);
        assert!(QuantumPow::<T>::register_topology(
            RawOrigin::Root.into(),
            topology_nodes,
            topology_edges,
            allowed_h,
            allowed_j,
            allowed_spin,
        )
        .is_ok());
        MineableTopologies::<T>::insert(topology_hash, ());
        Difficulties::<T>::insert(
            topology_hash,
            types::DifficultyConfig {
                // Only distinct answers count (W1-004); cap at what n spins hold.
                min_solutions: achievable_distinct(n, s),
                ..easy_difficulty()
            },
        );
        let proof = valid_proof_for::<T>(&caller, topology_hash, s);

        #[extrinsic_call]
        QuantumPow::submit_proof(RawOrigin::Signed(caller.clone()), proof);

        let miner = Miners::<T>::get(caller).expect("miner remains registered");
        assert_eq!(miner.proofs_submitted, 1);
        assert_eq!(BlockProofCount::<T>::get(), 1);
        assert!(BlockBestProof::<T>::get().is_some());
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

    impl_benchmark_test_suite!(QuantumPow, crate::mock::new_test_ext(), crate::mock::Test);
}

/// The `submit_proof` benchmark's generator at points with e < n and s > 1.
/// Every generated row must be its own answer under the W1-012 canonical form
/// W1-004 dedupes by, or the benchmark's `min_solutions` is unreachable. With a
/// 0 in `allowed_h_set`, a node outside every edge can draw a 0 field, become
/// isolated, and fold rows together. `impl_benchmark_test_suite!` runs the
/// component extremes with one nonce; at its only folding point, (16, 1, 8) at
/// the mock's maxima, that nonce draws non-zero fields on the uncovered spins,
/// so putting 0 back (mutation M3b) failed no test before this one.
/// Benchmark-preflight samples the other points, on the runtime.
#[cfg(test)]
mod generator_tests {
    use super::*;
    use crate::mock::{new_test_ext, Test};
    use quantum_validation::{
        generate_ising_model_indexed,
        packed::{spin_signs, unpack_solution},
        AnswerCanonicalizer, TopologyIndex,
    };

    #[test]
    fn submit_proof_rows_are_pairwise_distinct_answers_at_interior_points() {
        // (n, e, s), each e < n and s > 1. Rows differ in spins 1..=log2(s),
        // and with few edges most of those spins lie outside every edge. The
        // fields come from the proof nonce, which the miner account seeds, so
        // each point is checked for several miners: one nonce can happen to
        // put every 0 field on a covered spin.
        for miner_index in 0..8u32 {
            for (offset, (n, e, s)) in [
                (16u32, 1u32, 8u32),
                (16, 4, 8),
                (8, 2, 4),
                (12, 3, 8),
                (6, 1, 4),
            ]
            .into_iter()
            .enumerate()
            {
                new_test_ext().execute_with(|| {
                    let miner: <Test as frame_system::Config>::AccountId =
                        account("miner", miner_index, 0);
                    register_miner_for::<Test>(&miner);
                    let (allowed_h, allowed_j, allowed_spin) = (
                        allowed_h_set::<Test>(),
                        allowed_j_set::<Test>(),
                        allowed_spin_set::<Test>(),
                    );
                    let (nodes, edges, topology_hash) = topology_with_dimensions::<Test>(
                        1_000 * offset as u32,
                        n,
                        e,
                        &allowed_h,
                        &allowed_j,
                        &allowed_spin,
                    );
                    assert!(QuantumPow::<Test>::register_topology(
                        RawOrigin::Root.into(),
                        nodes,
                        edges,
                        allowed_h,
                        allowed_j,
                        allowed_spin,
                    )
                    .is_ok());
                    let proof = valid_proof_for::<Test>(&miner, topology_hash, s);

                    let topology = RegisteredTopologies::<Test>::get(topology_hash).unwrap();
                    let index =
                        TopologyIndex::new(topology.nodes.as_slice(), topology.edges.as_slice())
                            .unwrap();
                    let (h, j) = generate_ising_model_indexed(
                        proof.nonce,
                        topology.nodes.as_slice(),
                        topology.edges.as_slice(),
                        &topology.allowed_h_values.as_slice(),
                        &topology.allowed_j_values.as_slice(),
                        &index,
                    )
                    .unwrap();
                    let canonicalizer = AnswerCanonicalizer::new(&h, &j, &index).unwrap();
                    let spin_spec = topology.allowed_spin_values.as_slice();
                    let mut answers: Vec<Vec<i8>> = proof
                        .solutions
                        .iter()
                        .map(|packed| {
                            let milli =
                                unpack_solution(packed.as_slice(), n as usize, &spin_spec).unwrap();
                            canonicalizer
                                .canonical(&spin_signs(&milli).unwrap())
                                .unwrap()
                        })
                        .collect();
                    answers.sort();
                    answers.dedup();
                    assert_eq!(
                        answers.len() as u32,
                        achievable_distinct(n, s),
                        "miner {miner_index}, n={n}, e={e}, s={s}: the generator's rows must be \
                     pairwise distinct answers"
                    );
                });
            }
        }
    }
}

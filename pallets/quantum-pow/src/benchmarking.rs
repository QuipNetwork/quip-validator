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

fn allowed_h_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![-SCALE, 0, SCALE]))
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
                let spins: Vec<MilliValue> = (0..topology.nodes.len() as u32)
                    .map(|node_index| {
                        if (node_index ^ solution_index).count_ones() % 2 == 0 {
                            SCALE
                        } else {
                            -SCALE
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
                min_solutions: s,
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

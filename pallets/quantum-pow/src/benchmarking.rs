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
    derive_nonce,
    families::{zephyr_lattice, AdvertisedLimits, FamilyChallenge, FamilyError, Instance},
    packed::pack_solution,
    AllowedValueSpec, MilliValue, MILLI_SCALE,
};
use sp_core::H256;
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
        None,
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
        None,
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
    let solutions = pattern_solutions::<T>(topology.nodes.len() as u32, solution_count);

    types::QuantumProof {
        topology_hash,
        nonce,
        salt,
        solutions,
        device_access_time_us: 0,
    }
}

/// `solution_count` distinct binary spin vectors of `spin_count` spins,
/// packed one bit per spin.
fn pattern_solutions<T: Config>(spin_count: u32, solution_count: u32) -> PackedSolutionsOf<T> {
    let spin_spec = allowed_spin_set::<T>();
    bounded::<_, T::MaxSolutions>(
        (0..solution_count)
            .map(|solution_index| {
                let spins: Vec<MilliValue> = (0..spin_count)
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
    )
}

// Family fixtures.
//
// Every canonical family builds on the Zephyr lattice `z(m, t)`, so the
// worst admitted instance under the pallet bounds is a full lattice. The
// verifier cost of a family proof is dominated by admission and regeneration,
// which grow with the edge count for the ice and routing families and with the
// square-lattice side `L` for the polariton, flow, and defects families. The
// fixtures below search those parameters from the top so the benchmarks run
// at the most expensive configuration the configured bounds admit. The
// measurements behind that choice are in `docs/family-activation.md`.

/// The weight classes of the family proof path. Each class is benchmarked on
/// its own worst-case fixture.
#[derive(Clone, Copy)]
enum FixtureKind {
    /// `quip/ice-glass` on the lattice with the most edges.
    Ice,
    /// `quip/two-commodity-routing` on the lattice with the most edges that
    /// has `t >= 3`. Falls back to [`Self::Ice`] when no lattice admits it.
    Routing,
    /// `quip/polariton-static` with the largest side `L` and the most charge
    /// pairs that admit. Falls back to [`Self::Ice`] when no lattice admits it.
    Square,
}

struct FamilyFixture {
    nodes: Vec<u32>,
    edges: Vec<(u32, u32)>,
    descriptor: types::FamilyDescriptor,
    // A feasible all-positive witness for the square benchmark's non-skip path.
    uncharged_descriptor: Option<types::FamilyDescriptor>,
}

/// The canonical parameter encoding: the format byte, then every schema field
/// in order as `tag` (`u16` LE), payload length (`u32` LE), payload.
fn family_params(fields: &[(u16, &[u8])]) -> Vec<u8> {
    let mut out = vec![0x01];
    for (tag, payload) in fields {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
    }
    out
}

fn family_descriptor(family_id: &[u8], params: Vec<u8>) -> types::FamilyDescriptor {
    types::FamilyDescriptor {
        family_id: bounded(family_id.to_vec()),
        revision: 1,
        params: bounded(params),
    }
}

fn ice_glass_descriptor(m: u16, t: u8) -> types::FamilyDescriptor {
    family_descriptor(
        b"quip/ice-glass",
        family_params(&[(1, &m.to_le_bytes()), (2, &[t]), (3, &200u16.to_le_bytes())]),
    )
}

fn routing_descriptor(m: u16, t: u8) -> types::FamilyDescriptor {
    // Divisor 16: the measured worst case. Divisor 4 asks for more terminal
    // pairs than the template can route and skips every nonce early.
    family_descriptor(
        b"quip/two-commodity-routing",
        family_params(&[(1, &m.to_le_bytes()), (2, &[t]), (3, &16u16.to_le_bytes())]),
    )
}

fn polariton_descriptor(m: u16, t: u8, side: u16, pairs: u16) -> types::FamilyDescriptor {
    family_descriptor(
        b"quip/polariton-static",
        family_params(&[
            (1, &m.to_le_bytes()),
            (2, &[t]),
            (3, &side.to_le_bytes()),
            (4, &pairs.to_le_bytes()),
        ]),
    )
}

fn advertised_limits<T: Config>() -> AdvertisedLimits {
    AdvertisedLimits {
        max_nodes: u64::from(T::MaxNodes::get()),
        max_edges: u64::from(T::MaxEdges::get()),
        max_degree: 0,
    }
}

fn admit<'a, T: Config>(
    descriptor: &'a types::FamilyDescriptor,
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Option<FamilyChallenge<'a>> {
    FamilyChallenge::admit(
        T::PowFamilies::get(),
        &descriptor.family_id,
        descriptor.revision,
        &descriptor.params,
        nodes,
        edges,
        &advertised_limits::<T>(),
    )
    .ok()
}

/// Every full Zephyr lattice within the pallet bounds, as `(m, t, nodes,
/// edges)`, most edges first.
fn lattices_under_bounds<T: Config>() -> Vec<(u16, u8, Vec<u32>, Vec<(u32, u32)>)> {
    let min_nodes = u64::from(T::MinNodes::get());
    let max_nodes = u64::from(T::MaxNodes::get());
    let mut lattices = Vec::new();
    for t in 1..=8u8 {
        for m in 1..=32u16 {
            // `z(m, t)` has `4 * t * m * (2m + 1)` nodes. Check the count
            // before building the lattice: the largest ones are far beyond
            // any bound and take seconds each in a debug build.
            let node_count = 4 * u64::from(t) * u64::from(m) * (2 * u64::from(m) + 1);
            if node_count > max_nodes {
                break;
            }
            if node_count < min_nodes {
                continue;
            }
            let Some((nodes, edges)) = zephyr_lattice(m, t) else {
                continue;
            };
            if edges.len() <= T::MaxEdges::get() as usize {
                lattices.push((m, t, nodes, edges));
            }
        }
    }
    lattices.sort_by_key(|lattice| core::cmp::Reverse(lattice.3.len()));
    lattices
}

/// The worst-case fixture of `kind` under the pallet bounds.
fn family_fixture<T: Config>(kind: FixtureKind) -> FamilyFixture {
    let lattices = lattices_under_bounds::<T>();
    let found = match kind {
        FixtureKind::Ice => None,
        FixtureKind::Routing => lattices.iter().find_map(|(m, t, nodes, edges)| {
            let descriptor = routing_descriptor(*m, *t);
            admit::<T>(&descriptor, nodes, edges)
                .is_some()
                .then(|| FamilyFixture {
                    nodes: nodes.clone(),
                    edges: edges.clone(),
                    descriptor,
                    uncharged_descriptor: None,
                })
        }),
        FixtureKind::Square => {
            // The side `L` that fits grows with `m`, so search the widest
            // lattices first and the largest side first within each.
            let mut by_width: Vec<_> = lattices.iter().collect();
            by_width.sort_by_key(|lattice| core::cmp::Reverse((lattice.0, lattice.1)));
            by_width.iter().find_map(|(m, t, nodes, edges)| {
                (2..=64u16).rev().find_map(|side| {
                    // Every interior vertex may carry a charge, half of them
                    // `+2`: the most pairs the family admits for this side.
                    let interior = u32::from(side.saturating_sub(2)).pow(2);
                    let pairs = u16::try_from(interior / 2).unwrap_or(u16::MAX).min(2048);
                    let descriptor = polariton_descriptor(*m, *t, side, pairs);
                    admit::<T>(&descriptor, nodes, edges)
                        .is_some()
                        .then(|| FamilyFixture {
                            nodes: nodes.clone(),
                            edges: edges.clone(),
                            descriptor,
                            uncharged_descriptor: Some(polariton_descriptor(*m, *t, side, 0)),
                        })
                })
            })
        }
    };
    found.unwrap_or_else(|| {
        lattices
            .iter()
            .find_map(|(m, t, nodes, edges)| {
                let descriptor = ice_glass_descriptor(*m, *t);
                admit::<T>(&descriptor, nodes, edges)
                    .is_some()
                    .then(|| FamilyFixture {
                        nodes: nodes.clone(),
                        edges: edges.clone(),
                        descriptor,
                        uncharged_descriptor: None,
                    })
            })
            .expect("the configured bounds admit an ice-glass lattice")
    })
}

fn zero_h_set<T: Config>() -> AllowedValueSpec<AllowedValueSetOf<T>> {
    AllowedValueSpec::Set(bounded::<_, T::MaxAllowedValues>(vec![0]))
}

/// Register the fixture as a family topology and return its hash.
fn register_family_topology_for<T: Config>(fixture: &FamilyFixture) -> H256 {
    let nodes = bounded::<_, T::MaxNodes>(fixture.nodes.clone());
    let edges = bounded::<_, T::MaxEdges>(fixture.edges.clone());
    let legacy_hash = crate::topology::hash_topology(
        &nodes,
        &edges,
        &zero_h_set::<T>().as_slice(),
        &allowed_j_set::<T>().as_slice(),
        &allowed_spin_set::<T>().as_slice(),
    );
    let challenge = admit::<T>(&fixture.descriptor, &fixture.nodes, &fixture.edges)
        .expect("the fixture was admitted when it was chosen");
    let topology_hash = crate::topology::hash_family_topology(
        legacy_hash,
        challenge.descriptor_digest(),
        challenge.topology_order_digest(),
    );
    assert!(QuantumPow::<T>::register_topology(
        RawOrigin::Root.into(),
        nodes,
        edges,
        zero_h_set::<T>(),
        allowed_j_set::<T>(),
        allowed_spin_set::<T>(),
        Some(fixture.descriptor.clone()),
    )
    .is_ok());
    assert!(RegisteredTopologies::<T>::contains_key(topology_hash));
    topology_hash
}

/// Register the fixture, make it mineable with an easy threshold that asks
/// for `min_solutions` witnesses, and register `miner`.
fn mineable_family_topology_for<T: Config>(
    fixture: &FamilyFixture,
    miner: &T::AccountId,
    min_solutions: u32,
) -> H256 {
    let topology_hash = register_family_topology_for::<T>(fixture);
    assert!(QuantumPow::<T>::set_topology_energy_band(
        RawOrigin::Root.into(),
        topology_hash,
        crate::difficulty::EnergyBand {
            hard_milli: -2_000_000,
            easy_milli: -1_000_000,
        },
    )
    .is_ok());
    Difficulties::<T>::insert(
        topology_hash,
        types::DifficultyConfig {
            min_solutions,
            ..easy_difficulty()
        },
    );
    MineableTopologies::<T>::insert(topology_hash, ());
    register_miner_for::<T>(miner);
    topology_hash
}

/// The first salt (of 256) whose nonce the family treats as `want_skip`
/// asks: an instance, or a skipped nonce. The instance comes back for the
/// non-skip case. `None` when no salt in the range behaves that way.
fn family_salt_for<T: Config>(
    challenge: &FamilyChallenge<'_>,
    miner: &T::AccountId,
    want_skip: bool,
) -> Option<([u8; 32], Option<Instance>)> {
    frame_system::Pallet::<T>::set_block_number(1u32.into());
    let last_proof_block_hash_bytes = LastProofBlockHash::<T>::get().0;
    let miner_bytes = QuantumPow::<T>::account_to_bytes(miner);
    (0u8..=255).find_map(|counter| {
        let mut salt = [0u8; 32];
        salt[0] = counter;
        let nonce = derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &salt);
        match (challenge.regenerate(nonce.to_big_endian()), want_skip) {
            (Ok(instance), false) => Some((salt, Some(instance))),
            (Err(FamilyError::Skipped(_)), true) => Some((salt, None)),
            _ => None,
        }
    })
}

fn family_proof_for<T: Config>(
    miner: &T::AccountId,
    topology_hash: H256,
    salt: [u8; 32],
    solutions: PackedSolutionsOf<T>,
) -> QuantumProofOf<T> {
    let last_proof_block_hash_bytes = LastProofBlockHash::<T>::get().0;
    let miner_bytes = QuantumPow::<T>::account_to_bytes(miner);
    types::QuantumProof {
        topology_hash,
        nonce: derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &salt),
        salt,
        solutions,
        device_access_time_us: 0,
    }
}

/// A family proof of `solution_count` witnesses on the accept path.
fn accepted_family_proof<T: Config>(
    fixture: &FamilyFixture,
    miner: &T::AccountId,
    topology_hash: H256,
    solution_count: u32,
) -> QuantumProofOf<T> {
    let challenge = admit::<T>(&fixture.descriptor, &fixture.nodes, &fixture.edges)
        .expect("the fixture was admitted when it was chosen");
    let (salt, instance) =
        family_salt_for::<T>(&challenge, miner, false).expect("one of 256 salts gives an instance");
    let instance = instance.expect("the non-skip search returns the instance");
    let codec = challenge.prepare_witness_codec(&instance).expect("codec");
    let solutions = bounded(
        (0..solution_count)
            .map(|index| {
                let spins: Vec<i8> = (0..codec.spin_count())
                    .map(|node| {
                        if (node as u32 ^ index).count_ones().is_multiple_of(2) {
                            1
                        } else {
                            -1
                        }
                    })
                    .collect();
                bounded(codec.encode(&spins).expect("soft family witness"))
            })
            .collect(),
    );
    family_proof_for::<T>(miner, topology_hash, salt, solutions)
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
            None,
        );

        assert!(RegisteredTopologies::<T>::contains_key(topology_hash));
    }

    #[benchmark]
    fn set_default_topology() {
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        // Ensure the topology is on the mineable whitelist (register_topology
        // does not seed MineableTopologies; add_mineable_topology does).
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
    fn set_topology_energy_band() {
        // The call only requires a registered family topology. Production
        // runtimes admit no family, so write the descriptor directly instead
        // of going through `register_topology` admission.
        let (_nodes, _edges, topology_hash) = register_topology_for::<T>();
        RegisteredTopologies::<T>::mutate(topology_hash, |meta| {
            if let Some(meta) = meta {
                meta.family = Some(types::FamilyDescriptor {
                    family_id: bounded(b"quip/bench".to_vec()),
                    revision: 1,
                    params: bounded(Vec::new()),
                });
            }
        });
        let band = crate::difficulty::EnergyBand {
            hard_milli: -2_000_000,
            easy_milli: -1_000_000,
        };

        #[extrinsic_call]
        QuantumPow::set_topology_energy_band(RawOrigin::Root, topology_hash, band);

        assert_eq!(TopologyEnergyBand::<T>::get(topology_hash), Some(band));
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
            None,
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

    /// Admission of the routing fixture, the family whose topology check
    /// measured slowest. Charged on top of the legacy registration weight
    /// when a descriptor is present.
    #[benchmark]
    fn register_family_topology() {
        let fixture = family_fixture::<T>(FixtureKind::Routing);
        let nodes = bounded::<_, T::MaxNodes>(fixture.nodes.clone());
        let edges = bounded::<_, T::MaxEdges>(fixture.edges.clone());

        #[extrinsic_call]
        QuantumPow::register_topology(
            RawOrigin::Root,
            nodes,
            edges,
            zero_h_set::<T>(),
            allowed_j_set::<T>(),
            allowed_spin_set::<T>(),
            Some(fixture.descriptor),
        );

        assert_eq!(RegisteredTopologies::<T>::iter_keys().count(), 1);
    }

    /// The accept path of an ice-class proof: admission, regeneration, `s`
    /// witness checks, and scoring on the ice-glass fixture.
    #[benchmark]
    fn submit_family_proof_ice(s: Linear<1, { T::MaxSolutions::get() }>) {
        let caller: T::AccountId = whitelisted_caller();
        let fixture = family_fixture::<T>(FixtureKind::Ice);
        let topology_hash = mineable_family_topology_for::<T>(&fixture, &caller, s);
        let proof = accepted_family_proof::<T>(&fixture, &caller, topology_hash, s);

        #[extrinsic_call]
        QuantumPow::submit_proof(RawOrigin::Signed(caller.clone()), proof);

        assert_eq!(BlockProofCount::<T>::get(), 1);
        assert!(BlockBestProof::<T>::get().is_some());
    }

    /// The accept path of a routing-class proof on the routing fixture.
    #[benchmark]
    fn submit_family_proof_routing(s: Linear<1, { T::MaxSolutions::get() }>) {
        let caller: T::AccountId = whitelisted_caller();
        let fixture = family_fixture::<T>(FixtureKind::Routing);
        let topology_hash = mineable_family_topology_for::<T>(&fixture, &caller, s);
        let proof = accepted_family_proof::<T>(&fixture, &caller, topology_hash, s);

        #[extrinsic_call]
        QuantumPow::submit_proof(RawOrigin::Signed(caller.clone()), proof);

        assert_eq!(BlockProofCount::<T>::get(), 1);
        assert!(BlockBestProof::<T>::get().is_some());
    }

    /// The costliest path of a square-class proof: a nonce the polariton
    /// construction skips after every charge draw failed its flow check. The
    /// witness is never decoded on that path, so the benchmark has no
    /// solution dimension. When no salt skips, which is the case for the
    /// ice-glass fallback of the mock, the accept path with one witness is
    /// measured instead.
    #[benchmark]
    fn submit_family_proof_square() {
        let caller: T::AccountId = whitelisted_caller();
        let mut fixture = family_fixture::<T>(FixtureKind::Square);
        let skipped = {
            let challenge = admit::<T>(&fixture.descriptor, &fixture.nodes, &fixture.edges)
                .expect("the fixture was admitted when it was chosen");
            family_salt_for::<T>(&challenge, &caller, true)
        };
        if skipped.is_none() {
            // Charged polariton rejects all-positive spins. Without a skipped
            // nonce, benchmark a feasible uncharged instance on the same graph.
            if let Some(descriptor) = fixture.uncharged_descriptor.take() {
                fixture.descriptor = descriptor;
            }
        }
        let topology_hash = mineable_family_topology_for::<T>(&fixture, &caller, 1);
        let challenge = admit::<T>(&fixture.descriptor, &fixture.nodes, &fixture.edges)
            .expect("the fixture was admitted when it was chosen");
        let (salt, solutions) = match skipped {
            // No instance or codec exists for a skipped nonce. The nonempty
            // proof reaches generation, which rejects before reading bytes.
            Some((salt, _)) => (salt, bounded(vec![bounded(Vec::new())])),
            None => {
                let (salt, instance) = family_salt_for::<T>(&challenge, &caller, false)
                    .expect("one of 256 salts gives an instance");
                let instance = instance.expect("the non-skip search returns the instance");
                let codec = challenge.prepare_witness_codec(&instance).expect("codec");
                let spins = vec![1; codec.spin_count()];
                let bytes = codec.encode(&spins).expect("uncharged fallback witness");
                (salt, bounded(vec![bounded(bytes)]))
            }
        };
        let proof = family_proof_for::<T>(&caller, topology_hash, salt, solutions);

        #[block]
        {
            // A skipped nonce is a dispatch error that still consumed the
            // whole generation cost, which is the quantity under test.
            let _ = QuantumPow::<T>::submit_proof(RawOrigin::Signed(caller.clone()).into(), proof);
        }

        assert!(BlockProofCount::<T>::get() <= 1);
    }

    #[cfg(test)]
    mod compact_tests {
        use super::*;
        use crate::mock::{new_test_ext, MaxEdges, MaxNodes, Test};

        #[test]
        fn square_benchmark_encodes_a_witness_when_no_nonce_skips() {
            struct RestoreBounds;
            impl Drop for RestoreBounds {
                fn drop(&mut self) {
                    MaxNodes::set(32);
                    MaxEdges::set(128);
                }
            }
            let _restore = RestoreBounds;
            MaxNodes::set(84);
            MaxEdges::set(2_000);
            new_test_ext().execute_with(|| {
                assert!(
                    QuantumPow::<Test>::test_bench_by_name(b"submit_family_proof_square").is_ok()
                );
            });
        }
    }

    impl_benchmark_test_suite!(QuantumPow, crate::mock::new_test_ext(), crate::mock::Test);
}

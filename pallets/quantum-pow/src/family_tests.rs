//! Family topologies: registration, activation, and proof verification.
//!
//! The fixtures are the canonical solver-core vectors vendored under
//! `crates/quantum-validation/tests/fixtures/solver-core`. The tests register
//! the z12 Zephyr topology under the `quip/ice-glass` revision 1 descriptor
//! of the `ice-glass-zephyr-z12` case.

use super::mock::*;
use super::tests::{proof_for, registered_topology};
use crate::{
    difficulty::EnergyBand,
    topology,
    types::{DifficultyConfig, FamilyDescriptor},
    weights::{family_proof_weight, FamilyWeightClass, WeightInfo},
    AllowedValueSetOf, BlockBestProof, DefaultTopology, Difficulties, LastProofBlockHash,
    MineableTopologies, PackedSpinBytesOf, QBlocks, RegisteredTopologies, TopologyEnergyBand,
};
use frame_support::{assert_noop, assert_ok, traits::Hooks, BoundedVec};
use ising_generators::blob;
use quantum_validation::{
    derive_nonce,
    families::{
        instance_energy, AdvertisedLimits, FamilyChallenge, Instance, CONTRACTS, MAX_PARAMS_LEN,
    },
    packed::pack_solution,
    AllowedValueSpec, MilliValue, MILLI_SCALE,
};
use serde_json::Value;
use sp_core::H256;

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/quantum-validation/tests/fixtures/solver-core"
);
const SCALE: MilliValue = MILLI_SCALE as MilliValue;
const MINER: u64 = 1;

fn bounded<T, S: frame_support::traits::Get<u32>>(items: Vec<T>) -> BoundedVec<T, S> {
    items.try_into().ok().expect("fits the mock bounds")
}

fn set(values: &[MilliValue]) -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(values.to_vec()))
}

fn binary_spins() -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    set(&[-SCALE, SCALE])
}

/// The z12 Zephyr topology in its vendored registration order.
fn z12() -> (BoundedVec<u32, MaxNodes>, BoundedVec<(u32, u32), MaxEdges>) {
    let bytes = std::fs::read(format!("{FIXTURES}/topologies/z12-zephyr.bin.gz"))
        .expect("the z12 fixture is vendored");
    let topo = blob::decode(&bytes)
        .and_then(|body| body.topology())
        .expect("the z12 fixture decodes");
    (bounded(topo.nodes), bounded(topo.edges))
}

/// The `quip/ice-glass` revision 1 descriptor of the z12 case.
fn ice_glass() -> FamilyDescriptor {
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{FIXTURES}/families/ice_structure.json"))
            .expect("the ice-structure vectors are vendored"),
    )
    .expect("valid JSON");
    let case = doc["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["name"] == "ice-glass-zephyr-z12")
        .expect("the z12 case exists");
    let params = sp_core::bytes::from_hex(case["params_hex"].as_str().expect("params_hex"))
        .expect("params_hex is hex");
    FamilyDescriptor {
        family_id: bounded(
            case["family_id"]
                .as_str()
                .expect("family_id")
                .as_bytes()
                .to_vec(),
        ),
        revision: u32::try_from(case["revision"].as_u64().expect("revision")).expect("u32"),
        params: bounded(params),
    }
}

fn register(descriptor: Option<FamilyDescriptor>) -> frame_support::dispatch::DispatchResult {
    let (nodes, edges) = z12();
    QuantumPow::register_topology(
        RuntimeOrigin::root(),
        nodes,
        edges,
        set(&[0]),
        set(&[-SCALE, SCALE]),
        binary_spins(),
        descriptor,
    )
}

fn registered_hash() -> H256 {
    let mut hashes: Vec<H256> = RegisteredTopologies::<Test>::iter_keys().collect();
    assert_eq!(hashes.len(), 1, "exactly one topology is registered");
    hashes.pop().expect("one hash")
}

fn band() -> EnergyBand {
    EnergyBand {
        hard_milli: -40_000,
        easy_milli: -1_000,
    }
}

fn easy() -> DifficultyConfig {
    DifficultyConfig {
        min_solutions: 1,
        max_energy_milli: i64::MAX,
        min_diversity_milli: 0,
    }
}

/// The root steps that make a registered family topology mineable: band,
/// threshold, whitelist.
fn activate_family(hash: H256) {
    assert_ok!(QuantumPow::set_topology_energy_band(
        RuntimeOrigin::root(),
        hash,
        band()
    ));
    assert_ok!(QuantumPow::set_difficulty(
        RuntimeOrigin::root(),
        hash,
        easy()
    ));
    assert_ok!(QuantumPow::add_mineable_topology(
        RuntimeOrigin::root(),
        hash
    ));
}

/// Register the z12 ice-glass topology, give it a band and an easy
/// threshold, whitelist it, and register the miner.
fn mineable_family_topology() -> H256 {
    assert_ok!(register(Some(ice_glass())));
    let hash = registered_hash();
    activate_family(hash);
    assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(MINER)));
    hash
}

/// The first salt whose nonce gives an instance, with the instance the
/// verifier will rebuild for it.
fn salt_and_instance() -> ([u8; 32], Instance) {
    let (nodes, edges) = z12();
    let descriptor = ice_glass();
    let limits = AdvertisedLimits {
        max_nodes: u64::from(MaxNodes::get()),
        max_edges: u64::from(MaxEdges::get()),
        max_degree: 0,
    };
    let challenge = FamilyChallenge::admit(
        CONTRACTS,
        &descriptor.family_id,
        descriptor.revision,
        &descriptor.params,
        &nodes,
        &edges,
        &limits,
    )
    .expect("the fixture descriptor admits on z12");
    let last_proof_block_hash = LastProofBlockHash::<Test>::get().0;
    let miner = crate::Pallet::<Test>::account_to_bytes(&MINER);
    for counter in 0u8..=255 {
        let mut salt = [0u8; 32];
        salt[0] = counter;
        let nonce = derive_nonce(&last_proof_block_hash, &miner, &salt);
        if let Ok(instance) = challenge.regenerate(nonce.to_big_endian()) {
            return (salt, instance);
        }
    }
    panic!("no salt in 256 gives an ice-glass instance")
}

fn pack(spins: &[i8]) -> PackedSpinBytesOf<Test> {
    let milli: Vec<MilliValue> = spins.iter().map(|&s| MilliValue::from(s) * SCALE).collect();
    bounded(pack_solution(&milli, &binary_spins().as_slice()).expect("binary pack"))
}

fn proof(hash: H256, salt: [u8; 32], witnesses: &[&[i8]]) -> crate::QuantumProofOf<Test> {
    let last_proof_block_hash = LastProofBlockHash::<Test>::get().0;
    let miner = crate::Pallet::<Test>::account_to_bytes(&MINER);
    crate::types::QuantumProof {
        topology_hash: hash,
        nonce: derive_nonce(&last_proof_block_hash, &miner, &salt),
        salt,
        solutions: bounded(witnesses.iter().map(|w| pack(w)).collect()),
        device_access_time_us: 0,
    }
}

#[test]
fn family_topology_registers_under_its_own_hash() {
    new_test_ext().execute_with(|| {
        let (nodes, edges) = z12();
        let legacy_hash = topology::hash_topology(
            &nodes,
            &edges,
            &set(&[0]).as_slice(),
            &set(&[-SCALE, SCALE]).as_slice(),
            &binary_spins().as_slice(),
        );

        assert_ok!(register(Some(ice_glass())));
        let hash = registered_hash();
        assert_ne!(hash, legacy_hash, "the descriptor is part of the hash");
        let meta = RegisteredTopologies::<Test>::get(hash).expect("registered");
        assert_eq!(meta.family, Some(ice_glass()));
        assert_eq!(meta.nodes, nodes);

        // A family topology is never the auto-whitelisted default.
        assert_eq!(DefaultTopology::<Test>::get(), None);
        assert!(!MineableTopologies::<Test>::contains_key(hash));

        // The same graph registers again as a legacy topology, under the
        // legacy hash, without colliding.
        assert_ok!(register(None));
        assert!(RegisteredTopologies::<Test>::contains_key(legacy_hash));
        assert_eq!(DefaultTopology::<Test>::get(), Some(legacy_hash));

        // And the same descriptor cannot register twice.
        assert_noop!(
            register(Some(ice_glass())),
            crate::Error::<Test>::TopologyAlreadyRegistered
        );
    });
}

#[test]
fn family_registration_rejects_unknown_and_altered_descriptors() {
    new_test_ext().execute_with(|| {
        let mut unknown = ice_glass();
        unknown.family_id = bounded(b"quip/unknown".to_vec());
        assert_noop!(
            register(Some(unknown)),
            crate::Error::<Test>::FamilyRejected
        );

        let mut other_revision = ice_glass();
        other_revision.revision = 2;
        assert_noop!(
            register(Some(other_revision)),
            crate::Error::<Test>::FamilyRejected
        );

        let mut truncated = ice_glass();
        truncated.params = bounded(truncated.params[..truncated.params.len() - 1].to_vec());
        assert_noop!(
            register(Some(truncated)),
            crate::Error::<Test>::FamilyRejected
        );

        // The family is a ±1 model: a wider spin alphabet is refused.
        let (nodes, edges) = z12();
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                nodes,
                edges,
                set(&[0]),
                set(&[-SCALE, SCALE]),
                set(&[-SCALE, 0, SCALE]),
                Some(ice_glass()),
            ),
            crate::Error::<Test>::FamilySpinSetRequired
        );

        // A graph the family cannot build on is refused at registration.
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                bounded(vec![0, 1]),
                bounded(vec![(0, 1)]),
                set(&[0]),
                set(&[-SCALE, SCALE]),
                binary_spins(),
                Some(ice_glass()),
            ),
            crate::Error::<Test>::FamilyRejected
        );
        assert!(RegisteredTopologies::<Test>::iter_keys().next().is_none());
    });
}

#[test]
fn family_topology_needs_an_energy_band_before_it_can_mine() {
    new_test_ext().execute_with(|| {
        assert_ok!(register(Some(ice_glass())));
        let hash = registered_hash();

        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::root(), hash),
            crate::Error::<Test>::EnergyBandMissing
        );
        assert_eq!(QuantumPow::energy_curve_for(hash), None);

        assert_noop!(
            QuantumPow::set_topology_energy_band(RuntimeOrigin::signed(MINER), hash, band()),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            QuantumPow::set_topology_energy_band(
                RuntimeOrigin::root(),
                hash,
                EnergyBand {
                    hard_milli: -1_000,
                    easy_milli: -1_000,
                }
            ),
            crate::Error::<Test>::InvalidEnergyBand
        );
        assert_noop!(
            QuantumPow::set_topology_energy_band(RuntimeOrigin::root(), H256::zero(), band()),
            crate::Error::<Test>::TopologyNotRegistered
        );

        assert_ok!(QuantumPow::set_topology_energy_band(
            RuntimeOrigin::root(),
            hash,
            band()
        ));
        assert_eq!(TopologyEnergyBand::<Test>::get(hash), Some(band()));
        assert_eq!(QuantumPow::energy_curve_for(hash), band().curve());
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            hash
        ));
        assert!(MineableTopologies::<Test>::contains_key(hash));

        // A legacy topology keeps its `expected_gse` curve; a band is refused.
        assert_ok!(register(None));
        let legacy = DefaultTopology::<Test>::get().expect("legacy default");
        assert_noop!(
            QuantumPow::set_topology_energy_band(RuntimeOrigin::root(), legacy, band()),
            crate::Error::<Test>::FamilyRejected
        );
    });
}

#[test]
fn family_proof_is_verified_against_the_regenerated_instance() {
    new_test_ext().execute_with(|| {
        let hash = mineable_family_topology();
        let (salt, instance) = salt_and_instance();
        let all_up = vec![1i8; instance.node_ids().len()];
        let energy = instance_energy(&instance, &all_up).expect("energy");

        // Wrong witness length. Spins pack one per bit, so the witness must
        // lose a whole byte (eight spins) for the packed length to differ.
        assert_noop!(
            QuantumPow::submit_proof(
                RuntimeOrigin::signed(MINER),
                proof(hash, salt, &[&all_up[8..]])
            ),
            crate::Error::<Test>::PackedSolutionLengthMismatch
        );

        // The threshold is strict: the exact energy does not clear it.
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            hash,
            DifficultyConfig {
                min_solutions: 1,
                max_energy_milli: energy,
                min_diversity_milli: 0,
            }
        ));
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(MINER), proof(hash, salt, &[&all_up])),
            crate::Error::<Test>::InsufficientEnergy
        );

        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            hash,
            DifficultyConfig {
                min_solutions: 1,
                max_energy_milli: energy + 1,
                min_diversity_milli: 0,
            }
        ));
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(MINER),
            proof(hash, salt, &[&all_up])
        ));
        let record = BlockBestProof::<Test>::get().expect("best proof");
        assert_eq!(record.energy_milli, energy);
        assert_eq!(record.topology_hash, hash);

        // The win adjusts the family topology's difficulty on its band and
        // persists a qblock against the family hash.
        QuantumPow::on_finalize(1);
        let next = Difficulties::<Test>::get(hash).expect("adjusted");
        assert!(next.max_energy_milli <= energy + 1);
        assert!(next.max_energy_milli >= band().hard_milli);
        assert_eq!(QBlocks::<Test>::get(1).expect("qblock").topology_hash, hash);
    });
}

#[test]
fn family_proof_against_a_stale_or_unknown_hash_is_rejected() {
    new_test_ext().execute_with(|| {
        let hash = mineable_family_topology();
        let (salt, instance) = salt_and_instance();
        let all_up = vec![1i8; instance.node_ids().len()];

        // A hash nobody registered: the descriptor the miner assumed is
        // not the chain's.
        let mut other = [0u8; 32];
        other[..32].copy_from_slice(hash.as_bytes());
        other[0] ^= 1;
        assert_noop!(
            QuantumPow::submit_proof(
                RuntimeOrigin::signed(MINER),
                proof(H256::from(other), salt, &[&all_up])
            ),
            crate::Error::<Test>::TopologyNotRegistered
        );

        // The challenge rotated away from this topology: the hash still
        // exists but is no longer mineable.
        assert_ok!(QuantumPow::remove_mineable_topology(
            RuntimeOrigin::root(),
            hash
        ));
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(MINER), proof(hash, salt, &[&all_up])),
            crate::Error::<Test>::TopologyNotMineable
        );
    });
}

#[test]
fn family_snapshot_carries_the_descriptor() {
    new_test_ext().execute_with(|| {
        let hash = mineable_family_topology();
        let snapshot = QuantumPow::mining_snapshot(Some(hash)).expect("snapshot");
        assert_eq!(snapshot.family, Some(ice_glass()));
        assert_eq!(snapshot.topology_hash, hash);

        assert_ok!(register(None));
        let legacy = DefaultTopology::<Test>::get().expect("legacy default");
        assert_eq!(
            QuantumPow::mining_snapshot(Some(legacy))
                .expect("legacy snapshot")
                .family,
            None
        );
    });
}

/// The activation boundary: a legacy topology mines block 1, root switches
/// the single mineable slot to a family topology, the family mines block 2,
/// and block 1 replays unchanged.
#[test]
fn switching_the_mineable_topology_from_legacy_to_family_keeps_history() {
    new_test_ext().execute_with(|| {
        let (legacy_nodes, legacy_edges, legacy) = registered_topology();
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            legacy,
            easy()
        ));
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(MINER)));
        System::set_block_number(1);
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(MINER),
            proof_for(MINER, &legacy_nodes, &legacy_edges, legacy, &[0])
        ));
        QuantumPow::on_finalize(1);
        let legacy_block = QBlocks::<Test>::get(1).expect("block 1 is a legacy qblock");
        assert_eq!(legacy_block.topology_hash, legacy);
        let legacy_difficulty = Difficulties::<Test>::get(legacy).expect("adjusted on the win");

        // Block 2 opens a new round on block 1's hash.
        System::set_block_number(2);
        frame_system::Pallet::<Test>::set_parent_hash(H256::from([0x11; 32]));
        QuantumPow::on_initialize(2);

        // The whitelist holds the default plus one incoming topology, so the
        // switch is add, repoint the default, remove.
        assert_ok!(register(Some(ice_glass())));
        let family = RegisteredTopologies::<Test>::iter_keys()
            .find(|hash| *hash != legacy)
            .expect("the family topology is registered");
        activate_family(family);
        assert_ok!(QuantumPow::set_default_topology(
            RuntimeOrigin::root(),
            family
        ));
        assert_ok!(QuantumPow::remove_mineable_topology(
            RuntimeOrigin::root(),
            legacy
        ));

        // A legacy proof is refused before any topology work.
        assert_noop!(
            QuantumPow::submit_proof(
                RuntimeOrigin::signed(MINER),
                proof_for(MINER, &legacy_nodes, &legacy_edges, legacy, &[0])
            ),
            crate::Error::<Test>::TopologyNotMineable
        );

        // A family proof wins block 2.
        let (salt, instance) = salt_and_instance();
        let all_up = vec![1i8; instance.node_ids().len()];
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(MINER),
            proof(family, salt, &[&all_up])
        ));
        QuantumPow::on_finalize(2);
        assert_eq!(
            QBlocks::<Test>::get(2)
                .expect("block 2 is a family qblock")
                .topology_hash,
            family
        );

        // Block 1 replays as it was: same record, same derived nonce, and
        // the legacy difficulty entry did not move on the family win.
        let replay = QuantumPow::qblock_with_nonce(1).expect("block 1 replays");
        assert_eq!(replay.solution, legacy_block);
        assert_eq!(
            replay.nonce,
            derive_nonce(
                &legacy_block.last_proof_block_hash.0,
                &crate::Pallet::<Test>::account_to_bytes(&MINER),
                &legacy_block.salt
            )
        );
        assert_eq!(Difficulties::<Test>::get(legacy), Some(legacy_difficulty));
        let snapshot = QuantumPow::mining_snapshot(None).expect("default snapshot");
        assert_eq!(snapshot.topology_hash, family);
        assert_eq!(snapshot.family, Some(ice_glass()));

        // The switch back is the same steps in reverse.
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            legacy
        ));
        assert_ok!(QuantumPow::set_default_topology(
            RuntimeOrigin::root(),
            legacy
        ));
        assert_ok!(QuantumPow::remove_mineable_topology(
            RuntimeOrigin::root(),
            family
        ));
        assert_noop!(
            QuantumPow::submit_proof(
                RuntimeOrigin::signed(MINER),
                proof(family, salt, &[&all_up])
            ),
            crate::Error::<Test>::TopologyNotMineable
        );
    });
}

/// Every check that costs nothing runs before admission and regeneration,
/// so a rejected proof on those paths never pays for an instance.
#[test]
fn family_proof_cheap_rejections_come_before_generation() {
    new_test_ext().execute_with(|| {
        let hash = mineable_family_topology();
        let (salt, instance) = salt_and_instance();
        let all_up = vec![1i8; instance.node_ids().len()];

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(2), proof(hash, salt, &[&all_up])),
            crate::Error::<Test>::MinerNotRegistered
        );
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(MINER), proof(hash, salt, &[])),
            crate::Error::<Test>::NoSolutionsSubmitted
        );
        let mut stale = proof(hash, salt, &[&all_up]);
        stale.salt[31] ^= 1;
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(MINER), stale),
            crate::Error::<Test>::InvalidNonce
        );
    });
}

/// Registration bounds the descriptor: the parameter bytes have a hard cap,
/// and a descriptor that names a lattice other than the registered graph is
/// refused, so no accepted topology regenerates beyond the configured bounds.
#[test]
fn family_registration_bounds_the_descriptor() {
    new_test_ext().execute_with(|| {
        let mut oversized = ice_glass();
        let mut params = vec![0x01u8];
        params.resize(MAX_PARAMS_LEN, 0);
        oversized.params = bounded(params);
        assert_noop!(
            register(Some(oversized)),
            crate::Error::<Test>::FamilyRejected
        );

        // The z12 graph is z(1, 2). A descriptor for z(2, 2) does not fit it.
        let mut other_lattice = ice_glass();
        let mut params = other_lattice.params.to_vec();
        params[7] = 2;
        other_lattice.params = bounded(params);
        assert_noop!(
            register(Some(other_lattice)),
            crate::Error::<Test>::FamilyRejected
        );
        assert!(RegisteredTopologies::<Test>::iter_keys().next().is_none());

        // The admitted fixture regenerates to exactly the registered graph.
        assert_ok!(register(Some(ice_glass())));
        let hash = registered_hash();
        activate_family(hash);
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(MINER)));
        let (_, instance) = salt_and_instance();
        let meta = RegisteredTopologies::<Test>::get(hash).expect("registered");
        assert_eq!(instance.node_ids().len(), meta.nodes.len());
        assert_eq!(instance.edges().len(), meta.edges.len());
        assert!(instance.node_ids().len() <= MaxNodes::get() as usize);
        assert!(instance.edges().len() <= MaxEdges::get() as usize);
    });
}

/// The largest proof the mock accepts: `MaxSolutions` distinct witnesses,
/// all required by the threshold, on the fixture topology.
#[test]
fn family_proof_with_max_solutions_is_accepted() {
    new_test_ext().execute_with(|| {
        let hash = mineable_family_topology();
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            hash,
            DifficultyConfig {
                min_solutions: MaxSolutions::get(),
                ..easy()
            }
        ));
        let (salt, instance) = salt_and_instance();
        let spin_count = instance.node_ids().len();
        let witnesses: Vec<Vec<i8>> = (0..MaxSolutions::get())
            .map(|index| {
                (0..spin_count as u32)
                    .map(|node| {
                        if (node ^ index).count_ones() % 2 == 0 {
                            1
                        } else {
                            -1
                        }
                    })
                    .collect()
            })
            .collect();
        let refs: Vec<&[i8]> = witnesses.iter().map(Vec::as_slice).collect();
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(MINER),
            proof(hash, salt, &refs)
        ));
        let record = BlockBestProof::<Test>::get().expect("accepted");
        let best = witnesses
            .iter()
            .map(|w| instance_energy(&instance, w).expect("energy"))
            .min()
            .expect("one witness");
        assert_eq!(record.energy_milli, best);
    });
}

/// The weight model charges each family by its class, charges an unknown
/// family the maximum of the classes, and charges admission on top of the
/// legacy registration weight.
#[test]
fn family_weights_follow_the_class_of_the_family() {
    for (id, class) in [
        (&b"quip/ice-glass"[..], FamilyWeightClass::Ice),
        (b"quip/twisted-ice-dimer", FamilyWeightClass::Ice),
        (b"quip/charged-ice", FamilyWeightClass::Ice),
        (b"quip/ice-fields", FamilyWeightClass::Ice),
        (b"quip/two-commodity-routing", FamilyWeightClass::Routing),
        (b"quip/polariton-static", FamilyWeightClass::Square),
        (b"quip/flow-static", FamilyWeightClass::Square),
        (b"quip/defects-static", FamilyWeightClass::Square),
        (b"quip/not-a-family", FamilyWeightClass::Unknown),
    ] {
        assert_eq!(
            FamilyWeightClass::of(id),
            class,
            "{}",
            String::from_utf8_lossy(id)
        );
    }

    let solutions = MaxSolutions::get();
    let ice = family_proof_weight::<()>(b"quip/ice-glass", solutions);
    let routing = family_proof_weight::<()>(b"quip/two-commodity-routing", solutions);
    let square = family_proof_weight::<()>(b"quip/polariton-static", solutions);
    let unknown = family_proof_weight::<()>(b"quip/not-a-family", solutions);
    assert_eq!(ice, <() as WeightInfo>::submit_family_proof_ice(solutions));
    assert!(unknown.all_gte(ice));
    assert!(unknown.all_gte(routing));
    assert!(unknown.all_gte(square));
    assert!(
        family_proof_weight::<()>(b"quip/ice-glass", 1).all_lte(ice),
        "more witnesses never cost less"
    );
    assert!(<() as WeightInfo>::register_family_topology()
        .all_gt(frame_support::weights::Weight::zero()));
}

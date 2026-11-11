//! Family topologies: registration, activation, and proof verification.
//!
//! The fixtures are the canonical solver-core vectors vendored under
//! `crates/quantum-validation/tests/fixtures/solver-core`. The tests register
//! the z12 Zephyr topology under the `quip/ice-glass` revision 1 descriptor
//! of the `ice-glass-zephyr-z12` case.

use super::mock::*;
use crate::{
    difficulty::EnergyBand,
    topology,
    types::{DifficultyConfig, FamilyDescriptor},
    AllowedValueSetOf, BlockBestProof, DefaultTopology, Difficulties, LastProofBlockHash,
    MineableTopologies, PackedSpinBytesOf, QBlocks, RegisteredTopologies, TopologyEnergyBand,
};
use frame_support::{assert_noop, assert_ok, traits::Hooks, BoundedVec};
use ising_generators::blob;
use quantum_validation::{
    derive_nonce,
    families::{instance_energy, AdvertisedLimits, FamilyChallenge, Instance, CONTRACTS},
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

/// Register the z12 ice-glass topology, give it a band and an easy
/// threshold, whitelist it, and register the miner.
fn mineable_family_topology() -> H256 {
    assert_ok!(register(Some(ice_glass())));
    let hash = registered_hash();
    assert_ok!(QuantumPow::set_topology_energy_band(
        RuntimeOrigin::root(),
        hash,
        band()
    ));
    assert_ok!(QuantumPow::set_difficulty(
        RuntimeOrigin::root(),
        hash,
        DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: i64::MAX,
            min_diversity_milli: 0,
        }
    ));
    assert_ok!(QuantumPow::add_mineable_topology(
        RuntimeOrigin::root(),
        hash
    ));
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

use super::mock::*;
use crate::{
    difficulty, topology,
    types::{DifficultyConfig, ProofRecord, QuantumProof},
    AllowedValueSetOf, BlockBestProof, BlockProofCount, DefaultTopology, Difficulties,
    LastProofBlock, LastProofBlockHash, MineableTopologies, Miners, PackedSpinBytesOf,
    QBlockBlockById, QBlockCount, QBlockIdByBlock, QBlocks, RegisteredTopologies, WinnerStreak,
};
use frame_support::{
    assert_noop, assert_ok,
    traits::{Get, Hooks, StorageVersion},
    BoundedVec,
};
use quantum_validation::{
    derive_nonce, energy_of_solution, generate_ising_model, packed::pack_solution,
    AllowedValueSpec, MilliValue, MILLI_SCALE,
};

fn bounded<T, S>(items: Vec<T>) -> BoundedVec<T, S>
where
    S: frame_support::traits::Get<u32>,
{
    items.try_into().ok().unwrap()
}

const SCALE: MilliValue = MILLI_SCALE as MilliValue;

fn allowed_h_spec() -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![-SCALE, 0, SCALE]))
}

fn allowed_j_spec() -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![-SCALE, SCALE]))
}

fn allowed_spin_spec() -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![-SCALE, SCALE]))
}

fn easy_difficulty() -> DifficultyConfig {
    DifficultyConfig {
        min_solutions: 1,
        max_energy_milli: i64::MAX,
        min_diversity_milli: 0,
    }
}

fn default_hash() -> sp_core::H256 {
    DefaultTopology::<Test>::get().expect("a default topology is registered")
}

/// Set the difficulty baseline for the current default topology.
fn set_difficulty_default(difficulty: DifficultyConfig) {
    assert_ok!(QuantumPow::set_difficulty(
        RuntimeOrigin::root(),
        default_hash(),
        difficulty
    ));
}

/// Read the (raw, pre-decay) difficulty baseline for the default topology.
fn difficulty_default() -> DifficultyConfig {
    Difficulties::<Test>::get(default_hash()).unwrap_or_default()
}

fn test_curve_c() -> crate::difficulty::CurveC {
    crate::difficulty::CurveC {
        easy_milli: <<Test as crate::Config>::CurveCEasyMilli as Get<u32>>::get(),
        knee_milli: <<Test as crate::Config>::CurveCKneeMilli as Get<u32>>::get(),
        hard_milli: <<Test as crate::Config>::CurveCHardMilli as Get<u32>>::get(),
    }
}

/// Energy curve calibrated against the same `(2, 1)` topology and value
/// specs that `registered_topology()` registers. Tests that call
/// `apply_decay` or `adjust_on_proof` directly need this so their behaviour
/// matches what the pallet computes through `current_energy_curve()`.
fn test_curve() -> crate::difficulty::EnergyCurve {
    crate::difficulty::EnergyCurve::new(
        2,
        1,
        test_curve_c(),
        &allowed_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
    )
    .expect("registered specs are non-empty")
}

#[test]
fn energy_curve_matches_gse_for_registered_ternary_specs() {
    // The registered ternary-h/binary-J specs reproduce the legacy magnitudes,
    // so the spec-aware curve must equal expected_gse at all three calibration
    // points when fed those same specs.
    let curve = test_curve();
    let gse = |c_milli: u32| {
        quantum_validation::expected_gse(
            2,
            1,
            f64::from(c_milli) / 1000.0,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .unwrap()
    };
    assert_eq!(curve.min_milli, gse(test_curve_c().hard_milli));
    assert_eq!(curve.knee_milli, gse(test_curve_c().knee_milli));
    assert_eq!(curve.max_milli, gse(test_curve_c().easy_milli));
}

#[test]
fn energy_curve_zero_field_spec_drops_h_contribution() {
    let zero_h: AllowedValueSpec<AllowedValueSetOf<Test>> =
        AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![0]));
    let curve = crate::difficulty::EnergyCurve::new(
        2,
        1,
        test_curve_c(),
        &zero_h.as_slice(),
        &allowed_j_spec().as_slice(),
    )
    .expect("zero-field spec is valid");

    // Every calibration point must equal the pure-J estimate…
    for (actual, c) in [
        (curve.min_milli, test_curve_c().hard_milli),
        (curve.knee_milli, test_curve_c().knee_milli),
        (curve.max_milli, test_curve_c().easy_milli),
    ] {
        let expected = quantum_validation::expected_gse(
            2,
            1,
            f64::from(c) / 1000.0,
            &zero_h.as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .unwrap();
        assert_eq!(actual, expected);
    }

    // …and be strictly less negative than the ternary-h curve, which still
    // includes a field contribution.
    let legacy = test_curve();
    assert!(curve.min_milli > legacy.min_milli);
    assert!(curve.knee_milli > legacy.knee_milli);
    assert!(curve.max_milli > legacy.max_milli);
}

#[test]
fn energy_curve_rejects_empty_specs() {
    let empty: AllowedValueSpec<AllowedValueSetOf<Test>> =
        AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![]));
    assert!(crate::difficulty::EnergyCurve::new(
        2,
        1,
        test_curve_c(),
        &empty.as_slice(),
        &allowed_j_spec().as_slice(),
    )
    .is_err());
}

fn registered_topology() -> (
    BoundedVec<u32, MaxNodes>,
    BoundedVec<(u32, u32), MaxEdges>,
    sp_core::H256,
) {
    let nodes = bounded::<_, MaxNodes>(vec![0, 1]);
    let edges = bounded::<_, MaxEdges>(vec![(0, 1)]);
    let hash = topology::hash_topology(
        &nodes,
        &edges,
        &allowed_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
        &allowed_spin_spec().as_slice(),
    );
    assert_ok!(QuantumPow::register_topology(
        RuntimeOrigin::root(),
        nodes.clone(),
        edges.clone(),
        allowed_h_spec(),
        allowed_j_spec(),
        allowed_spin_spec(),
    ));
    MineableTopologies::<Test>::insert(hash, ());
    (nodes, edges, hash)
}

fn pack_spins(spins: &[i8]) -> PackedSpinBytesOf<Test> {
    let milli: Vec<MilliValue> = spins.iter().map(|&s| s as MilliValue * SCALE).collect();
    let spec = allowed_spin_spec();
    let bytes = pack_solution(&milli, &spec.as_slice()).expect("binary spin pack");
    bounded::<u8, MaxNodes>(bytes)
}

fn finalize_winner(miner: u64, block_number: u64) {
    System::set_block_number(block_number);
    BlockBestProof::<Test>::put(ProofRecord {
        miner,
        submitted_at: block_number,
        energy_milli: 0,
        salt: [0u8; 32],
        topology_hash: DefaultTopology::<Test>::get().unwrap_or_default(),
        device_access_time_us: 0,
    });
    QuantumPow::on_finalize(block_number);
}

fn proof_for(
    miner: u64,
    nodes: &BoundedVec<u32, MaxNodes>,
    edges: &BoundedVec<(u32, u32), MaxEdges>,
    topology_hash: sp_core::H256,
    solution_indexes: &[usize],
) -> QuantumProof<crate::PackedSolutionsOf<Test>> {
    let salt: [u8; 32] = {
        let mut s = [0u8; 32];
        s[..4].copy_from_slice(b"salt");
        s
    };
    // Mirror the chain-side lookup: the pallet reads from the cached
    // `LastProofBlockHash` storage value (populated lazily by
    // on_initialize), not directly from frame_system's ring buffer.
    let last_proof_block_hash_bytes = LastProofBlockHash::<Test>::get().0;
    let miner_bytes = crate::Pallet::<Test>::account_to_bytes(&miner);
    let nonce = derive_nonce(&last_proof_block_hash_bytes, &miner_bytes, &salt);

    let h_spec = allowed_h_spec();
    let j_spec = allowed_j_spec();
    let (h, j) = generate_ising_model(
        nonce,
        nodes.as_slice(),
        edges.as_slice(),
        &h_spec.as_slice(),
        &j_spec.as_slice(),
    )
    .unwrap();

    let candidates = [[-1i8, -1], [-1, 1], [1, -1], [1, 1]];
    let mut by_energy: Vec<(i64, Vec<i8>)> = candidates
        .iter()
        .map(|solution| {
            let energy =
                energy_of_solution(solution, &h, edges.as_slice(), &j, nodes.as_slice()).unwrap();
            (energy, solution.to_vec())
        })
        .collect();
    by_energy.sort_by_key(|(energy, _)| *energy);

    let solutions = solution_indexes
        .iter()
        .map(|&index| pack_spins(&by_energy[index].1))
        .collect::<Vec<_>>();

    QuantumProof {
        topology_hash,
        nonce,
        salt,
        solutions: bounded::<_, MaxSolutions>(solutions),
        device_access_time_us: 0,
    }
}

#[test]
fn register_miner_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));

        let miner = Miners::<Test>::get(1).unwrap();
        assert_eq!(miner.deposit, 100);
        assert_eq!(pallet_balances::Pallet::<Test>::reserved_balance(1), 100);
    });
}

#[test]
fn deregister_miner_works() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        assert_ok!(QuantumPow::deregister_miner(RuntimeOrigin::signed(1)));

        assert!(Miners::<Test>::get(1).is_none());
        assert_eq!(pallet_balances::Pallet::<Test>::reserved_balance(1), 0);
    });
}

#[test]
fn register_topology_works() {
    new_test_ext().execute_with(|| {
        let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2)]);
        let expected_hash = topology::hash_topology(
            &nodes,
            &edges,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );

        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes.clone(),
            edges.clone(),
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
        ));

        assert!(RegisteredTopologies::<Test>::contains_key(expected_hash));
        assert_eq!(DefaultTopology::<Test>::get(), Some(expected_hash));
    });
}

#[test]
fn first_registration_whitelists_default() {
    new_test_ext().execute_with(|| {
        // RAW register (not the auto-whitelisting helper). The first
        // registration must claim the default AND be auto-whitelisted so a
        // fresh chain can mine it immediately.
        let nodes = bounded::<_, MaxNodes>(vec![0, 1]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1)]);
        let hash = topology::hash_topology(
            &nodes,
            &edges,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes,
            edges,
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
        ));

        assert_eq!(DefaultTopology::<Test>::get(), Some(hash));
        assert!(MineableTopologies::<Test>::contains_key(hash));
    });
}

#[test]
fn register_topology_rejects_small_graph() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                bounded::<_, MaxNodes>(vec![0]),
                bounded::<_, MaxEdges>(vec![]),
                allowed_h_spec(),
                allowed_j_spec(),
                allowed_spin_spec(),
            ),
            crate::Error::<Test>::GraphTooSmall
        );
    });
}

/// Registers a second, zero-field topology (4 nodes, ring of 4 edges,
/// h = {0}) alongside whatever is already registered. Returns its hash.
fn registered_zero_field_topology() -> sp_core::H256 {
    let nodes = bounded::<_, MaxNodes>(vec![0u32, 1, 2, 3]);
    let edges = bounded::<_, MaxEdges>(vec![(0u32, 1), (1, 2), (2, 3), (0, 3)]);
    let zero_h: AllowedValueSpec<AllowedValueSetOf<Test>> =
        AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![0]));
    let hash = topology::hash_topology(
        &nodes,
        &edges,
        &zero_h.as_slice(),
        &allowed_j_spec().as_slice(),
        &allowed_spin_spec().as_slice(),
    );
    assert_ok!(QuantumPow::register_topology(
        RuntimeOrigin::root(),
        nodes,
        edges,
        zero_h,
        allowed_j_spec(),
        allowed_spin_spec(),
    ));
    MineableTopologies::<Test>::insert(hash, ());
    hash
}

/// Registers a distinct 2-node topology (nodes `[a, b]`, single edge) WITHOUT
/// whitelisting it, returning its hash. Distinct node ids yield a distinct
/// `hash_topology`, so callers get a registered-but-un-mineable topology to
/// drive the whitelist extrinsics through `add_mineable_topology`.
fn register_unwhitelisted(a: u32, b: u32) -> sp_core::H256 {
    let nodes = bounded::<_, MaxNodes>(vec![a, b]);
    let edges = bounded::<_, MaxEdges>(vec![(a, b)]);
    let hash = topology::hash_topology(
        &nodes,
        &edges,
        &allowed_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
        &allowed_spin_spec().as_slice(),
    );
    assert_ok!(QuantumPow::register_topology(
        RuntimeOrigin::root(),
        nodes,
        edges,
        allowed_h_spec(),
        allowed_j_spec(),
        allowed_spin_spec(),
    ));
    hash
}

#[test]
fn set_default_topology_requires_root() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        assert_noop!(
            QuantumPow::set_default_topology(RuntimeOrigin::signed(1), hash),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn set_default_topology_rejects_unregistered_hash() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology();
        assert_noop!(
            QuantumPow::set_default_topology(RuntimeOrigin::root(), sp_core::H256::repeat_byte(7)),
            crate::Error::<Test>::TopologyNotRegistered
        );
    });
}

#[test]
fn set_default_topology_repoints_default_and_curve() {
    new_test_ext().execute_with(|| {
        // Topology A becomes the default by first-registration.
        let (_, _, hash_a) = registered_topology();
        let hash_b = registered_zero_field_topology();
        assert_eq!(DefaultTopology::<Test>::get(), Some(hash_a));

        assert_ok!(QuantumPow::set_default_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert_eq!(DefaultTopology::<Test>::get(), Some(hash_b));

        // The no-argument mining snapshot now serves topology B…
        let snapshot = QuantumPow::mining_snapshot(None).expect("snapshot exists");
        assert_eq!(snapshot.topology_hash, hash_b);

        // …and difficulty decay is calibrated against B's zero-field curve,
        // not A's ternary-field curve.
        let zero_h: AllowedValueSpec<AllowedValueSetOf<Test>> =
            AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![0]));
        let curve_b = crate::difficulty::EnergyCurve::new(
            4,
            4,
            test_curve_c(),
            &zero_h.as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .expect("zero-field spec is valid");
        // Start inside B's range (B is the new default, so the pallet eases
        // against B's curve). A's range is disjoint from B's, so easing under
        // A's curve lands on a different value — the difference this test
        // asserts. An out-of-range start would decay floor-only and be
        // curve-independent, defeating the sanity check below.
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve_b.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);
        System::set_block_number(101); // (101 - 1) / 20 = 5 decay steps

        let expected = difficulty::apply_decay(initial, 5, curve_b);
        let decayed = QuantumPow::mining_snapshot(None).expect("snapshot exists");
        assert_eq!(decayed.difficulty, expected);
        assert_ne!(
            expected,
            difficulty::apply_decay(initial, 5, test_curve()),
            "sanity: A's and B's curves must differ for this test to mean anything"
        );
    });
}

#[test]
fn register_topology_requires_root() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::signed(1),
                bounded::<_, MaxNodes>(vec![0, 1]),
                bounded::<_, MaxEdges>(vec![(0, 1)]),
                allowed_h_spec(),
                allowed_j_spec(),
                allowed_spin_spec(),
            ),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn register_topology_rejects_empty_spin_spec() {
    new_test_ext().execute_with(|| {
        let empty_spec = AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![]));
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                bounded::<_, MaxNodes>(vec![0, 1]),
                bounded::<_, MaxEdges>(vec![(0, 1)]),
                allowed_h_spec(),
                allowed_j_spec(),
                empty_spec,
            ),
            crate::Error::<Test>::EmptyAllowedValues
        );
    });
}

#[test]
fn set_difficulty_requires_root() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let difficulty = DifficultyConfig {
            min_solutions: 2,
            max_energy_milli: -1_000,
            min_diversity_milli: 500,
        };
        assert_noop!(
            QuantumPow::set_difficulty(RuntimeOrigin::signed(1), hash, difficulty),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn set_difficulty_rejects_unregistered_topology() {
    new_test_ext().execute_with(|| {
        let difficulty = DifficultyConfig {
            min_solutions: 3,
            max_energy_milli: -2_000,
            min_diversity_milli: 800,
        };
        assert_noop!(
            QuantumPow::set_difficulty(
                RuntimeOrigin::root(),
                sp_core::H256::repeat_byte(5),
                difficulty
            ),
            crate::Error::<Test>::TopologyNotRegistered
        );
    });
}

#[test]
fn set_difficulty_rejects_positive_diversity_with_fewer_than_two_solutions() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();

        for min_solutions in [0, 1] {
            assert_noop!(
                QuantumPow::set_difficulty(
                    RuntimeOrigin::root(),
                    hash,
                    DifficultyConfig {
                        min_solutions,
                        max_energy_milli: -2_000,
                        min_diversity_milli: 1,
                    },
                ),
                crate::Error::<Test>::InvalidDiversityConfig
            );
        }

        assert_eq!(Difficulties::<Test>::get(hash), None);
    });
}

#[test]
fn set_difficulty_works() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let difficulty = DifficultyConfig {
            min_solutions: 3,
            max_energy_milli: -2_000,
            min_diversity_milli: 800,
        };
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            hash,
            difficulty
        ));
        assert_eq!(Difficulties::<Test>::get(hash), Some(difficulty));
    });
}

/// A valid per-topology curve override with a wider `c` spread than the
/// 700/725/750 runtime constants (still `easy < knee < hard`).
fn override_curve_c() -> crate::difficulty::CurveC {
    crate::difficulty::CurveC {
        easy_milli: 600,
        knee_milli: 700,
        hard_milli: 800,
    }
}

#[test]
fn set_topology_curve_requires_root() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        assert_noop!(
            QuantumPow::set_topology_curve(RuntimeOrigin::signed(1), hash, override_curve_c()),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn set_topology_curve_rejects_unregistered_topology() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology();
        assert_noop!(
            QuantumPow::set_topology_curve(
                RuntimeOrigin::root(),
                sp_core::H256::repeat_byte(7),
                override_curve_c()
            ),
            crate::Error::<Test>::TopologyNotRegistered
        );
    });
}

#[test]
fn set_topology_curve_rejects_misordered_c() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        // knee must lie strictly between easy and hard; this inverts the order.
        let bad = crate::difficulty::CurveC {
            easy_milli: 800,
            knee_milli: 725,
            hard_milli: 700,
        };
        assert_noop!(
            QuantumPow::set_topology_curve(RuntimeOrigin::root(), hash, bad),
            crate::Error::<Test>::InvalidCurve
        );
    });
}

#[test]
fn topology_curve_override_replaces_constant_curve() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        // Without an override the curve is built from the runtime constants.
        assert_eq!(QuantumPow::energy_curve_for(hash), Some(test_curve()));

        let c = override_curve_c();
        assert_ok!(QuantumPow::set_topology_curve(
            RuntimeOrigin::root(),
            hash,
            c
        ));

        let expected = crate::difficulty::EnergyCurve::new(
            2,
            1,
            c,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .unwrap();
        assert_ne!(
            expected,
            test_curve(),
            "override must produce a different curve"
        );
        assert_eq!(QuantumPow::energy_curve_for(hash), Some(expected));
    });
}

#[test]
fn topology_curve_override_is_per_topology() {
    new_test_ext().execute_with(|| {
        let (_, _, hash_a) = registered_topology();
        let hash_b = registered_zero_field_topology();
        let b_before = QuantumPow::energy_curve_for(hash_b);

        assert_ok!(QuantumPow::set_topology_curve(
            RuntimeOrigin::root(),
            hash_a,
            override_curve_c()
        ));

        // Only A's curve moves; B still resolves to the constant-derived curve.
        assert_eq!(QuantumPow::energy_curve_for(hash_b), b_before);
        assert_ne!(QuantumPow::energy_curve_for(hash_a), Some(test_curve()));
    });
}

#[test]
fn submit_proof_accepts_valid_proof() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));

        assert_eq!(BlockProofCount::<Test>::get(), 1);
        assert!(BlockBestProof::<Test>::get().is_some());
    });
}

#[test]
fn submit_proof_rejects_invalid_nonce() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let mut proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);
        proof.nonce = proof.nonce.saturating_add(sp_core::U256::one());

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof),
            crate::Error::<Test>::InvalidNonce
        );
    });
}

#[test]
fn submit_proof_rejects_solution_with_wrong_byte_length() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let mut proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);
        // Replace the packed solution with one byte too many for a 2-spin
        // binary-encoded solution (1 byte is enough; we send 2 bytes).
        proof.solutions = bounded::<_, MaxSolutions>(vec![bounded::<u8, MaxNodes>(vec![0, 0])]);

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof),
            crate::Error::<Test>::PackedSolutionLengthMismatch
        );
    });
}

#[test]
fn better_proof_replaces_worse_proof() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(2)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        let mut worse = proof_for(1, &nodes, &edges, topology_hash, &[3]);
        worse.device_access_time_us = 111;
        let mut better = proof_for(2, &nodes, &edges, topology_hash, &[0]);
        better.device_access_time_us = 777;

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), worse));
        let first = BlockBestProof::<Test>::get().unwrap();
        assert_eq!(first.device_access_time_us, 111);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(2), better));
        let second = BlockBestProof::<Test>::get().unwrap();

        assert!(second.energy_milli <= first.energy_milli);
        assert_eq!(second.miner, 2);
        assert_eq!(second.device_access_time_us, 777);
    });
}

#[test]
fn on_finalize_pays_block_reward_for_best_proof() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);
        let initial_balance = pallet_balances::Pallet::<Test>::free_balance(1);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        QuantumPow::on_finalize(System::block_number());

        assert_eq!(
            pallet_balances::Pallet::<Test>::free_balance(1),
            initial_balance + 50
        );
        assert!(BlockBestProof::<Test>::get().is_none());
        assert_eq!(LastProofBlock::<Test>::get(), System::block_number());
        assert_eq!(Miners::<Test>::get(1).unwrap().proofs_won, 1);
        assert_eq!(Miners::<Test>::get(1).unwrap().rewards_earned, 50);
    });
}

#[test]
fn submit_proof_uses_decayed_difficulty_after_block_gap() {
    new_test_ext().execute_with(|| {
        // Regression: submit_proof must validate against the *decayed*
        // current_difficulty_for(topology), not the raw Difficulties entry.
        // Only max_energy_milli decays, so we build the discriminator out of
        // energy: set the threshold to exactly the proof's best energy
        // (validation requires strict-less-than) so the same-block submit
        // fails, then wait an epoch for decay to ease the threshold up to the
        // easy cap — which lies above this proof's energy — and admit it.
        //
        // Miner 3's puzzle has a ground-state energy below the curve's easy
        // cap, which is the case that matters: decay eases the threshold only
        // up to `max_milli`, so the proof's energy must sit below `max_milli`
        // for decay to flip it from reject to admit. (Miner 1's GSE happens to
        // be easier than the easy cap, where no in-range threshold gates it.)
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(3)));
        let (nodes, edges, topology_hash) = registered_topology();

        // Fix the nonce-derivation seed so both submissions use the same
        // proof contents.
        LastProofBlock::<Test>::put(1);
        System::set_block_number(1);

        // Build the proof once and compute its best energy.
        let proof = proof_for(3, &nodes, &edges, topology_hash, &[0]);
        let h_spec = allowed_h_spec();
        let j_spec = allowed_j_spec();
        let (h, j) = generate_ising_model(
            proof.nonce,
            nodes.as_slice(),
            edges.as_slice(),
            &h_spec.as_slice(),
            &j_spec.as_slice(),
        )
        .unwrap();
        let best_energy_milli: i64 = [[-1i8, -1], [-1, 1], [1, -1], [1, 1]]
            .iter()
            .map(|s| energy_of_solution(s, &h, edges.as_slice(), &j, nodes.as_slice()).unwrap())
            .min()
            .unwrap();
        // The discriminator only works when the proof's energy is below the
        // easy cap decay eases toward; assert the precondition so a future
        // nonce-derivation change fails loudly rather than silently.
        assert!(
            best_energy_milli < test_curve().max_milli,
            "test precondition: proof energy ({best_energy_milli}) must be below the easy cap"
        );

        // Threshold == best energy: validation's strict-less-than gate
        // rejects same-block submissions (no decay yet).
        set_difficulty_default(DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: best_energy_milli,
            min_diversity_milli: 0,
        });
        let early_proof = proof_for(3, &nodes, &edges, topology_hash, &[0]);
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(3), early_proof),
            crate::Error::<Test>::InsufficientEnergy
        );

        // One full epoch later: decay eases the threshold up to the easy cap,
        // strictly above the proof's energy. Validation now admits the same
        // proof — proving submit_proof consulted current_difficulty(), not the
        // raw storage baseline.
        System::set_block_number(21); // (21 - 1) / EpochLength(20) = 1 decay step
        let decayed_proof = proof_for(3, &nodes, &edges, topology_hash, &[0]);
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(3),
            decayed_proof
        ));
        assert!(BlockBestProof::<Test>::get().is_some());
    });
}

#[test]
fn on_finalize_fast_proof_hardens_difficulty() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let initial = easy_difficulty();
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(initial);

        LastProofBlock::<Test>::put(1);
        System::set_block_number(10);
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        QuantumPow::on_finalize(System::block_number());

        let next = difficulty_default();
        // Only the energy threshold moves; the chain-static fields stay put.
        assert!(next.max_energy_milli < initial.max_energy_milli);
        assert_eq!(next.min_solutions, initial.min_solutions);
        assert_eq!(next.min_diversity_milli, initial.min_diversity_milli);
    });
}

#[test]
fn on_finalize_slow_proof_by_new_winner_hardens_from_decayed_base() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        // min_solutions / min_diversity_milli are chain-static under the new
        // curve policy; only max_energy_milli decays. Set the chain-static
        // fields permissively so the proof passes those gates. A slow win
        // by a non-dominant (first-streak) winner hardens gently from the
        // decayed base — the v0.1 rule restored from the original design.
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: 0,
            min_diversity_milli: 0,
        };
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(initial);

        LastProofBlock::<Test>::put(1);
        System::set_block_number(250);
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));

        let decayed = difficulty::apply_decay(
            initial,
            (250_u32 - 1) / EpochLength::get() as u32,
            test_curve(),
        );
        QuantumPow::on_finalize(System::block_number());

        let next = difficulty_default();
        // A slow proof by a non-dominant winner hardens the threshold below
        // the decayed value (gentle 5%±4% band — v0.1 different/new-winner
        // rule). Decay remains the easing pressure between wins.
        assert!(next.max_energy_milli < decayed.max_energy_milli);
        // Chain-static fields untouched throughout decay + adjust.
        assert_eq!(next.min_solutions, initial.min_solutions);
        assert_eq!(next.min_diversity_milli, initial.min_diversity_milli);
    });
}

#[test]
fn curve_constants_are_recalibrated() {
    assert_eq!(
        <<Test as crate::Config>::CurveCEasyMilli as Get<u32>>::get(),
        700
    );
    assert_eq!(
        <<Test as crate::Config>::CurveCKneeMilli as Get<u32>>::get(),
        725
    );
    assert_eq!(
        <<Test as crate::Config>::CurveCHardMilli as Get<u32>>::get(),
        750
    );

    let curve = test_curve();
    assert!(curve.min_milli < curve.knee_milli);
    assert!(curve.knee_milli < curve.max_milli);
}

#[test]
fn migration_v2_to_v3_carries_difficulty_and_whitelists_default() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology(); // also whitelists in the helper
                                                  // Simulate a pre-v3 chain: remove the per-topology entry + whitelist
                                                  // the helper added, write the OLD global value at its raw key, drop to v2.
        Difficulties::<Test>::remove(hash);
        MineableTopologies::<Test>::remove(hash);
        let old = DifficultyConfig {
            min_solutions: 7,
            max_energy_milli: -14_620,
            min_diversity_milli: 300,
        };
        let old_key = crate::migration::v3::old_difficulty_key::<Test>();
        frame_support::storage::unhashed::put(&old_key, &old);
        StorageVersion::new(2).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert_eq!(
            Difficulties::<Test>::get(hash),
            Some(old),
            "global difficulty carried to default topology"
        );
        assert!(
            MineableTopologies::<Test>::contains_key(hash),
            "default topology whitelisted"
        );
        assert!(
            frame_support::storage::unhashed::get::<DifficultyConfig>(&old_key).is_none(),
            "old global value removed"
        );
        // on_runtime_upgrade steps cumulatively through v5.
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(5));
        // The live threshold for the default now equals the carried value.
        assert_eq!(
            QuantumPow::current_difficulty_for(hash, System::block_number()),
            old
        );
    });
}

#[test]
fn dominant_winner_eases_at_fast_cutoff_but_hardens_below_it() {
    // The fast cutoff is strict `<`: exactly 60 elapsed blocks is a slow
    // win, so a dominant winner eases there — one block sooner and even a
    // dominant winner hardens (v0.1: fast wins always harden).
    new_test_ext().execute_with(|| {
        registered_topology();
        let curve = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);
        // Seed a streak one short of the threshold (3); the next win makes
        // the miner dominant.
        WinnerStreak::<Test>::put(crate::types::WinnerStreak { miner: 1, count: 2 });

        finalize_winner(1, 61); // elapsed 60 == cutoff -> slow, dominant

        let next = difficulty_default();
        assert!(
            next.max_energy_milli > initial.max_energy_milli,
            "a dominant winner at 60 elapsed blocks must ease"
        );
    });

    new_test_ext().execute_with(|| {
        registered_topology();
        let curve = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);
        WinnerStreak::<Test>::put(crate::types::WinnerStreak { miner: 1, count: 2 });

        finalize_winner(1, 60); // elapsed 59 < cutoff -> fast, dominance ignored

        let next = difficulty_default();
        assert!(
            next.max_energy_milli < initial.max_energy_milli,
            "a fast win must harden even for a dominant winner"
        );
    });
}

#[test]
fn slow_win_by_different_miner_hardens() {
    // Restored v0.1 rule: a slow block won by a *different* miner hardens
    // difficulty; only dominant repeat winners ease.
    new_test_ext().execute_with(|| {
        registered_topology();
        let curve = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);

        // Make miner 1 dominant so its slow win eases the threshold up to
        // the curve ceiling — giving the next assertion room to observe a
        // strict hardening move.
        WinnerStreak::<Test>::put(crate::types::WinnerStreak { miner: 1, count: 2 });
        finalize_winner(1, 100);
        let after_dominant = difficulty_default();
        assert!(after_dominant.max_energy_milli > initial.max_energy_milli);

        finalize_winner(2, 200); // different miner, slow win

        let after_switch = difficulty_default();
        let streak = WinnerStreak::<Test>::get().expect("winner streak tracked");
        assert_eq!(streak.miner, 2);
        assert_eq!(streak.count, 1);
        assert!(
            after_switch.max_energy_milli < after_dominant.max_energy_milli,
            "a slow win by a different miner must harden (v0.1 rule)"
        );
    });
}

#[test]
fn repeated_same_winner_forces_easing() {
    new_test_ext().execute_with(|| {
        registered_topology();
        let curve = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);

        // Slow wins (elapsed >= 60 blocks): dominance easing only applies
        // past the fast cutoff, so space the wins ~100 blocks apart.
        finalize_winner(1, 100);
        let after_first = difficulty_default();
        assert!(after_first.max_energy_milli < initial.max_energy_milli);

        finalize_winner(1, 200);
        let after_second = difficulty_default();
        // Streak count 2 is still below the threshold (3): slow wins by a
        // non-dominant winner must keep hardening (or hold at the clamp
        // floor) — easing here would fire one win early and raise the value.
        assert!(
            after_second.max_energy_milli <= after_first.max_energy_milli,
            "second consecutive win is below the easing threshold and must not ease"
        );

        finalize_winner(1, 300);
        let after_third = difficulty_default();
        let streak = WinnerStreak::<Test>::get().expect("winner streak tracked");

        assert_eq!(streak.miner, 1);
        assert_eq!(streak.count, 3);
        assert!(
            after_third.max_energy_milli > after_second.max_energy_milli,
            "third consecutive slow win must ease for the dominant winner"
        );
    });
}

#[test]
fn migration_below_v2_wipes_then_bumps_to_v5() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        QBlockCount::<Test>::put(9);
        StorageVersion::new(1).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert!(RegisteredTopologies::<Test>::iter().next().is_none());
        assert_eq!(DefaultTopology::<Test>::get(), None);
        assert_eq!(QBlockCount::<Test>::get(), 0);
        assert!(!MineableTopologies::<Test>::contains_key(hash));
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(5));
    });
}

/// Pre-v4 `QBlock` layout (no `topology_hash`), used to plant an old-format
/// entry so the v3 → v4 backfill can be exercised end-to-end.
#[derive(codec::Encode)]
struct OldQBlockV3 {
    miner: u64,
    salt: [u8; 32],
    energy_milli: i64,
    reward: u128,
    submitted_at: u64,
    difficulty: DifficultyConfig,
    last_proof_block_hash: sp_core::H256,
}

#[test]
fn migration_pre_v4_backfills_topology_and_device_time() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        DefaultTopology::<Test>::put(hash);
        StorageVersion::new(3).put::<QuantumPow>();

        // A stale pre-111 BlockBestProof would decode-fail post-upgrade
        // (ProofRecord gained trailing fields); v5 kills it like v3 did.
        let old_record_key =
            frame_support::storage::storage_prefix(b"QuantumPow", b"BlockBestProof");
        frame_support::storage::unhashed::put(&old_record_key, &[7u8; 4]);

        // Plant an old-layout qblock straight at its storage key so it decodes
        // only under the pre-v4 shape.
        let block: u64 = 7;
        let old = OldQBlockV3 {
            miner: 1,
            salt: [3u8; 32],
            energy_milli: -42_000,
            reward: 50,
            submitted_at: block,
            difficulty: easy_difficulty(),
            last_proof_block_hash: sp_core::H256::repeat_byte(9),
        };
        let key = QBlocks::<Test>::hashed_key_for(block);
        frame_support::storage::unhashed::put(&key, &old);

        QuantumPow::on_runtime_upgrade();

        let raw = frame_support::storage::unhashed::get_raw(&old_record_key);
        assert!(
            raw.is_none(),
            "kill() must remove the raw BlockBestProof bytes, not just decode to None"
        );

        let migrated = QBlocks::<Test>::get(block).expect("qblock survives the v5 re-encode");
        assert_eq!(migrated.miner, 1);
        assert_eq!(migrated.salt, [3u8; 32]);
        assert_eq!(migrated.energy_milli, -42_000);
        assert_eq!(migrated.reward, 50);
        assert_eq!(migrated.difficulty, easy_difficulty());
        assert_eq!(
            migrated.last_proof_block_hash,
            sp_core::H256::repeat_byte(9)
        );
        // Backfilled with the default topology — correct for a pre-binding block.
        assert_eq!(migrated.topology_hash, hash);
        // Pre-112 blocks carry no self-reported compute time — backfilled 0.
        assert_eq!(migrated.device_access_time_us, 0);
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(5));
    });
}

/// Shipped-v4 `QBlock` layout (`topology_hash`, no `device_access_time_us`) —
/// the shape the deployed spec-111 chain holds — used to plant an entry so
/// the v4 → v5 append can be exercised end-to-end.
#[derive(codec::Encode)]
struct OldQBlockV4 {
    miner: u64,
    salt: [u8; 32],
    energy_milli: i64,
    reward: u128,
    submitted_at: u64,
    difficulty: DifficultyConfig,
    last_proof_block_hash: sp_core::H256,
    topology_hash: sp_core::H256,
}

#[test]
fn migration_v4_to_v5_appends_device_time_preserving_topology() {
    new_test_ext().execute_with(|| {
        let (_, _, default_hash) = registered_topology();
        DefaultTopology::<Test>::put(default_hash);
        StorageVersion::new(4).put::<QuantumPow>();

        // The planted block records a NON-default topology: the v4 → v5 path
        // must preserve it, not re-backfill with the default.
        let recorded = sp_core::H256::repeat_byte(0xCD);
        let block: u64 = 11;
        let old = OldQBlockV4 {
            miner: 2,
            salt: [4u8; 32],
            energy_milli: -13_000,
            reward: 75,
            submitted_at: block,
            difficulty: easy_difficulty(),
            last_proof_block_hash: sp_core::H256::repeat_byte(8),
            topology_hash: recorded,
        };
        let key = QBlocks::<Test>::hashed_key_for(block);
        frame_support::storage::unhashed::put(&key, &old);

        QuantumPow::on_runtime_upgrade();

        let migrated = QBlocks::<Test>::get(block).expect("qblock survives the v5 append");
        assert_eq!(migrated.miner, 2);
        assert_eq!(migrated.energy_milli, -13_000);
        assert_eq!(migrated.reward, 75);
        assert_eq!(
            migrated.topology_hash, recorded,
            "v4 → v5 must preserve the stored topology, not backfill the default"
        );
        assert_eq!(migrated.device_access_time_us, 0);
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(5));
    });
}

#[test]
fn migration_noop_at_v5() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let d = DifficultyConfig {
            min_solutions: 7,
            max_energy_milli: -1_000,
            min_diversity_milli: 300,
        };
        Difficulties::<Test>::insert(hash, d);
        QBlockCount::<Test>::put(9);
        StorageVersion::new(5).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert!(RegisteredTopologies::<Test>::contains_key(hash));
        assert_eq!(Difficulties::<Test>::get(hash), Some(d));
        assert_eq!(QBlockCount::<Test>::get(), 9);
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(5));
    });
}

#[test]
fn zero_easing_threshold_disables_forced_easing() {
    new_test_ext().execute_with(|| {
        ConsecutiveWinnerEasingThreshold::set(0);
        registered_topology();
        let curve = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);

        // Slow wins: with the default threshold (3) the third one would
        // ease for the dominant winner, so this spacing discriminates.
        finalize_winner(1, 100);
        finalize_winner(1, 200);
        let after_second = difficulty_default();
        finalize_winner(1, 300);
        let after_third = difficulty_default();

        // Without the `threshold > 0` guard, `count >= 0` would force
        // easing on every slow win. A threshold of 0 must mean "disabled":
        // slow wins keep hardening — easing would raise the threshold and trip
        // this. (Hardening may walk below the hard estimate `min_milli`; that
        // is by design, so we assert only the no-easing direction here.)
        assert!(
            after_third.max_energy_milli <= after_second.max_energy_milli,
            "threshold 0 disables streak easing; a slow repeat win must never ease"
        );
    });
}

#[test]
fn single_adjustment_never_slams_past_a_cap() {
    let curve = test_curve();
    // Regression for the convergence bug this MR fixes: the old total-range
    // step let a max-roll fast win overshoot far past `min_milli` and pin the
    // ceiling. Under the geometric model a single adjustment moves by at most
    // the remaining room (or one floor step at the tail), so it can overshoot
    // the hard cap by at most one energy unit and never eases past the easy
    // cap. Sweep many seeds, mining times, starts, and dominance.
    const MIN_DELTA: i64 = 1000; // mirrors MIN_ENERGY_DELTA_MILLI
    for seed_byte in 0_u8..64 {
        for &mining_time in &[1_u64, 30, 59, 60, 61, 150, 200, 201, 500] {
            for &start in &[curve.min_milli, curve.knee_milli, curve.max_milli] {
                for dominant in [false, true] {
                    let adjusted = difficulty::adjust_on_proof_with_dominance(
                        DifficultyConfig {
                            min_solutions: 1,
                            max_energy_milli: start,
                            min_diversity_milli: 0,
                        },
                        mining_time,
                        curve,
                        &[seed_byte],
                        dominant,
                    );
                    assert!(
                        adjusted.max_energy_milli >= curve.min_milli - MIN_DELTA
                            && adjusted.max_energy_milli <= curve.max_milli,
                        "seed {seed_byte}, time {mining_time}, start {start}, \
                         dominant {dominant}: adjusted threshold {} slammed past a cap \
                         (allowed [{}, {}])",
                        adjusted.max_energy_milli,
                        curve.min_milli - MIN_DELTA,
                        curve.max_milli,
                    );
                }
            }
        }
    }
}

#[test]
fn winner_streak_resets_for_different_miner() {
    new_test_ext().execute_with(|| {
        registered_topology();
        let curve = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);

        finalize_winner(1, 10);
        finalize_winner(1, 20);
        let before_reset = difficulty_default();

        finalize_winner(2, 30);
        let after_reset = difficulty_default();
        let streak = WinnerStreak::<Test>::get().expect("winner streak tracked");

        assert_eq!(streak.miner, 2);
        assert_eq!(streak.count, 1);
        // The regression this guards is the streak *not* resetting: count 3
        // would force easing, raising the threshold. Hardening may walk below
        // the hard estimate `min_milli` (by design), so we assert only that
        // the reset win keeps hardening rather than easing.
        assert!(
            after_reset.max_energy_milli <= before_reset.max_energy_milli,
            "new winner below cutoff must use normal hardening, never easing"
        );
    });
}

#[test]
fn on_finalize_persists_qblock_with_recoverable_nonce() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        // Capture the seed the round will use, before submit_proof, so the
        // assertion below pins the chain-stored value against the value
        // the helper actually fed into derive_nonce.
        let expected_last_proof_block_hash =
            sp_core::H256::from(crate::Pallet::<Test>::hash_to_bytes_32(
                frame_system::Pallet::<Test>::block_hash(LastProofBlock::<Test>::get()),
            ));
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);
        let original_nonce = proof.nonce;
        let original_salt = proof.salt;

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        let block = System::block_number();
        QuantumPow::on_finalize(block);

        let stored = QBlocks::<Test>::get(block).expect("qblock persisted");
        assert_eq!(stored.miner, 1);
        assert_eq!(stored.salt, original_salt);
        assert_eq!(stored.reward, 50);
        assert_eq!(QBlockCount::<Test>::get(), 1);
        assert_eq!(QBlockBlockById::<Test>::get(1), Some(block));
        assert_eq!(QBlockIdByBlock::<Test>::get(block), Some(1));
        assert_eq!(crate::Pallet::<Test>::latest_qblock_id(), Some(1));
        assert_eq!(crate::Pallet::<Test>::qblock_id_by_block(block), Some(1));
        assert_eq!(crate::Pallet::<Test>::qblock_block_by_id(1), Some(block));
        // LastProofBlock was zero before this proof, so no decay applied —
        // the active threshold equals whatever set_difficulty just stored.
        assert_eq!(stored.difficulty, easy_difficulty());
        assert_eq!(stored.last_proof_block_hash, expected_last_proof_block_hash);
        // The winning proof's topology is persisted on the qblock, so a block's
        // topology provenance is recoverable from state alone.
        assert_eq!(stored.topology_hash, topology_hash);

        // Re-derive the nonce via the runtime helper and confirm it matches
        // the value that was on the submitted proof. This is the round-trip
        // that lets dashboards recover the nonce from on-chain state alone.
        let view = crate::Pallet::<Test>::qblock_with_nonce(block)
            .expect("nonce derivation succeeds for a real winner");
        assert_eq!(view.nonce, original_nonce);
        assert_eq!(view.solution.salt, original_salt);
        assert_eq!(view.solution.difficulty, easy_difficulty());
        assert_eq!(
            view.solution.last_proof_block_hash,
            expected_last_proof_block_hash
        );

        let by_id = crate::Pallet::<Test>::qblock_with_nonce_by_id(1)
            .expect("qblock id resolves to the persisted qblock");
        assert_eq!(by_id, view);
    });
}

#[test]
fn qblock_returns_none_for_genesis_block() {
    new_test_ext().execute_with(|| {
        // Genesis (block 0) never had a `submit_proof` call, so the storage
        // entry is absent and the helper short-circuits before any block-hash
        // arithmetic. Pins the contract that saturating subtraction on
        // `block_number - 1 == 0u32 - 1` never reaches the nonce derivation.
        assert!(crate::Pallet::<Test>::qblock_with_nonce(0).is_none());
        assert!(crate::Pallet::<Test>::latest_qblock_id().is_none());
        assert!(crate::Pallet::<Test>::qblock_with_nonce_by_id(1).is_none());
    });
}

#[test]
fn qblock_ids_increment_only_for_winning_qblocks() {
    new_test_ext().execute_with(|| {
        registered_topology();

        finalize_winner(1, 10);
        finalize_winner(2, 15);

        assert_eq!(QBlockCount::<Test>::get(), 2);
        assert_eq!(crate::Pallet::<Test>::latest_qblock_id(), Some(2));
        assert_eq!(QBlockBlockById::<Test>::get(1), Some(10));
        assert_eq!(QBlockBlockById::<Test>::get(2), Some(15));
        assert_eq!(QBlockIdByBlock::<Test>::get(10), Some(1));
        assert_eq!(QBlockIdByBlock::<Test>::get(15), Some(2));
        assert!(QBlockIdByBlock::<Test>::get(11).is_none());
        assert_eq!(
            crate::Pallet::<Test>::qblock_with_nonce_by_id(2)
                .expect("second qblock exists")
                .solution
                .miner,
            2
        );
    });
}

#[test]
fn mining_snapshot_returns_default_and_selected_topology_views() {
    new_test_ext().execute_with(|| {
        let (default_nodes, default_edges, default_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        let other_nodes = bounded::<_, MaxNodes>(vec![10, 11, 12]);
        let other_edges = bounded::<_, MaxEdges>(vec![(10, 11), (11, 12)]);
        let other_hash = topology::hash_topology(
            &other_nodes,
            &other_edges,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            other_nodes.clone(),
            other_edges.clone(),
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
        ));

        let expected_last_proof_block_hash =
            sp_core::H256::from(crate::Pallet::<Test>::hash_to_bytes_32(
                frame_system::Pallet::<Test>::block_hash(LastProofBlock::<Test>::get()),
            ));

        let default_snapshot =
            QuantumPow::mining_snapshot(None).expect("default topology snapshot exists");
        assert_eq!(default_snapshot.topology_hash, default_hash);
        assert_eq!(default_snapshot.nodes, default_nodes);
        assert_eq!(default_snapshot.edges, default_edges);
        assert_eq!(default_snapshot.difficulty, easy_difficulty());
        assert_eq!(
            default_snapshot.last_proof_block_hash,
            expected_last_proof_block_hash
        );

        let selected_snapshot = QuantumPow::mining_snapshot(Some(other_hash))
            .expect("selected topology snapshot exists");
        assert_eq!(selected_snapshot.topology_hash, other_hash);
        assert_eq!(selected_snapshot.nodes, other_nodes);
        assert_eq!(selected_snapshot.edges, other_edges);
        assert_eq!(
            selected_snapshot.last_proof_block_hash,
            expected_last_proof_block_hash
        );
    });
}

#[test]
fn qblock_records_active_difficulty_threshold() {
    new_test_ext().execute_with(|| {
        // Pin the contract that the QBlock stores the *active* threshold
        // a proof had to clear (decay applied, pre-adjust) rather than the
        // post-adjustment value that lives in Difficulty<T> after on_finalize.
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: i64::MAX,
            min_diversity_milli: 0,
        };
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(initial);

        LastProofBlock::<Test>::put(1);
        System::set_block_number(45); // (45 - 1) / 20 = 2 decay steps
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);
        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        QuantumPow::on_finalize(System::block_number());

        let expected_active = difficulty::apply_decay(initial, 2, test_curve());
        let stored = QBlocks::<Test>::get(45).expect("winner persisted");
        assert_eq!(
            stored.difficulty, expected_active,
            "stored difficulty must be the decayed-but-pre-adjust threshold"
        );

        // Mining time blocks = 45 - 1 = 44 < harden cutoff (60), so the
        // adjustment hardens. Only the energy threshold moves now; chain-static
        // fields stay put.
        let next = difficulty_default();
        assert!(
            next.max_energy_milli < expected_active.max_energy_milli,
            "post-adjust energy must be harder (more negative) than the active threshold"
        );
        assert_eq!(next.min_solutions, expected_active.min_solutions);
        assert_eq!(
            next.min_diversity_milli,
            expected_active.min_diversity_milli
        );
    });
}

#[test]
fn mining_snapshot_returns_decayed_difficulty_after_epochs() {
    new_test_ext().execute_with(|| {
        // Pin the contract that mining_snapshot.difficulty applies decay so
        // miners querying the runtime API get the live threshold — not the
        // stale Difficulty<T> baseline that polkadot.js storage queries return.
        // In-range baseline (between the curve's hard and easy caps) so decay
        // actually eases it; a value past the easy cap would be a decay no-op.
        let initial = DifficultyConfig {
            min_solutions: 5,
            max_energy_milli: -2_300,
            min_diversity_milli: 100,
        };
        let _ = registered_topology();
        set_difficulty_default(initial);

        LastProofBlock::<Test>::put(1);
        System::set_block_number(121); // (121 - 1) / 20 = 6 decay steps
        let snapshot =
            QuantumPow::mining_snapshot(None).expect("snapshot exists for default topology");
        let expected = difficulty::apply_decay(initial, 6, test_curve());
        assert_eq!(snapshot.difficulty, expected);
        assert_ne!(
            snapshot.difficulty, initial,
            "snapshot must not echo the raw storage baseline once decay has elapsed"
        );

        // Direct storage query (the polkadot.js default path) must still
        // return the undecayed baseline — this is the visibility gap that the
        // runtime API closes.
        assert_eq!(difficulty_default(), initial);
    });
}

#[test]
fn submit_proof_survives_long_block_gap() {
    // Regression test for the txpool-delay race: a proof derived against
    // the current round's `last_proof_block_hash` must remain valid for as long
    // as no new proof has won, no matter how many blocks elapse between
    // derivation and validation. Under the old block-number-bound contract
    // this scenario produced InvalidNonce.
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        // Derive at block 1 (default after new_test_ext).
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        // Simulate a txpool that backed up by 500 blocks before the
        // extrinsic finally landed. No win happened in between, so
        // `LastProofBlock` is unchanged and the round is still the same.
        System::set_block_number(501);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        assert_eq!(BlockProofCount::<Test>::get(), 1);
        assert!(BlockBestProof::<Test>::get().is_some());
    });
}

#[test]
fn submit_proof_rejected_after_intervening_win() {
    // Mirror of the survives-gap test: if a *different* round closed
    // between derivation and submission, the last proof block hash has changed and
    // the proof must be rejected. Otherwise old proofs could replay
    // across rounds.
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        // Derive against the current last proof block hash (`block_hash(0)` in the
        // test env — defaults to zero, but the value itself is incidental
        // here; what matters is that we later mutate the lookup target).
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        // Force the last proof block hash to a distinct value by (a) pointing
        // `LastProofBlock` at a fresh block, (b) populating `block_hash`
        // for that block, and (c) writing the cached `LastProofBlockHash`
        // that the pallet's submit_proof reads. The on_initialize hook is
        // what would normally refresh the cache from parent_hash() on the
        // block immediately after the proof block; tests don't run hooks
        // automatically, so we mimic that side-effect explicitly.
        System::set_block_number(50);
        LastProofBlock::<Test>::put(10);
        frame_system::BlockHash::<Test>::insert(10u64, sp_core::H256::from([0xAB; 32]));
        LastProofBlockHash::<Test>::put(sp_core::H256::from([0xAB; 32]));

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof),
            crate::Error::<Test>::InvalidNonce
        );
    });
}

// ---------------------------------------------------------------------------
// Curve and pure-function tests (no chain state required)
// ---------------------------------------------------------------------------

#[test]
fn current_difficulty_passes_through_when_no_decay_steps() {
    let curve = test_curve();
    let base = DifficultyConfig {
        min_solutions: 5,
        max_energy_milli: -2_500,
        min_diversity_milli: 200,
    };
    // block_number == last_proof_block: zero elapsed → no decay.
    assert_eq!(
        difficulty::current_difficulty(100, base, 100, 10, Some(curve)),
        base,
    );
    // Less than one full epoch elapsed: still no decay.
    assert_eq!(
        difficulty::current_difficulty(109, base, 100, 10, Some(curve)),
        base,
    );
}

#[test]
fn current_difficulty_applies_decay_per_full_epoch() {
    let curve = test_curve();
    let base = DifficultyConfig {
        min_solutions: 5,
        max_energy_milli: -2_500,
        min_diversity_milli: 200,
    };
    // 25 blocks elapsed, epoch_length=10 → 2 decay steps.
    let result = difficulty::current_difficulty(125, base, 100, 10, Some(curve));
    let expected = difficulty::apply_decay(base, 2, curve);
    assert_eq!(result, expected);
}

#[test]
fn current_difficulty_short_circuits_without_curve() {
    // Even with elapsed > epoch_length, missing curve → no decay applied.
    let base = DifficultyConfig {
        min_solutions: 5,
        max_energy_milli: -2_500,
        min_diversity_milli: 200,
    };
    assert_eq!(
        difficulty::current_difficulty(200, base, 100, 10, None),
        base,
    );
}

#[test]
fn current_difficulty_short_circuits_at_genesis() {
    let curve = test_curve();
    let base = DifficultyConfig {
        min_solutions: 5,
        max_energy_milli: -2_500,
        min_diversity_milli: 200,
    };
    // last_proof_block == 0 → genesis path → no decay.
    assert_eq!(
        difficulty::current_difficulty(500, base, 0, 10, Some(curve)),
        base,
    );
}

#[test]
fn adjust_on_proof_only_mutates_max_energy() {
    let curve = test_curve();
    // In-range start so a fast (hardening) proof actually moves the threshold;
    // a value past the hard cap would be a no-op (nothing harder to reach).
    let before = DifficultyConfig {
        min_solutions: 7,
        max_energy_milli: -2_300,
        min_diversity_milli: 400,
    };
    let after = difficulty::adjust_on_proof(before, 30, curve, b"seed");
    assert_eq!(after.min_solutions, before.min_solutions);
    assert_eq!(after.min_diversity_milli, before.min_diversity_milli);
    assert_ne!(after.max_energy_milli, before.max_energy_milli);
}

#[test]
fn apply_decay_only_mutates_max_energy() {
    let curve = test_curve();
    let before = DifficultyConfig {
        min_solutions: 7,
        max_energy_milli: -2_500,
        min_diversity_milli: 400,
    };
    let after = difficulty::apply_decay(before, 3, curve);
    assert_eq!(after.min_solutions, before.min_solutions);
    assert_eq!(after.min_diversity_milli, before.min_diversity_milli);
    assert!(
        after.max_energy_milli > before.max_energy_milli,
        "decay must ease the threshold (move toward zero)"
    );
}

#[test]
fn decay_moves_less_than_hardening_per_step() {
    // Start from the curve midpoint so the distance to each cap is equal: that
    // isolates the rate difference (fast harden ≥ 5% of the remaining gap vs
    // decay's fixed 2.5%) from the geometric room asymmetry. On the tiny
    // test_curve every step floors to a bound, so use the production-scale
    // curve where the geometric rates are visible.
    let curve = walkup_curve();
    let midpoint = (curve.min_milli + curve.max_milli) / 2;
    let start = DifficultyConfig {
        min_solutions: 1,
        max_energy_milli: midpoint,
        min_diversity_milli: 0,
    };
    let after_decay = difficulty::apply_decay(start, 5, curve);
    let after_harden = (0..5).fold(start, |d, _| {
        // mining_time = 30 blocks → fast/hardening branch.
        difficulty::adjust_on_proof(d, 30, curve, b"seed")
    });
    let decay_move = after_decay.max_energy_milli - start.max_energy_milli;
    let harden_move = start.max_energy_milli - after_harden.max_energy_milli;
    assert!(
        harden_move > decay_move,
        "hardening must move energy farther than decay at equal step count \
         (harden={harden_move}, decay={decay_move})",
    );
}

#[test]
fn harden_motion_grows_with_distance_from_hard_cap() {
    // The geometric model steps a fraction of the gap *remaining* to the hard
    // cap, so a threshold far from the cap moves farther than one near it —
    // the inverse of the retired curve_factor, which compressed motion at the
    // edges and let mid-curve fast wins slam the ceiling. Both points use the
    // same seed/mining-time, so they sample the same rate; only the room
    // differs.
    let curve = walkup_curve();
    let near_cap = curve.min_milli + 50_000; // little room to the hard cap
    let far_from_cap = curve.max_milli; // maximum room to the hard cap
    let harden = |start: i64| {
        start
            - difficulty::adjust_on_proof(
                DifficultyConfig {
                    min_solutions: 1,
                    max_energy_milli: start,
                    min_diversity_milli: 0,
                },
                30,
                curve,
                b"seed",
            )
            .max_energy_milli
    };
    let near_move = harden(near_cap);
    let far_move = harden(far_from_cap);
    assert!(
        far_move > near_move,
        "a harden far from the hard cap must move more than one near it \
         (far={far_move}, near={near_move})",
    );
}

/// Enforces the miner-independence invariant from GitLab issue #5: the
/// energy curve must depend only on `DefaultTopology`, never on any
/// other registered topology. We register two topologies of different
/// sizes and verify that the decay magnitude observed via
/// `mining_snapshot` matches what the *default* topology's curve
/// produces — not the second registered topology's curve. If a future
/// "optimisation" routes `current_energy_curve()` through some
/// non-default topology, this test fails.
#[test]
fn energy_curve_uses_default_topology_not_other_registered() {
    new_test_ext().execute_with(|| {
        // Topology A is (2 nodes, 1 edge) — becomes DefaultTopology.
        let _ = registered_topology();
        let default_hash = DefaultTopology::<Test>::get().expect("default registered");

        // Register topology B with clearly different size so its curve
        // produces a clearly different decay magnitude.
        let b_nodes = bounded::<_, MaxNodes>(vec![0u32, 1, 2, 3]);
        let b_edges = bounded::<_, MaxEdges>(vec![(0u32, 1), (1, 2), (2, 3), (0, 3)]);
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            b_nodes,
            b_edges,
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
        ));
        // First-write-wins keeps A as the default.
        assert_eq!(DefaultTopology::<Test>::get(), Some(default_hash));

        // Set a stored difficulty and let decay elapse. Start inside A's
        // curve range so decay actually eases the threshold (a start past a
        // curve's easy cap would be a decay no-op). A and B have different
        // easy caps, so easing under each curve yields observably different
        // decayed values — which is what the sanity check below relies on.
        let curve_a = test_curve();
        let initial = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: curve_a.knee_milli,
            min_diversity_milli: 0,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);
        System::set_block_number(101); // (101 - 1) / 20 = 5 decay steps

        // `mining_snapshot` populates `difficulty` via current_difficulty,
        // which builds its curve from DefaultTopology.
        let snapshot =
            QuantumPow::mining_snapshot(None).expect("snapshot exists for default topology");

        // Expected decay using A's curve (the default).
        let expected_default = difficulty::apply_decay(initial, 5, test_curve());
        // Decay using B's curve (the *non-default* topology — this is what
        // a miner-controlled curve would produce if the invariant were broken).
        let expected_other = difficulty::apply_decay(
            initial,
            5,
            crate::difficulty::EnergyCurve::new(
                4,
                4,
                test_curve_c(),
                &allowed_h_spec().as_slice(),
                &allowed_j_spec().as_slice(),
            )
            .expect("registered specs are non-empty"),
        );

        assert_eq!(
            snapshot.difficulty, expected_default,
            "current_difficulty must calibrate the curve on DefaultTopology"
        );
        assert_ne!(
            expected_default.max_energy_milli, expected_other.max_energy_milli,
            "sanity: A and B must produce different decay magnitudes for the test to mean anything"
        );
    });
}

/// Two registrations with identical Set values in different orders must
/// collide via `topology_hash` (the design intent of `canonical_bytes`
/// sorting). Before the canonicalize_spec fix the stored order matched the
/// caller-supplied order, so the second registration would be rejected as
/// already-registered but the stored representation would be whichever
/// order won the race — meaning sample()/decode_value() walked the unsorted
/// order while the hash claimed canonical-sorted equivalence.
#[test]
fn register_topology_canonicalizes_set_order() {
    new_test_ext().execute_with(|| {
        let nodes = bounded::<_, MaxNodes>(vec![0, 1]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1)]);
        let reordered_spin_spec: AllowedValueSpec<AllowedValueSetOf<Test>> =
            AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![SCALE, -SCALE]));

        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes.clone(),
            edges.clone(),
            allowed_h_spec(),
            allowed_j_spec(),
            reordered_spin_spec,
        ));

        let hash = topology::hash_topology(
            &nodes,
            &edges,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        let stored =
            RegisteredTopologies::<Test>::get(hash).expect("topology stored under canonical hash");
        match stored.allowed_spin_values {
            AllowedValueSpec::Set(values) => {
                let v: Vec<MilliValue> = values.into_inner();
                assert_eq!(v, vec![-SCALE, SCALE], "stored Set must be sorted");
            }
            _ => panic!("expected Set variant"),
        }
    });
}

/// `ContinuousRange` spin specs need 4 bytes per spin, which would overflow
/// the `BoundedVec<u8, MaxNodes>` packed-solution bound for any topology
/// with `nodes > MaxNodes / 4`. Reject at registration so operators see a
/// concrete error instead of shipping a topology that no valid proof can
/// satisfy.
#[test]
fn register_topology_rejects_unmineable_continuous_spin_spec() {
    new_test_ext().execute_with(|| {
        let mut node_ids: Vec<u32> = (0..(MaxNodes::get() / 2)).collect();
        // Drop one node to make sure the test still exceeds MaxNodes/4 even
        // for very small MaxNodes. (MaxNodes/2 > MaxNodes/4 for MaxNodes >= 2.)
        if node_ids.len() < 2 {
            node_ids = vec![0, 1, 2, 3];
        }
        let nodes = bounded::<_, MaxNodes>(node_ids);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1)]);
        let continuous_spin: AllowedValueSpec<AllowedValueSetOf<Test>> =
            AllowedValueSpec::ContinuousRange {
                min: -SCALE,
                max: SCALE,
            };

        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                nodes,
                edges,
                allowed_h_spec(),
                allowed_j_spec(),
                continuous_spin,
            ),
            crate::Error::<Test>::PackedSolutionTooLarge
        );
    });
}

/// `LastProofBlockHash` must remain stable for the entire mining round even
/// after `frame_system::block_hash(LastProofBlock)` ages out of its
/// `BlockHashCount` ring buffer. Before the fix, a long-running round
/// (longer than `BlockHashCount`) would see the nonce seed silently flip
/// to the zero hash mid-round and reject every in-flight proof.
#[test]
fn last_proof_block_hash_stable_after_block_hash_ages_out() {
    use frame_support::traits::Hooks;

    new_test_ext().execute_with(|| {
        // Simulate a winning proof at block 5: write LastProofBlock and the
        // expected parent_hash, then run on_initialize for block 6 — which
        // is what captures the cache from parent_hash() (== block_hash(5)
        // in production). `set_parent_hash` is the test-only frame_system
        // helper for setting up this storage value.
        let proof_block_hash = sp_core::H256::from([0x77; 32]);
        LastProofBlock::<Test>::put(5u64);
        System::set_block_number(6);
        frame_system::Pallet::<Test>::set_parent_hash(proof_block_hash);
        QuantumPow::on_initialize(6);

        let cached = LastProofBlockHash::<Test>::get();
        assert_eq!(
            cached, proof_block_hash,
            "captured hash must equal block_hash(LastProofBlock)"
        );

        // Now jump far past `BlockHashCount` (production default 256) and
        // run on_initialize again with a different parent_hash. The cache
        // must NOT be overwritten — LastProofBlock hasn't changed, so the
        // round's nonce seed is still bound to block 5's hash.
        System::set_block_number(1000);
        frame_system::Pallet::<Test>::set_parent_hash(sp_core::H256::from([0x99; 32]));
        QuantumPow::on_initialize(1000);

        assert_eq!(
            LastProofBlockHash::<Test>::get(),
            cached,
            "LastProofBlockHash must not change on a no-op on_initialize"
        );
    });
}

/// `adjust_energy_along_curve` must apply `min_delta_milli` when the
/// geometric step `room * rate` rounds to zero — otherwise difficulty would
/// stall instead of advancing the last sliver toward the bound.
#[test]
fn difficulty_adjust_applies_min_delta_for_small_positive_floats() {
    // In-range start with a tiny rate so the geometric step rounds to 0:
    // room = max - min = 159 milli, rate = 0.001 → round(0.159) = 0. The
    // floor must lift the step to `min_delta_milli` (capped at the 159-milli
    // gap, which exceeds 100 here so the floor binds without overshoot).
    let curve = test_curve();
    let current = curve.max_milli; // greatest room to the hard cap

    let result = crate::difficulty::adjust_energy_along_curve(
        current,
        /* rate_milli */ 1,
        crate::difficulty::Direction::Harder,
        curve,
        /* min_delta_milli */ 100,
    );
    assert_eq!(
        result,
        current - 100,
        "the floor must advance difficulty by exactly min_delta_milli when the \
         geometric step rounds to zero"
    );
    assert!(
        result > curve.min_milli,
        "the floored step must not overshoot the hard cap"
    );
}

/// Production-scale curve (energy units in milli) used by the geometric
/// walk-up tests. The tiny `test_curve()` range (159 milli) sits below the
/// delta floor, which would flatten every step to the floor and hide the
/// geometric behaviour these tests pin.
fn walkup_curve() -> crate::difficulty::EnergyCurve {
    crate::difficulty::EnergyCurve {
        min_milli: -16_000_000,
        knee_milli: -15_600_000,
        max_milli: -14_000_000,
    }
}

/// Core of the slam-and-stall fix: well inside the curve a single harden steps
/// exactly `room * rate` of the gap *remaining* to the hard cap and, because
/// `rate < 1`, stays strictly inside it. The retired total-range step let one
/// fast win overshoot `min_milli` and pin the ceiling. (The floored tail, which
/// walks past the cap, is covered by `harden_tail_walks_past_cap_by_min_delta`.)
#[test]
fn harden_steps_geometric_fraction_in_the_interior() {
    let curve = walkup_curve();
    for &current in &[
        curve.max_milli,
        curve.knee_milli,
        -15_000_000,
        curve.min_milli + 100_000,
    ] {
        for rate_milli in [50_u32, 350, 650] {
            let result = crate::difficulty::adjust_energy_along_curve(
                current,
                rate_milli,
                crate::difficulty::Direction::Harder,
                curve,
                /* min_delta_milli */ 5000,
            );
            let room = current - curve.min_milli;
            let expected_delta = libm::round(room as f64 * f64::from(rate_milli) / 1000.0) as i64;
            assert_eq!(
                result,
                current - expected_delta,
                "harden must step exactly room*rate (current={current}, rate={rate_milli})"
            );
            // Every sampled room here is >= 100_000 milli, far above the
            // 5000 floor, so the geometric step keeps the result strictly
            // inside the hard cap (the floored tail that walks *past* the cap
            // is covered separately by `harden_tail_walks_past_cap_by_min_delta`).
            assert!(
                result > curve.min_milli,
                "a geometric harden (room >> floor) must stay strictly inside \
                 the hard cap (current={current}, rate={rate_milli}, result={result})"
            );
        }
    }
}

/// Tail behaviour: near, at, and below the hard cap the geometric term has
/// shrunk below the floor, so a harden steps by exactly one floor — walking
/// the threshold *past* `min_milli` to keep tracking a stronger-than-estimated
/// field. Hardening is uncapped at this end, so it crosses the cap rather than
/// landing on it.
#[test]
fn harden_tail_walks_past_cap_by_min_delta() {
    let curve = walkup_curve();
    let floor = 1000; // the production MIN_ENERGY_DELTA_MILLI
                      // `room` runs from a few units inside the cap down to below it (negative).
    for room in [5 * floor, floor, 1, 0, -floor] {
        let current = curve.min_milli + room;
        let result = crate::difficulty::adjust_energy_along_curve(
            current,
            /* rate_milli */
            1, // tiny rate so the geometric step rounds below the floor
            crate::difficulty::Direction::Harder,
            curve,
            floor,
        );
        assert_eq!(
            result,
            current - floor,
            "a tail harden steps by exactly one floor (room={room})"
        );
    }
    // Concretely: from the hard cap itself, a harden walks one floor below it —
    // the threshold is free to track past the hard estimate.
    let at_cap = crate::difficulty::adjust_energy_along_curve(
        curve.min_milli,
        1,
        crate::difficulty::Direction::Harder,
        curve,
        floor,
    );
    assert!(
        at_cap < curve.min_milli,
        "harden from the hard cap must cross below it, got {at_cap}"
    );
}

/// End-to-end regression for the observed testnet slam: replay the documented
/// inter-win gap series (29, 280, 27, 200, 100, 300, 1401 blocks) as
/// decay-then-harden rounds and assert the base threshold stays strictly
/// interior to the curve — it never pins `min_milli` (the slam-and-stall the
/// rewrite removes) nor runs away to `max_milli`. (Plan "Convergence
/// simulation" test.)
#[test]
fn observed_win_series_never_pins_the_hard_cap() {
    let curve = walkup_curve();
    let epoch_len = 100_u64;
    let mut base = DifficultyConfig {
        min_solutions: 1,
        max_energy_milli: curve.knee_milli, // start at the field level
        min_diversity_milli: 0,
    };
    for (i, gap) in [29_u64, 280, 27, 200, 100, 300, 1401]
        .into_iter()
        .enumerate()
    {
        // Decay eases the live threshold by one step per elapsed epoch …
        let steps = (gap / epoch_len) as u32;
        let active = difficulty::apply_decay(base, steps, curve);
        // … then the winning proof adjusts from that decayed base. Use a
        // distinct seed per round so the sampled rate varies like real wins.
        let seed = [i as u8];
        base = difficulty::adjust_on_proof(active, gap, curve, &seed);
        assert!(
            base.max_energy_milli > curve.min_milli && base.max_energy_milli < curve.max_milli,
            "round {i} (gap {gap}) left the curve interior: {} not in ({}, {})",
            base.max_energy_milli,
            curve.min_milli,
            curve.max_milli,
        );
    }
}

/// Walk-up property: climbing from the field level to the hard cap takes many
/// wins, never one. The old total-range step saturated the curve on a single
/// 35% fast win; the geometric step closes ~35% of the *remaining* gap, so it
/// takes well over ten consecutive wins to approach the cap.
#[test]
fn fast_wins_walk_up_the_curve_over_many_steps() {
    let curve = walkup_curve();
    let mut current = curve.knee_milli; // field level, mid curve
    let mut wins = 0;
    // Stop within one energy unit (1000 milli) of the cap.
    while current - curve.min_milli > 1000 && wins < 1000 {
        current = crate::difficulty::adjust_energy_along_curve(
            current,
            /* rate_milli */ 350, // median fast-harden roll
            crate::difficulty::Direction::Harder,
            curve,
            /* min_delta_milli */ 5000,
        );
        wins += 1;
    }
    assert!(
        wins >= 10,
        "a 35% geometric harden must take >=10 wins to reach the cap, got {wins}"
    );
    assert!(
        current <= curve.min_milli + 1000,
        "the walk-up must actually reach the hard cap, ended at {current}"
    );
}

/// Decay easing is geometric in the distance to the *easy* cap, so its largest
/// step is taken exactly at the hard ceiling — the inverse of the retired
/// curve, which eased slowest there and caused multi-thousand-block recovery
/// stalls. A threshold pinned at `min_milli` therefore recovers fastest.
#[test]
fn decay_recovers_fastest_from_the_hard_cap() {
    let curve = walkup_curve();
    let decay_step = |start: i64| {
        crate::difficulty::adjust_energy_along_curve(
            start,
            /* DECAY rate per epoch */ 25,
            crate::difficulty::Direction::Easier,
            curve,
            /* min_delta_milli */ 3000,
        ) - start
    };
    let from_cap = decay_step(curve.min_milli); // full range remaining
    let from_mid = decay_step(curve.knee_milli);
    let from_near_max = decay_step(curve.max_milli - 50_000);
    assert!(
        from_cap > from_mid && from_mid > from_near_max,
        "decay must ease most at the hard cap, least near the easy cap \
         (cap={from_cap}, mid={from_mid}, near_max={from_near_max})"
    );
}

#[test]
fn submit_proof_records_topology_hash() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));

        let record = BlockBestProof::<Test>::get().expect("best proof recorded");
        assert_eq!(record.topology_hash, topology_hash);
    });
}

#[test]
fn winning_topology_does_not_move_other_topology_difficulty() {
    new_test_ext().execute_with(|| {
        // Topology A (ternary-h) is default; topology B (zero-field) is a
        // second registered topology. Each gets its own difficulty entry.
        let (_, _, hash_a) = registered_topology();
        let hash_b = registered_zero_field_topology();

        let diff_a = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: -10_000,
            min_diversity_milli: 0,
        };
        let diff_b = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: -20_000,
            min_diversity_milli: 0,
        };
        Difficulties::<Test>::insert(hash_a, diff_a);
        Difficulties::<Test>::insert(hash_b, diff_b);

        // A wins (finalize against A). B's difficulty must be untouched.
        LastProofBlock::<Test>::put(1);
        System::set_block_number(80);
        BlockBestProof::<Test>::put(ProofRecord {
            miner: 1,
            submitted_at: 80,
            energy_milli: -10_000,
            salt: [0u8; 32],
            topology_hash: hash_a,
            device_access_time_us: 0,
        });
        QuantumPow::on_finalize(80);

        assert_ne!(
            Difficulties::<Test>::get(hash_a),
            Some(diff_a),
            "A's difficulty must have been adjusted"
        );
        assert_eq!(
            Difficulties::<Test>::get(hash_b),
            Some(diff_b),
            "B's difficulty must NOT move when A wins"
        );
    });
}

#[test]
fn qblock_persists_device_access_time() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        DefaultTopology::<Test>::put(hash);
        System::set_block_number(80);
        BlockBestProof::<Test>::put(ProofRecord {
            miner: 1,
            submitted_at: 80,
            energy_milli: -10_000,
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 1_234_567,
        });
        QuantumPow::on_finalize(80);

        let qblock = QBlocks::<Test>::get(80).expect("qblock stored");
        assert_eq!(qblock.device_access_time_us, 1_234_567);
    });
}

#[test]
fn submit_proof_rejects_non_mineable_topology() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        // Register topology A FIRST via the helper so it claims the
        // auto-whitelisted default. The topology-under-test must NOT be the
        // first registration — otherwise it would be auto-whitelisted too.
        let (nodes, edges, _) = registered_topology();

        // Register topology B WITHOUT whitelisting (raw call, not the helper),
        // with a distinct two-node graph so it gets a distinct hash and stays
        // un-mineable. Distinct node ids ([5, 6] vs A's [0, 1]) → distinct
        // `hash_topology` output; same value specs as A so the proof we build
        // against it is well-formed up to the mineable check.
        let nodes_b = bounded::<_, MaxNodes>(vec![5, 6]);
        let edges_b = bounded::<_, MaxEdges>(vec![(5, 6)]);
        let topology_hash = topology::hash_topology(
            &nodes_b,
            &edges_b,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes_b,
            edges_b,
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
        ));
        assert!(
            !MineableTopologies::<Test>::contains_key(topology_hash),
            "second registration must not be auto-whitelisted"
        );
        Difficulties::<Test>::insert(topology_hash, easy_difficulty());
        // Build the proof against A's graph (so `proof_for`'s two-spin
        // candidate set is consistent) but claim B's un-mineable hash. The
        // mineable-whitelist check fires before any solution validation, so
        // the rejection is `TopologyNotMineable` regardless of proof contents.
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof),
            crate::Error::<Test>::TopologyNotMineable
        );
    });
}

#[test]
fn add_mineable_topology_requires_root() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::signed(1), hash),
            sp_runtime::DispatchError::BadOrigin
        );
    });
}

#[test]
fn add_mineable_topology_rejects_unregistered() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::root(), sp_core::H256::repeat_byte(9)),
            crate::Error::<Test>::TopologyNotRegistered
        );
    });
}

#[test]
fn remove_mineable_topology_refuses_default() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology(); // becomes default + whitelisted
        assert_noop!(
            QuantumPow::remove_mineable_topology(RuntimeOrigin::root(), hash),
            crate::Error::<Test>::TopologyIsDefault
        );
        assert!(MineableTopologies::<Test>::contains_key(hash));
    });
}

#[test]
fn remove_mineable_topology_works_for_non_default() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology(); // default A
        let hash_b = registered_zero_field_topology(); // whitelisted, not default
        assert_ok!(QuantumPow::remove_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert!(!MineableTopologies::<Test>::contains_key(hash_b));
    });
}

#[test]
fn add_mineable_topology_works_and_is_idempotent() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology(); // A: default + whitelisted
        let hash_b = register_unwhitelisted(5, 6);
        assert!(!MineableTopologies::<Test>::contains_key(hash_b));

        // First add whitelists B (the core mechanism: membership here is what
        // makes a topology mineable).
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert!(MineableTopologies::<Test>::contains_key(hash_b));

        // Re-adding the same topology is a no-op success that hits the
        // `!contains_key` skip branch; B stays mineable.
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert!(MineableTopologies::<Test>::contains_key(hash_b));
    });
}

#[test]
fn add_mineable_topology_rejects_second_non_default() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology(); // A: default + whitelisted
        let hash_b = register_unwhitelisted(5, 6);
        let hash_c = register_unwhitelisted(7, 8);

        // One non-default topology may be whitelisted: the model-A switch
        // window of {default, one incoming}.
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));

        // A second concurrent non-default topology is refused — the global
        // decay anchor only supports one active topology at a time.
        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::root(), hash_c),
            crate::Error::<Test>::MineableTopologyConflict
        );

        // Remove-old-before-switch: dropping B frees the slot for C.
        assert_ok!(QuantumPow::remove_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            hash_c
        ));
        assert!(MineableTopologies::<Test>::contains_key(hash_c));
        assert!(!MineableTopologies::<Test>::contains_key(hash_b));
    });
}

#[test]
fn remove_mineable_topology_is_noop_for_non_whitelisted() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology(); // A: default + whitelisted
        let hash_b = register_unwhitelisted(5, 6); // registered, not mineable
        assert!(!MineableTopologies::<Test>::contains_key(hash_b));

        // Removing a registered-but-not-whitelisted (non-default) topology is
        // a silent success that hits the `contains_key` skip branch.
        assert_ok!(QuantumPow::remove_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert!(!MineableTopologies::<Test>::contains_key(hash_b));
    });
}

#[test]
fn set_default_topology_rejects_non_mineable() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology(); // default A, whitelisted
                                       // Register B without whitelisting.
        let nodes = bounded::<_, MaxNodes>(vec![0u32, 1, 2, 3]);
        let edges = bounded::<_, MaxEdges>(vec![(0u32, 1), (1, 2), (2, 3), (0, 3)]);
        let zero_h: AllowedValueSpec<AllowedValueSetOf<Test>> =
            AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![0]));
        let hash_b = topology::hash_topology(
            &nodes,
            &edges,
            &zero_h.as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes,
            edges,
            zero_h,
            allowed_j_spec(),
            allowed_spin_spec(),
        ));
        assert_noop!(
            QuantumPow::set_default_topology(RuntimeOrigin::root(), hash_b),
            crate::Error::<Test>::TopologyNotMineable
        );
    });
}

#[test]
fn difficulty_for_api_returns_per_topology() {
    new_test_ext().execute_with(|| {
        let (_, _, hash_a) = registered_topology();
        let hash_b = registered_zero_field_topology();
        let da = DifficultyConfig {
            min_solutions: 1,
            max_energy_milli: -10_000,
            min_diversity_milli: 0,
        };
        let db = DifficultyConfig {
            min_solutions: 2,
            max_energy_milli: -20_000,
            min_diversity_milli: 5,
        };
        set_difficulty_default(da); // A is the default
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            hash_b,
            db
        ));

        assert_eq!(QuantumPow::difficulty_for_api(hash_a), Some(da));
        assert_eq!(QuantumPow::difficulty_for_api(hash_b), Some(db));
        assert_eq!(
            QuantumPow::difficulty_for_api(sp_core::H256::repeat_byte(3)),
            None
        );
    });
}

#[test]
fn mining_snapshot_some_returns_that_topology_difficulty() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology();
        let hash_b = registered_zero_field_topology();
        let db = DifficultyConfig {
            min_solutions: 2,
            max_energy_milli: -20_000,
            min_diversity_milli: 5,
        };
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            hash_b,
            db
        ));

        let snap = QuantumPow::mining_snapshot(Some(hash_b)).expect("snapshot exists");
        assert_eq!(snap.topology_hash, hash_b);
        assert_eq!(snap.difficulty, db); // B's difficulty, not the default's
    });
}

#[test]
fn mineable_topologies_enumerates_whitelist() {
    new_test_ext().execute_with(|| {
        let (_, _, hash_a) = registered_topology(); // whitelisted (default)
        let hash_b = registered_zero_field_topology(); // whitelisted
        let mut got = QuantumPow::mineable_topologies();
        got.sort();
        let mut want = vec![hash_a, hash_b];
        want.sort();
        assert_eq!(got, want);
    });
}

#[test]
fn unset_whitelisted_topology_reads_default_hard_difficulty() {
    new_test_ext().execute_with(|| {
        let _ = registered_topology();
        let hash_b = registered_zero_field_topology(); // whitelisted, no set_difficulty
                                                       // Fails closed: returns the conservative (hard) default until calibrated.
        assert_eq!(
            QuantumPow::current_difficulty_for(hash_b, System::block_number()),
            DifficultyConfig::default()
        );
    });
}

// ============================================================================
// Weight Regression Tests (QIP-03: Parameterized Weight Accounting)
// ============================================================================

use crate::weights::WeightInfo;

/// Test helper: calculate weight for given proof dimensions
fn calculate_weight(nodes: u32, edges: u32, solutions: u32) -> frame_support::weights::Weight {
    <() as WeightInfo>::submit_proof(nodes, edges, solutions)
}

#[test]
fn weight_scales_with_proof_dimensions() {
    // Mathematical invariant: Weight(n,e,s) should increase with dimensions

    let weight_small = calculate_weight(2, 1, 1);
    let weight_medium = calculate_weight(100, 200, 8);
    let weight_large = calculate_weight(1000, 5000, 32);

    // Monotonicity: larger inputs -> larger weight
    assert!(
        weight_medium.ref_time() > weight_small.ref_time(),
        "Weight should increase with proof size (medium > small)"
    );
    assert!(
        weight_large.ref_time() > weight_medium.ref_time(),
        "Weight should increase with proof size (large > medium)"
    );
}

#[test]
fn register_topology_weight_scales_with_all_dimensions() {
    let base = <() as WeightInfo>::register_topology(2, 1, 3);
    assert!(<() as WeightInfo>::register_topology(3, 1, 3).ref_time() > base.ref_time());
    assert!(<() as WeightInfo>::register_topology(2, 2, 3).ref_time() > base.ref_time());
    assert!(<() as WeightInfo>::register_topology(2, 1, 4).ref_time() > base.ref_time());
}

#[test]
fn register_topology_dispatch_info_uses_input_dimensions() {
    use frame_support::dispatch::GetDispatchInfo;

    let call = crate::Call::<Test>::register_topology {
        nodes: bounded::<_, MaxNodes>(vec![0, 1, 2]),
        edges: bounded::<_, MaxEdges>(vec![(0, 1), (1, 2)]),
        allowed_h_values: allowed_h_spec(),
        allowed_j_values: allowed_j_spec(),
        allowed_spin_values: allowed_spin_spec(),
    };

    assert_eq!(
        call.get_dispatch_info().call_weight,
        <() as WeightInfo>::register_topology(3, 2, 7),
    );
}

#[test]
fn weight_formula_components_are_present() {
    // Verify the independently isolatable components of
    // W(n,e,s) =
    // BASE + k₁·n + k₂·e + k₃·s·n + k₄·s·e + k₅·s²·n + k₆·n·e.

    let base_weight = calculate_weight(0, 0, 0);
    assert!(base_weight.ref_time() > 0, "Base weight should be non-zero");

    // Component k₁·n: nodes contribute linearly
    let w_100_nodes = calculate_weight(100, 0, 0);
    let w_200_nodes = calculate_weight(200, 0, 0);
    assert!(
        w_200_nodes.ref_time() > w_100_nodes.ref_time(),
        "Node component (k₁·n) should increase with node count"
    );

    // Component k₂·e: edges contribute linearly
    let w_100_edges = calculate_weight(0, 100, 0);
    let w_200_edges = calculate_weight(0, 200, 0);
    assert!(
        w_200_edges.ref_time() > w_100_edges.ref_time(),
        "Edge component (k₂·e) should increase with edge count"
    );

    // Component k₄·s·e: solutions × edges (dominant term)
    let w_1_sol_100_edge = calculate_weight(0, 100, 1);
    let w_2_sol_100_edge = calculate_weight(0, 100, 2);
    assert!(
        w_2_sol_100_edge.ref_time() > w_1_sol_100_edge.ref_time(),
        "Solution-edge component (k₄·s·e) should increase with solutions"
    );

    // Component k₆·n·e: the edge slope grows with the node count.
    let edge_slope_at_1_node =
        calculate_weight(1, 200, 0).ref_time() - calculate_weight(1, 100, 0).ref_time();
    let edge_slope_at_100_nodes =
        calculate_weight(100, 200, 0).ref_time() - calculate_weight(100, 100, 0).ref_time();
    assert!(
        edge_slope_at_100_nodes > edge_slope_at_1_node,
        "Node-edge component (k₆·n·e) should increase the edge slope"
    );
}

#[test]
fn weight_prevents_undercharging_for_large_proofs() {
    // Critical invariant: large proofs must cost far more than small ones, so a
    // worst-case proof can't be under-charged. The mock's Max* bounds are tiny
    // (16/32/8), so exercise the formula directly at the documented production
    // bounds where the dimensional terms must dominate the fixed base.
    let small_proof_weight = calculate_weight(2, 1, 1); // Minimal proof
    let large_proof_weight = calculate_weight(5_000, 50_000, 32); // Production worst case

    // Large proof should cost significantly more (at least 10x).
    let ratio = large_proof_weight.ref_time() / small_proof_weight.ref_time().max(1);
    assert!(
        ratio >= 10,
        "Worst-case proof should cost at least 10x more than minimal proof, got {ratio}x"
    );
}

const EXPECTED_SWEEP_POINTS: [(&str, u32, u32, u32); 6] = [
    ("minimum", 16, 1, 1),
    ("nodes", 5_000, 1, 1),
    ("edges", 16, 50_000, 1),
    ("solution_nodes", 5_000, 1, 32),
    ("solution_edges", 16, 50_000, 32),
    ("worst_case", 5_000, 50_000, 32),
];

fn expected_sweep_dimensions(point: &str) -> (u32, u32, u32) {
    EXPECTED_SWEEP_POINTS
        .iter()
        .find_map(|(name, nodes, edges, solutions)| {
            (*name == point).then_some((*nodes, *edges, *solutions))
        })
        .unwrap_or_else(|| panic!("unexpected sweep point: {point}"))
}

fn assert_weight_covers_observation(
    source: &str,
    point: &str,
    nodes: u32,
    edges: u32,
    solutions: u32,
    maximum_ns: u64,
    required_margin_percent: u128,
) {
    let charged = u128::from(calculate_weight(nodes, edges, solutions).ref_time());
    let observed = u128::from(maximum_ns) * 1_000;
    let margin_basis_points = charged
        .saturating_mul(10_000)
        .checked_div(observed)
        .unwrap_or_default()
        .saturating_sub(10_000);
    let margin_whole = margin_basis_points / 100;
    let margin_fraction = margin_basis_points % 100;

    assert!(
        charged * 100 >= observed * (100 + required_margin_percent),
        "{source} {point} charge {charged} ps covers observed {observed} ps by \
         {margin_whole}.{margin_fraction:02}%; required {required_margin_percent}%"
    );
}

#[test]
fn weight_covers_recorded_node02_sweeps_with_twenty_percent_target() {
    const RECORDED_SWEEPS: &str = include_str!("../testdata/node02-submit-proof-sweeps.tsv");
    const EXPECTED_HEADER: &str =
        "job_id\tcommit_sha\tpoint\tnodes\tedges\tsolutions\tsamples\tmax_ns";
    const EXPECTED_JOBS: [u64; 3] = [15_552_403_591, 15_618_730_781, 15_638_128_172];

    let mut lines = RECORDED_SWEEPS.lines();
    assert_eq!(lines.next(), Some(EXPECTED_HEADER));
    let mut seen = std::collections::BTreeSet::new();

    for line in lines {
        assert!(
            !line.is_empty(),
            "recorded sweep fixture contains an empty row"
        );
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 8, "invalid recorded sweep row: {line}");

        let job_id = columns[0].parse::<u64>().expect("recorded job ID");
        let commit_sha = columns[1];
        let point = columns[2];
        let nodes = columns[3].parse::<u32>().expect("recorded nodes");
        let edges = columns[4].parse::<u32>().expect("recorded edges");
        let solutions = columns[5].parse::<u32>().expect("recorded solutions");
        let samples = columns[6].parse::<u32>().expect("recorded samples");
        let maximum_ns = columns[7].parse::<u64>().expect("recorded maximum");

        assert_eq!(commit_sha.len(), 40, "invalid commit SHA in row: {line}");
        assert!(
            commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "non-hex commit SHA in row: {line}"
        );
        assert!(samples > 0, "recorded sample count must be nonzero");
        assert_eq!(
            (nodes, edges, solutions),
            expected_sweep_dimensions(point),
            "recorded dimensions do not match point {point}"
        );
        assert!(
            seen.insert((job_id, point)),
            "duplicate recorded sweep row for job {job_id}, point {point}"
        );

        assert_weight_covers_observation(
            &format!("job {job_id}"),
            point,
            nodes,
            edges,
            solutions,
            maximum_ns,
            20,
        );
    }

    for job_id in EXPECTED_JOBS {
        for (point, _, _, _) in EXPECTED_SWEEP_POINTS {
            assert!(
                seen.contains(&(job_id, point)),
                "missing recorded sweep row for job {job_id}, point {point}"
            );
        }
    }
    assert_eq!(
        seen.len(),
        18,
        "recorded fixture must contain three full sweeps"
    );
}

#[test]
#[ignore = "requires QUANTUM_POW_SWEEP_SUMMARY from a completed sweep"]
fn weight_covers_executed_sweep_with_ten_percent_floor() {
    const EXPECTED_HEADER: &str =
        "point\tnodes\tedges\tsolutions\tsamples\tmin_ns\tmedian_ns\tmax_ns";

    let summary_path = std::env::var("QUANTUM_POW_SWEEP_SUMMARY")
        .expect("QUANTUM_POW_SWEEP_SUMMARY must point to the completed summary.tsv");
    let summary =
        std::fs::read_to_string(&summary_path).expect("completed sweep summary must be readable");
    let mut lines = summary.lines();
    assert_eq!(lines.next(), Some(EXPECTED_HEADER), "invalid sweep header");
    let mut seen = std::collections::BTreeSet::new();

    for line in lines {
        assert!(!line.is_empty(), "sweep summary contains an empty row");
        let columns = line.split('\t').collect::<Vec<_>>();
        assert_eq!(columns.len(), 8, "invalid sweep summary row: {line}");

        let point = columns[0];
        let nodes = columns[1].parse::<u32>().expect("summary nodes");
        let edges = columns[2].parse::<u32>().expect("summary edges");
        let solutions = columns[3].parse::<u32>().expect("summary solutions");
        let samples = columns[4].parse::<u32>().expect("summary samples");
        let minimum_ns = columns[5].parse::<u64>().expect("summary minimum");
        let median_ns = columns[6].parse::<u64>().expect("summary median");
        let maximum_ns = columns[7].parse::<u64>().expect("summary maximum");

        assert!(samples > 0, "summary sample count must be nonzero");
        assert!(
            minimum_ns <= median_ns && median_ns <= maximum_ns,
            "summary timings are not ordered for point {point}"
        );
        assert_eq!(
            (nodes, edges, solutions),
            expected_sweep_dimensions(point),
            "summary dimensions do not match point {point}"
        );
        assert!(
            seen.insert(point),
            "duplicate sweep summary row for point {point}"
        );

        assert_weight_covers_observation(
            &summary_path,
            point,
            nodes,
            edges,
            solutions,
            maximum_ns,
            10,
        );
    }

    for (point, _, _, _) in EXPECTED_SWEEP_POINTS {
        assert!(seen.contains(point), "missing sweep summary point {point}");
    }
    assert_eq!(seen.len(), 6, "sweep summary must contain all six points");
}

#[test]
fn weight_is_monotonic_across_solution_bounds() {
    for (nodes, edges) in [(16, 1), (5_000, 50_000)] {
        let mut previous = calculate_weight(nodes, edges, 0).ref_time();
        for solutions in 1..=32 {
            let current = calculate_weight(nodes, edges, solutions).ref_time();
            assert!(
                current >= previous,
                "weight decreased at n={nodes}, e={edges}, s={solutions}"
            );
            previous = current;
        }
    }
}

#[test]
fn weight_scales_quadratically_with_solutions_for_diversity() {
    // Diversity/quality calculation is O(s²·n). Superlinearity is verified by
    // comparing successive increments (which cancel the fixed base) rather than
    // ratios of totals — the large base term makes total ratios ~1 regardless of
    // curvature, so they cannot reveal the quadratic component.
    let w_2_sols = calculate_weight(100, 100, 2).ref_time();
    let w_4_sols = calculate_weight(100, 100, 4).ref_time();
    let w_8_sols = calculate_weight(100, 100, 8).ref_time();

    // The cost of each additional pair of solutions must itself grow, which is
    // the signature of the s²·n term.
    let inc_2_to_4 = w_4_sols - w_2_sols;
    let inc_4_to_8 = w_8_sols - w_4_sols;
    assert!(
        inc_4_to_8 > inc_2_to_4,
        "Per-solution cost should grow with solution count (quadratic term), \
         got increment {inc_2_to_4} then {inc_4_to_8}"
    );
}

#[test]
fn weight_calculation_saturates_safely() {
    // Pathological inputs must saturate, not wrap: the s²·n term alone is
    // ~2^96 at u32::MAX inputs, so a saturating implementation pins ref_time
    // at u64::MAX while a wrapping one would land on an arbitrary value.
    let max_weight = calculate_weight(u32::MAX, u32::MAX, u32::MAX);
    assert_eq!(
        max_weight.ref_time(),
        u64::MAX,
        "weight must saturate to u64::MAX on overflow"
    );
}

#[test]
fn submit_proof_uses_parameterized_weight() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        assert_ok!(QuantumPow::set_difficulty(
            RuntimeOrigin::root(),
            topology_hash,
            easy_difficulty()
        ));

        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);

        // The charge for this proof is the parameterized formula keyed to the
        // registered topology's dimensions (node/edge counts come from the
        // topology, not the proof), so it covers at least the base cost.
        let charged = <() as WeightInfo>::submit_proof(
            nodes.len() as u32,
            edges.len() as u32,
            proof.solutions.len() as u32,
        );
        assert!(
            charged.ref_time() >= 10_000_000,
            "parameterized weight must cover the base extrinsic cost"
        );

        // QIP-03 guarantee: a large proof is charged well above the retired 60M
        // flat weight, so large proofs can no longer be under-charged.
        let large = calculate_weight(1_000, 5_000, 32);
        assert!(
            large.ref_time() > 60_000_000,
            "large proofs must exceed the retired 60M flat weight"
        );

        // Submission succeeds end-to-end.
        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
    });
}

#[test]
fn submit_proof_dispatch_info_charges_topology_scaled_weight() {
    // The QIP-03 fix lives in the #[pallet::weight] closure, not in
    // WeightInfo: this pins the *dispatched* charge to the formula evaluated
    // at the registered topology's dimensions (n/e come from storage via
    // proof.topology_hash, s from the proof). A regression to the flat
    // weight — or transposed nodes/edges arguments — fails here.
    use frame_support::dispatch::GetDispatchInfo;
    new_test_ext().execute_with(|| {
        let (nodes, edges, topology_hash) = registered_topology();
        let proof = proof_for(1, &nodes, &edges, topology_hash, &[0]);
        let expected = <() as WeightInfo>::submit_proof(
            nodes.len() as u32,
            edges.len() as u32,
            proof.solutions.len() as u32,
        );
        let call = crate::Call::<Test>::submit_proof { proof };
        assert_eq!(
            call.get_dispatch_info().call_weight,
            expected,
            "dispatched weight must equal the formula at the topology's dimensions"
        );
    });
}

#[test]
fn submit_proof_dispatch_info_charges_base_for_unregistered_topology() {
    // An unregistered topology_hash must be charged exactly the
    // zero-dimension base: every solution-scaled term multiplies by n or e,
    // so solution count adds nothing, and dispatch rejects after O(1) work.
    // This pins the closure's unwrap_or((0, 0)) fallback.
    use frame_support::dispatch::GetDispatchInfo;
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, _) = registered_topology();
        let bogus = sp_core::H256::repeat_byte(0xAB);
        let proof = proof_for(1, &nodes, &edges, bogus, &[0]);

        let base_only = <() as WeightInfo>::submit_proof(0, 0, proof.solutions.len() as u32);
        assert_eq!(
            base_only,
            <() as WeightInfo>::submit_proof(0, 0, 0),
            "solutions must not add weight when no topology dimensions apply"
        );

        let call = crate::Call::<Test>::submit_proof {
            proof: proof.clone(),
        };
        assert_eq!(
            call.get_dispatch_info().call_weight,
            base_only,
            "unregistered topology must be charged the base weight only"
        );

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof),
            crate::Error::<Test>::TopologyNotRegistered
        );
    });
}

#[test]
fn weight_regression_small_proof_cost_increased() {
    // Regression test: ensure small proofs still pay reasonable weight

    let tiny_weight = calculate_weight(2, 1, 1);

    // Even minimal proofs should pay at least the base cost
    assert!(
        tiny_weight.ref_time() >= 10_000_000,
        "Minimal proof should pay at least base cost"
    );
}

#[test]
fn weight_proportionality_constant_is_reasonable() {
    // Verify the per-unit weight constants are within reasonable bounds. Each
    // marginal cost is isolated by differencing against the zero-dimension
    // weight, since calculate_weight always includes the fixed extrinsic + DB
    // base (~535M ref_time) that would otherwise swamp the per-unit terms.
    let base = calculate_weight(0, 0, 0).ref_time();
    let per_node = calculate_weight(1, 0, 0).ref_time() - base;
    let per_edge = calculate_weight(0, 1, 0).ref_time() - base;
    let per_solution_node = calculate_weight(1, 0, 1).ref_time() - base;

    // Isolated marginal costs should be reasonable (not zero, not excessive).
    assert!(
        per_node > 0 && per_node < 100_000,
        "Per-node marginal cost should be reasonable, got {per_node}"
    );
    assert!(
        per_edge > 0 && per_edge < 200_000,
        "Per-edge marginal cost should be reasonable, got {per_edge}"
    );
    assert!(
        per_solution_node > per_node,
        "Solution validation adds cost beyond a bare node"
    );
}

// ============================================================================
// End QIP-03 Weight Regression Tests

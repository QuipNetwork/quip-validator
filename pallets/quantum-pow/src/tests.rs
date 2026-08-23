use super::mock::*;
use crate::{
    difficulty, topology,
    types::{DifficultyConfig, ProofRecord, QuantumProof, TopologyHardness},
    AllowedValueSetOf, BlockBestProof, BlockProofCount, DefaultTopology, Difficulties,
    EpochQBlocks, EpochStart, LastProofBlock, LastProofBlockHash, MineableTopologies, Miners,
    PackedSpinBytesOf, QBlockBlockById, QBlockCount, QBlockIdByBlock, QBlocks,
    RegisteredTopologies, TopologyDims, TopologyHardnessOf, WinnerStreak,
};
use frame_support::{
    assert_noop, assert_ok,
    traits::{Get, Hooks, StorageVersion},
    BoundedVec,
};
use quantum_validation::{
    derive_nonce, energy_of_solution, generate_ising_model, packed::pack_solution,
    AllowedValueSpec, MilliValue, Regime, MILLI_SCALE,
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

/// `FrustrationDraw` shorthand for tests. The struct exists because transposing
/// the two `u32`s compiles and flips the handicap's sign; one positional helper
/// states the field order once instead of at ~10 sites.
fn draw(realized_milli: u32, expected_milli: u32, cycles: u64) -> difficulty::FrustrationDraw {
    difficulty::FrustrationDraw {
        realized_milli,
        expected_milli,
        cycles,
    }
}

const SLOPE: i64 = difficulty::DEFAULT_HANDICAP_PER_SIGMA_PERMILLE;
const MAX_STEP: i64 = difficulty::DEFAULT_RETARGET_MAX_STEP_PERMILLE;

fn easy_difficulty() -> DifficultyConfig {
    DifficultyConfig {
        max_energy_milli: i64::MAX,
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

/// Energy curve for the same `(2, 1)` topology and specs `registered_topology()`
/// uses. Tests calling `apply_decay`/`adjust_on_proof` directly need it to match
/// what the pallet computes via `current_energy_curve()`.
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
    // points.
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
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // Acyclic: no fundamental cycles, so no handicap.
            expected_frustration_milli: 0,
        },
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
        frustration_milli: 0,
        instance_bar_milli: 0,
        anchor_milli: i64::MIN / 4,
        difficulty: easy_difficulty(),
    });
    QuantumPow::on_finalize(block_number);
}

fn proof_for(
    miner: u64,
    nodes: &BoundedVec<u32, MaxNodes>,
    edges: &BoundedVec<(u32, u32), MaxEdges>,
    topology_hash: sp_core::H256,
    solution_index: usize,
) -> QuantumProof<crate::PackedSpinBytesOf<Test>> {
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

    let solutions = pack_spins(&by_energy[solution_index].1);

    QuantumProof {
        topology_hash,
        nonce,
        salt,
        solutions,
        device_access_time_us: 0,
    }
}

/// A 4-cycle: one fundamental cycle, so roughly half of all salts draw an
/// unfrustrated instance. Registered as Heuristic: a topology can carry real
/// width and still hand out gauge-trivial draws.
fn registered_cycle_topology() -> (
    BoundedVec<u32, MaxNodes>,
    BoundedVec<(u32, u32), MaxEdges>,
    sp_core::H256,
) {
    let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2, 3]);
    let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (2, 3), (3, 0)]);
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
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            expected_frustration_milli: 500,
        },
    ));
    MineableTopologies::<Test>::insert(hash, ());
    (nodes, edges, hash)
}

/// Best proof this miner can make for `salt` on the 4-cycle, plus the instance's
/// frustration. Brute-forces all 16 assignments, so the proof is always the
/// exact ground state -- the gate must reject on the instance, not the answer.
fn cycle_proof_for(
    miner: u64,
    nodes: &BoundedVec<u32, MaxNodes>,
    edges: &BoundedVec<(u32, u32), MaxEdges>,
    topology_hash: sp_core::H256,
    salt_byte: u8,
) -> (QuantumProof<crate::PackedSpinBytesOf<Test>>, u32) {
    let mut salt = [0u8; 32];
    salt[0] = salt_byte;
    let last = LastProofBlockHash::<Test>::get().0;
    let miner_bytes = crate::Pallet::<Test>::account_to_bytes(&miner);
    let nonce = derive_nonce(&last, &miner_bytes, &salt);
    let (h, j) = generate_ising_model(
        nonce,
        nodes.as_slice(),
        edges.as_slice(),
        &allowed_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
    )
    .unwrap();
    let (frustration_milli, _) =
        quantum_validation::frustration_index(nodes.as_slice(), edges.as_slice(), &j);

    let best = (0..16u32)
        .map(|mask| {
            let spins: Vec<i8> = (0..4)
                .map(|b| if mask >> b & 1 == 1 { 1 } else { -1 })
                .collect();
            let energy =
                energy_of_solution(&spins, &h, edges.as_slice(), &j, nodes.as_slice()).unwrap();
            (energy, spins)
        })
        .min_by_key(|(energy, _)| *energy)
        .unwrap()
        .1;

    (
        QuantumProof {
            topology_hash,
            nonce,
            salt,
            solutions: pack_spins(&best),
            device_access_time_us: 0,
        },
        frustration_milli,
    )
}

/// An unfrustrated draw is gauge-equivalent to a ferromagnet, so its ground state
/// falls out of an O(m) walk. The bar cannot price that away — at any real cycle
/// count a zero draw saturates the ±3σ clamp and buys only 1.8%.
#[test]
fn a_gauge_trivial_instance_is_refused_however_good_the_answer() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, hash) = registered_cycle_topology();
        set_difficulty_default(easy_difficulty());
        Difficulties::<Test>::insert(hash, easy_difficulty());

        let mut trivial = None;
        let mut frustrated = None;
        for salt_byte in 0..64u8 {
            let (proof, phi) = cycle_proof_for(1, &nodes, &edges, hash, salt_byte);
            if phi == 0 && trivial.is_none() {
                trivial = Some(proof);
            } else if phi > 0 && frustrated.is_none() {
                frustrated = Some(proof);
            }
        }
        let trivial = trivial.expect("a 4-cycle draws unfrustrated about half the time");
        let frustrated = frustrated.expect("and frustrated the other half");

        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(1), trivial),
            crate::Error::<Test>::InstanceIsGaugeTrivial
        );
        // The discriminator is the instance: same miner, topology and
        // exact-ground-state solver goes through on a frustrated draw.
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(1),
            frustrated
        ));
    });
}

/// One retarget window in blocks. The loop measures qblock *counts*, so the
/// window deliberately spans many target intervals -- a test that advances a
/// single `EpochLength` will not close one.
fn retarget_window() -> u64 {
    <<Test as crate::Config>::EpochLength as Get<u64>>::get()
        * u64::from(<<Test as crate::Config>::RetargetWindowEpochs as Get<
            u32,
        >>::get())
}

/// The controller must have a proportional band. With a one-interval window the
/// only reachable errors were "no qblocks" (max ease), "one" (dead zone), "two
/// or more" (past the clamp) -- three states, so a chain between them oscillated
/// across a quarter of the curve span and never settled.
#[test]
fn the_retarget_responds_proportionally_around_target() {
    let curve = walkup_curve();
    let epoch = 100_u64;
    let window = epoch * 10; // expects ~10 qblocks
    let bar = curve.knee_milli;
    let step = |qblocks: u32| {
        difficulty::retarget_bar_milli(
            bar,
            curve,
            difficulty::RetargetWindow {
                qblocks,
                epoch_blocks: window,
                target_blocks_per_qblock: epoch,
            },
            MAX_STEP,
        ) - bar
    };

    let span = i64::from(curve.max_milli - curve.min_milli);
    let clamp = span / 4; // RETARGET_MAX_STEP_PERMILLE = 250

    // Slow (few qblocks) eases, fast tightens, on target holds.
    assert_eq!(step(10), 0, "exactly on target must hold");
    assert!(step(8) > 0, "short of target must ease");
    assert!(step(13) < 0, "past target must tighten");

    // Monotone everywhere: more qblocks is never a looser bar. Weakly only,
    // because far from target both counts saturate the same clamp.
    for q in 1..20u32 {
        assert!(
            step(q) >= step(q + 1),
            "response must be non-increasing in qblocks, but {q} gave {} and \
             {} gave {}",
            step(q),
            q + 1,
            step(q + 1),
        );
    }
    // Strictly monotone across the unclamped band -- the property the old
    // controller lacked entirely.
    for q in 8..12u32 {
        assert!(
            step(q) > step(q + 1),
            "response must be strictly decreasing inside the band, but {q} \
             gave {} and {} gave {}",
            step(q),
            q + 1,
            step(q + 1),
        );
    }

    // The band is genuinely interior -- the old controller could only answer
    // these counts with a hold or a full slam.
    for q in [8_u32, 9, 11, 12] {
        let moved = step(q).abs();
        assert!(
            moved > 0 && moved < clamp,
            "qblocks {q} must land strictly inside the clamp, got {moved} \
             against a clamp of {clamp}"
        );
    }
}

/// Decay must stay a pure view: the retarget must never commit it into the
/// stored baseline. Committing compounds -- `LastProofBlock` does not advance
/// when a window closes, so the next window restacks its steps on the
/// already-eased value -- and it is a free miner lever, since withholding the
/// qblock due in the last `EpochLength` blocks banks a permanent ease while the
/// controller still reads on-target. Both windows here are exactly on target, so
/// only committed decay could move the baseline.
#[test]
fn the_retarget_never_banks_decay_into_the_baseline() {
    let bar_after = |last_proof_block: u64| {
        let mut ext = new_test_ext();
        ext.execute_with(|| {
            let (_, _, hash) = registered_topology();
            let curve = QuantumPow::energy_curve_for(hash).expect("curve");
            let stored = DifficultyConfig {
                max_energy_milli: curve.knee_milli,
            };
            Difficulties::<Test>::insert(hash, stored);

            let epoch = <<Test as crate::Config>::EpochLength as Get<u64>>::get();
            let window = retarget_window();
            EpochStart::<Test>::put(1);
            LastProofBlock::<Test>::put(last_proof_block);
            EpochQBlocks::<Test>::insert(hash, (window / epoch) as u32);

            System::set_block_number(window + 1);
            QuantumPow::on_finalize(window + 1);
            (
                Difficulties::<Test>::get(hash).unwrap().max_energy_milli,
                curve.knee_milli,
            )
        })
    };

    // Stale: many decay steps pending. Fresh: none.
    let (stale, knee) = bar_after(1);
    let (fresh, _) = bar_after(retarget_window());
    assert_eq!(
        stale, knee,
        "an on-target window must leave the baseline alone, however much \
         decay the view function was showing"
    );
    assert_eq!(
        stale, fresh,
        "and the baseline must not depend on when the last proof landed, or \
         a miner can bank a permanent ease by timing submissions around the \
         window boundary"
    );
}

/// Decay must terminate in bounded work however long the chain has been silent:
/// `steps` is `elapsed / EpochLength`, reachable from `submit_proof` too.
/// Asserted on the ITERATION COUNT, not the result -- the early exit leaves the
/// result unchanged, so a result-only assertion (the first version of this test)
/// cannot detect its removal.
#[test]
fn decay_stops_iterating_once_it_saturates() {
    let curve = walkup_curve();
    let start = DifficultyConfig {
        max_energy_milli: curve.min_milli,
    };

    let (settled, ran) = difficulty::apply_decay_counted(start, u32::MAX, curve);
    assert_eq!(
        settled.max_energy_milli, curve.max_milli,
        "decay must settle exactly on the easy cap"
    );
    assert!(
        ran < 5_000,
        "a saturating decay must stop early, but it ran {ran} of u32::MAX \
         iterations"
    );

    // The early exit is free: a longer bound gives the identical answer, so
    // every skipped iteration was a no-op.
    let (long, _) = difficulty::apply_decay_counted(start, ran + 10_000, curve);
    assert_eq!(long.max_energy_milli, settled.max_energy_milli);
}

/// Closes the gap the pure-function proportional-band test leaves: it picks its
/// own window and target, so it cannot catch the pallet passing the wrong pair.
/// If the pallet passed the window as both arguments, expected would be one, a
/// full window of qblocks would read as far too fast, and the bar would slam a
/// quarter of the span.
#[test]
fn the_pallet_measures_the_window_against_the_target_interval() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let curve = QuantumPow::energy_curve_for(hash).expect("curve");
        let stored = DifficultyConfig {
            max_energy_milli: curve.knee_milli,
        };
        Difficulties::<Test>::insert(hash, stored);

        let epoch = <<Test as crate::Config>::EpochLength as Get<u64>>::get();
        let window = retarget_window();
        let expected = (window / epoch) as u32;
        assert!(
            expected >= 4,
            "the window must span several target intervals or the controller \
             has no band to be proportional over"
        );

        EpochStart::<Test>::put(1);
        LastProofBlock::<Test>::put(window);
        EpochQBlocks::<Test>::insert(hash, expected);
        System::set_block_number(window + 1);
        QuantumPow::on_finalize(window + 1);

        assert_eq!(
            Difficulties::<Test>::get(hash).unwrap().max_energy_milli,
            stored.max_energy_milli,
            "a window producing exactly the expected qblocks is on target and \
             must not move the bar"
        );
    });
}

/// `expected_frustration_milli` must stay away from BOTH ends of its range, not
/// just below 1000. The handicap divides by `sigma = sqrt(p(1-p)/cycles)`; at
/// either endpoint sigma collapses and the handicap becomes a step function.
#[test]
fn a_degenerate_expected_frustration_is_refused() {
    new_test_ext().execute_with(|| {
        let nodes = bounded::<_, MaxNodes>(vec![0, 1]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1)]);
        let register = |expected_frustration_milli| {
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                nodes.clone(),
                edges.clone(),
                allowed_h_spec(),
                allowed_j_spec(),
                allowed_spin_spec(),
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    expected_frustration_milli,
                },
            )
        };
        // Zero variance: sigma floors to one micro.
        assert_noop!(register(1000), crate::Error::<Test>::InvalidHardnessRecord);
        // Near-zero variance, same failure less obviously.
        assert_noop!(register(1), crate::Error::<Test>::InvalidHardnessRecord);
        // Acyclic topologies legitimately declare zero and skip the handicap.
        assert_ok!(register(0));
    });
}

/// A curve calibrated past the topology's own typical weight disconnects the
/// retarget: the per-instance bar is `max(curve_bar, anchor)`, so an anchor above
/// the easiest curve value wins for every bar and the chain can never ease.
#[test]
fn a_curve_the_typical_anchor_would_override_is_refused() {
    new_test_ext().execute_with(|| {
        let (nodes, edges, hash) = registered_topology();
        let curve = QuantumPow::energy_curve_for(hash).expect("curve");

        // Computed here, not read off the curve: the expected bound is no longer
        // a field -- only the registration guard read it, yet every difficulty
        // read paid to produce it.
        let expected_bound = quantum_validation::expected_bound_milli(
            nodes.len() as u32,
            edges.len() as u32,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .expect("the registered specs are valid");

        // Sanity: the registered curve sits inside the topology's typical
        // weight, which is why registration accepted it.
        assert!(
            -(expected_bound as i64) <= curve.max_milli,
            "fixture precondition: typical anchor {} must sit at or below the \
             easy cap {}",
            -(expected_bound as i64),
            curve.max_milli
        );

        // Demanding several times the total available weight puts the whole
        // curve below the typical anchor.
        assert_noop!(
            QuantumPow::set_topology_curve(
                RuntimeOrigin::root(),
                hash,
                crate::difficulty::CurveC {
                    easy_milli: 5_000,
                    knee_milli: 5_001,
                    hard_milli: 5_002,
                },
            ),
            crate::Error::<Test>::InvalidCurve
        );
        // A sane recalibration still goes through: the refusal is about the
        // anchor, not about touching the curve at all.
        assert_ok!(QuantumPow::set_topology_curve(
            RuntimeOrigin::root(),
            hash,
            test_curve_c(),
        ));
    });
}

/// An oversized rotation must be refused on its shape, before it is copied. The
/// weight is dimensioned on the TOPOLOGY, so without an early shape check a
/// caller pays topology-sized for submission-sized work: a 16-node topology can
/// still make the node materialize a block-length witness.
#[test]
fn an_oversized_planar_witness_is_refused_on_shape() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        // Zero-field, or the field gate refuses before shape is looked at --
        // correct, but it would not exercise this.
        let hash = registered_zero_field_topology();
        let nodes = RegisteredTopologies::<Test>::get(hash)
            .expect("registered")
            .nodes;

        // 4x the topology's node count, each entry padded -- the largest
        // overshoot `MaxNodes` admits, i.e. the bound alone does not keep the
        // work proportional.
        let bloated = bounded::<_, MaxNodes>(
            (0..nodes.len() * 4)
                .map(|_| bounded::<u32, MaxEdges>(vec![0, 1, 0, 1]))
                .collect::<Vec<_>>(),
        );
        assert_noop!(
            QuantumPow::prove_topology_planar(RuntimeOrigin::signed(1), hash, bloated),
            crate::Error::<Test>::InvalidPlanarEmbedding
        );

        // Right node count but the wrong number of dart slots is refused too:
        // every edge must appear exactly once at each endpoint.
        let wrong_slots = bounded::<_, MaxNodes>(
            (0..nodes.len())
                .map(|_| bounded::<u32, MaxEdges>(vec![0, 0, 0, 0, 0, 0]))
                .collect::<Vec<_>>(),
        );
        assert_noop!(
            QuantumPow::prove_topology_planar(RuntimeOrigin::signed(1), hash, wrong_slots),
            crate::Error::<Test>::InvalidPlanarEmbedding
        );
    });
}

/// The topology hash must be a commitment to the instance it identifies.
/// `hash_topology` sorts nodes and orients-then-sorts edges, but
/// `generate_ising_model` maps `h[i]`/`j[k]` to `nodes[i]`/`edges[k]`
/// POSITIONALLY. Without a canonical stored graph, two registrations differing
/// only in edge order hash identically yet generate DIFFERENT puzzles.
#[test]
fn the_stored_graph_is_canonical_so_the_hash_pins_the_instance() {
    let stored_for = |nodes: Vec<u32>, edges: Vec<(u32, u32)>| {
        let mut ext = new_test_ext();
        ext.execute_with(|| {
            let nodes = bounded::<_, MaxNodes>(nodes);
            let edges = bounded::<_, MaxEdges>(edges);
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
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    expected_frustration_milli: 500,
                },
            ));
            let stored = RegisteredTopologies::<Test>::get(hash).expect("registered");
            (hash, stored.nodes.to_vec(), stored.edges.to_vec())
        })
    };

    // Same graph, submitted with nodes and edges permuted and some edges
    // written backwards.
    let (hash_a, nodes_a, edges_a) =
        stored_for(vec![0, 1, 2, 3], vec![(0, 1), (1, 2), (2, 3), (0, 3)]);
    let (hash_b, nodes_b, edges_b) =
        stored_for(vec![3, 1, 0, 2], vec![(3, 2), (1, 0), (3, 0), (2, 1)]);

    assert_eq!(hash_a, hash_b, "the hash is already order-independent");
    assert_eq!(
        (nodes_a, edges_a),
        (nodes_b, edges_b),
        "so the STORED graph must be too, or the same hash names two \
         different instance generations"
    );
}

/// Beyond the handicap's reach an instance stops being valid rather than
/// becoming under-priced. The handicap is denominated in sampling noise, and
/// tractability is not noise: past +3σ the clamp returns a bar only 1.8%
/// tighter than typical, which is exactly where a grinder wants to be.
#[test]
fn a_draw_past_the_handicaps_reach_is_refused() {
    // The gate reads the same function the handicap does, so the boundary
    // cannot drift between them.
    let sigmas = |phi| difficulty::frustration_sigmas(phi, 500, 10_000);
    assert_eq!(sigmas(500), 0, "an exactly typical draw is zero sigma");
    assert!(sigmas(400) > difficulty::HANDICAP_MAX_SIGMA);
    assert!(sigmas(600) < -difficulty::HANDICAP_MAX_SIGMA);

    // Symmetric: both tails are outside the band, and both are refused.
    assert!(sigmas(490).abs() <= difficulty::HANDICAP_MAX_SIGMA);
    assert!(sigmas(510).abs() <= difficulty::HANDICAP_MAX_SIGMA);
    assert!(sigmas(400).abs() > difficulty::HANDICAP_MAX_SIGMA);
    assert!(sigmas(600).abs() > difficulty::HANDICAP_MAX_SIGMA);

    // The handicap saturates at the same place, which is what makes the gate
    // the right boundary rather than an arbitrary one.
    let bar = |phi| difficulty::instance_bar_milli(-6_000, 10_000, draw(phi, 500, 10_000), SLOPE);
    assert_eq!(
        bar(400),
        bar(300),
        "past the clamp the handicap is flat -- so the bar cannot distinguish \
         a somewhat easy draw from a very easy one, and the gate must"
    );
}

/// A coupling spec that is NOT sign-symmetric, so `expected_frustration_for`
/// returns `None` and the DECLARED `expected_frustration_milli` survives
/// registration instead of being overwritten with the derived 500.
///
/// That is the only lever that moves the expectation away from what draws
/// actually produce, which is what the band test needs.
fn asymmetric_j_spec() -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![-SCALE, 2 * SCALE]))
}

/// Five nodes, eight edges: `m - n + 1 == 4` fundamental cycles, registered
/// with a declared expectation the chain will not overwrite.
///
/// Four cycles rather than the 4-cycle's one, because sigma is
/// `sqrt(p(1-p)/cycles)` and one cycle makes it 500 milli — wide enough that no
/// reachable draw is 3 sigma out. One cycle also admits only `phi in {0, 1000}`,
/// and `phi == 0` is refused as gauge-trivial BEFORE the band is consulted, so
/// the easy side is unreachable there at all. Four cycles give `phi` a step of
/// 250 and shrink sigma by half, which puts both sides in range.
fn registered_multicycle_topology(
    expected_frustration_milli: u32,
) -> (
    BoundedVec<u32, MaxNodes>,
    BoundedVec<(u32, u32), MaxEdges>,
    sp_core::H256,
) {
    let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2, 3, 4]);
    let edges = bounded::<_, MaxEdges>(vec![
        (0, 1),
        (0, 2),
        (0, 3),
        (1, 2),
        (1, 3),
        (2, 3),
        (2, 4),
        (3, 4),
    ]);
    let hash = topology::hash_topology(
        &nodes,
        &edges,
        &allowed_h_spec().as_slice(),
        &asymmetric_j_spec().as_slice(),
        &allowed_spin_spec().as_slice(),
    );
    assert_ok!(QuantumPow::register_topology(
        RuntimeOrigin::root(),
        nodes.clone(),
        edges.clone(),
        allowed_h_spec(),
        asymmetric_j_spec(),
        allowed_spin_spec(),
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            expected_frustration_milli,
        },
    ));
    MineableTopologies::<Test>::insert(hash, ());
    assert_eq!(
        TopologyHardnessOf::<Test>::get(hash)
            .expect("classified")
            .expected_frustration_milli,
        expected_frustration_milli,
        "an asymmetric spec must keep its DECLARED expectation; if this is 500 \
         the spec became sign-symmetric and every case below is measuring the \
         derived value instead"
    );
    (nodes, edges, hash)
}

/// The instance's frustration under `salt`, plus a proof carrying it.
///
/// Spins are all `+1` and are NOT optimized. The band gate runs BEFORE
/// `validate_proof` and before the energy comparison, so proof quality cannot
/// influence the verdict — and with `easy_difficulty` the bar is `i64::MAX`, so
/// an unoptimized answer still clears it. That is what lets the accepted case
/// below isolate the band as the only difference.
fn drawn_proof_for(
    miner: u64,
    nodes: &BoundedVec<u32, MaxNodes>,
    edges: &BoundedVec<(u32, u32), MaxEdges>,
    topology_hash: sp_core::H256,
    salt_byte: u8,
) -> (QuantumProof<crate::PackedSpinBytesOf<Test>>, u32, u64) {
    let mut salt = [0u8; 32];
    salt[0] = salt_byte;
    let last = LastProofBlockHash::<Test>::get().0;
    let miner_bytes = crate::Pallet::<Test>::account_to_bytes(&miner);
    let nonce = derive_nonce(&last, &miner_bytes, &salt);
    let (_, j) = generate_ising_model(
        nonce,
        nodes.as_slice(),
        edges.as_slice(),
        &allowed_h_spec().as_slice(),
        &asymmetric_j_spec().as_slice(),
    )
    .unwrap();
    let (frustration_milli, cycles) =
        quantum_validation::frustration_index(nodes.as_slice(), edges.as_slice(), &j);
    let spins: Vec<i8> = vec![1; nodes.len()];
    (
        QuantumProof {
            topology_hash,
            nonce,
            salt,
            solutions: pack_spins(&spins),
            device_access_time_us: 0,
        },
        frustration_milli,
        cycles,
    )
}

/// The band gate, THROUGH THE EXTRINSIC, on both sides.
///
/// This test previously asserted no refusal at all despite its name: it
/// submitted an accepted draw, checked the expectation was derived, and checked
/// an honest draw was inside the band — three positives under a name promising a
/// rejection. Deleting `submit_proof`'s `ensure!` left it green, which is the
/// one thing it existed to prevent.
///
/// The gate must be two-sided. Refusing only the easy tail leaves grinding
/// UPWARD free, and the upper side is the valuable half: the handicap LOOSENS
/// the bar for a hard draw, so an over-stated slope pays for reaching into the
/// hard tail. Each side is asserted separately, because a one-sided
/// implementation passes any test that only exercises the other.
#[test]
fn submit_proof_refuses_a_draw_outside_the_expected_frustration_band() {
    // `expected` far ABOVE what draws produce => the draw reads as unusually
    // easy => positive sigma. Far BELOW => unusually hard => negative sigma.
    // Both are refused; the middle is accepted.
    //
    // Asserted INSIDE the externalities: `assert_noop!` also checks that
    // storage is unchanged, and handing it an already-evaluated `Result` would
    // make that half trivially true. Returns the sigma only so the caller can
    // report it.
    let case = |expected: u32, refused: bool| -> i64 {
        new_test_ext().execute_with(|| {
            assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
            let (nodes, edges, hash) = registered_multicycle_topology(expected);
            set_difficulty_default(easy_difficulty());
            Difficulties::<Test>::insert(hash, easy_difficulty());

            // SEARCH for a salt on the side under test. Four cycles give `phi`
            // a step of 250, and only some steps are past the band for a given
            // expectation — at `expected = 950`, `phi = 750` is 1 sigma (inside)
            // while `phi = 500` is 4 (outside). Taking the first non-zero draw
            // silently tested the wrong thing.
            //
            // `phi > 0` throughout: a zero draw is refused as gauge-trivial one
            // `ensure!` EARLIER, so it would satisfy the refusal for the wrong
            // reason. That is the failure mode this whole test was rewritten
            // for, and it must not come back through the fixture.
            let (proof, phi, cycles) = (0..64u8)
                .find_map(|salt| {
                    let (proof, phi, cycles) = drawn_proof_for(1, &nodes, &edges, hash, salt);
                    let sigmas = difficulty::frustration_sigmas(phi, expected, cycles);
                    let outside = sigmas.abs() > difficulty::HANDICAP_MAX_SIGMA;
                    (phi > 0 && outside == refused).then_some((proof, phi, cycles))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no salt in 0..64 draws a non-zero frustration \
                         {} the band at expected={expected}",
                        if refused { "outside" } else { "inside" }
                    )
                });
            let sigmas = difficulty::frustration_sigmas(phi, expected, cycles);

            if refused {
                assert_noop!(
                    QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof),
                    crate::Error::<Test>::InstanceOutsideFrustrationBand
                );
            } else {
                assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
            }
            sigmas
        })
    };

    // ACCEPTED: the expectation matches the draw distribution, so sigma is
    // small. Without this the two refusals prove only that the fixture fails.
    let mid = case(500, false);
    // REFUSED, easy side. `MAX_EXPECTED_FRUSTRATION_MILLI` is 950.
    let easy = case(950, true);
    // REFUSED, hard side. `MIN_EXPECTED_FRUSTRATION_MILLI` is 50. This is the
    // side a one-sided gate would let through.
    let hard = case(50, true);

    assert!(
        easy > 0 && hard < 0,
        "the two refusals must sit on OPPOSITE sides, or the gate is only \
         proven in one direction: easy={easy} hard={hard} (control {mid})"
    );
}

/// A genesis chain whose default topology has never been mined must still ease
/// itself out. `Difficulties` is written only by the v3 migration, root
/// `set_difficulty` and the retarget, so a never-won default has no entry -- and
/// decay is off while `LastProofBlock` is the genesis zero. If the retarget
/// skips it too, an unclearable knee bar bricks the chain.
#[test]
fn a_never_mined_default_still_eases_from_genesis() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let curve = QuantumPow::energy_curve_for(hash).expect("curve");
        // Exactly the genesis state: no stored bar, no proof ever.
        assert!(Difficulties::<Test>::get(hash).is_none());
        assert_eq!(LastProofBlock::<Test>::get(), 0);

        let window = retarget_window();
        EpochStart::<Test>::put(1);
        for w in 1..=6u64 {
            System::set_block_number(w * window + 1);
            QuantumPow::on_finalize(w * window + 1);
        }

        let bar = QuantumPow::current_difficulty_for(hash, 6 * window + 1).max_energy_milli;
        assert!(
            bar > curve.knee_milli,
            "six empty windows must have eased the bar off the knee, got {bar}"
        );
        assert_eq!(
            bar, curve.max_milli,
            "and a chain producing nothing must reach the easiest calibrated bar"
        );
    });
}

/// A spec that can draw an all-zero instance is refused only where that draw is
/// reachable. Zero bound means every configuration has energy zero, so any bar
/// is cleared for no work. But on a large graph every node and edge would have
/// to land on zero at once; refusing for that would bar an ordinary ternary
/// coupling spec.
#[test]
fn a_topology_that_can_draw_all_zeros_is_refused_only_when_reachable() {
    new_test_ext().execute_with(|| {
        let zeroable_j =
            || AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![-1000, 0, 1000]));
        let hardness = |expected_frustration_milli| TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            expected_frustration_milli,
        };

        // Two nodes and one edge: three draws, so all-zero is reachable.
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                bounded::<_, MaxNodes>(vec![0, 1]),
                bounded::<_, MaxEdges>(vec![(0, 1)]),
                allowed_h_spec(),
                zeroable_j(),
                allowed_spin_spec(),
                hardness(0),
            ),
            crate::Error::<Test>::InstanceIsGaugeTrivial
        );

        // 16 nodes, 32 edges: 48 draws, so all-zero sits below 2^-48 and the
        // realized near-zero case is the per-instance gates' job.
        let big_edges: Vec<(u32, u32)> = (0..16u32)
            .flat_map(|u| ((u + 1)..16).map(move |v| (u, v)))
            .take(32)
            .collect();
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            bounded::<_, MaxNodes>((0..16u32).collect::<Vec<_>>()),
            bounded::<_, MaxEdges>(big_edges),
            allowed_h_spec(),
            zeroable_j(),
            allowed_spin_spec(),
            hardness(500),
        ));
    });
}

/// The v6 canonicalization step must fix graphs registered before the change.
/// Registration canonicalizes now, but every topology already on chain (including
/// a live default) still carries submission order, and until it does not the hash
/// does not name the instance the chain generates.
#[test]
fn the_v6_migration_canonicalizes_an_already_registered_graph() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let meta = RegisteredTopologies::<Test>::get(hash).expect("registered");

        // Write it back in a deliberately non-canonical order, as a pre-v6
        // runtime would have.
        RegisteredTopologies::<Test>::insert(
            hash,
            crate::types::TopologyMeta {
                nodes: bounded::<_, MaxNodes>(vec![1, 0]),
                edges: bounded::<_, MaxEdges>(vec![(1, 0)]),
                ..meta
            },
        );
        // 5 is where the deployed chain actually is, so this drives the real
        // upgrade path rather than a version that only ever existed on a branch.
        StorageVersion::new(5).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        let after = RegisteredTopologies::<Test>::get(hash).expect("still registered");
        assert_eq!(after.nodes.to_vec(), vec![0, 1], "nodes must be sorted");
        assert_eq!(
            after.edges.to_vec(),
            vec![(0, 1)],
            "edges must be oriented and sorted"
        );
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
    });
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
        let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (0, 2)]);
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
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
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    // Acyclic: no fundamental cycles, so no handicap.
                    expected_frustration_milli: 0,
                },
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
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // Acyclic: no fundamental cycles, so no handicap.
            expected_frustration_milli: 0,
        },
    ));
    MineableTopologies::<Test>::insert(hash, ());
    hash
}

/// Registers a distinct 2-node topology (nodes `[a, b]`, single edge) WITHOUT
/// whitelisting it. Distinct node ids give a distinct hash, so callers get a
/// registered-but-un-mineable topology to drive `add_mineable_topology`.
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
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // Acyclic: no fundamental cycles, so no handicap.
            expected_frustration_milli: 0,
        },
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
        // Start inside B's range. A's range is disjoint, so easing under A's
        // curve lands elsewhere — the difference this test asserts. An
        // out-of-range start would decay floor-only and be curve-independent,
        // defeating the sanity check below.
        let initial = DifficultyConfig {
            max_energy_milli: curve_b.knee_milli,
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
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    // Acyclic: no fundamental cycles, so no handicap.
                    expected_frustration_milli: 0,
                },
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
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    expected_frustration_milli: 0,
                },
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
            max_energy_milli: -1_000,
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
            max_energy_milli: -2_000,
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
fn set_difficulty_works() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let difficulty = DifficultyConfig {
            max_energy_milli: -2_000,
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
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);

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
        let mut proof = proof_for(1, &nodes, &edges, topology_hash, 0);
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
        let mut proof = proof_for(1, &nodes, &edges, topology_hash, 0);
        // Replace the packed solution with one byte too many for a 2-spin
        // binary-encoded solution (1 byte is enough; we send 2 bytes).
        proof.solutions = bounded::<u8, MaxNodes>(vec![0, 0]);

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

        let mut worse = proof_for(1, &nodes, &edges, topology_hash, 3);
        worse.device_access_time_us = 111;
        let mut better = proof_for(2, &nodes, &edges, topology_hash, 0);
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

/// Selection uses the same yardstick as admission. The incumbent has far deeper
/// absolute energy but never beat its own bar; the challenger clears its bar.
/// Ranking on raw energy pays whoever ground the largest instance, not the work.
#[test]
fn the_block_goes_to_the_best_margin_not_the_deepest_energy() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        // Ground a huge instance, then miss its bar by 1000 milli.
        BlockBestProof::<Test>::put(ProofRecord {
            miner: 99,
            submitted_at: 1,
            energy_milli: -5_000,
            salt: [0u8; 32],
            topology_hash,
            device_access_time_us: 0,
            frustration_milli: 500,
            instance_bar_milli: -6_000,
            anchor_milli: i64::MIN / 4,
            difficulty: easy_difficulty(),
        });

        let challenger = proof_for(1, &nodes, &edges, topology_hash, 0);
        assert_ok!(QuantumPow::submit_proof(
            RuntimeOrigin::signed(1),
            challenger
        ));

        let best = BlockBestProof::<Test>::get().unwrap();
        assert_eq!(best.miner, 1, "the proof that beat its own bar must win");
        assert!(
            best.energy_milli > -5_000,
            "and it must win despite far shallower absolute energy, or this \
             test is not exercising the margin rule at all"
        );
        assert!(
            best.energy_milli.saturating_sub(best.instance_bar_milli) < 1_000,
            "winner's margin must beat the incumbent's +1000"
        );
    });
}

/// Selection must not be winnable by drawing a bigger instance -- the attack a
/// raw `energy - bar` margin does not stop. Where the anchor binds, the best
/// reachable margin is `anchor - bar`, which grows with `Σ|h| + Σ|J|`, so
/// grinding salts for magnitude (O(n + m) per try, no solving) beats every weak
/// draw however well it was solved. Here the strong draw finds a sliver of its
/// room and the weak draw is solved exactly; normalized ranking picks the weak.
#[test]
fn a_bigger_draw_does_not_beat_a_better_solve() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();

        // Ground exactly: all of its (small) room used. The room is
        // deliberately nonzero -- a bar clamped flush to the anchor scores
        // lowest, a different case tested elsewhere.
        let weak_but_perfect = ProofRecord {
            miner: 1,
            submitted_at: 1,
            energy_milli: -1_200,
            anchor_milli: -1_200,
            difficulty: easy_difficulty(),
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 0,
            frustration_milli: 500,
            instance_bar_milli: -1_000,
        };
        // 3x the reach, barely used: deeper absolutely and a better raw margin,
        // but only a tenth of its own room.
        let strong_but_lazy = ProofRecord {
            miner: 2,
            submitted_at: 1,
            energy_milli: -2_500,
            anchor_milli: -12_000,
            difficulty: easy_difficulty(),
            salt: [1u8; 32],
            topology_hash: hash,
            device_access_time_us: 0,
            frustration_milli: 500,
            instance_bar_milli: -1_500,
        };

        // The strong draw wins on both of the rules this replaces.
        assert!(strong_but_lazy.energy_milli < weak_but_perfect.energy_milli);
        assert!(
            strong_but_lazy.energy_milli - strong_but_lazy.instance_bar_milli
                < weak_but_perfect.energy_milli - weak_but_perfect.instance_bar_milli
        );

        BlockBestProof::<Test>::put(strong_but_lazy);
        assert!(
            QuantumPow::proof_outranks_block_best(&weak_but_perfect),
            "a proof that used all of its instance's room must beat one that \
             used a tenth of a bigger instance's"
        );
    });
}

#[test]
fn on_finalize_pays_block_reward_for_best_proof() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);
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
        // current_difficulty_for(topology), not the raw Difficulties entry. Park
        // the baseline just under the proof's best energy so the same-block
        // submit is refused, then let one epoch of decay admit the same proof.
        //
        // Miner 7's triangle makes this expressible: frustrated, so the optimum
        // (-4000) sits strictly above the absolute floor (-6000) — leaving room
        // for a rejecting bar the clamp would otherwise raise back to the floor
        // — and below the curve's easy cap (-3841), decay's limit.
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(7)));
        let (nodes, edges, topology_hash) = triangle_topology();
        LastProofBlock::<Test>::put(1);

        let (by_energy, anchor) = triangle_energies(7, &nodes, &edges, topology_hash);
        let energy_milli = by_energy[0].0;
        let curve = crate::difficulty::EnergyCurve::new(
            3,
            3,
            test_curve_c(),
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .unwrap();
        // Pin the preconditions so a future nonce-derivation change fails
        // loudly here rather than silently making the test vacuous.
        assert!(
            anchor < energy_milli,
            "precondition: the optimum ({energy_milli}) must sit above the \
             absolute floor ({anchor}), or no rejecting bar survives the clamp"
        );
        assert!(
            energy_milli <= curve.max_milli,
            "precondition: the optimum ({energy_milli}) must sit at or below \
             the easy cap ({}), which is as far as decay eases",
            curve.max_milli
        );

        // 2% under the best energy, not one milli: sign-symmetric couplings mean
        // a derived expected frustration of 500, which turns the handicap on.
        // The handicap can loosen the bar by 3 sigma x 6 per-mille = 1.8%, so a
        // one-milli margin sits inside that noise.
        let tight = energy_milli * 102 / 100;
        set_difficulty_default(DifficultyConfig {
            max_energy_milli: tight,
        });
        let early = triangle_proof(7, &nodes, &edges, topology_hash, 0);
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(7), early),
            crate::Error::<Test>::InsufficientEnergy
        );

        // One epoch later the identical proof is admitted, which is only
        // possible if validation read the decayed value.
        System::set_block_number(21); // (21 - 1) / EpochLength(20) = 1 step
        let decayed = QuantumPow::current_difficulty_for(topology_hash, 21);
        assert!(
            decayed.max_energy_milli > tight,
            "decay must ease the bar above the rejecting baseline"
        );
        let late = triangle_proof(7, &nodes, &edges, topology_hash, 0);
        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(7), late));
    });
}

// NOTE: the end-to-end per-proof difficulty tests were removed with the
// per-proof rate-band walk itself — it fought the epoch retarget on a slow chain
// (hardened on a slow win while the retarget eased for being behind cadence), so
// difficulty has one dial. The band's policy is still pinned against
// `adjust_on_proof_with_dominance` in `repeated_same_winner_forces_easing`; the
// retarget is covered by `retarget_*` and `a_stalled_chain_*`.

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
        // A v2 chain holds the pre-shrink three-field layout.
        let old = LegacyDifficultyConfig {
            min_solutions: 5,
            max_energy_milli: -14_620,
            min_diversity_milli: 200,
        };
        let old_key = crate::migration::v3::old_difficulty_key::<Test>();
        frame_support::storage::unhashed::put(&old_key, &old);
        StorageVersion::new(2).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert_eq!(
            Difficulties::<Test>::get(hash),
            Some(DifficultyConfig {
                max_energy_milli: old.max_energy_milli
            }),
            "global difficulty carried to default topology, shrunk to the bar"
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
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
        // The live threshold for the default now equals the carried value.
        assert_eq!(
            QuantumPow::current_difficulty_for(hash, System::block_number()),
            DifficultyConfig {
                max_energy_milli: old.max_energy_milli
            }
        );
    });
}

#[test]
fn winner_streak_is_tracked_across_consecutive_wins() {
    new_test_ext().execute_with(|| {
        registered_topology();
        set_difficulty_default(DifficultyConfig {
            max_energy_milli: test_curve().knee_milli,
        });
        LastProofBlock::<Test>::put(1);

        finalize_winner(1, 100);
        finalize_winner(1, 200);
        finalize_winner(1, 300);

        let streak = WinnerStreak::<Test>::get().expect("winner streak tracked");
        assert_eq!(streak.miner, 1);
        assert_eq!(streak.count, 3);
    });
}

#[test]
fn migration_below_v2_wipes_then_bumps_to_current() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        QBlockCount::<Test>::put(9);
        StorageVersion::new(1).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert!(RegisteredTopologies::<Test>::iter().next().is_none());
        assert_eq!(DefaultTopology::<Test>::get(), None);
        assert_eq!(QBlockCount::<Test>::get(), 0);
        assert!(!MineableTopologies::<Test>::contains_key(hash));
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
    });
}

/// `DifficultyConfig` as encoded through storage v5, before the shrink to a
/// bare energy bar. Migration tests plant it so the translate paths decode
/// what a real pre-v6 chain holds rather than today's layout.
#[derive(codec::Encode, Clone, Copy, Debug)]
struct LegacyDifficultyConfig {
    min_solutions: u32,
    max_energy_milli: i64,
    min_diversity_milli: u32,
}

fn legacy_easy_difficulty() -> LegacyDifficultyConfig {
    LegacyDifficultyConfig {
        min_solutions: 1,
        max_energy_milli: i64::MAX,
        min_diversity_milli: 0,
    }
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
    difficulty: LegacyDifficultyConfig,
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
            difficulty: legacy_easy_difficulty(),
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
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
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
    difficulty: LegacyDifficultyConfig,
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
            difficulty: legacy_easy_difficulty(),
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
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
    });
}

#[test]
fn migration_v5_to_v6_shrinks_difficulty_preserving_the_bar() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let legacy = LegacyDifficultyConfig {
            min_solutions: 7,
            max_energy_milli: -1_000,
            min_diversity_milli: 300,
        };
        frame_support::storage::unhashed::put(&Difficulties::<Test>::hashed_key_for(hash), &legacy);
        let d = DifficultyConfig {
            max_energy_milli: -1_000,
        };
        QBlockCount::<Test>::put(9);
        StorageVersion::new(5).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert!(RegisteredTopologies::<Test>::contains_key(hash));
        assert_eq!(Difficulties::<Test>::get(hash), Some(d));
        assert_eq!(QBlockCount::<Test>::get(), 9);
        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
    });
}

#[test]
fn single_adjustment_never_slams_past_a_cap() {
    let curve = test_curve();
    // Regression: the old total-range step let a max-roll adjustment overshoot
    // far past `min_milli` and pin the ceiling. Under the geometric model a
    // single adjustment moves at most the remaining room (or one floor step at
    // the tail), so it overshoots the hard cap by at most one energy unit and
    // never eases past the easy cap.
    const MIN_DELTA: i64 = 1000; // mirrors MIN_ENERGY_DELTA_MILLI
    for rate_milli in (0_u32..=650).step_by(13) {
        for &start in &[curve.min_milli, curve.knee_milli, curve.max_milli] {
            for direction in [difficulty::Direction::Harder, difficulty::Direction::Easier] {
                let adjusted = difficulty::adjust_energy_along_curve(
                    start, rate_milli, direction, curve, MIN_DELTA,
                );
                assert!(
                    adjusted >= curve.min_milli - MIN_DELTA && adjusted <= curve.max_milli,
                    "rate {rate_milli}, start {start}, {direction:?}: adjusted \
                     threshold {adjusted} slammed past a cap (allowed [{}, {}])",
                    curve.min_milli - MIN_DELTA,
                    curve.max_milli,
                );
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
            max_energy_milli: curve.knee_milli,
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
        // Guards the streak *not* resetting: count 3 would force easing, raising
        // the threshold. Hardening may walk below `min_milli` by design, so
        // assert only that the reset win still hardens.
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
        // Capture the round's seed before submit_proof, so the assertion pins
        // the chain-stored value against what the helper fed derive_nonce.
        let expected_last_proof_block_hash =
            sp_core::H256::from(crate::Pallet::<Test>::hash_to_bytes_32(
                frame_system::Pallet::<Test>::block_hash(LastProofBlock::<Test>::get()),
            ));
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);
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

        // The round-trip that lets dashboards recover the nonce from on-chain
        // state alone.
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
        // Genesis (block 0) has no storage entry, so the helper short-circuits
        // before any block-hash arithmetic: saturating `0u32 - 1` never reaches
        // the nonce derivation.
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
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
        // The QBlock stores the *active* threshold the proof had to clear (decay
        // applied, pre-adjust), not the post-adjustment Difficulty<T> value.
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let initial = DifficultyConfig {
            max_energy_milli: i64::MAX,
        };
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(initial);

        LastProofBlock::<Test>::put(1);
        System::set_block_number(45); // (45 - 1) / 20 = 2 decay steps
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);
        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        QuantumPow::on_finalize(System::block_number());

        let expected_active = difficulty::apply_decay(initial, 2, test_curve());
        let stored = QBlocks::<Test>::get(45).expect("winner persisted");
        assert_eq!(
            stored.difficulty, expected_active,
            "stored difficulty must be the decayed-but-pre-adjust threshold"
        );

        // The win itself leaves the baseline alone: only the retarget moves it.
        assert_eq!(difficulty_default(), initial);
    });
}

#[test]
fn mining_snapshot_returns_decayed_difficulty_after_epochs() {
    new_test_ext().execute_with(|| {
        // mining_snapshot.difficulty applies decay, so the runtime API gives the
        // live threshold, not the stale Difficulty<T> baseline polkadot.js
        // storage queries return. The baseline is in-range so decay actually
        // eases it; past the easy cap it would be a no-op (vacuous).
        let initial = DifficultyConfig {
            max_energy_milli: -2_300,
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

        // The direct storage query still returns the undecayed baseline — the
        // visibility gap the runtime API closes.
        assert_eq!(difficulty_default(), initial);
    });
}

#[test]
fn submit_proof_survives_long_block_gap() {
    // Regression for the txpool-delay race: a proof derived against the round's
    // `last_proof_block_hash` stays valid while no new proof has won, however
    // many blocks elapse. The old block-number-bound contract gave InvalidNonce.
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        // Derive at block 1 (default after new_test_ext).
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);

        // A txpool backed up by 500 blocks. No win in between, so
        // `LastProofBlock` is unchanged and the round is the same.
        System::set_block_number(501);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
        assert_eq!(BlockProofCount::<Test>::get(), 1);
        assert!(BlockBestProof::<Test>::get().is_some());
    });
}

#[test]
fn submit_proof_rejected_after_intervening_win() {
    // Mirror of the survives-gap test: if a *different* round closed between
    // derivation and submission the hash has changed and the proof must be
    // rejected, or old proofs replay across rounds.
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, topology_hash) = registered_topology();
        set_difficulty_default(easy_difficulty());

        // Derive against the current hash; its value is incidental — what
        // matters is that the lookup target is mutated below.
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);

        // Force a distinct hash: point `LastProofBlock` at a fresh block,
        // populate its `block_hash`, and write the cached `LastProofBlockHash`
        // submit_proof reads. on_initialize normally refreshes that cache from
        // parent_hash(); tests don't run hooks, so mimic it explicitly.
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
        max_energy_milli: -2_500,
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
        max_energy_milli: -2_500,
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
        max_energy_milli: -2_500,
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
        max_energy_milli: -2_500,
    };
    // last_proof_block == 0 → genesis path → no decay.
    assert_eq!(
        difficulty::current_difficulty(500, base, 0, 10, Some(curve)),
        base,
    );
}

#[test]
fn apply_decay_only_mutates_max_energy() {
    let curve = test_curve();
    let before = DifficultyConfig {
        max_energy_milli: -2_500,
    };
    let after = difficulty::apply_decay(before, 3, curve);
    assert!(
        after.max_energy_milli > before.max_energy_milli,
        "decay must ease the threshold (move toward zero)"
    );
}

#[test]
fn decay_moves_less_than_hardening_per_step() {
    // Midpoint start equalizes the distance to each cap, isolating the rate
    // difference (fast harden ≥ 5% of remaining gap vs decay's fixed 2.5%) from
    // the geometric room asymmetry. On the tiny test_curve every step floors to
    // a bound, so use the production-scale curve.
    let curve = walkup_curve();
    let midpoint = (curve.min_milli + curve.max_milli) / 2;
    let start = DifficultyConfig {
        max_energy_milli: midpoint,
    };
    let after_decay = difficulty::apply_decay(start, 5, curve);
    // 350 per-mille is the hardening band's fast end; decay is a fixed 25.
    let after_harden = (0..5).fold(midpoint, |e, _| {
        difficulty::adjust_energy_along_curve(e, 350, difficulty::Direction::Harder, curve, 1_000)
    });
    let decay_move = after_decay.max_energy_milli - midpoint;
    let harden_move = midpoint - after_harden;
    assert!(
        harden_move > decay_move,
        "hardening must move energy farther than decay at equal step count \
         (harden={harden_move}, decay={decay_move})",
    );
}

#[test]
fn harden_motion_grows_with_distance_from_hard_cap() {
    // The geometric model steps a fraction of the gap *remaining* to the hard
    // cap, so a threshold far from the cap moves farther — the inverse of the
    // retired curve_factor, which compressed motion at the edges and let
    // mid-curve fast wins slam the ceiling. Same rate, only the room differs.
    let curve = walkup_curve();
    let near_cap = curve.min_milli + 50_000; // little room to the hard cap
    let far_from_cap = curve.max_milli; // maximum room to the hard cap
    let harden = |start: i64| {
        start
            - difficulty::adjust_energy_along_curve(
                start,
                350,
                difficulty::Direction::Harder,
                curve,
                1_000,
            )
    };
    let near_move = harden(near_cap);
    let far_move = harden(far_from_cap);
    assert!(
        far_move > near_move,
        "a harden far from the hard cap must move more than one near it \
         (far={far_move}, near={near_move})",
    );
}

/// Miner-independence invariant (GitLab issue #5): the energy curve depends only
/// on `DefaultTopology`. Two topologies of different sizes; the decay seen via
/// `mining_snapshot` must match the default's curve, so routing
/// `current_energy_curve()` through a non-default topology fails here.
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
        ));
        // First-write-wins keeps A as the default.
        assert_eq!(DefaultTopology::<Test>::get(), Some(default_hash));

        // Start inside A's range so decay actually eases (past the easy cap it
        // would be a no-op). A and B have different easy caps, so each curve
        // yields a different decayed value — what the sanity check relies on.
        let curve_a = test_curve();
        let initial = DifficultyConfig {
            max_energy_milli: curve_a.knee_milli,
        };
        set_difficulty_default(initial);
        LastProofBlock::<Test>::put(1);
        System::set_block_number(101); // (101 - 1) / 20 = 5 decay steps

        // `mining_snapshot` populates `difficulty` via current_difficulty.
        let snapshot =
            QuantumPow::mining_snapshot(None).expect("snapshot exists for default topology");

        // Expected decay using A's curve (the default).
        let expected_default = difficulty::apply_decay(initial, 5, test_curve());
        // Decay under B's curve — what a miner-controlled curve would produce if
        // the invariant were broken.
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

/// Identical Set values in different orders must collide via `topology_hash`.
/// Before the canonicalize_spec fix the STORED order matched the caller's, so
/// sample()/decode_value() walked unsorted while the hash claimed canonical.
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
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

/// `ContinuousRange` spin specs need 4 bytes per spin, overflowing the
/// `BoundedVec<u8, MaxNodes>` packed-solution bound when `nodes > MaxNodes / 4`.
/// Reject at registration rather than ship a topology no proof can satisfy.
#[test]
fn register_topology_rejects_unmineable_continuous_spin_spec() {
    new_test_ext().execute_with(|| {
        let mut node_ids: Vec<u32> = (0..(MaxNodes::get() / 2)).collect();
        // Keep the test above MaxNodes/4 even for very small MaxNodes.
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
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    expected_frustration_milli: 0,
                },
            ),
            crate::Error::<Test>::PackedSolutionTooLarge
        );
    });
}

/// `LastProofBlockHash` must stay stable for the whole round even after
/// `frame_system::block_hash(LastProofBlock)` ages out of the `BlockHashCount`
/// ring. Before the fix a round longer than `BlockHashCount` saw the nonce seed
/// flip to the zero hash mid-round and rejected every in-flight proof.
#[test]
fn last_proof_block_hash_stable_after_block_hash_ages_out() {
    use frame_support::traits::Hooks;

    new_test_ext().execute_with(|| {
        // A winning proof at block 5: on_initialize for block 6 is what captures
        // the cache from parent_hash() (== block_hash(5) in production).
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

        // Jump far past `BlockHashCount` (production default 256) with a
        // different parent_hash. The cache must NOT be overwritten:
        // LastProofBlock is unchanged, so the seed stays bound to block 5.
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
    // Tiny rate so the geometric step rounds to 0: room = 159 milli, rate =
    // 0.001 → round(0.159) = 0. The floor lifts the step to `min_delta_milli`
    // (100 < the 159-milli gap, so it binds without overshoot).
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

/// Production-scale curve for the geometric walk-up tests. `test_curve()`'s
/// 159-milli range sits below the delta floor, which would flatten every step
/// and hide the geometric behaviour these tests pin.
fn walkup_curve() -> crate::difficulty::EnergyCurve {
    crate::difficulty::EnergyCurve {
        min_milli: -16_000_000,
        knee_milli: -15_600_000,
        max_milli: -14_000_000,
        // Matches the knee's magnitude, so the scale term is ~1 and these tests
        // measure the geometric walk rather than the ratio.
    }
}

/// Core of the slam-and-stall fix: well inside the curve a single harden steps
/// exactly `room * rate` of the gap remaining to the hard cap and, since
/// `rate < 1`, stays strictly inside it. The retired total-range step let one
/// fast win overshoot `min_milli` and pin the ceiling. The floored tail is
/// covered by `harden_tail_walks_past_cap_by_min_delta`.
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
            // Every sampled room is >= 100_000 milli, far above the 5000 floor,
            // so the geometric step stays strictly inside the hard cap.
            assert!(
                result > curve.min_milli,
                "a geometric harden (room >> floor) must stay strictly inside \
                 the hard cap (current={current}, rate={rate_milli}, result={result})"
            );
        }
    }
}

/// Tail behaviour: near, at and below the hard cap the geometric term is under
/// the floor, so a harden steps exactly one floor — walking the threshold *past*
/// `min_milli` to track a stronger-than-estimated field. Hardening is uncapped
/// at this end, so it crosses the cap rather than landing on it.
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
    // From the hard cap itself a harden walks one floor below it: the threshold
    // is free to track past the hard estimate.
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

/// End-to-end regression for the observed testnet slam: replay the inter-win gap
/// series (29, 280, 27, 200, 100, 300, 1401 blocks) as decay-then-retarget
/// rounds; the base threshold must never pin `min_milli`. The easy cap IS
/// allowed, unlike under the retired per-proof walk — the series averages a
/// qblock every ~330 blocks against a 100-block target, so most epochs close
/// empty and take the full clamped ease. Saturating `max_milli` is correct for a
/// chain running 3x slow, and the first win tightens again.
#[test]
fn observed_win_series_never_pins_the_hard_cap() {
    let curve = walkup_curve();
    let epoch_len = 100_u64;
    let mut base = DifficultyConfig {
        max_energy_milli: curve.knee_milli, // start at the field level
    };
    for (i, gap) in [29_u64, 280, 27, 200, 100, 300, 1401]
        .into_iter()
        .enumerate()
    {
        // Decay eases the live threshold by one step per elapsed epoch …
        let steps = (gap / epoch_len) as u32;
        let active = difficulty::apply_decay(base, steps, curve);
        // … and each of those epochs closed with no qblock, so the retarget
        // eases; the epoch the win lands in closes with one.
        base.max_energy_milli = active.max_energy_milli;
        for epoch in 0..=steps {
            let qblocks = u32::from(epoch == steps);
            base.max_energy_milli = difficulty::retarget_bar_milli(
                base.max_energy_milli,
                curve,
                difficulty::RetargetWindow {
                    qblocks,
                    epoch_blocks: epoch_len,
                    target_blocks_per_qblock: epoch_len,
                },
                MAX_STEP,
            );
        }
        assert!(
            base.max_energy_milli > curve.min_milli && base.max_energy_milli <= curve.max_milli,
            "round {i} (gap {gap}) left the curve: {} not in ({}, {}]",
            base.max_energy_milli,
            curve.min_milli,
            curve.max_milli,
        );
    }
}

/// Climbing from the field level to the hard cap takes many wins, never one. The
/// old total-range step saturated the curve on a single 35% fast win; the
/// geometric step closes ~35% of the *remaining* gap, so it takes >10 wins.
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
/// step is at the hard ceiling — the inverse of the retired curve, which eased
/// slowest there and caused multi-thousand-block recovery stalls.
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
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);

        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));

        let record = BlockBestProof::<Test>::get().expect("best proof recorded");
        assert_eq!(record.topology_hash, topology_hash);
    });
}

#[test]
fn the_retarget_attributes_qblocks_to_the_topology_that_won_them() {
    new_test_ext().execute_with(|| {
        // Two whitelisted topologies; A wins every qblock this epoch, B none. A
        // global counter would credit A's throughput to B and retarget B as if
        // it were keeping pace — the window a `set_default_topology` switch
        // opens, the one time two topologies are mineable at once.
        let (_, _, hash_a) = registered_topology();
        let hash_b = registered_zero_field_topology();
        let diff_a = DifficultyConfig {
            max_energy_milli: -10_000,
        };
        let diff_b = DifficultyConfig {
            max_energy_milli: -20_000,
        };
        Difficulties::<Test>::insert(hash_a, diff_a);
        Difficulties::<Test>::insert(hash_b, diff_b);

        let epoch = retarget_window();
        EpochStart::<Test>::put(1);
        LastProofBlock::<Test>::put(1);

        // One win for A, then close the window.
        System::set_block_number(epoch + 1);
        BlockBestProof::<Test>::put(ProofRecord {
            miner: 1,
            submitted_at: epoch + 1,
            energy_milli: -10_000,
            salt: [0u8; 32],
            topology_hash: hash_a,
            device_access_time_us: 0,
            frustration_milli: 0,
            instance_bar_milli: 0,
            anchor_milli: i64::MIN / 4,
            difficulty: easy_difficulty(),
        });
        QuantumPow::on_finalize(epoch + 1);

        // Each topology retargets on ITS OWN count. B is whitelisted but not
        // default, so its zero qblocks mean idle, not stalled: it is held, or an
        // incoming topology would walk to its easy cap during the handover and
        // hand the repoint a burst of near-free qblocks. `expect` mirrors the
        // pallet's arguments exactly -- window measured against `EpochLength` as
        // the target interval; passing the window as both would expect one
        // qblock and reproduce the degenerate controller.
        let expect = |hash, before: DifficultyConfig, qblocks| {
            difficulty::retarget_bar_milli(
                before.max_energy_milli,
                QuantumPow::energy_curve_for(hash).expect("curve"),
                difficulty::RetargetWindow {
                    qblocks,
                    epoch_blocks: epoch,
                    target_blocks_per_qblock:
                        <<Test as crate::Config>::EpochLength as Get<u64>>::get(),
                },
                MAX_STEP,
            )
        };
        assert_eq!(
            Difficulties::<Test>::get(hash_a).unwrap().max_energy_milli,
            expect(hash_a, diff_a, 1)
        );
        assert_eq!(
            Difficulties::<Test>::get(hash_b).unwrap().max_energy_milli,
            diff_b.max_energy_milli,
            "an idle non-default topology must be held, not eased"
        );
        assert_ne!(
            expect(hash_b, diff_b, 0),
            diff_b.max_energy_milli,
            "and the hold must be the gate, not a retarget that happens to \
             be a no-op here"
        );
        // And the counters reset for the next epoch.
        assert_eq!(EpochQBlocks::<Test>::get(hash_a), 0);
        assert_eq!(EpochQBlocks::<Test>::get(hash_b), 0);
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
            frustration_milli: 0,
            instance_bar_milli: 0,
            anchor_milli: i64::MIN / 4,
            difficulty: easy_difficulty(),
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
        // A registers first so it claims the auto-whitelisted default; the
        // topology under test must not be the first registration.
        let (nodes, edges, _) = registered_topology();

        // B: raw call, no whitelisting. Distinct node ids ([5, 6] vs [0, 1]) →
        // distinct hash; same value specs as A so the proof is well-formed up to
        // the mineable check.
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
        ));
        assert!(
            !MineableTopologies::<Test>::contains_key(topology_hash),
            "second registration must not be auto-whitelisted"
        );
        Difficulties::<Test>::insert(topology_hash, easy_difficulty());
        // Build against A's graph (so `proof_for`'s two-spin candidate set is
        // consistent) but claim B's un-mineable hash. The whitelist check fires
        // before any solution validation.
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);

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

        // Membership here is what makes a topology mineable.
        assert_ok!(QuantumPow::add_mineable_topology(
            RuntimeOrigin::root(),
            hash_b
        ));
        assert!(MineableTopologies::<Test>::contains_key(hash_b));

        // Re-adding is a no-op success hitting the `!contains_key` skip branch.
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

        // A silent success hitting the `contains_key` skip branch.
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
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
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
            max_energy_milli: -10_000,
        };
        let db = DifficultyConfig {
            max_energy_milli: -20_000,
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
            max_energy_milli: -20_000,
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
                                                       // Falls back to THIS topology's curve knee, not the hardcoded
                                                       // -1_200_000 default: that predates per-topology curves and is
                                                       // trivially clearable on a large graph, unclearable on a small one.
        let curve = QuantumPow::energy_curve_for(hash_b).expect("registered topology has a curve");
        assert_eq!(
            QuantumPow::current_difficulty_for(hash_b, System::block_number()),
            DifficultyConfig {
                max_energy_milli: curve.knee_milli
            }
        );
        assert_ne!(
            curve.knee_milli,
            DifficultyConfig::default().max_energy_milli
        );
    });
}

// ============================================================================
// Weight Regression Tests (QIP-03: Parameterized Weight Accounting)
// ============================================================================

use crate::weights::WeightInfo;

fn calculate_weight(nodes: u32, edges: u32) -> frame_support::weights::Weight {
    <() as WeightInfo>::submit_proof(nodes, edges)
}

#[test]
fn weight_scales_with_proof_dimensions() {
    // Mathematical invariant: W(n, e) should increase with dimensions

    let weight_small = calculate_weight(2, 1);
    let weight_medium = calculate_weight(100, 200);
    let weight_large = calculate_weight(1000, 5000);

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
        hardness: TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            expected_frustration_milli: 0,
        },
    };

    assert_eq!(
        call.get_dispatch_info().call_weight,
        <() as WeightInfo>::register_topology(3, 2, 7),
    );
}

#[test]
fn weight_formula_components_are_present() {
    // Verify both components of W(n, e) = BASE + Kₙ·n + Kₑ·e

    let base_weight = calculate_weight(0, 0);
    assert!(base_weight.ref_time() > 0, "Base weight should be non-zero");

    // Component Kₙ·n: nodes contribute linearly
    let w_100_nodes = calculate_weight(100, 0);
    let w_200_nodes = calculate_weight(200, 0);
    assert!(
        w_200_nodes.ref_time() > w_100_nodes.ref_time(),
        "Node component (Kₙ·n) should increase with node count"
    );

    // Component Kₑ·e: edges contribute linearly
    let w_100_edges = calculate_weight(0, 100);
    let w_200_edges = calculate_weight(0, 200);
    assert!(
        w_200_edges.ref_time() > w_100_edges.ref_time(),
        "Edge component (Kₑ·e) should increase with edge count"
    );
}

#[test]
fn weight_prevents_undercharging_for_large_proofs() {
    // Large proofs must cost far more than small ones. The mock's Max* bounds
    // are tiny (16/32/8), so exercise the formula directly at the production
    // bounds where the dimensional terms dominate the fixed base.
    let small_proof_weight = calculate_weight(2, 1); // Minimal proof
    let large_proof_weight = calculate_weight(5_000, 50_000); // Production worst case

    // Large proof should cost significantly more (at least 10x).
    let ratio = large_proof_weight.ref_time() / small_proof_weight.ref_time().max(1);
    assert!(
        ratio >= 10,
        "Worst-case proof should cost at least 10x more than minimal proof, got {ratio}x"
    );
}

// `weight_scales_quadratically_with_solutions_for_diversity` is gone with its subject. It was
// already vacuous before the type change: `k₅` (the only s² coefficient) was 0, so the weight
// was linear in s and its `f(8) - f(4) > f(4) - f(2)` check held for any increasing f.

// ============================================================================
// Sweep coverage: the charged envelope against recorded reference sweeps
// ============================================================================

/// The four fixed points `scripts/run-quantum-pow-sweeps.sh` records. Node and
/// edge floors mirror the benchmark's independence clamps: nodes floor where a
/// simple graph carries `MaxEdges` (317), edges floor at `n + 4`.
const EXPECTED_SWEEP_POINTS: [(&str, u32, u32); 4] = [
    ("minimum", 317, 321),
    ("nodes", 5_000, 5_004),
    ("edges", 317, 50_000),
    ("worst_case", 5_000, 50_000),
];

fn expected_sweep_dimensions(point: &str) -> (u32, u32) {
    EXPECTED_SWEEP_POINTS
        .iter()
        .find_map(|(name, nodes, edges)| (*name == point).then_some((*nodes, *edges)))
        .unwrap_or_else(|| panic!("unexpected sweep point: {point}"))
}

fn assert_weight_covers_observation(
    source: &str,
    point: &str,
    nodes: u32,
    edges: u32,
    maximum_ns: u64,
    required_margin_percent: u128,
) {
    let charged = u128::from(calculate_weight(nodes, edges).ref_time());
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
         {margin_whole}.{margin_fraction:02}% margin; required {required_margin_percent}%"
    );
}

#[test]
fn weight_covers_recorded_sweeps_with_twenty_percent_target() {
    // Gated on the recorded steady-state MINIMUM, not the maximum: the current
    // fixture was recorded on a non-isolated arm64 host whose scheduler tail
    // inflates maxima by up to 50x over the sample body, which carries no
    // information about verifier cost. The minimum is the one statistic that
    // survives that noise, and it is already the stricter side of the
    // undercharge question (a charge below even the FLOOR is definitely
    // wrong). The reference-machine CI job gates the maximum via
    // `weight_covers_executed_sweep_with_ten_percent_floor`, where isolation
    // keeps the spread tight. `max_ns` is recorded for review, not gated.
    const RECORDED_SWEEPS: &str = include_str!("../testdata/submit-proof-sweeps.tsv");
    const EXPECTED_HEADER: &str = "host\tcommit_sha\tpoint\tnodes\tedges\tsamples\tmin_ns\tmax_ns";

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

        let host = columns[0];
        let commit_sha = columns[1];
        let point = columns[2];
        let nodes = columns[3].parse::<u32>().expect("recorded nodes");
        let edges = columns[4].parse::<u32>().expect("recorded edges");
        let samples = columns[5].parse::<u32>().expect("recorded samples");
        let minimum_ns = columns[6].parse::<u64>().expect("recorded minimum");
        let maximum_ns = columns[7].parse::<u64>().expect("recorded maximum");

        assert!(!host.is_empty(), "missing host in row: {line}");
        assert_eq!(commit_sha.len(), 40, "invalid commit SHA in row: {line}");
        assert!(
            commit_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "non-hex commit SHA in row: {line}"
        );
        assert!(samples > 0, "recorded sample count must be nonzero");
        assert!(
            minimum_ns <= maximum_ns,
            "recorded timings are not ordered for point {point}"
        );
        assert_eq!(
            (nodes, edges),
            expected_sweep_dimensions(point),
            "recorded dimensions do not match point {point}"
        );
        assert!(
            seen.insert((host.to_owned(), point)),
            "duplicate recorded sweep row for host {host}, point {point}"
        );

        assert_weight_covers_observation(
            &format!("host {host}"),
            point,
            nodes,
            edges,
            minimum_ns,
            20,
        );
    }

    for (point, _, _) in EXPECTED_SWEEP_POINTS {
        assert!(
            seen.iter().any(|(_, seen_point)| *seen_point == point),
            "missing recorded sweep row for point {point}"
        );
    }
}

#[test]
#[ignore = "requires QUANTUM_POW_SWEEP_SUMMARY from a completed sweep"]
fn weight_covers_executed_sweep_with_ten_percent_floor() {
    const EXPECTED_HEADER: &str = "point\tnodes\tedges\tsamples\tmin_ns\tmedian_ns\tmax_ns";

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
        assert_eq!(columns.len(), 7, "invalid sweep summary row: {line}");

        let point = columns[0];
        let nodes = columns[1].parse::<u32>().expect("summary nodes");
        let edges = columns[2].parse::<u32>().expect("summary edges");
        let samples = columns[3].parse::<u32>().expect("summary samples");
        let minimum_ns = columns[4].parse::<u64>().expect("summary minimum");
        let median_ns = columns[5].parse::<u64>().expect("summary median");
        let maximum_ns = columns[6].parse::<u64>().expect("summary maximum");

        assert!(samples > 0, "summary sample count must be nonzero");
        assert!(
            minimum_ns <= median_ns && median_ns <= maximum_ns,
            "summary timings are not ordered for point {point}"
        );
        assert_eq!(
            (nodes, edges),
            expected_sweep_dimensions(point),
            "summary dimensions do not match point {point}"
        );
        assert!(
            seen.insert(point.to_owned()),
            "duplicate sweep summary row for point {point}"
        );

        assert_weight_covers_observation(&summary_path, point, nodes, edges, maximum_ns, 10);
    }

    for (point, _, _) in EXPECTED_SWEEP_POINTS {
        assert!(seen.contains(point), "missing sweep summary point {point}");
    }
    assert_eq!(seen.len(), 4, "sweep summary must contain all four points");
}

#[test]
fn weight_cannot_overflow_at_the_input_bounds() {
    // The predecessor asserted saturation to u64::MAX, because the old s²·n term reached
    // ~2^96 at u32::MAX and had to be clamped. Two terms cannot reach the clamp — the worst
    // case is ~9.4e14 — so assert exactness instead: no input collapses the charge onto
    // u64::MAX and loses all dimensional information.
    let max_weight = calculate_weight(u32::MAX, u32::MAX);
    // Differenced against the zero-dimension weight so this does not restate the DB costs
    // (`calculate_weight` adds reads/writes on top of the formula).
    let base = calculate_weight(0, 0).ref_time();
    let expected = base + 6_000 * u32::MAX as u64 + 500_000 * u32::MAX as u64;
    assert_eq!(
        max_weight.ref_time(),
        expected,
        "worst case must be exact, not clamped"
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

        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);

        // The charge is the parameterized formula keyed to the registered
        // topology's dimensions (n/e from the topology, not the proof).
        let charged = <() as WeightInfo>::submit_proof(nodes.len() as u32, edges.len() as u32);
        assert!(
            charged.ref_time() >= 50_000_000,
            "parameterized weight must cover the base extrinsic cost"
        );

        // QIP-03 guarantee: a large proof is charged well above the retired 60M
        // flat weight, so large proofs can no longer be under-charged.
        let large = calculate_weight(1_000, 5_000);
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
    // The QIP-03 fix lives in the #[pallet::weight] closure, not WeightInfo:
    // pins the *dispatched* charge to the formula at the registered topology's
    // dimensions (n/e from storage via proof.topology_hash, s from the proof).
    // A regression to the flat weight, or transposed n/e, fails here.
    use frame_support::dispatch::GetDispatchInfo;
    new_test_ext().execute_with(|| {
        let (nodes, edges, topology_hash) = registered_topology();
        let proof = proof_for(1, &nodes, &edges, topology_hash, 0);
        let expected = <() as WeightInfo>::submit_proof(nodes.len() as u32, edges.len() as u32);
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
    // An unregistered topology_hash is charged the zero-dimension base, and no
    // attacker-supplied payload can raise or lower it. Pins the closure's
    // unwrap_or((0, 0)) fallback.
    use frame_support::dispatch::GetDispatchInfo;
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
        let (nodes, edges, _) = registered_topology();
        let bogus = sp_core::H256::repeat_byte(0xAB);
        let proof = proof_for(1, &nodes, &edges, bogus, 0);

        // Read/write counts spelled as literals, not as the `SUBMIT_PROOF_READS/WRITES`
        // constants: the mock's `DbWeight` is `RocksDbWeight`, the same one the `()` impl
        // uses, so referencing them would compare each side against itself and let the
        // counts drift unnoticed.
        let base_only = calculate_weight(0, 0);
        assert_eq!(
            base_only.ref_time(),
            50_000_000
                + <Test as frame_system::Config>::DbWeight::get()
                    .reads(9)
                    .ref_time()
                + <Test as frame_system::Config>::DbWeight::get()
                    .writes(4)
                    .ref_time(),
            "an unregistered hash pays BASE plus charged DB weight, nothing dimensional"
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

    let tiny_weight = calculate_weight(2, 1);

    // Even minimal proofs should pay at least the base cost
    assert!(
        tiny_weight.ref_time() >= 50_000_000,
        "Minimal proof should pay at least base cost"
    );
}

#[test]
fn weight_proportionality_constant_is_reasonable() {
    // Each marginal cost is isolated by differencing against the zero-dimension
    // weight: calculate_weight always includes the fixed extrinsic + DB base
    // (~675M ref_time), which would otherwise swamp the per-unit terms.
    let base = calculate_weight(0, 0).ref_time();
    let per_node = calculate_weight(1, 0).ref_time() - base;
    let per_edge = calculate_weight(0, 1).ref_time() - base;

    // Pinned exactly, not bounded: the constants are the whole claim of the two-term
    // formula. Kₙ is still the collapse k₁ + k₃ + k₅; Kₑ was recalibrated to the sweep
    // fixture after the collapsed 212_000 measurably undercharged the end-to-end
    // verifier (see the constants' comment in weights.rs).
    assert_eq!(per_node, 6_000, "Kₙ is k₁ + k₃ + k₅");
    assert_eq!(
        per_edge, 500_000,
        "Kₑ is the sweep-recalibrated per-edge charge"
    );
}

// ============================================================================
// End QIP-03 Weight Regression Tests

// ───────────────── per-instance bar (the salt handicap) ─────────────────

#[test]
fn instance_bar_equalizes_in_units_of_sigma() {
    use difficulty::instance_bar_milli;
    // Curve -6_000, floor -10_000, expected frustration 500 over 100 cycles,
    // so sigma = sqrt(500*500/100) = 50 milli and one sigma is worth 6
    // per-mille of the curve bar.
    let bar = |frustration| instance_bar_milli(-6_000, 10_000, draw(frustration, 500, 100), SLOPE);

    // A typical draw clears exactly the curve.
    assert_eq!(bar(500), -6_000);
    // One sigma easier: 6 per-mille tighter, not 20x. The handicap scales to how
    // much instances actually differ, not to an anchor nothing reaches.
    assert_eq!(bar(450), -6_036);
    // One sigma harder earns the mirrored slack, so grinding upward buys
    // nothing — the old one-sided form punished only honest miners.
    assert_eq!(bar(550), -5_964);
    // The swing is capped at +/-3 sigma either way.
    assert_eq!(bar(0), bar(350));
    assert_eq!(bar(1_000), bar(650));
    let span = bar(0) - bar(1_000);
    assert_eq!(
        span, -216,
        "total swing must stay within +/-1.8% of the curve"
    );

    // Monotone: less frustration never buys a looser bar.
    for phi in 0..1_000u32 {
        assert!(bar(phi) <= bar(phi + 1), "bar must not loosen as phi falls");
    }
}

/// The defect riff-hv9.14 caught, at the scale it caught it: with Z(12,4)'s
/// numbers the old form gave a -1 sigma draw a bar ~19% tighter purely from
/// sampling noise, and let a grinder reach the loosest bar in ~2 draws.
#[test]
fn production_scale_bar_is_bounded_and_grinding_upward_is_worthless() {
    use difficulty::instance_bar_milli;
    // Z(12,4): m = 45_864 unit couplings, so the floor is -45_864_000 milli
    // and there are m - n + 1 = 41_065 fundamental cycles.
    let bound = 45_864_000u64;
    let cycles = 41_065u64;
    let curve = -1_200_000i64;
    let bar = |phi| instance_bar_milli(curve, bound, draw(phi, 500, cycles), SLOPE);

    // sigma = sqrt(500*500/41065) = 2 milli at this scale.
    let honest = bar(500);
    assert_eq!(honest, curve);

    // An ordinary -1 sigma draw must cost a couple of percent, not a fifth.
    let unlucky = bar(498);
    let penalty = (honest - unlucky) as f64 / curve.abs() as f64;
    assert!(
        penalty.abs() <= 0.01,
        "a 1-sigma draw moved the bar by {:.1}% of the curve",
        penalty.abs() * 100.0
    );

    // Across the plausible range the bar stays within the clamp, so the retarget
    // sees a difficulty dial rather than a lottery.
    //
    // The bound is DERIVED, not a literal: the handicap is at most
    // `HANDICAP_MAX_SIGMA * SLOPE` per-mille of the curve — 3 x 6 = 18 per-mille
    // = 1.8% — and that is the same clamp `bar(0) == bar(350)` demonstrates
    // above. A hardcoded number here disagreed with the assertion it sat on
    // (the comment claimed 6%), which is exactly the drift computing it prevents:
    // retuning `SLOPE` now moves the prose, the bound and the mechanism together.
    let max_drift = (difficulty::HANDICAP_MAX_SIGMA * SLOPE) as f64 / 1000.0;
    assert_eq!(max_drift, 0.018, "the clamp's width moved");
    for phi in 480..=520u32 {
        let drift = (bar(phi) - curve) as f64 / curve.abs() as f64;
        assert!(
            drift.abs() <= max_drift,
            "phi {phi} moved the bar {:.1}% off the curve, past the \
             {:.1}% the +/-{}sigma clamp allows at slope {SLOPE}",
            drift.abs() * 100.0,
            max_drift * 100.0,
            difficulty::HANDICAP_MAX_SIGMA
        );
    }

    // Grinding upward is worthless: symmetric handicap, and past the +3 sigma
    // clamp every draw gets the same bar. Asserted as properties rather than
    // hand-computed indices, so a change in sigma's precision cannot silently
    // rewrite the claim while still passing.
    assert_eq!(bar(500) - bar(502), bar(498) - bar(500));
    let loosest = bar(1_000);
    for phi in 512..=1_000u32 {
        assert_eq!(bar(phi), loosest, "phi {phi} must sit on the clamp");
    }
    assert!(
        bar(500) < loosest,
        "the clamp must be looser than an average draw, or it buys nothing"
    );
    assert!(
        (loosest - curve) as f64 / curve.abs() as f64 <= 0.019,
        "and the whole upward swing stays inside the stated bound"
    );
}

#[test]
fn instance_bar_clamps_an_unreachable_curve_to_the_optimum() {
    use difficulty::instance_bar_milli;
    // `expected_gse` overshoots on low-degree graphs — a bare cycle's knee sits
    // below -Sum|J|, so no configuration clears it. The clamp is what keeps such
    // a topology mineable.
    assert_eq!(
        instance_bar_milli(-65_620, 64_000, draw(500, 500, 100), SLOPE),
        -64_000
    );
    // With no registered expectation the bar is the clamped curve.
    assert_eq!(
        instance_bar_milli(-65_620, 64_000, draw(500, 0, 100), SLOPE),
        -64_000
    );
    assert_eq!(
        instance_bar_milli(-6_000, 64_000, draw(500, 0, 100), SLOPE),
        -6_000
    );
}

#[test]
fn grinding_a_less_frustrated_instance_tightens_its_bar() {
    new_test_ext().execute_with(|| {
        let (nodes, edges, hash) = registered_topology();
        assert_ok!(QuantumPow::set_topology_hardness(
            RuntimeOrigin::root(),
            hash,
            TopologyHardness {
                residual_difficulty: 31,
                core_width: 31,
                expected_frustration_milli: 500,
            }
        ));
        let hardness = TopologyHardnessOf::<Test>::get(hash).expect("record stored");
        assert_eq!(hardness.expected_frustration_milli, 500);

        // Reproduce the bar the pallet computes for this miner's instance.
        let proof = proof_for(3, &nodes, &edges, hash, 0);
        let (h, j) = generate_ising_model(
            proof.nonce,
            nodes.as_slice(),
            edges.as_slice(),
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
        )
        .unwrap();
        let bound = quantum_validation::energy_bound_milli(&h, &j);
        let realized =
            quantum_validation::frustration_index_milli(nodes.as_slice(), edges.as_slice(), &j);
        let curve = -1_000i64;
        let at_realized =
            difficulty::instance_bar_milli(curve, bound, draw(realized, 500, 100), SLOPE);
        // Half the realized frustration must be a STRICTLY tighter bar, not
        // merely "not looser" — the earlier form compared the same call to
        // itself and would have passed for any implementation at all.
        let half =
            difficulty::instance_bar_milli(curve, bound, draw(realized / 2, 500, 100), SLOPE);
        assert!(
            half < at_realized || realized == 0,
            "an easier draw ({}) must face a tighter bar than {realized}: {half} vs {at_realized}",
            realized / 2
        );
        assert!(
            difficulty::instance_bar_milli(curve, bound, draw(0, 500, 100), SLOPE) <= at_realized
        );
    });
}

#[test]
fn set_topology_hardness_is_root_only_and_replaceable() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let record = TopologyHardness {
            residual_difficulty: 3,
            core_width: 3,
            expected_frustration_milli: 500,
        };
        assert_noop!(
            QuantumPow::set_topology_hardness(RuntimeOrigin::signed(1), hash, record),
            sp_runtime::DispatchError::BadOrigin
        );
        assert_noop!(
            QuantumPow::set_topology_hardness(
                RuntimeOrigin::root(),
                sp_core::H256::repeat_byte(9),
                record
            ),
            crate::Error::<Test>::TopologyNotRegistered
        );
        assert_ok!(QuantumPow::set_topology_hardness(
            RuntimeOrigin::root(),
            hash,
            record
        ));
        assert_eq!(TopologyHardnessOf::<Test>::get(hash), Some(record));

        // Governance can re-classify downward — the toolkit's verdict can
        // tighten without the chain having to refuse the topology.
        let narrower = TopologyHardness {
            residual_difficulty: 2,
            core_width: 2,
            expected_frustration_milli: 480,
        };
        assert_ok!(QuantumPow::set_topology_hardness(
            RuntimeOrigin::root(),
            hash,
            narrower
        ));
        assert_eq!(TopologyHardnessOf::<Test>::get(hash), Some(narrower));

        // But not upward: re-inflating a topology's apparent hardness would
        // undo a ratchet a witness established, so root cannot do it either.
        assert_noop!(
            QuantumPow::set_topology_hardness(
                RuntimeOrigin::root(),
                hash,
                TopologyHardness {
                    residual_difficulty: 31,
                    core_width: 31,
                    expected_frustration_milli: 480,
                }
            ),
            crate::Error::<Test>::WidthWidened
        );

        // A per-mille field above 1000 describes no graph and is refused.
        assert_noop!(
            QuantumPow::set_topology_hardness(
                RuntimeOrigin::root(),
                hash,
                TopologyHardness {
                    residual_difficulty: 1,
                    core_width: 1,
                    expected_frustration_milli: 1_001,
                }
            ),
            crate::Error::<Test>::InvalidHardnessRecord
        );
    });
}

/// The refusal half is gone: a two-configuration proof is no longer a constructible value,
/// so the guarantee moved from a runtime `ensure!` to the type. The dead `TooManySolutions`
/// variant is retained for consensus-visible error indices — see its declaration in `lib.rs`.
#[test]
fn submit_proof_refuses_an_empty_packed_solution() {
    // REPLACES a positive test that duplicated `submit_proof_accepts_valid_proof`
    // with fewer assertions, and covers the one guard the type change left naked.
    //
    // `NoSolutionsSubmitted` had ZERO test coverage after `TooManySolutions` became
    // unrepresentable: deleting the `ensure!` outright passed the whole suite. It is
    // an early reject rather than a correctness gate — `unpack_solution` would refuse
    // a zero-byte payload regardless, with `PackedSolutionLengthMismatch` — so the
    // thing worth pinning is precisely WHICH refusal fires, because that is the only
    // observable difference between the guard being present and absent.
    //
    // Emptying `solutions` after construction keeps the nonce valid: the nonce is
    // derived from (last_proof_block_hash, miner, salt) and never from the payload,
    // so this reaches the guard instead of dying earlier on `InvalidNonce`.
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(3)));
        let (nodes, edges, hash) = registered_topology();
        set_difficulty_default(easy_difficulty());
        let mut proof = proof_for(3, &nodes, &edges, hash, 0);
        proof.solutions = bounded::<u8, MaxNodes>(vec![]);
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(3), proof),
            crate::Error::<Test>::NoSolutionsSubmitted
        );
    });
}

// ─────────────── exactness fraud proof (permissionless) ────────────────

/// Register a topology whose graph is a path over `n` nodes, plus a hardness
/// record claiming a width far above the ceiling. A path eliminates at width
/// 1, so the claim is falsifiable by anyone.
fn overclaimed_path_topology(n: u32) -> (BoundedVec<u32, MaxNodes>, sp_core::H256) {
    // Claim the default slot with an unrelated topology first: the first
    // registration becomes `DefaultTopology`, which is exempt from demotion.
    registered_topology();
    let nodes = bounded::<_, MaxNodes>((0..n).collect::<Vec<u32>>());
    let edges = bounded::<_, MaxEdges>((0..n - 1).map(|i| (i, i + 1)).collect::<Vec<_>>());
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
        edges,
        allowed_h_spec(),
        allowed_j_spec(),
        allowed_spin_spec(),
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            // Acyclic: no fundamental cycles, so no handicap.
            expected_frustration_milli: 0,
        },
    ));
    MineableTopologies::<Test>::insert(hash, ());
    assert_ok!(QuantumPow::set_topology_hardness(
        RuntimeOrigin::root(),
        hash,
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            expected_frustration_milli: 0,
        }
    ));
    (nodes, hash)
}

#[test]
fn anyone_can_disprove_an_overclaimed_width() {
    new_test_ext().execute_with(|| {
        // Claim a width of 64; the graph is a path, so leaves-first
        // eliminates at width 1 and any account can show it.
        let (nodes, hash) = overclaimed_path_topology(6);
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Heuristic));

        let order = bounded::<_, MaxNodes>(nodes.to_vec());
        assert_ok!(QuantumPow::prove_topology_exact(
            RuntimeOrigin::signed(9),
            hash,
            order
        ));

        // The width ratchets down to the witnessed one; the regime follows from
        // it without being stored.
        let hardness = TopologyHardnessOf::<Test>::get(hash).unwrap();
        assert_eq!(hardness.residual_difficulty, 1);
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));
        // The expected frustration the bar divides by is untouched.
        assert_eq!(hardness.expected_frustration_milli, 0);
        // Exact work is not useful work: the topology is retired.
        assert!(!MineableTopologies::<Test>::contains_key(hash));
    });
}

/// A malformed witness must name its defect, not just say "invalid". These are
/// permissionless fee-paying calls whose whole product is a verdict; four
/// elimination-order defects used to collapse into one error. The likeliest
/// cause today is a witness built against PRE-canonicalization edge order (see
/// the v6 canonicalization). The reason travels as a dispatch ERROR, not an event: a
/// failing extrinsic rolls its events back, discarding it when it is needed.
#[test]
fn a_bogus_order_witnesses_nothing() {
    new_test_ext().execute_with(|| {
        let (nodes, hash) = overclaimed_path_topology(6);
        let claimed = TopologyHardnessOf::<Test>::get(hash)
            .unwrap()
            .residual_difficulty;

        // Not a permutation: repeats a node.
        let mut repeated = nodes.to_vec();
        repeated[5] = repeated[0];
        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(9),
                hash,
                bounded::<_, MaxNodes>(repeated)
            ),
            crate::Error::<Test>::WitnessDuplicateNode
        );

        // Names a node the topology does not have.
        let mut foreign = nodes.to_vec();
        foreign[0] = 999;
        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(9),
                hash,
                bounded::<_, MaxNodes>(foreign)
            ),
            crate::Error::<Test>::WitnessUnknownNode
        );

        // Too short.
        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(9),
                hash,
                bounded::<_, MaxNodes>(nodes[..3].to_vec())
            ),
            crate::Error::<Test>::WitnessOrderLength
        );

        // Nothing moved, and the topology stays mineable.
        assert_eq!(
            TopologyHardnessOf::<Test>::get(hash)
                .unwrap()
                .residual_difficulty,
            claimed
        );
        assert!(MineableTopologies::<Test>::contains_key(hash));
    });
}

#[test]
fn a_genuinely_wide_graph_cannot_be_proven_exact() {
    new_test_ext().execute_with(|| {
        // K8 has induced width 7 under every order. With the ceiling lowered
        // below that, no witness exists and the replay aborts.
        registered_topology();
        let n = 8u32;
        let nodes = bounded::<_, MaxNodes>((0..n).collect::<Vec<u32>>());
        let mut pairs = Vec::new();
        for a in 0..n {
            for b in (a + 1)..n {
                pairs.push((a, b));
            }
        }
        let edges = bounded::<_, MaxEdges>(pairs);
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
            edges,
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                // Acyclic: no fundamental cycles, so no handicap.
                expected_frustration_milli: 0,
            },
        ));
        assert_ok!(QuantumPow::set_topology_hardness(
            RuntimeOrigin::root(),
            hash,
            TopologyHardness {
                residual_difficulty: 7,
                core_width: 7,
                expected_frustration_milli: 500,
            }
        ));
        ExactSolveCeilingOverride::set(3);

        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(9),
                hash,
                bounded::<_, MaxNodes>(nodes.to_vec())
            ),
            crate::Error::<Test>::WidthAboveCeiling
        );
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Heuristic));
    });
}

#[test]
fn the_width_only_ever_ratchets_down() {
    new_test_ext().execute_with(|| {
        let (nodes, hash) = overclaimed_path_topology(6);
        let leaves_first = bounded::<_, MaxNodes>(nodes.to_vec());
        assert_ok!(QuantumPow::prove_topology_exact(
            RuntimeOrigin::signed(9),
            hash,
            leaves_first
        ));
        assert_eq!(
            TopologyHardnessOf::<Test>::get(hash)
                .unwrap()
                .residual_difficulty,
            1
        );

        // The ratchet accepts only strictly narrower orders; a path cannot go
        // below width 1, so re-submitting witnesses nothing.
        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(1),
                hash,
                bounded::<_, MaxNodes>(nodes.to_vec())
            ),
            crate::Error::<Test>::WidthAboveCeiling
        );
        assert_eq!(
            TopologyHardnessOf::<Test>::get(hash)
                .unwrap()
                .residual_difficulty,
            1
        );
    });
}

#[test]
fn proving_the_default_topology_exact_leaves_it_whitelisted() {
    new_test_ext().execute_with(|| {
        let (nodes, hash) = overclaimed_path_topology(6);
        assert_ok!(QuantumPow::set_default_topology(
            RuntimeOrigin::root(),
            hash
        ));

        assert_ok!(QuantumPow::prove_topology_exact(
            RuntimeOrigin::signed(9),
            hash,
            bounded::<_, MaxNodes>(nodes.to_vec())
        ));

        // The verdict is recorded but the live default stays mineable —
        // retiring it here would stop qblocks. Governance repoints.
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));
        assert!(MineableTopologies::<Test>::contains_key(hash));
    });
}

#[test]
fn registration_classifies_so_every_topology_is_challengeable() {
    new_test_ext().execute_with(|| {
        let (nodes, _, hash) = registered_topology();
        // Classification is part of registration, so no window exists where a
        // topology is mineable but immune to challenge.
        assert!(TopologyHardnessOf::<Test>::contains_key(hash));
        assert_ok!(QuantumPow::prove_topology_exact(
            RuntimeOrigin::signed(9),
            hash,
            bounded::<_, MaxNodes>(nodes.to_vec())
        ));
        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(9),
                sp_core::H256::repeat_byte(7),
                bounded::<_, MaxNodes>(nodes.to_vec())
            ),
            crate::Error::<Test>::TopologyNotRegistered
        );
    });
}

/// A 3-node triangle topology and a proof builder over its 8 spin configurations.
/// The 2-node fixture cannot express a decay test: with one coupling its optimum
/// *is* its absolute floor `-(Σ|h| + Σ|J|)`, and the bar is clamped never to sit
/// below that floor, so no rejecting threshold exists. A triangle carries a
/// cycle, so a frustrated draw leaves the optimum strictly above the floor —
/// restoring the window between "rejects now" and "admits after decay".
fn triangle_topology() -> (
    BoundedVec<u32, MaxNodes>,
    BoundedVec<(u32, u32), MaxEdges>,
    sp_core::H256,
) {
    let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2]);
    let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (0, 2)]);
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
        TopologyHardness {
            residual_difficulty: 64,
            core_width: 64,
            expected_frustration_milli: 0,
        },
    ));
    MineableTopologies::<Test>::insert(hash, ());
    (nodes, edges, hash)
}

/// The 8 spin configurations of [`triangle_topology`], sorted by exact energy.
fn triangle_energies(
    miner: u64,
    nodes: &BoundedVec<u32, MaxNodes>,
    edges: &BoundedVec<(u32, u32), MaxEdges>,
    hash: sp_core::H256,
) -> (Vec<(i64, Vec<i8>)>, i64) {
    let proof = triangle_proof(miner, nodes, edges, hash, 0);
    let (h, j) = generate_ising_model(
        proof.nonce,
        nodes.as_slice(),
        edges.as_slice(),
        &allowed_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
    )
    .unwrap();
    let mut by_energy: Vec<(i64, Vec<i8>)> = (0..8u8)
        .map(|code| {
            let spins: Vec<i8> = (0..3)
                .map(|bit| if code >> bit & 1 == 1 { 1i8 } else { -1 })
                .collect();
            let energy =
                energy_of_solution(&spins, &h, edges.as_slice(), &j, nodes.as_slice()).unwrap();
            (energy, spins)
        })
        .collect();
    by_energy.sort_by_key(|(energy, _)| *energy);
    let anchor = -(quantum_validation::energy_bound_milli(&h, &j) as i64);
    (by_energy, anchor)
}

/// Build a proof over [`triangle_topology`] carrying the `rank`-th best
/// configuration.
fn triangle_proof(
    miner: u64,
    nodes: &BoundedVec<u32, MaxNodes>,
    edges: &BoundedVec<(u32, u32), MaxEdges>,
    hash: sp_core::H256,
    rank: usize,
) -> QuantumProof<crate::PackedSpinBytesOf<Test>> {
    let salt = {
        let mut s = [0u8; 32];
        s[..4].copy_from_slice(b"salt");
        s
    };
    let nonce = derive_nonce(
        &LastProofBlockHash::<Test>::get().0,
        &crate::Pallet::<Test>::account_to_bytes(&miner),
        &salt,
    );
    let (h, j) = generate_ising_model(
        nonce,
        nodes.as_slice(),
        edges.as_slice(),
        &allowed_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
    )
    .unwrap();
    let mut by_energy: Vec<(i64, Vec<i8>)> = (0..8u8)
        .map(|code| {
            let spins: Vec<i8> = (0..3)
                .map(|bit| if code >> bit & 1 == 1 { 1i8 } else { -1 })
                .collect();
            let energy =
                energy_of_solution(&spins, &h, edges.as_slice(), &j, nodes.as_slice()).unwrap();
            (energy, spins)
        })
        .collect();
    by_energy.sort_by_key(|(energy, _)| *energy);
    QuantumProof {
        topology_hash: hash,
        nonce,
        salt,
        solutions: pack_spins(&by_energy[rank].1),
        device_access_time_us: 0,
    }
}

#[test]
fn an_exact_topology_never_becomes_mineable() {
    new_test_ext().execute_with(|| {
        registered_topology();
        // A second topology whose width is already at or below the ceiling: its
        // ground state is tractable, so it must not enter the whitelist.
        let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (0, 2)]);
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
            TopologyHardness {
                residual_difficulty: 5,
                core_width: 5,
                expected_frustration_milli: 500,
            },
        ));
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));
        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::root(), hash),
            crate::Error::<Test>::TopologyIsExact
        );
        assert!(!MineableTopologies::<Test>::contains_key(hash));
    });
}

#[test]
fn an_exact_first_registration_does_not_become_the_default() {
    new_test_ext().execute_with(|| {
        // The first registration seeds `DefaultTopology`, but an exactly-solvable
        // one there would make the default a puzzle no proof should ever win.
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
            TopologyHardness {
                residual_difficulty: 3,
                core_width: 3,
                expected_frustration_milli: 0,
            },
        ));
        assert!(RegisteredTopologies::<Test>::contains_key(hash));
        assert_eq!(DefaultTopology::<Test>::get(), None);
        assert!(!MineableTopologies::<Test>::contains_key(hash));
    });
}

// ───────────── planarity challenge (tractability without width) ─────────

fn zero_h_spec() -> AllowedValueSpec<AllowedValueSetOf<Test>> {
    AllowedValueSpec::Set(bounded::<_, MaxAllowedValues>(vec![0]))
}

/// A zero-field triangle registered as if it were wide, plus the rotation
/// system that proves it planar.
fn planar_zero_field_topology() -> (sp_core::H256, Vec<Vec<u32>>) {
    registered_topology();
    let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2]);
    let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (2, 0)]);
    let hash = topology::hash_topology(
        &nodes,
        &edges,
        &zero_h_spec().as_slice(),
        &allowed_j_spec().as_slice(),
        &allowed_spin_spec().as_slice(),
    );
    assert_ok!(QuantumPow::register_topology(
        RuntimeOrigin::root(),
        nodes,
        edges,
        zero_h_spec(),
        allowed_j_spec(),
        allowed_spin_spec(),
        TopologyHardness {
            residual_difficulty: 4_000,
            core_width: 4_000,
            expected_frustration_milli: 500,
        },
    ));
    MineableTopologies::<Test>::insert(hash, ());
    // Rotation entries are EDGE INDICES into the stored (canonicalized) edge
    // list, so `[(0,1), (1,2), (2,0)]` is stored as `[(0,1), (0,2), (1,2)]`:
    // node 0 on edges 0,1; node 1 on 0,2; node 2 on 1,2. Every vertex has
    // degree two, so any cyclic order of its pair is valid.
    (hash, vec![vec![0, 1], vec![0, 2], vec![1, 2]])
}

fn bounded_rotation(rotation: Vec<Vec<u32>>) -> BoundedVec<BoundedVec<u32, MaxEdges>, MaxNodes> {
    bounded::<_, MaxNodes>(
        rotation
            .into_iter()
            .map(|order| bounded::<_, MaxEdges>(order))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn a_planar_zero_field_topology_is_retired_however_wide_it_claims_to_be() {
    new_test_ext().execute_with(|| {
        let (hash, rotation) = planar_zero_field_topology();
        // The triangle stands in for a large grid: the mock's MaxNodes cannot
        // hold a 21x21 lattice, so lower the ceiling under the triangle's width
        // instead. The shape that matters is the same — too wide for any
        // exactness witness, yet planar and so exactly solvable.
        ExactSolveCeilingOverride::set(1);
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Heuristic));
        assert_noop!(
            QuantumPow::prove_topology_exact(
                RuntimeOrigin::signed(9),
                hash,
                bounded::<_, MaxNodes>(vec![0, 1, 2])
            ),
            crate::Error::<Test>::WidthAboveCeiling
        );

        // A zero-field planar Ising is max-cut on a planar graph: polynomial
        // whatever the width. The embedding proves it.
        assert_ok!(QuantumPow::prove_topology_planar(
            RuntimeOrigin::signed(9),
            hash,
            bounded_rotation(rotation)
        ));
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));
        assert_eq!(
            TopologyHardnessOf::<Test>::get(hash)
                .unwrap()
                .residual_difficulty,
            0
        );
        assert!(!MineableTopologies::<Test>::contains_key(hash));
    });
}

#[test]
fn a_field_bearing_topology_survives_a_planar_embedding() {
    new_test_ext().execute_with(|| {
        // Same triangle, but with the ternary field spec. Planar Ising WITH a
        // magnetic field is NP-hard, so planarity proves nothing here.
        registered_topology();
        let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (2, 0)]);
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
            TopologyHardness {
                residual_difficulty: 4_000,
                core_width: 4_000,
                expected_frustration_milli: 500,
            },
        ));
        assert_noop!(
            QuantumPow::prove_topology_planar(
                RuntimeOrigin::signed(9),
                hash,
                bounded_rotation(vec![vec![0, 2], vec![1, 0], vec![2, 1]])
            ),
            crate::Error::<Test>::TopologyIsFieldBearing
        );
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Heuristic));
    });
}

#[test]
fn a_bogus_rotation_system_witnesses_no_planarity() {
    new_test_ext().execute_with(|| {
        let (hash, _) = planar_zero_field_topology();
        // Omits an incident edge, which would otherwise let a non-planar
        // graph pass as the planar subgraph that remains.
        assert_noop!(
            QuantumPow::prove_topology_planar(
                RuntimeOrigin::signed(9),
                hash,
                bounded_rotation(vec![vec![0], vec![1, 0], vec![2, 1]])
            ),
            crate::Error::<Test>::InvalidPlanarEmbedding
        );
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Heuristic));
        assert!(MineableTopologies::<Test>::contains_key(hash));
    });
}

/// `register_topology` must refuse a node list with a duplicate.
/// `Error::InvalidTopology` had ZERO tests here, so the guard could not fail.
/// `canonical_graph` runs FIRST and only sorts — count-preserving by design (the
/// `TopologyDims` backfill relies on that) — so `[0, 1, 1, 2]` reaches the
/// consistency check still holding its duplicate, and nothing downstream
/// notices: one logical spin gets two positions.
#[test]
fn register_topology_refuses_a_duplicate_node() {
    new_test_ext().execute_with(|| {
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                bounded::<_, MaxNodes>(vec![0, 1, 1, 2]),
                bounded::<_, MaxEdges>(vec![(0, 1), (1, 2)]),
                allowed_h_spec(),
                allowed_j_spec(),
                allowed_spin_spec(),
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    expected_frustration_milli: 500,
                },
            ),
            crate::Error::<Test>::InvalidTopology
        );
    });
}

/// A self-loop is a REGISTRATION-time refusal, not something left for the
/// planarity witness. A review claimed `validate_topology_consistency` checks
/// only duplicate nodes and unknown endpoints, so `(u, u)` would register
/// cleanly; it does not — that function has an explicit self-loop branch.
/// `WitnessError::SelfLoop` stays live and correctly NOT submitter-fixable:
/// `canonical_graph` preserves self-loops and v6 carries them forward, so a
/// pre-existing topology can hold one — a stored-state path no new registration
/// can reach, which is why the fee should not fall on the submitter.
#[test]
fn a_self_loop_is_refused_at_registration() {
    new_test_ext().execute_with(|| {
        registered_topology();
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                bounded::<_, MaxNodes>(vec![0, 1, 2]),
                bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (2, 0), (1, 1)]),
                zero_h_spec(),
                allowed_j_spec(),
                allowed_spin_spec(),
                TopologyHardness {
                    residual_difficulty: 4_000,
                    core_width: 4_000,
                    expected_frustration_milli: 500,
                },
            ),
            crate::Error::<Test>::InvalidTopology
        );
    });
}

#[test]
fn proving_the_default_topology_planar_never_halts_production() {
    new_test_ext().execute_with(|| {
        let (hash, rotation) = planar_zero_field_topology();
        assert_ok!(QuantumPow::set_default_topology(
            RuntimeOrigin::root(),
            hash
        ));
        assert_ok!(QuantumPow::prove_topology_planar(
            RuntimeOrigin::signed(9),
            hash,
            bounded_rotation(rotation)
        ));
        // The live default stays mineable so qblock production cannot stop.
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));
        assert!(MineableTopologies::<Test>::contains_key(hash));
    });
}

// ─────────────── per-epoch retarget (the one difficulty dial) ───────────

#[test]
fn retarget_drives_the_bar_toward_target_cadence() {
    use difficulty::retarget_bar_milli;
    let curve = test_curve();
    let bar = curve.knee_milli;

    // On target: no correction.
    assert_eq!(
        retarget_bar_milli(
            bar,
            curve,
            difficulty::RetargetWindow {
                qblocks: 5,
                epoch_blocks: 100,
                target_blocks_per_qblock: 20
            },
            MAX_STEP
        ),
        bar
    );

    // Slow (100 blocks per qblock against a 20-block target) eases the bar.
    let slow = retarget_bar_milli(
        bar,
        curve,
        difficulty::RetargetWindow {
            qblocks: 1,
            epoch_blocks: 100,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert!(slow > bar, "a slow epoch must ease the bar");

    // Running fast tightens it.
    let fast = retarget_bar_milli(
        bar,
        curve,
        difficulty::RetargetWindow {
            qblocks: 20,
            epoch_blocks: 100,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert!(fast < bar, "a fast epoch must tighten the bar");

    // No qblocks reads as maximally slow — the recovery path from a bar nobody
    // can clear.
    let stalled = retarget_bar_milli(
        bar,
        curve,
        difficulty::RetargetWindow {
            qblocks: 0,
            epoch_blocks: 100,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert!(stalled > bar);
}

#[test]
fn retarget_step_is_clamped_and_never_eases_past_the_easy_cap() {
    use difficulty::retarget_bar_milli;
    let curve = test_curve();
    let span = curve.max_milli - curve.min_milli;

    // A catastrophically slow epoch is bounded to 25% of the span, so one
    // anomaly cannot slam the bar across the range.
    let bar = curve.min_milli;
    let eased = retarget_bar_milli(
        bar,
        curve,
        difficulty::RetargetWindow {
            qblocks: 1,
            epoch_blocks: 1_000_000,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert!(
        eased - bar <= span / 4 + 1,
        "one epoch moved the bar {} against a span of {span}",
        eased - bar
    );

    // Easing stops at the easiest calibrated puzzle.
    let at_cap = retarget_bar_milli(
        curve.max_milli,
        curve,
        difficulty::RetargetWindow {
            qblocks: 0,
            epoch_blocks: 1_000_000,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert_eq!(at_cap, curve.max_milli);

    // Tightening is deliberately uncapped: the curve bounds are mean-field
    // estimates, and a stronger-than-estimated field wins below min_milli.
    let tightened = retarget_bar_milli(
        curve.min_milli,
        curve,
        difficulty::RetargetWindow {
            qblocks: 1_000,
            epoch_blocks: 20,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert!(tightened < curve.min_milli);
}

#[test]
fn a_stalled_chain_eases_itself_out_without_any_winning_proof() {
    new_test_ext().execute_with(|| {
        registered_topology();
        // Park the bar at the hard end, where nothing clears it.
        let curve = test_curve();
        set_difficulty_default(DifficultyConfig {
            max_energy_milli: curve.min_milli,
        });
        let before = difficulty_default().max_energy_milli;

        // The retarget must fire from on_finalize's no-winner path, or a chain
        // that stalls stays stalled.
        for block in 1..=(3 * retarget_window()) {
            System::set_block_number(block);
            QuantumPow::on_finalize(block);
        }

        let after = difficulty_default().max_energy_milli;
        assert!(
            after > before,
            "an epoch with no qblocks must ease the bar ({before} -> {after})"
        );
        assert_eq!(EpochQBlocks::<Test>::get(default_hash()), 0);
    });
}

#[test]
fn a_collapsible_core_makes_a_topology_exact_however_wide_the_raw_graph_is() {
    new_test_ext().execute_with(|| {
        registered_topology();
        let nodes = bounded::<_, MaxNodes>(vec![0, 1, 2]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1), (1, 2), (0, 2)]);
        let hash = topology::hash_topology(
            &nodes,
            &edges,
            &allowed_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        // Raw graph claimed wide, but the reduction stack collapses it — a
        // combination the old single field could not represent.
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes,
            edges,
            allowed_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
            TopologyHardness {
                residual_difficulty: 4_000,
                core_width: 3,
                expected_frustration_milli: 500,
            },
        ));

        // Solving the core solves the original, so the topology is exact and not
        // mineable — even though its raw width clears the ceiling and no
        // elimination order over the raw graph could witness this.
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));
        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::root(), hash),
            crate::Error::<Test>::TopologyIsExact
        );
    });
}

#[test]
fn neither_width_may_be_widened_after_the_fact() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let base = TopologyHardnessOf::<Test>::get(hash).unwrap();
        // Narrowing either is fine.
        assert_ok!(QuantumPow::set_topology_hardness(
            RuntimeOrigin::root(),
            hash,
            TopologyHardness {
                residual_difficulty: base.residual_difficulty - 1,
                core_width: base.core_width - 1,
                expected_frustration_milli: base.expected_frustration_milli,
            }
        ));
        // Widening the core alone is refused, just as widening the raw width is.
        assert_noop!(
            QuantumPow::set_topology_hardness(
                RuntimeOrigin::root(),
                hash,
                TopologyHardness {
                    residual_difficulty: base.residual_difficulty - 1,
                    core_width: base.core_width,
                    expected_frustration_milli: base.expected_frustration_milli,
                }
            ),
            crate::Error::<Test>::WidthWidened
        );
    });
}

#[test]
fn a_proven_tractable_default_stops_paying_even_though_it_stays_whitelisted() {
    new_test_ext().execute_with(|| {
        // Account 3 is funded in the mock; 9 only ever acts as a prover.
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(3)));
        let (hash, rotation) = planar_zero_field_topology();
        assert_ok!(QuantumPow::set_default_topology(
            RuntimeOrigin::root(),
            hash
        ));
        set_difficulty_default(easy_difficulty());

        // Left whitelisted on purpose, so a challenge cannot halt the chain.
        assert_ok!(QuantumPow::prove_topology_planar(
            RuntimeOrigin::signed(9),
            hash,
            bounded_rotation(rotation)
        ));
        assert!(MineableTopologies::<Test>::contains_key(hash));
        assert_eq!(QuantumPow::regime_of(hash), Some(Regime::Exact));

        // But it must stop paying: the puzzle is polynomial now. The regime is
        // checked before any instance work, so the proof's contents don't matter.
        let proof = QuantumProof {
            topology_hash: hash,
            nonce: sp_core::U256::zero(),
            salt: [0u8; 32],
            solutions: pack_spins(&[1, -1, 1]),
            device_access_time_us: 0,
        };
        assert_noop!(
            QuantumPow::submit_proof(RuntimeOrigin::signed(3), proof),
            crate::Error::<Test>::TopologyIsExact
        );
    });
}

#[test]
fn the_retarget_saturates_instead_of_wrapping() {
    // `i64::MAX` is a live sentinel — the genesis "no bar yet" value, and
    // `set_difficulty` accepts it. Easing it must not wrap to a hugely negative
    // bar no proof could clear and no later retarget could walk back.
    let curve = test_curve();
    let eased = difficulty::retarget_bar_milli(
        i64::MAX,
        curve,
        difficulty::RetargetWindow {
            qblocks: 0,
            epoch_blocks: 1_000,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert!(
        eased <= curve.max_milli && eased > i64::MIN / 2,
        "saturating ease produced {eased}"
    );
    // Mirrored: a bar at the floor, tightened hard. Pinning at `i64::MIN` is
    // saturation working; the guarded failure is a WRAP to a positive
    // (trivially clearable) bar.
    let tightened = difficulty::retarget_bar_milli(
        i64::MIN,
        curve,
        difficulty::RetargetWindow {
            qblocks: 1_000,
            epoch_blocks: 1,
            target_blocks_per_qblock: 20,
        },
        MAX_STEP,
    );
    assert_eq!(tightened, i64::MIN, "tighten must saturate, not wrap");
    assert!(tightened < 0, "a wrapped bar would read positive");
}

#[test]
fn the_handicap_is_withheld_where_the_floor_leaves_no_room_to_tighten() {
    use difficulty::instance_bar_milli;
    // Curve bar below the attainable floor, so `typical` clamps to the floor and
    // there is nowhere left to tighten. Applying the handicap there would loosen
    // for a high-frustration draw while a low one got nothing — paying for
    // grinding upward.
    let bound = 10_000u64;
    let clamped = |phi| instance_bar_milli(-1_000_000, bound, draw(phi, 500, 100), SLOPE);
    assert_eq!(clamped(500), -(bound as i64));
    assert_eq!(
        clamped(1_000),
        clamped(500),
        "grinding up must not loosen at the floor"
    );
    assert_eq!(
        clamped(0),
        clamped(500),
        "and grinding down must not tighten either"
    );

    // With headroom the handicap applies symmetrically as before.
    let roomy = |phi| instance_bar_milli(-6_000, bound, draw(phi, 500, 100), SLOPE);
    assert!(roomy(450) < roomy(500));
    assert!(roomy(550) > roomy(500));
}

// ───────────── coverage the adversarial review found missing ─────────────

/// hv9.33: the per-instance handicap was never exercised through `submit_proof`
/// — every test topology set `expected_frustration_milli = 0`, the branch that
/// disables it. Drives the live path with cycles and a non-zero expectation.
#[test]
fn the_instance_handicap_is_exercised_through_submit_proof() {
    new_test_ext().execute_with(|| {
        assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(7)));
        let (nodes, edges, hash) = triangle_topology();
        assert_ok!(QuantumPow::set_topology_hardness(
            RuntimeOrigin::root(),
            hash,
            TopologyHardness {
                residual_difficulty: 64,
                core_width: 64,
                expected_frustration_milli: 500,
            }
        ));
        LastProofBlock::<Test>::put(1);
        // A realistic negative bar: `easy_difficulty()` is `i64::MAX`, a positive
        // threshold that makes the handicap meaningless. Miner 7's instance
        // reaches -4000, so -2000 clears even after the handicap tightens it.
        Difficulties::<Test>::insert(
            hash,
            DifficultyConfig {
                max_energy_milli: -2_000,
            },
        );

        let proof = triangle_proof(7, &nodes, &edges, hash, 0);
        assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(7), proof));

        // The record carries the handicapped bar, so the mechanism ran rather
        // than being short-circuited by a zero expectation.
        let record = BlockBestProof::<Test>::get().expect("proof accepted");
        assert!(record.instance_bar_milli < 0);
        assert!(record.energy_milli <= record.instance_bar_milli);
    });
}

/// hv9.38: the retarget was only ever driven down the zero-qblock path, so
/// the counter increment, the reset, and the event were unasserted.
#[test]
fn a_won_epoch_retargets_and_emits_its_cadence() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();
        let epoch = retarget_window();
        Difficulties::<Test>::insert(
            hash,
            DifficultyConfig {
                max_energy_milli: test_curve().knee_milli,
            },
        );
        EpochStart::<Test>::put(1);

        // A win mid-epoch increments this topology's counter.
        System::set_block_number(2);
        BlockBestProof::<Test>::put(ProofRecord {
            miner: 1,
            submitted_at: 2,
            energy_milli: -1,
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 0,
            frustration_milli: 0,
            instance_bar_milli: 0,
            anchor_milli: i64::MIN / 4,
            difficulty: easy_difficulty(),
        });
        QuantumPow::on_finalize(2);
        assert_eq!(EpochQBlocks::<Test>::get(hash), 1);

        // Close well past target cadence (one win in three epochs' blocks) so
        // the retarget has an error to correct. At exactly target it would
        // correctly leave the bar alone and emit nothing.
        System::set_block_number(3 * epoch + 1);
        QuantumPow::on_finalize(3 * epoch + 1);
        assert_eq!(EpochQBlocks::<Test>::get(hash), 0);
        assert_eq!(EpochStart::<Test>::get(), 3 * epoch + 1);
        assert!(
            System::events().iter().any(|e| matches!(
                e.event,
                RuntimeEvent::QuantumPow(crate::Event::DifficultyRetargeted { .. })
            )),
            "closing a won epoch must report its cadence"
        );
    });
}

/// hv9.39: `check_hardness` was only reachable through
/// `set_topology_hardness` in tests, and `add_mineable_topology`'s
/// `TopologyNotClassified` arm was never driven at all.
#[test]
fn registration_validates_its_record_and_mineability_requires_one() {
    new_test_ext().execute_with(|| {
        let nodes = bounded::<_, MaxNodes>(vec![0, 1]);
        let edges = bounded::<_, MaxEdges>(vec![(0, 1)]);
        assert_noop!(
            QuantumPow::register_topology(
                RuntimeOrigin::root(),
                nodes.clone(),
                edges.clone(),
                allowed_h_spec(),
                allowed_j_spec(),
                allowed_spin_spec(),
                TopologyHardness {
                    residual_difficulty: 64,
                    core_width: 64,
                    // A per-mille fraction above 1000 describes no graph.
                    expected_frustration_milli: 1_001,
                },
            ),
            crate::Error::<Test>::InvalidHardnessRecord
        );

        // A topology whose record was removed cannot be whitelisted.
        let (_, _, hash) = registered_topology();
        TopologyHardnessOf::<Test>::remove(hash);
        assert_noop!(
            QuantumPow::add_mineable_topology(RuntimeOrigin::root(), hash),
            crate::Error::<Test>::TopologyNotClassified
        );
    });
}

/// hv9.29: every migration test entered below storage v5, so the v5 → v6
/// `QBlocks::translate` — the one that re-encodes a *deployed* chain's blocks —
/// was never executed. A translate that dropped entries (failure mode: the
/// old-shape struct not matching what v5 wrote) passed CI silently.
#[test]
fn migration_v5_to_v6_reencodes_qblocks_without_losing_any() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();

        // Plant blocks in the v5 layout: three-field difficulty, and none of
        // the instance fields v6 appends.
        #[derive(codec::Encode)]
        struct V5QBlock {
            miner: u64,
            salt: [u8; 32],
            energy_milli: i64,
            reward: u128,
            submitted_at: u64,
            difficulty: LegacyDifficultyConfig,
            last_proof_block_hash: sp_core::H256,
            topology_hash: sp_core::H256,
            device_access_time_us: u64,
        }
        let legacy = LegacyDifficultyConfig {
            min_solutions: 5,
            max_energy_milli: -4_242,
            min_diversity_milli: 200,
        };
        for block in [3u64, 11, 29] {
            let old = V5QBlock {
                miner: 1,
                salt: [block as u8; 32],
                energy_milli: -1_000 * block as i64,
                reward: 50,
                submitted_at: block,
                difficulty: legacy,
                last_proof_block_hash: sp_core::H256::repeat_byte(block as u8),
                topology_hash: hash,
                device_access_time_us: 7,
            };
            frame_support::storage::unhashed::put(&QBlocks::<Test>::hashed_key_for(block), &old);
        }
        // And a Difficulties entry in the same legacy shape.
        frame_support::storage::unhashed::put(&Difficulties::<Test>::hashed_key_for(hash), &legacy);
        StorageVersion::new(5).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert_eq!(StorageVersion::get::<QuantumPow>(), StorageVersion::new(6));
        // Not one entry may be dropped — that is the failure this covers.
        assert_eq!(QBlocks::<Test>::iter().count(), 3);
        for block in [3u64, 11, 29] {
            let migrated = QBlocks::<Test>::get(block).expect("qblock survives the re-encode");
            assert_eq!(migrated.salt, [block as u8; 32]);
            assert_eq!(migrated.energy_milli, -1_000 * block as i64);
            assert_eq!(migrated.device_access_time_us, 7);
            assert_eq!(migrated.topology_hash, hash);
            // The bar those blocks cleared is preserved exactly, not defaulted.
            assert_eq!(migrated.difficulty.max_energy_milli, -4_242);
            assert_eq!(migrated.instance_bar_milli, -4_242);
            assert_eq!(migrated.frustration_milli, 0);
        }
        assert_eq!(
            Difficulties::<Test>::get(hash).unwrap().max_energy_milli,
            -4_242
        );
    });
}

/// A zero-room proof scores LOWEST, not full marks. `proof_quality` normalizes
/// depth by the instance's own room; a bar clamped flush to the anchor has none,
/// so it returns the minimum — reaching a bar that sits at the optimum says
/// nothing about difficulty, and zero-room draws are the cheapest to grind at
/// `O(n + m)` per salt. `submit_proof`'s comment used to assert the OPPOSITE,
/// and this case was claimed "tested elsewhere" when it was not: without this
/// test, "fixing" the branch to full marks gives a green suite and a grinder.
#[test]
fn a_zero_room_proof_scores_lowest_rather_than_full_marks() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();

        // Bar flush to the anchor: room == 0.
        let zero_room = ProofRecord {
            miner: 1,
            submitted_at: 1,
            energy_milli: -1_200,
            anchor_milli: -1_200,
            difficulty: easy_difficulty(),
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 0,
            frustration_milli: 500,
            instance_bar_milli: -1_200,
        };
        // Any positive room at all, barely used.
        let some_room = ProofRecord {
            miner: 2,
            submitted_at: 1,
            energy_milli: -1_001,
            anchor_milli: -2_000,
            difficulty: easy_difficulty(),
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 0,
            frustration_milli: 500,
            instance_bar_milli: -1_000,
        };

        assert_eq!(
            QuantumPow::proof_quality(&zero_room),
            0,
            "a bar clamped flush to the anchor must score zero, not QUALITY_SCALE"
        );
        assert!(
            QuantumPow::proof_quality(&some_room) > QuantumPow::proof_quality(&zero_room),
            "a proof with real room must outscore a zero-room one even when it \
             used almost none of that room"
        );

        // And the ordering must actually decide selection, not merely differ.
        BlockBestProof::<Test>::put(some_room);
        assert!(
            !QuantumPow::proof_outranks_block_best(&zero_room),
            "a zero-room proof must not displace one that measured real depth"
        );
    });
}

/// `integrity_test`'s `ExactSolveCeiling` bound must actually reject: it sits at
/// the mock's value, so weakening it to `<= 30` leaves the suite green.
/// `prove_topology_exact` is permissionless and quadratic in the ceiling, with a
/// prior 50x undercharge on record as a block-stuffing vector.
#[test]
fn the_exact_solve_ceiling_guard_refuses_an_unbenchmarked_ceiling() {
    new_test_ext().execute_with(|| {
        ExactSolveCeilingOverride::set(20);
        QuantumPow::integrity_test();

        ExactSolveCeilingOverride::set(21);
        let raised = std::panic::catch_unwind(|| QuantumPow::integrity_test());
        assert!(
            raised.is_err(),
            "a ceiling above the benchmarked 20 must fail integrity_test; \
             PROVE_EXACT_K1_NODE folds that value in and the cost is quadratic"
        );

        ExactSolveCeilingOverride::set(20);
    });
}

/// The loosest-energy-bound headroom assertion must actually reject.
/// `ensure_specs_cannot_draw_zero`'s `i64::try_from` guard was deleted as
/// unreachable — true only because `(MaxNodes + MaxEdges)` times the widest
/// `IntegerRange` milli term stays inside `i64`, a relationship nothing recorded
/// until `integrity_test` did.
#[test]
fn the_loosest_bound_headroom_guard_refuses_absurd_graph_bounds() {
    new_test_ext().execute_with(|| {
        QuantumPow::integrity_test();

        // At today's bounds there is ~78x of room; it takes millions of nodes to
        // close it. Recomputed, not a literal, so a MaxNodes change moves both.
        let widest_term =
            u64::from(i32::MIN.unsigned_abs()) * (quantum_validation::MILLI_SCALE as u64);
        let n = u64::from(MaxNodes::get());
        let e = u64::from(MaxEdges::get());
        assert!(
            (n + e) * widest_term <= i64::MAX as u64,
            "today's bounds must clear it"
        );
        // The true threshold is `i64::MAX / widest_term` ~ 4.29M nodes+edges.
        // Straddle it, so the test pins where the guard flips.
        let threshold = (i64::MAX as u64) / widest_term;
        assert!(
            threshold.saturating_mul(widest_term) <= i64::MAX as u64,
            "at the threshold the bound still fits"
        );
        assert!(
            (threshold + 1).saturating_mul(widest_term) > i64::MAX as u64,
            "one past it must not, or the guard is checking nothing"
        );
        assert!(
            (4_000_000..5_000_000).contains(&threshold),
            "threshold moved: expected ~4.29M nodes+edges, got {threshold}"
        );
    });
}

/// `on_finalize` must record the difficulty `submit_proof` priced against, not
/// one it recomputes at finalize time. The two agree on every ordinary path;
/// they diverge when `Difficulties` is written LATER in the same block, where a
/// recompute stamps the qblock with a threshold the miner was never held to.
/// Writing the difficulty between the accepted proof and `on_finalize` is the
/// only way to tell carried from recomputed.
#[test]
fn the_qblock_records_the_bar_the_proof_actually_cleared() {
    new_test_ext().execute_with(|| {
        let (_, _, hash) = registered_topology();

        let priced_against = DifficultyConfig {
            max_energy_milli: -4_242,
        };
        let record = ProofRecord {
            miner: 1,
            submitted_at: 1,
            energy_milli: -5_000,
            anchor_milli: -9_000,
            difficulty: priced_against,
            salt: [0u8; 32],
            topology_hash: hash,
            device_access_time_us: 0,
            frustration_milli: 500,
            instance_bar_milli: -4_242,
        };
        BlockBestProof::<Test>::put(record);

        // Move the live difficulty AFTER the proof was accepted. A recompute
        // at finalize would pick this up; the carried value must not.
        set_difficulty_default(DifficultyConfig {
            max_energy_milli: -1,
        });

        QuantumPow::on_finalize(1);

        let stored: Vec<_> = QBlocks::<Test>::iter().collect();
        assert_eq!(stored.len(), 1, "the winning proof must produce a qblock");
        assert_eq!(
            stored[0].1.difficulty, priced_against,
            "the qblock recorded the difficulty live at finalize, not the one \
             the accepted proof was actually priced against"
        );
    });
}

/// The weight closures must not touch `RegisteredTopologies`. `get_dispatch_info`
/// runs inside `validate_transaction`, so a closure reading the topology makes
/// every node SCALE-decode ~370 KB per gossiped extrinsic — unpaid, including
/// ones about to be rejected. Killing the meta while leaving `TopologyDims` is
/// the only way to observe which one it reads.
#[test]
fn the_weight_closures_price_from_dims_not_from_the_topology() {
    new_test_ext().execute_with(|| {
        let (nodes, edges, hash) = registered_topology();
        let (n, e) = (nodes.len() as u32, edges.len() as u32);

        assert_eq!(
            TopologyDims::<Test>::get(hash),
            Some(crate::TopologyDim { nodes: n, edges: e }),
            "register_topology must record the dims beside the meta"
        );

        let priced = crate::weights::SubstrateWeight::<Test>::prove_topology_exact(n, e);

        // Remove the meta. Any closure still reading it now prices at (0, 0).
        RegisteredTopologies::<Test>::remove(hash);

        use frame_support::dispatch::GetDispatchInfo;
        let call = crate::Call::<Test>::prove_topology_exact {
            topology_hash: hash,
            order: bounded::<_, MaxNodes>(nodes.to_vec()),
        };
        assert_eq!(
            call.get_dispatch_info().call_weight,
            priced,
            "the weight closure fell back to (0, 0), so it is decoding the \
             topology rather than reading TopologyDims"
        );
    });
}

/// The dims backfill must cover topologies registered before it, and be
/// idempotent — it runs from every prior storage version.
#[test]
fn the_v8_migration_backfills_dims_and_reruns_cleanly() {
    new_test_ext().execute_with(|| {
        let (nodes, edges, hash) = registered_topology();
        let expected = crate::TopologyDim {
            nodes: nodes.len() as u32,
            edges: edges.len() as u32,
        };

        // Simulate a pre-backfill chain: meta present, dims absent.
        TopologyDims::<Test>::remove(hash);
        assert!(TopologyDims::<Test>::get(hash).is_none());

        crate::migration::v6::backfill_topology_dims::<Test>();
        assert_eq!(TopologyDims::<Test>::get(hash), Some(expected));

        // Idempotent: a second pass must not change anything.
        crate::migration::v6::backfill_topology_dims::<Test>();
        assert_eq!(TopologyDims::<Test>::get(hash), Some(expected));
    });
}

/// The handicap slope must actually be a dial. As a code constant the mock could
/// not sweep it, so its sensitivity had no test. Its doc records the measurement
/// scattering from 4 to 29 per-mille per sigma (correlations under 0.3), so it
/// is expected to move.
#[test]
fn the_handicap_slope_scales_the_bar_it_moves() {
    // Same draw, three slopes. A less-frustrated-than-typical draw (400 against
    // an expected 500) is EASIER, so the handicap tightens, and a steeper slope
    // tightens further.
    let easy_draw = draw(400, 500, 10_000);
    let at_zero = difficulty::instance_bar_milli(-6_000, 10_000, easy_draw, 0);
    let at_default = difficulty::instance_bar_milli(-6_000, 10_000, easy_draw, SLOPE);
    let at_steep = difficulty::instance_bar_milli(-6_000, 10_000, easy_draw, SLOPE * 4);

    assert_eq!(
        at_zero, -6_000,
        "a zero slope must leave the curve bar alone"
    );
    assert!(
        at_default < at_zero,
        "the default slope must tighten an easier-than-typical draw"
    );
    assert!(
        at_steep < at_default,
        "a steeper slope must tighten further: {at_steep} vs {at_default}"
    );
}

/// Narrowing the retarget clamp without widening the window must fail
/// `integrity_test` — the coupling `min_window_for_step` exists to enforce, only
/// testable once the clamp became a `Config` value. Note the direction: a
/// TIGHTER clamp needs MORE averaging, so narrowing trips this, not widening.
#[test]
fn narrowing_the_retarget_clamp_requires_a_wider_window() {
    // The mock runs a 20-epoch window against a 250-per-mille clamp, needing
    // 16. Comfortable.
    assert!(
        <<Test as crate::Config>::RetargetWindowEpochs as Get<u32>>::get()
            >= difficulty::min_window_for_step(MAX_STEP)
    );

    // Halve the clamp and the requirement quadruples to 64, past the mock's
    // 20 — so this configuration would be refused.
    assert_eq!(difficulty::min_window_for_step(MAX_STEP / 2), 64);
    assert!(
        <<Test as crate::Config>::RetargetWindowEpochs as Get<u32>>::get()
            < difficulty::min_window_for_step(MAX_STEP / 2),
        "a halved clamp must outrun the mock's window, or this proves nothing"
    );

    // AND `integrity_test` MUST BE THE THING THAT REFUSES IT. Everything above
    // is arithmetic on `min_window_for_step`; until this block, deleting the
    // coupling assertion from `integrity_test` left this test green.
    new_test_ext().execute_with(|| {
        RetargetMaxStepPermille::set(MAX_STEP);
        QuantumPow::integrity_test();

        RetargetMaxStepPermille::set(MAX_STEP / 2);
        let refusal = integrity_test_panic_message();
        RetargetMaxStepPermille::set(MAX_STEP);
        let refusal = refusal.expect(
            "halving the clamp past the mock's window must fail `integrity_test`; \
             the coupling assertion is the only thing enforcing that a tighter \
             clamp needs more averaging",
        );
        // On the MESSAGE, not merely that something unwound — see
        // `integrity_test_panic_message`.
        assert!(
            refusal.contains("RetargetWindowEpochs"),
            "the wrong assertion fired: {refusal}"
        );
    });
}

/// Run `integrity_test` and return the panic MESSAGE, or `None` if it passed.
/// `catch_unwind(..).is_err()` is satisfied by ANY panic, and these tests
/// install absurd `Config` values that have more ways to panic than the guard
/// under test. Concretely: `RetargetMaxStepPermille = 0` satisfied `is_err()`
/// via a divide-by-zero inside `min_window_for_step`, one assertion BELOW the
/// positivity guard the test named.
#[cfg(test)]
fn integrity_test_panic_message() -> Option<String> {
    // The panic is expected, so suppress the default hook's backtrace noise.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(|| QuantumPow::integrity_test());
    std::panic::set_hook(previous);

    outcome.err().map(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string())
    })
}

/// The Config tunables must actually be reachable THROUGH the pallet. This test
/// used to call `instance_bar_milli`/`retarget_bar_milli` DIRECTLY with a value
/// it fetched itself, proving only that `set` changes what `get` returns —
/// neither real call site ran, so reverting either to its `DEFAULT_*` constant
/// left it green. It now drives the EXTRINSIC and the HOOK and asserts on what
/// they STORE, with a fresh `new_test_ext()` per arm.
#[test]
fn the_config_tunables_reach_the_pallet() {
    // --- The handicap slope, through `submit_proof` ---
    //
    // Same miner, topology and salt, so the same nonce, instance and draw. Only
    // the slope differs, so any difference in the STORED bar travelled through
    // `T::HandicapPerSigmaPermille::get()` inside the extrinsic.
    let submit_under_slope = |slope: i64| -> i64 {
        HandicapPerSigmaPermille::set(slope);
        let bar = new_test_ext().execute_with(|| {
            assert_ok!(QuantumPow::register_miner(RuntimeOrigin::signed(1)));
            let (nodes, edges, hash) = registered_cycle_topology();
            set_difficulty_default(easy_difficulty());
            Difficulties::<Test>::insert(hash, easy_difficulty());

            // A frustrated draw. An unfrustrated one is gauge-trivial (refused)
            // and saturates the handicap, so it could not separate the arms.
            let (proof, phi) = (0..64u8)
                .find_map(|salt| {
                    let (proof, phi) = cycle_proof_for(1, &nodes, &edges, hash, salt);
                    (phi > 0).then_some((proof, phi))
                })
                .expect("a 4-cycle draws frustrated about half the time");
            assert!(phi > 0);
            assert_ok!(QuantumPow::submit_proof(RuntimeOrigin::signed(1), proof));
            BlockBestProof::<Test>::get()
                .expect("the proof was accepted, so it is the block's best")
                .instance_bar_milli
        });
        bar
    };
    let flat = submit_under_slope(0);
    let steep = submit_under_slope(SLOPE * 4);
    HandicapPerSigmaPermille::set(SLOPE);
    assert_ne!(
        flat,
        steep,
        "the configured handicap slope never reached `submit_proof`: the same \
         instance priced identically at slope 0 and slope {}. Revert \
         `T::HandicapPerSigmaPermille::get()` to `DEFAULT_HANDICAP_PER_SIGMA_PERMILLE` \
         and this is what you would see",
        SLOPE * 4
    );

    // --- The retarget clamp, through `on_finalize` ---
    //
    // 4x the expected qblocks reads as far too fast, so the controller tightens
    // by the full clamped step. A tenth of the clamp must move the bar less.
    let retarget_under_clamp = |max_step: i64| -> i64 {
        RetargetMaxStepPermille::set(max_step);
        new_test_ext().execute_with(|| {
            let (_, _, hash) = registered_topology();
            let curve = QuantumPow::energy_curve_for(hash).expect("curve");
            let stored = DifficultyConfig {
                max_energy_milli: curve.knee_milli,
            };
            Difficulties::<Test>::insert(hash, stored);

            let epoch = <<Test as crate::Config>::EpochLength as Get<u64>>::get();
            let window = retarget_window();
            let expected = (window / epoch) as u32;
            EpochStart::<Test>::put(1);
            LastProofBlock::<Test>::put(window);
            EpochQBlocks::<Test>::insert(hash, expected * 4);
            System::set_block_number(window + 1);
            QuantumPow::on_finalize(window + 1);

            Difficulties::<Test>::get(hash).unwrap().max_energy_milli - curve.knee_milli
        })
    };
    let wide = retarget_under_clamp(MAX_STEP);
    let tight = retarget_under_clamp(MAX_STEP / 10);
    RetargetMaxStepPermille::set(MAX_STEP);
    assert!(
        wide != 0,
        "the wide-clamp arm did not move the stored bar at all, so the \
         comparison below cannot distinguish a working clamp from a dead one"
    );
    assert!(
        tight.abs() < wide.abs(),
        "the configured retarget clamp never reached `on_finalize`: a clamp a \
         tenth as wide moved the STORED bar by {tight} against {wide}"
    );
}

/// `integrity_test` must refuse a clamp that would panic `on_finalize`.
/// `Ord::clamp` asserts `min <= max`, so a NEGATIVE `RetargetMaxStepPermille`
/// makes `retarget_bar_milli`'s `clamp(-max, max)` panic inside a hook that
/// cannot be refused — a chain halt, not a rejected extrinsic.
#[test]
fn the_retarget_clamp_guard_refuses_a_value_that_would_halt_the_chain() {
    new_test_ext().execute_with(|| {
        RetargetMaxStepPermille::set(MAX_STEP);
        QuantumPow::integrity_test();

        for bad in [-1i64, 0, i64::MIN] {
            RetargetMaxStepPermille::set(bad);
            let refusal = integrity_test_panic_message();
            let refusal = refusal
                .unwrap_or_else(|| panic!("RetargetMaxStepPermille = {bad} must be refused"));
            // ON THE MESSAGE. `is_err()` alone was HALF VACUOUS: for `bad == 0`
            // it was satisfied by a divide-by-zero inside `min_window_for_step`,
            // one assertion BELOW the positivity guard this test names. Ordering
            // was fixed in 653340b; this keeps the guard reached first, since an
            // assertion inserted above would silently take the test back.
            assert!(
                refusal.contains("RetargetMaxStepPermille must be positive"),
                "RetargetMaxStepPermille = {bad} panicked for the wrong reason: {refusal}"
            );
        }
        RetargetMaxStepPermille::set(MAX_STEP);
    });
}

/// `integrity_test` must refuse a handicap slope outside `0..=1000`. The guard
/// shipped with only its positive path exercised — it could have been
/// `(i64::MIN..=i64::MAX)` unnoticed. It defends against a SIGN FLIP, not a
/// panic: a negative slope makes `max_swing` negative in `instance_bar_milli`,
/// so `headroom < max_swing` is false, execution proceeds, and the handicap runs
/// backwards — an amplifier that pays for salt-shopping. An absurd positive
/// value wraps the `sigmas * slope` i64 multiply (unchecked in release) the same
/// way. Neither panics; both silently invert a consensus mechanism.
#[test]
fn the_handicap_slope_guard_refuses_a_value_that_would_invert_the_handicap() {
    new_test_ext().execute_with(|| {
        HandicapPerSigmaPermille::set(SLOPE);
        QuantumPow::integrity_test();

        for bad in [
            -1i64,
            difficulty::HANDICAP_SLOPE_MAX_PERMILLE + 1,
            i64::MIN,
            i64::MAX,
        ] {
            HandicapPerSigmaPermille::set(bad);
            let refusal = integrity_test_panic_message();
            let refusal = refusal
                .unwrap_or_else(|| panic!("HandicapPerSigmaPermille = {bad} must be refused"));
            assert!(
                refusal.contains("HandicapPerSigmaPermille"),
                "HandicapPerSigmaPermille = {bad} panicked for the wrong reason: {refusal}"
            );
        }
        // The BOUNDARIES must pass — a guard that refuses its own endpoints is
        // a different guard than the documented one.
        for good in [0i64, difficulty::HANDICAP_SLOPE_MAX_PERMILLE] {
            HandicapPerSigmaPermille::set(good);
            assert_eq!(
                integrity_test_panic_message(),
                None,
                "HandicapPerSigmaPermille = {good} is inside 0..=1000 and must pass"
            );
        }
        HandicapPerSigmaPermille::set(SLOPE);
    });
}

/// A handicap slope that escaped `integrity_test` must not INVERT the handicap.
/// `integrity_test` is emitted inside `#[cfg(test)]` by `construct_runtime!` and
/// never runs on a live chain, so a governance `set_code` with wasm from outside
/// this pipeline is unguarded without the use-site clamp (same argument as
/// `retarget_bar_milli`'s fallback). The failure is silent: the bar moves the
/// wrong way, and the only observable is grinding toward easy draws paying.
#[test]
fn a_negative_handicap_slope_cannot_invert_the_bar() {
    // An EASIER-than-typical draw (400 against an expected 500). The handicap
    // must TIGHTEN it, whatever nonsense the slope carries.
    let easy_draw = draw(400, 500, 10_000);
    let honest = difficulty::instance_bar_milli(-6_000, 10_000, easy_draw, SLOPE);
    assert!(
        honest < -6_000,
        "the honest slope must tighten an easy draw"
    );

    for inverting in [-1i64, -SLOPE, i64::MIN] {
        let clamped = difficulty::instance_bar_milli(-6_000, 10_000, easy_draw, inverting);
        assert_eq!(
            clamped, -6_000,
            "slope {inverting} must clamp to zero and leave the bar at the \
             curve value; a LOOSER bar than -6000 means the handicap inverted"
        );
    }
    // The wrap: `i64::MAX` per sigma would overflow `sigmas * slope`; the clamp
    // caps it at the documented maximum.
    let saturated = difficulty::instance_bar_milli(-6_000, 10_000, easy_draw, i64::MAX);
    let at_max = difficulty::instance_bar_milli(
        -6_000,
        10_000,
        easy_draw,
        difficulty::HANDICAP_SLOPE_MAX_PERMILLE,
    );
    assert_eq!(
        saturated, at_max,
        "an absurd slope must behave exactly as the documented maximum, not wrap"
    );
}

/// A nonsensical clamp must RECOVER, not panic and not freeze. `integrity_test`
/// is CI-only, so the hook must defend itself against foreign wasm. The first
/// defence was `max_step_permille.max(0)`, which avoided the panic by producing
/// `bound == 0` — zeroing the step on BOTH arms, so `retargeted == current` and
/// `maybe_retarget` writes and emits nothing: a silent permanent freeze of the
/// only difficulty dial. Hence the assertion that the bar actually MOVES.
#[test]
fn a_negative_clamp_does_not_panic_the_retarget() {
    let curve = walkup_curve();
    let window = difficulty::RetargetWindow {
        qblocks: 40,
        epoch_blocks: 100,
        target_blocks_per_qblock: 20,
    };
    // `clamp(250, -250)` — a panic with no guard, a silent freeze under `.max(0)`.
    let out = difficulty::retarget_bar_milli(curve.knee_milli, curve, window, -250);
    assert_ne!(
        out, curve.knee_milli,
        "a nonsensical clamp must fall back to the default and still move the \
         bar, not freeze the controller"
    );
    // And it lands exactly where the default clamp would have put it.
    let with_default = difficulty::retarget_bar_milli(curve.knee_milli, curve, window, MAX_STEP);
    assert_eq!(
        out, with_default,
        "the fallback must be the documented default"
    );

    // Stall recovery too: zero qblocks must still ease. The freeze silently
    // disabled exactly this case.
    let stalled = difficulty::RetargetWindow {
        qblocks: 0,
        epoch_blocks: 100,
        target_blocks_per_qblock: 20,
    };
    let eased = difficulty::retarget_bar_milli(curve.knee_milli, curve, stalled, -250);
    assert!(
        eased > curve.knee_milli,
        "a stalled chain must ease even under a nonsensical clamp"
    );
}

/// The rotation-side witness errors must be REACHABLE, and distinct.
/// `prove_topology_planar` pre-checks `rotation.len() == nodes.len()` and
/// `declared_slots == 2m`; every pre-existing negative rotation test violated
/// one, dying with `InvalidPlanarEmbedding`, so the verifier's error mapping was
/// unreachable — permuting those match arms broke nothing. These fixtures are
/// well-SHAPED and wrong. The triangle stores edges canonically as
/// `[(0,1), (0,2), (1,2)]`: node 0 on edges 0,1; node 1 on 0,2; node 2 on 1,2.
#[test]
fn each_rotation_defect_reports_its_own_error() {
    new_test_ext().execute_with(|| {
        let (hash, _good) = planar_zero_field_topology();
        let submit = |rot: Vec<Vec<u32>>| {
            QuantumPow::prove_topology_planar(RuntimeOrigin::signed(9), hash, bounded_rotation(rot))
        };

        // Right total (6), wrong per-vertex: node 0 claims three incident edges
        // but has degree two.
        assert_noop!(
            submit(vec![vec![0, 1, 2], vec![0], vec![1, 2]]),
            crate::Error::<Test>::WitnessRotationLength
        );
        // Edge index past the end of the edge list.
        assert_noop!(
            submit(vec![vec![0, 99], vec![0, 2], vec![1, 2]]),
            crate::Error::<Test>::WitnessEdgeIndexOutOfRange
        );
        // Well-formed lengths, but node 0 names edge 2 = (1,2), which is not
        // incident to it.
        assert_noop!(
            submit(vec![vec![0, 2], vec![0, 2], vec![1, 2]]),
            crate::Error::<Test>::WitnessEdgeNotIncident
        );
    });
}

/// A well-formed rotation over a genuinely non-planar graph must report
/// `NotPlanar` — a FALSE CLAIM, not a malformed witness. The submitter's
/// encoding is perfect, so "fix it" would send them round the loop paying a fee
/// each time. K5 is the smallest non-planar graph; Euler refuses it under every
/// rotation order.
#[test]
fn a_valid_rotation_over_a_non_planar_graph_is_a_false_claim() {
    new_test_ext().execute_with(|| {
        registered_topology();
        // K5: 5 nodes, 10 edges, canonical order.
        let nodes: Vec<u32> = (0..5).collect();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for u in 0..5u32 {
            for v in (u + 1)..5 {
                edges.push((u, v));
            }
        }
        let nodes_bv = bounded::<_, MaxNodes>(nodes);
        let edges_bv = bounded::<_, MaxEdges>(edges.clone());
        let hash = topology::hash_topology(
            &nodes_bv,
            &edges_bv,
            &zero_h_spec().as_slice(),
            &allowed_j_spec().as_slice(),
            &allowed_spin_spec().as_slice(),
        );
        assert_ok!(QuantumPow::register_topology(
            RuntimeOrigin::root(),
            nodes_bv,
            edges_bv,
            zero_h_spec(),
            allowed_j_spec(),
            allowed_spin_spec(),
            TopologyHardness {
                residual_difficulty: 4_000,
                core_width: 4_000,
                expected_frustration_milli: 500,
            },
        ));

        // Each vertex lists its incident edges exactly once, so both pre-checks
        // pass and the rotation is legitimate — only the GENUS is wrong.
        let rotation: Vec<Vec<u32>> = (0..5u32)
            .map(|v| {
                edges
                    .iter()
                    .enumerate()
                    .filter(|(_, &(a, b))| a == v || b == v)
                    .map(|(i, _)| i as u32)
                    .collect()
            })
            .collect();

        assert_noop!(
            QuantumPow::prove_topology_planar(
                RuntimeOrigin::signed(9),
                hash,
                bounded_rotation(rotation),
            ),
            crate::Error::<Test>::NotPlanar
        );
    });
}

/// The dims backfill must cover EVERY topology, not just the first, and must be
/// wired into `on_runtime_upgrade`. The original test registered one topology
/// and called `migration::v6::backfill_topology_dims` DIRECTLY, so a backfill
/// that `break`s after the first entry passed, as did deleting its line from
/// `on_runtime_upgrade`. This drives the real upgrade path over three
/// topologies with dims missing from two.
#[test]
fn the_dims_backfill_covers_every_topology_through_the_upgrade_path() {
    new_test_ext().execute_with(|| {
        // `planar_zero_field_topology` registers the base topology itself, so
        // calling `registered_topology` too would double-register.
        let (h3, _) = planar_zero_field_topology();
        let (n2, e2, h2) = registered_cycle_topology();
        let h1 = default_hash();
        let base = RegisteredTopologies::<Test>::get(h1).expect("base registered");
        let (n1, e1) = (base.nodes.len() as u32, base.edges.len() as u32);
        let planar = RegisteredTopologies::<Test>::get(h3).expect("planar registered");
        let (n3, e3) = (planar.nodes.len() as u32, planar.edges.len() as u32);

        // Pre-backfill chain: metas present, dims missing from two of three.
        // The one left in place exercises the `contains_key` skip. Version 5 is
        // what the deployed chain is actually at, so this is the real path.
        TopologyDims::<Test>::remove(h1);
        TopologyDims::<Test>::remove(h3);
        StorageVersion::new(5).put::<QuantumPow>();

        QuantumPow::on_runtime_upgrade();

        assert_eq!(
            TopologyDims::<Test>::get(h1),
            Some(crate::TopologyDim {
                nodes: n1,
                edges: e1
            })
        );
        assert_eq!(
            TopologyDims::<Test>::get(h2),
            Some(crate::TopologyDim {
                nodes: n2.len() as u32,
                edges: e2.len() as u32
            })
        );
        assert_eq!(
            TopologyDims::<Test>::get(h3),
            Some(crate::TopologyDim {
                nodes: n3,
                edges: e3
            })
        );
        // The invariant the weight closures depend on.
        assert_eq!(
            RegisteredTopologies::<Test>::iter_keys().count(),
            TopologyDims::<Test>::iter_keys().count(),
            "every registered topology must carry its dims"
        );
    });
}

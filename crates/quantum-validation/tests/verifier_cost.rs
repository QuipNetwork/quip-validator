//! Relative cost of the per-proof verifier passes, at production topology
//! scale (Z(12,4): n = 4800, m = 45864).
//!
//! Run with `cargo test --release -p quantum-validation --test verifier_cost
//! -- --ignored --nocapture`. Ignored by default: it is a characterization
//! measurement, not an assertion, and only meaningful in release.

use quantum_validation::{
    derive_nonce, energy_bound_milli, energy_of_solution, frustration_index_milli,
    generate_ising_model, induced_width_at_most, AllowedValueSpec, MilliValue,
};
use std::collections::BTreeSet;
use std::time::Instant;

const N: usize = 4_800;
const M: usize = 45_864;

/// splitmix64, so the synthetic instance is deterministic.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `(nodes, edges, h, j, spins)` -- one synthetic instance.
type Instance = (
    Vec<u32>,
    Vec<(u32, u32)>,
    Vec<MilliValue>,
    Vec<MilliValue>,
    Vec<i8>,
);

fn synthetic() -> Instance {
    let mut state = 0xC0FFEE_u64;
    let nodes: Vec<u32> = (0..N as u32).collect();
    let mut edges = Vec::with_capacity(M);
    while edges.len() < M {
        let u = (splitmix64(&mut state) as usize % N) as u32;
        let v = (splitmix64(&mut state) as usize % N) as u32;
        if u != v {
            edges.push((u, v));
        }
    }
    let j: Vec<MilliValue> = (0..M)
        .map(|_| {
            if splitmix64(&mut state) & 1 == 0 {
                -1_000
            } else {
                1_000
            }
        })
        .collect();
    let h = vec![0; N];
    let spins: Vec<i8> = (0..N)
        .map(|_| {
            if splitmix64(&mut state) & 1 == 0 {
                -1
            } else {
                1
            }
        })
        .collect();
    (nodes, edges, h, j, spins)
}

#[test]
#[ignore = "characterization measurement; release-only"]
fn per_proof_pass_costs() {
    let (nodes, edges, h, j, spins) = synthetic();

    // Warm the caches so the first pass is not charged for page faults.
    let mut sink = 0u64;
    for _ in 0..8 {
        sink = sink.wrapping_add(energy_bound_milli(&h, &j));
    }

    let t0 = Instant::now();
    let iters = 100;
    for _ in 0..iters {
        sink = sink.wrapping_add(energy_bound_milli(&h, &j));
    }
    let sum_us = t0.elapsed().as_secs_f64() * 1e6 / f64::from(iters);

    let t0 = Instant::now();
    let iters = 20;
    for _ in 0..iters {
        sink = sink.wrapping_add(u64::from(frustration_index_milli(&nodes, &edges, &j)));
    }
    let frustration_us = t0.elapsed().as_secs_f64() * 1e6 / f64::from(iters);

    // The pass the verifier already runs, once per submitted solution.
    let t0 = Instant::now();
    let energy = energy_of_solution(&spins, &h, &edges, &j, &nodes).unwrap();
    let energy_us = t0.elapsed().as_secs_f64() * 1e6;
    sink = sink.wrapping_add(energy as u64);

    println!("\n  n = {N}, m = {M}  (Z(12,4) scale)");
    println!("  energy_bound_milli      {sum_us:>12.1} us");
    println!(
        "  frustration_index_milli {frustration_us:>12.1} us   ({:.0}x bound)",
        frustration_us / sum_us
    );
    println!(
        "  energy_of_solution x1   {energy_us:>12.1} us   ({:.0}x bound)",
        energy_us / sum_us
    );
    println!("  (all three now run exactly once per proof)\n");
    assert_ne!(sink, 0);
}

/// Cost of the permissionless `prove_topology_exact` replay, which its weight
/// has to charge for. Worst case is an order that stays just under the ceiling
/// for every vertex, so the abort never fires early.
#[test]
#[ignore = "characterization measurement; release-only"]
fn elimination_witness_cost() {
    const CEILING: u32 = 20;
    // A "caterpillar of cliques" — consecutive windows of CEILING+1 vertices
    // all mutually adjacent — saturates the bag at every elimination step
    // without exceeding the ceiling.
    let n = N;
    let nodes: Vec<u32> = (0..n as u32).collect();
    // Shared with the pallet benchmark that calibrates against this number.
    let edges = quantum_validation::caterpillar_of_cliques(n, CEILING as usize, usize::MAX);
    let order: Vec<u32> = nodes.clone();

    let t0 = Instant::now();
    let width = induced_width_at_most(&nodes, &edges, &order, CEILING);
    let us = t0.elapsed().as_secs_f64() * 1e6;
    println!("\n  prove_topology_exact replay, ceiling-saturating");
    println!("  n = {n}, m = {}, width = {width:?}", edges.len());
    println!("  {us:.1} us total = {:.3} us/node\n", us / n as f64);
    assert_eq!(width, Ok(CEILING));
}

/// Cost of the permissionless `prove_topology_planar` face trace, which its
/// weight has to charge for. The trace is linear, so the per-edge figure from a
/// grid transfers to any planar topology.
#[test]
#[ignore = "characterization measurement; release-only"]
fn planar_witness_cost() {
    // A rows x cols grid, embedded in reading order.
    let (rows, cols) = (60usize, 80usize);
    let n = rows * cols;
    let nodes: Vec<u32> = (0..n as u32).collect();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let v = (r * cols + c) as u32;
            if c + 1 < cols {
                edges.push((v, v + 1));
            }
            if r + 1 < rows {
                edges.push((v, v + cols as u32));
            }
        }
    }
    // Incident edges in index order — not a planar embedding for a grid, so the
    // check rejects. Rejection still pays the full index build and face trace,
    // which is the worst case the weight must cover.
    let mut rotation: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (idx, &(a, b)) in edges.iter().enumerate() {
        rotation[a as usize].push(idx as u32);
        rotation[b as usize].push(idx as u32);
    }

    let t0 = Instant::now();
    let faces = quantum_validation::verify_planar_embedding(&nodes, &edges, &rotation);
    let us = t0.elapsed().as_secs_f64() * 1e6;
    println!("\n  prove_topology_planar trace");
    println!("  n = {n}, m = {}, verdict = {faces:?}", edges.len());
    println!(
        "  {us:.1} us total = {:.4} us/edge\n",
        us / edges.len() as f64
    );
}

/// Instance *scale* is only a grinding dimension where the specs let it vary,
/// and the production configuration does not.
///
/// The bar is an absolute energy while an instance's reachable depth grows with
/// its own `Σ|h| + Σ|J|`, so a heavier-than-average draw clears a fixed bar with
/// less search — and finding one costs `O(n + m)` per salt with no solving at
/// all. A real hole, but reachable only when the bound moves between draws.
///
/// For the zero-field, fixed-magnitude coupling spec production topologies use
/// it cannot: `Σ|h| = 0` and `Σ|J| = m·1000` whatever the nonce. Pinned here so
/// a future spec change that reopens the hole fails a test.
#[test]
fn the_production_spec_gives_every_draw_the_same_energy_bound() {
    let nodes: Vec<u32> = (0..64).collect();
    let edges: Vec<(u32, u32)> = (0..64u32)
        .flat_map(|u| ((u + 1)..64).map(move |v| (u, v)))
        .take(180)
        .collect();
    let zero_field = AllowedValueSpec::Set(&[0][..]);
    let binary = AllowedValueSpec::Set(&[-1000, 1000][..]);

    let expected = (edges.len() as u64) * 1000;
    for salt_byte in 0..64u8 {
        let mut salt = [0u8; 32];
        salt[0] = salt_byte;
        let nonce = derive_nonce(&[0x11; 32], &[0x22; 32], &salt);
        let (h, j) =
            generate_ising_model(nonce, &nodes, &edges, &zero_field, &binary).expect("valid specs");
        assert_eq!(
            energy_bound_milli(&h, &j),
            expected,
            "salt {salt_byte} moved the energy bound, so instance scale is \
             grindable on this spec after all"
        );
    }

    // The counterexample, so the test is about the spec and not the graph: a
    // ternary field spec DOES move the bound, which is where the hole is live.
    let ternary = AllowedValueSpec::Set(&[-1000, 0, 1000][..]);
    let bounds: BTreeSet<u64> = (0..32u8)
        .map(|salt_byte| {
            let mut salt = [0u8; 32];
            salt[0] = salt_byte;
            let nonce = derive_nonce(&[0x11; 32], &[0x22; 32], &salt);
            let (h, j) =
                generate_ising_model(nonce, &nodes, &edges, &ternary, &binary).expect("valid");
            energy_bound_milli(&h, &j)
        })
        .collect();
    assert!(
        bounds.len() > 1,
        "a ternary field spec must vary the bound, or this test proves nothing"
    );
}

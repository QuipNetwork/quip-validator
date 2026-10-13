//! Emit the parity vectors riff-morph pins so its analysis harnesses can
//! reproduce the exact instance the chain generates for a given nonce.
//!
//! Run with `cargo test -p quantum-validation --test parity_export --
//! --ignored --nocapture`. Ignored by default: it prints a fixture rather
//! than asserting one, and its output is the source of truth for
//! `riff_morph::chain_instance`'s parity test.

use quantum_validation::{derive_nonce, generate_ising_model, AllowedValueSpec, MilliValue};

/// The vectors riff-morph pins, asserted here too.
///
/// Printing them was not enough: an ignored generator that asserts nothing lets
/// a change to this crate's sampling silently invalidate riff-morph's copy with
/// nothing failing on either side. Same constants as
/// `riff_morph::chain_instance::tests`, so a divergence breaks whichever side
/// changed.
#[test]
fn chain_instance_vectors_are_stable() {
    let nonce = derive_nonce(&[0x11u8; 32], &[0x22u8; 32], &[0x33u8; 32]);
    assert_eq!(
        nonce.to_big_endian(),
        [
            208, 224, 118, 152, 27, 122, 135, 114, 221, 90, 227, 142, 60, 174, 81, 170, 6, 87, 43,
            44, 132, 167, 177, 201, 55, 181, 144, 21, 151, 43, 213, 56,
        ]
    );

    let nodes: Vec<u32> = (0..8).collect();
    let edges: Vec<(u32, u32)> = vec![
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 4),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 0),
    ];
    let ternary: [MilliValue; 3] = [-1000, 0, 1000];
    let binary: [MilliValue; 2] = [-1000, 1000];

    let (h, j) = generate_ising_model(
        nonce,
        &nodes,
        &edges,
        &AllowedValueSpec::Set(&ternary[..]),
        &AllowedValueSpec::Set(&binary[..]),
    )
    .unwrap();
    assert_eq!(h, vec![-1000, 0, 0, 0, 1000, 0, 0, 0]);
    assert_eq!(j, vec![1000, -1000, 1000, 1000, 1000, -1000, 1000, 1000]);

    let (h_range, _) = generate_ising_model(
        nonce,
        &nodes,
        &edges,
        &AllowedValueSpec::IntegerRange { min: -2, max: 2 },
        &AllowedValueSpec::Set(&binary[..]),
    )
    .unwrap();
    assert_eq!(h_range, vec![-1000, 0, 1000, -1000, 0, -2000, 0, 0]);
}

#[test]
#[ignore = "fixture generator; prints the vectors riff-morph pins"]
fn export_chain_instance_vectors() {
    let last = [0x11u8; 32];
    let miner = [0x22u8; 32];
    let salt = [0x33u8; 32];
    let nonce = derive_nonce(&last, &miner, &salt);
    println!("nonce_be = {:?}", nonce.to_big_endian());

    let nodes: Vec<u32> = (0..8).collect();
    let edges: Vec<(u32, u32)> = vec![
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 4),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 0),
    ];

    let ternary: [MilliValue; 3] = [-1000, 0, 1000];
    let binary: [MilliValue; 2] = [-1000, 1000];
    let h_spec = AllowedValueSpec::Set(&ternary[..]);
    let j_spec = AllowedValueSpec::Set(&binary[..]);

    let (h, j) = generate_ising_model(nonce, &nodes, &edges, &h_spec, &j_spec).unwrap();
    println!("h = {h:?}");
    println!("j = {j:?}");

    // A range spec too, so the other sampling branches are pinned.
    let h_range: AllowedValueSpec<&[MilliValue]> =
        AllowedValueSpec::IntegerRange { min: -2, max: 2 };
    let (h2, j2) = generate_ising_model(nonce, &nodes, &edges, &h_range, &j_spec).unwrap();
    println!("h_range = {h2:?}");
    println!("j_range = {j2:?}");
}

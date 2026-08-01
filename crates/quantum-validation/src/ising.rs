//! Deterministic Ising-model derivation helpers.

use alloc::vec::Vec;

use blake3::Hasher;
use rand_chacha::ChaCha8Rng;
use rand_core::SeedableRng;
use sp_core::U256;

use crate::errors::ValidationError;
use crate::fixed::MilliValue;
use crate::puzzle_spec::AllowedValueSpec;
use crate::validation::TopologyIndex;

/// Derive the deterministic puzzle nonce for a `submit_proof` call.
///
/// Inputs are three fixed-size 32-byte buffers so the PoW search space is
/// statically known and identical across every call:
///
/// - `last_proof_block_hash` — `block_hash(LastProofBlock)`, i.e. the header
///   hash of the most recent winning block. Stable across the entire round
///   (only changes on the next win), so miners can submit proofs without
///   racing the txpool / executing-block-number.
/// - `miner` — 32-byte representation of the submitting account (the pallet
///   derives this by hashing the SCALE-encoded `AccountId`, so the input is
///   always 32 bytes regardless of the underlying `AccountId` width)
/// - `salt` — the only freely-chosen miner input, 32 bytes
///
/// Returns the full 256-bit BLAKE3 digest as a `U256` so all 256 bits seed
/// downstream RNG state (no truncation).
pub fn derive_nonce(last_proof_block_hash: &[u8; 32], miner: &[u8; 32], salt: &[u8; 32]) -> U256 {
    let mut hasher = Hasher::new();
    hasher.update(last_proof_block_hash);
    hasher.update(miner);
    hasher.update(salt);
    U256::from_big_endian(hasher.finalize().as_bytes())
}

/// Generate deterministic fixed-point Ising parameters from a nonce.
///
/// The full 256-bit nonce seeds [`ChaCha8Rng`] via [`SeedableRng::from_seed`];
/// the prior implementation truncated to `u64` and discarded 192 bits.
///
/// Per-node h fields are sampled from `allowed_h`; per-edge j couplings are
/// sampled from `allowed_j`. Both specs follow [`AllowedValueSpec`]'s
/// per-variant sampling rules.
pub fn generate_ising_model(
    nonce: U256,
    nodes: &[u32],
    edges: &[(u32, u32)],
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
) -> Result<(Vec<MilliValue>, Vec<MilliValue>), ValidationError> {
    let topology = TopologyIndex::new(nodes, edges)?;
    generate_ising_model_indexed(nonce, nodes, edges, allowed_h, allowed_j, &topology)
}

/// Generate an Ising model while reusing a topology index validated by the
/// caller.
///
/// Proof validation builds the index once and then reuses it for both model
/// generation and energy scoring. This avoids rebuilding the node-id map for
/// every submitted solution.
pub fn generate_ising_model_indexed(
    nonce: U256,
    nodes: &[u32],
    edges: &[(u32, u32)],
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
    topology: &TopologyIndex,
) -> Result<(Vec<MilliValue>, Vec<MilliValue>), ValidationError> {
    if nodes.is_empty() {
        return Err(ValidationError::EmptyNodes);
    }
    // Surface empty-set errors with the legacy variant names so existing
    // callers still match on EmptyFieldValues for h and a distinct error for j.
    if let Err(err) = allowed_h.bits_per_value() {
        return Err(match err {
            ValidationError::EmptyAllowedValues => ValidationError::EmptyFieldValues,
            other => other,
        });
    }
    let _ = allowed_j.bits_per_value()?;

    if topology.len() != nodes.len() {
        return Err(ValidationError::FieldLengthMismatch {
            expected: nodes.len(),
            actual: topology.len(),
        });
    }

    let seed: [u8; 32] = nonce.to_big_endian();
    let mut rng = ChaCha8Rng::from_seed(seed);

    let mut h = Vec::with_capacity(nodes.len());
    for _ in nodes {
        h.push(allowed_h.sample(&mut rng)?);
    }

    let mut j = Vec::with_capacity(edges.len());
    for _ in edges {
        j.push(allowed_j.sample(&mut rng)?);
    }

    Ok((h, j))
}

#[cfg(test)]
mod tests {
    use super::{derive_nonce, generate_ising_model, generate_ising_model_indexed};
    use crate::puzzle_spec::AllowedValueSpec;
    use crate::validation::TopologyIndex;

    const ALICE_BYTES: [u8; 32] = [0xA1; 32];
    const BOB_BYTES: [u8; 32] = [0xB0; 32];
    const SALT_A: [u8; 32] = [0x01; 32];
    const SALT_B: [u8; 32] = [0x02; 32];

    #[test]
    fn nonce_derivation_is_deterministic() {
        let first = derive_nonce(&[1; 32], &ALICE_BYTES, &SALT_A);
        let second = derive_nonce(&[1; 32], &ALICE_BYTES, &SALT_A);
        assert_eq!(first, second);
    }

    #[test]
    fn nonce_derivation_changes_with_input_parts() {
        let baseline = derive_nonce(&[1; 32], &ALICE_BYTES, &SALT_A);
        assert_ne!(baseline, derive_nonce(&[2; 32], &ALICE_BYTES, &SALT_A));
        assert_ne!(baseline, derive_nonce(&[1; 32], &BOB_BYTES, &SALT_A));
        assert_ne!(baseline, derive_nonce(&[1; 32], &ALICE_BYTES, &SALT_B));
    }

    #[test]
    fn generated_model_has_expected_lengths() {
        let nodes = [0, 1, 2];
        let edges = [(0, 1), (1, 2)];
        let allowed_h: &[i32] = &[-1_000, 0, 1_000];
        let allowed_j: &[i32] = &[-1_000, 1_000];

        let nonce = derive_nonce(&[1; 32], &ALICE_BYTES, &SALT_A);
        let (h, j) = generate_ising_model(
            nonce,
            &nodes,
            &edges,
            &AllowedValueSpec::Set(allowed_h),
            &AllowedValueSpec::Set(allowed_j),
        )
        .unwrap();

        assert_eq!(h.len(), nodes.len());
        assert_eq!(j.len(), edges.len());
        // Every sampled h is drawn from the allowed set.
        for value in h {
            assert!(allowed_h.contains(&value));
        }
        for value in j {
            assert!(allowed_j.contains(&value));
        }
    }

    #[test]
    fn indexed_generation_matches_standalone_generation() {
        let nodes = [10, 20, 30];
        let edges = [(10, 20), (20, 30)];
        let allowed_h = AllowedValueSpec::Set(&[-1_000, 0, 1_000][..]);
        let allowed_j = AllowedValueSpec::Set(&[-1_000, 1_000][..]);
        let nonce = derive_nonce(&[3; 32], &ALICE_BYTES, &SALT_A);
        let topology = TopologyIndex::new(&nodes, &edges).unwrap();

        assert_eq!(
            generate_ising_model(nonce, &nodes, &edges, &allowed_h, &allowed_j).unwrap(),
            generate_ising_model_indexed(nonce, &nodes, &edges, &allowed_h, &allowed_j, &topology)
                .unwrap()
        );
    }
}

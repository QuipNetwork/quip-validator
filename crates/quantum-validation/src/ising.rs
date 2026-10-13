//! Deterministic Ising-model derivation helpers.

use alloc::vec::Vec;

use blake3::Hasher;
use rand_chacha::ChaCha8Rng;
use rand_core::SeedableRng;
use sp_core::U256;

use crate::errors::ValidationError;
use crate::fixed::MilliValue;
use crate::hardness::{frustration_index_with_index, NodeIndex};
use crate::puzzle_spec::AllowedValueSpec;
use crate::validation::ensure_valid_topology_with_index;

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
    generate_ising_model_with_index(
        &NodeIndex::new(nodes),
        nonce,
        nodes,
        edges,
        allowed_h,
        allowed_j,
    )
}

/// Generate the model and price the instance it produced, building the node
/// index exactly once.
///
/// `submit_proof` needs both back to back over the same immutable
/// `topology.nodes`; called separately, [`generate_ising_model`] and
/// [`crate::hardness::frustration_index`] each build and discard their own —
/// two extra `O(n)` scans, or two `Vec`s plus `O(n log n)` sorts on the table
/// path.
///
/// Otherwise untouched: same validation, same errors in the same ORDER, same
/// `(milli, cycles)` pair. Nodes are validated before any sampling, so the
/// frustration pass never sees an endpoint outside `nodes`.
pub fn generate_ising_model_and_frustration(
    nonce: U256,
    nodes: &[u32],
    edges: &[(u32, u32)],
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
) -> Result<IsingInstance, ValidationError> {
    let index = NodeIndex::new(nodes);
    let (h, j) =
        generate_ising_model_with_index(&index, nonce, nodes, edges, allowed_h, allowed_j)?;
    let (frustration_milli, frustration_cycles) = frustration_index_with_index(&index, edges, &j);
    Ok(IsingInstance {
        h,
        j,
        frustration_milli,
        frustration_cycles,
    })
}

/// One realized puzzle instance: the sampled model plus its gauge-invariant
/// hardness measure. Named rather than a four-tuple so the `submit_proof` call
/// site destructures into the bindings it used when these were two calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsingInstance {
    /// Per-node local fields, aligned with `nodes`.
    pub h: Vec<MilliValue>,
    /// Per-edge couplings, aligned with `edges`.
    pub j: Vec<MilliValue>,
    /// [`crate::hardness::frustration_index_milli`] of this draw.
    pub frustration_milli: u32,
    /// How many fundamental cycles that fraction averaged over.
    pub frustration_cycles: u64,
}

fn generate_ising_model_with_index(
    index: &NodeIndex<'_>,
    nonce: U256,
    nodes: &[u32],
    edges: &[(u32, u32)],
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
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

    // NOT removed, though `register_topology` already validated these exact
    // stored slices. Dropping it would turn a refusal into an acceptance if that
    // stored invariant were ever violated — a consensus-behaviour change. The
    // invariant is re-proved, just without rebuilding the index to do it.
    ensure_valid_topology_with_index(index, nodes, edges)?;

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
    use super::{derive_nonce, generate_ising_model};
    use crate::puzzle_spec::AllowedValueSpec;

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
        // Sharing one `NodeIndex` must change nothing observable.
        let instance = super::generate_ising_model_and_frustration(
            nonce,
            &nodes,
            &edges,
            &AllowedValueSpec::Set(allowed_h),
            &AllowedValueSpec::Set(allowed_j),
        )
        .unwrap();
        assert_eq!((&instance.h, &instance.j), (&h, &j));
        assert_eq!(
            (instance.frustration_milli, instance.frustration_cycles),
            crate::hardness::frustration_index(&nodes, &edges, &instance.j)
        );
        // Every sampled h is drawn from the allowed set.
        for value in h {
            assert!(allowed_h.contains(&value));
        }
        for value in j {
            assert!(allowed_j.contains(&value));
        }
    }
}

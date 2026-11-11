use alloc::vec::Vec;
use codec::Encode;
use quantum_validation::{AllowedValueSpec, MilliValue};
use sp_core::H256;

/// Compute the topology hash of a family topology.
///
/// `legacy_hash` is [`hash_topology`] over the same graph and value sets.
/// The family descriptor digest and the topology order digest are mixed in
/// under a domain tag, so a family topology never collides with the legacy
/// registration of the same graph, and the same graph under two descriptors
/// or two node orders gives two challenges.
pub fn hash_family_topology(
    legacy_hash: H256,
    descriptor_digest: [u8; 32],
    topology_order_digest: [u8; 32],
) -> H256 {
    H256::from(sp_io::hashing::blake2_256(
        &(
            b"quip-family-topology-v1",
            legacy_hash,
            descriptor_digest,
            topology_order_digest,
        )
            .encode(),
    ))
}

/// Compute the canonical topology hash for a registered puzzle definition.
///
/// The hash binds the graph structure together with the allowed value sets
/// for h, j, and spins. Two topologies that differ in any of these inputs
/// receive distinct hashes; topologies that differ only by node/edge ordering
/// or `Set` element ordering receive the same hash (inputs are sorted before
/// hashing).
pub fn hash_topology(
    nodes: &[u32],
    edges: &[(u32, u32)],
    allowed_h: &AllowedValueSpec<&[MilliValue]>,
    allowed_j: &AllowedValueSpec<&[MilliValue]>,
    allowed_spin: &AllowedValueSpec<&[MilliValue]>,
) -> H256 {
    let mut canonical_nodes = nodes.to_vec();
    canonical_nodes.sort_unstable();

    let mut canonical_edges: Vec<(u32, u32)> = edges
        .iter()
        .map(|&(u, v)| if u <= v { (u, v) } else { (v, u) })
        .collect();
    canonical_edges.sort_unstable();

    H256::from(sp_io::hashing::blake2_256(
        &(
            canonical_nodes,
            canonical_edges,
            allowed_h.canonical_bytes(),
            allowed_j.canonical_bytes(),
            allowed_spin.canonical_bytes(),
        )
            .encode(),
    ))
}

use codec::Encode;
use quantum_validation::{AllowedValueSpec, MilliValue};
use sp_core::H256;

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
    let (canonical_nodes, canonical_edges) = quantum_validation::canonical_graph(nodes, edges);

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

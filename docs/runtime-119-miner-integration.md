# Runtime 119: what miners and clients must change

Runtime 119 folds !71 (single-dial PoUW difficulty) into the !87 controller.
A miner built for 118 is refused on 119. Ship the miner release together with
the runtime upgrade.

## Transactions

- `transaction_version` is 8. Signed extrinsics built for 7 are invalid.
- `submit_proof` (call index 4): `QuantumProof::solutions` is one packed
  configuration (`PackedSpinBytes`), not a `BoundedVec` of them. Submit the
  single best configuration.
- `register_topology` gains a trailing `TopologyHardness` argument (root only).
- New calls: `set_topology_hardness` (9, root), `prove_topology_exact` (10)
  and `prove_topology_planar` (11), both permissionless.

## Puzzle generation

- Stored graphs are canonical: nodes sorted ascending, each edge oriented
  `(min, max)`, then edges sorted. The v7 migration rewrites topologies
  registered before 119. The hash does not change, but the same nonce now
  generates different `h` and `j`. Re-read `nodes`/`edges` from
  `RegisteredTopologies` (or `mining_snapshot`) after the upgrade, and drop
  any in-flight proofs.
- See `docs/ising-topology.md` and `docs/fixtures/ising-topology.json` for a
  worked vector.

## Admission

A proof clears its own instance bar, not the topology threshold:

- Draws more than three σ from the topology's expected frustration are
  refused (`InstanceOutsideFrustrationBand`).
- Draws with no frustrated cycle on a cyclic topology are refused
  (`InstanceIsGaugeTrivial`).
- Admitted draws get a bar of `max(curve_bar, -(Σ|h| + Σ|J|))`, nudged by
  `HandicapPerSigmaPermille` per σ. A proof must reach `energy <= bar`.
- The block's winner is the proof with the highest `proof_quality`: depth
  below its own bar divided by the bar's distance to the instance optimum.
  Raw energy no longer decides.

Do not re-implement these checks. Call the runtime API below before solving a
draw, and re-salt on any error.

## Runtime API (`QuantumPowApi` version 3)

- `instance_check(topology_hash, miner, salt) -> Result<InstanceCheck,
  DispatchError>` is new. It returns the nonce, frustration, σ, the instance
  bar, the anchor and the live threshold, or the exact error `submit_proof`
  would raise for that draw.
- `DifficultyConfig` has one field, `max_energy_milli`.
- `QBlock` appends `frustration_milli` and `instance_bar_milli`. In raw
  storage, qblocks won before 119 keep the old (v6) encoding; the runtime API
  converts them. Indexers that decode `QuantumPow.QBlocks` directly must try
  the current layout, then the v6 layout.
- The old shapes stay decodable for pre-119 blocks through the
  `*_before_version_3` client methods. Feature-detect with the reported API
  version.

## Events and errors

New events are appended, so existing indices are unchanged.
`ProofAccepted` gains `frustration_milli` and `instance_bar_milli`. Indexers
should also handle `DefaultProvenTractable`, `DefaultFailedOver` and
`DefaultRetired`: a default topology proven tractable keeps paying for one day,
then a whitelisted standby takes over. Mine whatever `DefaultTopology` names.

## Release checklist

1. Single-configuration `submit_proof`, `transaction_version` 8.
2. Topology read from chain state after the upgrade, in stored order.
3. `instance_check` before solving; re-salt on refusal.
4. Follow `DefaultTopology` changes from events, not a pinned hash.
5. Test against a local `--dev` node on runtime 119 before the aglais upgrade.

# Family activation gate

No family is mineable on a Quip runtime today. Both runtime crates set the
family allowlist, `Config::PowFamilies`, to an empty slice. A family topology
registers only when its contract is on that list, so the gate opens with a
runtime upgrade and with nothing else.

This page records what the gate is, the steps that open it, the verifier
costs that were measured before any activation, and how old and new software
behave across the boundary.

## Status

| Horizon | Item | Confidence |
|---------|------|------------|
| Now | Gate closed. The allowlist is empty on `quip` and `quip-testnet`. | Committed |
| Now | The runtime charges family proofs by weight class, with reference-machine measurements. | Committed |
| Now | The square-class charge exceeds the normal-class block budget, so no square proof fits in a block. | Committed |
| Next | A nonce sweep on the reference machine narrows the variance margin. | Directional |
| Later | A side-aware weight for the square-lattice families. | Exploratory, not committed |
| Later | Compact per-family witness codecs in the generator crate. | Exploratory, not committed |

## What the gate controls

A family topology is a registered topology that carries a family descriptor.
The descriptor holds the family id and revision together with the canonical
parameter bytes. The chain draws
the Ising coefficients of each round through the canonical generator of that
family instead of through the allowed value sets. The descriptor is part of
the topology hash, so a proof names the challenge and nothing in a proof can
select or change a descriptor.

`register_topology` admits a descriptor through three checks, in this order:

1. The contract is on `Config::PowFamilies`. An absent contract fails with
   `FamilyRejected` before any parameter is parsed.
2. The parameters parse against the contract schema and are within its
   bounds. The parameter bytes are capped at 4,096 bytes.
3. The family topology check accepts the registered graph, and the instance
   sizes stay within `MaxNodes` and `MaxEdges`. Every instance that a nonce
   can produce is bounded by this step, so the verifier never regenerates
   beyond the configured limits.

The allowlist is the only gate. The benchmark build of the runtime lists every
canonical family so the benchmarks can run their fixtures. A release blob
never enables `runtime-benchmarks`, and the `on-chain-release-build` feature
does not include it.

## Activation steps

Promotion is a reviewed decision. Complete the steps in order and record the
decision with a link to the review.

1. **Accepted science.** The family revision has registry status `accepted`
   in solver-core, its golden vectors are vendored under
   `crates/quantum-validation/tests/fixtures/solver-core`, and the parity
   tests pass on the pinned generator revision.
2. **Measured verifier limits.** The family maps to a weight class in
   `FamilyWeightClass`, and the `benchmark-weights` job has run on the
   reference machine with that class fixture. A family outside the three
   classes needs a class and a fixture first. Compare the measured weight
   with the normal-class block budget of 1.5 seconds and record how many
   proofs fit in a block.
3. **Matching consumer releases.** Miners and coordinators that serve the
   family read runtime API version 3 and handle the `family` field of
   the mining snapshot. See the compatibility matrix below.
4. **Runtime upgrade.** List the contract in `QuantumPowFamilies` in both
   runtime crates, keep the two trees in parity, bump `spec_version` only
   if the running specification has shipped, and regenerate the signing fixture if
   the transaction version changes.
5. **Root sequence after the upgrade.** Each call checks the one before it.

   | Step | Call | Check |
   |------|------|-------|
   | 1 | `register_topology` with the descriptor | admission, hash under the family domain tag |
   | 2 | `set_difficulty` | topology registered |
   | 3 | `set_topology_energy_band` | topology registered and is a family topology |
   | 4 | `add_mineable_topology` | band present, at most one topology besides the default |
   | 5 | `set_default_topology` | topology mineable |
   | 6 | `remove_mineable_topology` on the old topology | old topology is no longer the default |

6. **Explicit decision.** The activation is recorded as a decision with the
   measured limits, the consumer releases, and the block in which the
   switch is planned. Live activation and weight publication stay with the
   release process.

A rollback runs step 5 in reverse. Add the old topology back to the whitelist
and repoint the default to it. Then remove the family topology. History does
not move. Each qblock keeps the
topology hash and the difficulty of its own round, and the nonce of an
old block re-derives from the stored proof hash. The test
`switching_the_mineable_topology_from_legacy_to_family_keeps_history` covers
that sequence.

## Measured verifier limits

The runtime bounds are 5,000 nodes and 50,000 edges. Every canonical family
builds on the Zephyr lattice `z(m, t)`, so the largest admitted instance is a
full lattice. The measurement ran the validation crate natively in release
mode on a developer machine on 2026-11-11. Native numbers show the shape of
the cost. The reference machine measures the Wasm numbers that become
weights.

### Cost per proof

The verifier runs admission and regeneration once per proof and the witness
check once per witness. Generation sets the cost. A witness costs about one
hundredth of it.

| Family | Fixture | Base nodes / edges | Instance nodes / edges | Admission | Regeneration |
|--------|---------|-------------------:|-----------------------:|----------:|-------------:|
| ice-glass | z(10,5) | 4,200 / 47,770 | 4,200 / 47,770 | 15 ms | 18 ms |
| ice-fields | z(10,5) | 4,200 / 47,770 | 4,200 / 47,770 | 16 ms | 18 ms |
| charged-ice | z(10,5) | 4,200 / 47,770 | 4,200 / 47,770 | 15 ms | 17 ms |
| twisted-ice-dimer | z(10,5) | 4,200 / 47,770 | 4,200 / 47,770 | 16 ms | 17 ms |
| two-commodity-routing, divisor 16 | z(10,5) | 4,200 / 47,770 | 2,040 / 5,224 | 35 ms | 35 ms |
| two-commodity-routing, divisor 4 | z(10,5) | 4,200 / 47,770 | no instance | 35 ms | 28 ms per skipped nonce |
| polariton-static, L=20, 81 pairs | z(12,4) | 4,800 / 45,864 | 840 / 2,166 | 6 ms | 9 ms, or 55 ms when skipped |
| polariton-static, L=40, 361 pairs | z(24,1) | 4,704 / 18,330 | 3,280 / 9,126 | 7 ms | 0.5 s to 1.3 s, every nonce skipped |
| polariton-static, L=40, 1 pair | z(24,1) | 4,704 / 18,330 | 3,280 / 9,126 | 7 ms | 0.11 s to 0.25 s |
| flow-static, L=40 | z(24,1) | 4,704 / 18,330 | 3,280 / 9,598 | 6 ms | 82 ms |
| defects-static, L=3 | z(24,1) | 4,704 / 18,330 | 24 / 52 | 14 ms | 7 ms |

Three facts follow from the table.

- The ice families cost the same within 10%. Their cost grows with
  the edge count, so the lattice with the most edges under the bounds is the
  worst case.
- Routing costs twice an ice family at the same lattice, although its
  instance is smaller. The terminal checks run a maximum flow per pair.
- Polariton costs up to twenty times an ice family. Each nonce draws charge
  labels up to four times and runs a maximum flow per draw. With many pairs
  every draw fails. The nonce is then skipped after the verifier has paid the
  full cost. Flow has no draw loop and stays near 0.1 s at the largest side.

### Cost per witness

| Instance nodes / edges | Feasibility and energy | Energy alone |
|-----------------------:|-----------------------:|-------------:|
| 4,200 / 47,770 (ice) | 0.07 ms | 0.07 ms |
| 2,040 / 5,224 (routing) | 0.013 ms | 0.009 ms |
| 3,280 / 9,126 (polariton) | 0.8 ms | 0.014 ms |

### Variance across nonces

A FRAME benchmark fixes one salt. The regeneration cost of a nonce varies
with the instance it draws. Across 24 nonces, under concurrent load:

| Fixture | Mean | Slowest | Slowest over mean |
|---------|-----:|--------:|------------------:|
| ice-fields, z(10,5) | 36 ms | 54 ms | 1.5 |
| routing, divisor 16, z(10,5) | 137 ms | 288 ms | 2.1 |
| polariton, L=24, 121 pairs, z(24,1), skipped nonces | 121 ms | 179 ms | 1.5 |

The weight wrapper scales every family weight by a margin of 150% to cover
nonces the benchmark did not draw. The constant and its calibration
note live in `pallets/quantum-pow/src/weights.rs`.

### Weight model

The pre-dispatch weight of `submit_proof` on a family topology depends on
the family id and the witness count. It does not read the instance, because
admission bounds every instance by `MaxNodes` and `MaxEdges` and each class
is benchmarked at the largest lattice those bounds admit.

| Class | Families | Benchmark | Fixture |
|-------|----------|-----------|---------|
| Ice | ice-glass, twisted-ice-dimer, charged-ice, ice-fields | `submit_family_proof_ice(s)` | ice-glass on the lattice with the most edges |
| Routing | two-commodity-routing | `submit_family_proof_routing(s)` | divisor 16 on the lattice with the most edges and `t >= 3` |
| Square | polariton-static, flow-static, defects-static | `submit_family_proof_square()` | polariton with the largest side and the most pairs that admit, on a skipped nonce |
| Unknown | any other id | maximum of the three | |

`register_topology` with a descriptor adds `register_family_topology`, the
admission cost of the routing fixture, to the legacy registration weight.

The values in `benchmark_weights.rs` come from the `benchmark-weights` job on
the reference machine. The margin applies to the whole weight. The charges
with the margin applied are in this table.

| Class | Measured weight | Charge with margin |
|-------|-----------------|--------------------|
| Ice | 61.5 ms + 0.19 ms per solution | 0.16 to 0.17 seconds |
| Routing | 119 ms + 0.07 ms per solution | about 0.30 seconds |
| Square | 902 ms | about 2.26 seconds |

The normal-class block budget is 1.5 seconds. That budget holds about eight
ice proofs or four routing proofs. A square proof does not fit, so no miner can
mine a square family until the square charge falls below the budget.
A square-lattice family with a small side pays the large-side weight until a
side-aware weight exists.

### Rejection order

A rejected proof on a cheap path never pays for an instance. The verifier first
checks the miner registration and the proof limit of the block. It then
requires at least one solution, a registered and mineable topology, the
minimum graph size, and the expected nonce. Only then does it admit the
descriptor and regenerate. A nonce the
family skips, a witness of the wrong length, and an infeasible witness are
rejected after regeneration and pay the full generation cost, which the
weight already charges.

## Compatibility matrix

"Old" software predates this change. It speaks runtime API version 2, where
the mining snapshot and the topology record have no `family` field, and it
sends `register_topology` with six arguments. "New" software is this change.

| Peer | Against a legacy topology | Against a family topology |
|------|---------------------------|---------------------------|
| Old miner, new chain | Mines as before. | Not supported. The miner draws `h` and `J` from the value sets, so every proof fails with `FamilyWitnessInvalid` or `InsufficientEnergy` and the QPU time is lost. |
| New miner, old chain | Mines as before. The snapshot has no `family` field and the miner treats the topology as legacy. | Does not arise. An old chain has no family topologies. |
| Old coordinator, new chain | Reads the snapshot and topology record as before. | Not supported. The coordinator cannot see the descriptor and hands miners a legacy challenge. |
| New coordinator, old chain | Reads the snapshot and topology record. | Does not arise. |
| Old root tooling, new chain | `register_topology` must send the seventh argument, `None`. A six-argument call fails to decode. | Cannot register a family topology. |

Old peers are not supported on a family topology. A family activation
requires the consumer releases in step 3 of the activation steps, and the
miner registry should refuse to route a family challenge to a miner that
reports an API version below 3.

## Verification

The commands below ran on the branch on 2026-11-11.

```text
cargo test -p pallet-quantum-pow family_
cargo test -p pallet-quantum-pow --features runtime-benchmarks
cargo test -p quantum-validation
vale docs/family-activation.md
```

The benchmark preflight job runs every benchmark, including the four family
benchmarks, against the production runtime constants. The Wasm runtime
build and the node crate are covered by CI only on this branch.

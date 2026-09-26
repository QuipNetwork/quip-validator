# Live Topology Upgrade (Operator Procedure)

How to repoint a running chain to a new default quantum PoW topology — for
example tracking D-Wave `Advantage2_system1` working-graph snapshots across
calibrations, or switching the puzzle class (the v0.2 → h = 0 spin-glass
upgrade).

Topologies are immutable once registered, and `register_topology` only seeds
`DefaultTopology` and `MineableTopologies` on the very first registration. A
later registration lands only in `RegisteredTopologies`. Upgrading a live chain
is a five-step sequence of root calls (the quip-testnet sudo holder is operator
1; see `docs/genesis-quip-testnet.md`). Run the steps in order; each one says
why it comes where it does.

## 1. Register the new topology

`Sudo.sudo(QuantumPow.register_topology(nodes, edges, allowed_h_values,
allowed_j_values, allowed_spin_values))`

For the h = 0 spin-glass class on Advantage2_system1:

| Field | Value | Notes |
| --- | --- | --- |
| `nodes`, `edges` | working-graph snapshot | Active qubits/couplers from `solver.properties` (`qubits`, `couplers`) at the current calibration, **not** the pristine `zephyr(12,4)` graph. Node labels must match the sampler's linear indices. |
| `allowed_h_values` | `Set([0])` | Every puzzle h is exactly 0. A single-value set is valid; h is never wire-encoded (only spins are packed), so payload size is unaffected. |
| `allowed_j_values` | `Set([-1000, 1000])` | Binary ±J in milli units; inside every solver's `j_range = [-1, 1]`, 1 bit per coupling. |
| `allowed_spin_values` | `Set([-1000, 1000])` | Binary spins, 1 bit per spin in packed solutions. |

Snapshot registrations are expected to recur: each D-Wave recalibration that
changes the working graph gets a fresh registration (a new hash) and a
change of default. For the registration bounds, the hash inputs, the
allowed-value spec rules and where a topology binds, see
[`docs/ising-topology.md`](ising-topology.md).

The `TopologyRegistered` event carries the new topology's hash, written
`new_hash` below; `old_hash` is the current default.

## 2. Set the new topology's difficulty

`Sudo.sudo(QuantumPow.set_difficulty(new_hash, difficulty))`

`difficulty` is a `DifficultyConfig { min_solutions, max_energy_milli,
min_diversity_milli }`. The hash must be registered (`TopologyNotRegistered`
otherwise). Difficulty is per topology: `Difficulties` is keyed by hash, and a
hash with no entry reads back as `DifficultyConfig::default()`, whose
`max_energy_milli` of -1,200,000 was not chosen for the new graph.

Each topology also has its own energy curve. `energy_curve_for` builds it with
`EnergyCurve::new` from that topology's node and edge counts, its h/J value
specs and its curve `c` values, which are the `set_topology_curve` override if
one is set and the runtime `QuantumPowCurveC*Milli` constants otherwise. The
curve points come from `quantum_validation::expected_gse`. Pick a starting
`max_energy_milli` inside the new curve's `[min, max]` band. A zero-field
curve is strictly less negative than a ternary-field curve of the same graph,
so the old topology's threshold is not a safe starting point. The `c = 0.700`
easy point of the new curve is a sane reset; decay and proof adjustment take
over from there. If the new topology needs its own `c` values, run
`set_topology_curve(new_hash, curve_c)` before this step.

Do this before step 3. Once the topology is mineable, `submit_proof` accepts
proofs against it at whatever difficulty it reads.

## 3. Make it mineable

`Sudo.sudo(QuantumPow.add_mineable_topology(new_hash))`

At most one non-default topology may be mineable at a time. If a non-default
entry is already in `MineableTopologies`, this fails with
`MineableTopologyConflict`; remove that entry first. Emits
`TopologyMineableAdded`.

## 4. Switch the default

`Sudo.sudo(QuantumPow.set_default_topology(new_hash))`

The hash must be registered (`TopologyNotRegistered`) and already mineable
(`TopologyNotMineable`); step 3 provides the second. Changing the default does
not change any topology's curve or difficulty, because both are per topology.
It changes which topology the chain advertises as the one to mine. Emits
`DefaultTopologySet`.

## 5. Retire the old topology from mining

`Sudo.sudo(QuantumPow.remove_mineable_topology(old_hash))`

After step 4 the old default is an ordinary non-default mineable entry. Until
it is removed, `submit_proof` still accepts proofs against it, and it blocks
the next upgrade's step 3. The registration itself stays in
`RegisteredTopologies`; the runtime has no path that removes it. Emits
`TopologyMineableRemoved`.

## Why h = 0 changes the curve

The expected ground-state energy estimate is

```
E ≈ -c·⟨|J|⟩·√(d̄)·n  -  c·α·⟨|h|⟩·n/√(d̄)        (d̄ = mean degree, α = 0.88)
```

with `⟨|h|⟩` and `⟨|J|⟩` derived from the registered specs; the authoritative
form is the `expected_gse` doc comment
(`crates/quantum-validation/src/energy.rs:83-106`). The legacy ternary spec
has `⟨|h|⟩ = 2/3`; `Set([0])` has `⟨|h|⟩ = 0`, dropping the field term.
Without the spec-aware curve, an h = 0 topology would inherit thresholds
targeting energy that no zero-field puzzle can produce.

The empirical `c` calibration constants
(`QuantumPowCurveC{Easy,Knee,Hard}Milli`) were fitted with the field term
present; expect to re-measure them against real solver runs on the new
puzzle class and adjust via runtime upgrade if mining times drift.

## Follow-ups outside this repo

- `quip-protocol-2`: working-graph snapshot tooling (dump `solver.properties`
  qubits/couplers into a `register_topology` payload) belongs next to
  `shared/miner_bootstrap.py`, which already builds dev-chain payloads; its
  `--seed-chain` defaults still register the legacy ternary-h spec.
- Miner energy/expectation models that mirror `expected_gse` must mirror the
  spec-aware form for the new default topology.

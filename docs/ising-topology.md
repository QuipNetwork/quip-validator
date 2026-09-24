# Ising Topology (Protocol Specification)

A topology is the puzzle definition that quantum PoW mines against: a graph
together with the value sets its fields, couplings and spins may take.
`docs/topology-upgrade.md` is the operator procedure for pointing a live chain
at a new one; this document specifies the object that procedure moves.

Everything specified here lives in `pallets/quantum-pow`,
`pallets/quantum-compute-mempool` and the `quantum-validation` crate. Path and
line citations were verified against the commit that added this document.
Nothing in CI re-checks them, so treat the symbol names as authoritative if a
line number has since drifted.

## The topology object

A topology is a five-tuple:

```text
(nodes, edges, allowed_h_values, allowed_j_values, allowed_spin_values)
```

`TopologyMeta` (`pallets/quantum-pow/src/types.rs:79-92`) is the stored form.
It carries those five fields plus `registered_at`, the block number at which
the registration landed. `registered_at` is not part of the hash.

A topology is immutable once written. There is no mutation extrinsic and no
`unregister_topology`: the current runtime API has no path that removes a
registration. Superseding a topology means registering a new one and pointing
the default at it; the old entry stays in state.

## Labels and positions

Node ids are arbitrary `u32` hardware labels. In practice they are a QPU's
linear qubit indices, and they may be sparse and non-contiguous: a registered
working graph carries the labels the hardware reports at that calibration,
with a gap wherever a qubit is offline.

`TopologyIndex` (`crates/quantum-validation/src/validation.rs:42-81`) is the
map from label to position. Its doc comment gives the reason it exists as an
object rather than a helper: building it once turns repeated edge-endpoint
resolution from a linear scan over `nodes` into a logarithmic map read, and a
caller scoring several solutions against one topology should reuse a single
index for the whole set.

What is indexed by what:

| Array | Indexed by |
| --- | --- |
| `nodes` | nothing; this array defines the position order |
| `edges` | pairs of hardware labels, each of which must appear in `nodes` |
| `h` | position in `nodes` |
| `j` | position in `edges` |
| a returned spin vector | position in `nodes` |

Reading a label as a position is the failure mode this distinction exists to
prevent. On a sparse node set the two differ, and code that treats a native id
as a dense index reads the wrong element or runs off the end of the array.

## Allowed value specs

`AllowedValueSpec` (`crates/quantum-validation/src/puzzle_spec.rs:30-38`) has
three variants. Each one fixes **both** the distribution the nonce-seeded RNG
draws from and the on-chain bit width used to encode a drawn value
(`puzzle_spec.rs:1-7`):

| Variant | Sampling | Encoded width |
| --- | --- | --- |
| `Set(values)` | `values[next_u32() % len]`; the encoded value is the index into the set | `ceil(log2(len))`, minimum 1 bit |
| `IntegerRange { min, max }` | `min + next_u32() % span`, scaled by `MILLI_SCALE` on return | `ceil(log2(span))`, minimum 1 bit |
| `ContinuousRange { min, max }` | `min + next_u32() % span`, in milli units, no scaling | 32 bits, a raw `MilliValue` |

Here `span` is `max - min + 1`, computed in `u64`. The sampling column is the
exact operation `sample` performs (`puzzle_spec.rs:155-192`), not an ideal
uniform draw: when `len` or `span` does not divide 2^32 the reduction has
modulo bias, and a second implementation must reproduce the same operation to
reproduce the same puzzle.

`bits_per_value` (`puzzle_spec.rs:72-112`) computes the width; its doc comment
(`:62-71`) is the per-variant contract. A single-value set still costs one
bit, because a zero-bit encoding is not valid. `MAX_INDEXED_BITS = 8` (`:19`)
caps a `Set` at 256 entries and an `IntegerRange` at a span of 256; for `Set`
the runtime's `MaxAllowedValues` bound binds first, at 32.

A `Set` is sorted at registration, but duplicates are kept. A repeated value
stays in the set as a separate entry: it counts toward `len`, so it widens the
encoding, and it is drawn proportionally more often.

The spin spec is the one with a size consequence. A submitted solution is a
`BoundedVec<u8, MaxNodes>` (`PackedSpinBytesOf<T>`,
`pallets/quantum-pow/src/lib.rs:36`), so `ContinuousRange` spins at four bytes
each make any topology with more than `MaxNodes / 4` nodes impossible to mine.
Registration rejects that case outright rather than accepting a topology no
miner could ever submit against (`lib.rs:728-741`).

## The topology hash

`hash_topology` (`pallets/quantum-pow/src/topology.rs:13-39`) is `blake2_256`
over the SCALE encoding of five canonical inputs, in this order:

1. `nodes`, sorted ascending
2. `edges`, each normalized to `u <= v`, then sorted
3. `allowed_h_values.canonical_bytes()`
4. `allowed_j_values.canonical_bytes()`
5. `allowed_spin_values.canonical_bytes()`

`canonical_bytes` (`crates/quantum-validation/src/puzzle_spec.rs:194-227`)
emits a discriminant byte -- `0` for `Set`, `1` for `IntegerRange`, `2` for
`ContinuousRange` -- followed by big-endian values, sorting a `Set`'s contents
first. That discriminant is why `Set([0])`, `IntegerRange { min: 0, max: 0 }`
and `ContinuousRange { min: 0, max: 0 }` hash distinctly even though all three
admit only the value zero. `canonical_bytes_distinguish_variants`
(`puzzle_spec.rs:376-385`) guards it.

The equivalence class is stated on `hash_topology` itself
(`topology.rs:6-12`):

> The hash binds the graph structure together with the allowed value sets
> for h, j, and spins. Two topologies that differ in any of these inputs
> receive distinct hashes; topologies that differ only by node/edge ordering
> or `Set` element ordering receive the same hash (inputs are sorted before
> hashing).

Because `registered_at` is excluded, the hash does not depend on when or where
a topology was registered: two chains that register the same five-tuple derive
the same hash at any block.

### The hash names an equivalence class; the stored order names the puzzle

The hash identifies an equivalence class of inputs, not one concrete puzzle.
Puzzle generation is order-sensitive, and `register_topology` sorts only part
of what it stores.

The value sets are sorted. `register_topology` calls
`canonicalize_spec` on each spec before storing it (`lib.rs:724-726`), and the
comment above that call says why (`lib.rs:719-723`): the hash sorts its
inputs, but `sample` indexes into the stored slice. Without canonicalization,
registering `[a, b, c]` and `[b, a, c]` would hash to the same value and yet
produce different deterministic puzzles from the same nonce.

The graph is not. `nodes` and `edges` are stored in the order the caller
supplied (`lib.rs:768-778`), and generation draws `h` in stored node order and
`j` in stored edge order (see [Puzzle generation](#puzzle-generation)). Two
registrations of the same graph in different node or edge order share a hash
and would generate different coefficients from the same nonce. Only one of
them can land: the second fails with `TopologyAlreadyRegistered`. Whichever
order is registered first becomes the concrete puzzle for that hash.

On a live chain this is consensus-safe, because every node reads the one
stored form from shared state. It does constrain a second implementation. To
reproduce a puzzle from a topology hash, build it from the `nodes` and `edges`
stored under that hash in `RegisteredTopologies`, in stored order. Do not
rebuild them from the graph in any canonical order of your own, including the
sorted order the hash uses.

### The same working graph does not hash the same everywhere

Two deployments running the same QPU model should not expect to share a
topology hash. The registered graph is a calibration snapshot -- the active
qubits and couplers reported by `solver.properties` at that moment, not the
pristine `zephyr(12,4)` lattice -- and calibrations differ per machine and
drift over time. On top of that, two deployments that somehow registered an
identical graph would still diverge if their allowed-value specs differed,
because the specs are inside the hash. A topology hash identifies a class of
puzzle definitions, never a hardware model.

## Puzzle generation

`submit_proof` rebuilds the puzzle from the nonce and the stored topology with
`generate_ising_model_indexed`
(`crates/quantum-validation/src/ising.rs:64-106`). A second implementation
has to match every step:

1. Seed a `rand_chacha` `ChaCha8Rng` with `from_seed`, passing the nonce as
   its 32 big-endian bytes (`ising.rs:92-93`).
2. Draw every `h` first: one `sample` of `allowed_h_values` per node, in
   stored node order. Then draw every `j`: one `sample` of `allowed_j_values`
   per edge, in stored edge order (`:95-103`). The two passes share one RNG
   stream, and each `sample` consumes exactly one `next_u32()`.
3. `h[i]` belongs to `nodes[i]` and `j[k]` to `edges[k]`, the positional
   mapping in [Labels and positions](#labels-and-positions).

Spins are not drawn. The miner submits them, one packed byte vector per
solution, and the pallet decodes them:

- `unpack_solution` (`crates/quantum-validation/src/packed.rs:31-107`) reads
  one value per node, in node order. For the indexed variants, value `i`
  occupies `bits_per_value` bits starting at bit offset `i * bits_per_value`.
  Bits are numbered from the least-significant bit of byte 0 upward, and the
  raw index is LSB-first. A `ContinuousRange` value is four bytes, big-endian.
  The vector must be exactly `ceil(nodes * bits_per_value / 8)` bytes long.
  `pack_solution` (`:109-146`) is the inverse.
- A raw index is decoded through the spin spec: an index into a `Set`, or an
  offset from `min` for an `IntegerRange`.
- `validate_proof` then collapses each decoded spin to its sign with
  `spin_signs` (`packed.rs:177-192`) and rejects a zero with
  `InvalidSpinValues`. Only the sign reaches the energy function
  (`packed.rs:9-13` records this as a v0.2 limitation). The magnitudes in
  `allowed_spin_values` are therefore inert today; see the gaps below.

The energy of a solution `s` is
`sum(h[i] * s[i]) + sum(j[k] * s[pos(u)] * s[pos(v)])` over every node
position `i` and every edge `k = (u, v)`, where `pos` is the `TopologyIndex`
lookup from label to position. The result is in milli units
(`crates/quantum-validation/src/energy.rs:48-81`).

`docs/fixtures/ising-topology.json` pins all of the above with one worked
vector: topology inputs to their canonical hash and its SCALE preimage, a
nonce to its `h` and `j` sequence, and two packed solutions to their decoded
spins, signs and energy. The hash is taken over `Set` specs given out of
order and matches the hash of the sorted specs that registration stores.
Generation and decoding use the stored specs, since those are the only form a
chain holds. The graph has five nodes, below the runtime's `MinNodes`, so it
cannot be registered on chain; none of the pinned functions check that bound.
The crate code generates the fixture, and a test regenerates it and fails on
any difference:

```sh
cargo run -p pallet-quantum-pow --example generate_ising_topology_fixture -- --write
cargo test -p pallet-quantum-pow --test ising_topology_fixture
```

A change to hashing, sampling or encoding therefore fails CI until the fixture
is regenerated on purpose.

## Registration

`register_topology` (`pallets/quantum-pow/src/lib.rs:697-794`) is the only way
a topology enters state. It is `ensure_root` (`:705`); there is no signed path
and no deposit-backed path.

Validation runs in this order:

| Step | Line | Error |
| --- | --- | --- |
| `nodes.len() >= MinNodes` | `:707-710` | `GraphTooSmall` |
| `check_spec` on h, j and spin | `:715-717` | `EmptyAllowedValues` / `EncodingTooWide` / `InvalidTopology` |
| `canonicalize_spec` on h, j and spin | `:724-726` | `InvalidTopology` |
| `packed_solution_byte_len(..) <= MaxNodes` | `:735-741` | `PackedSolutionTooLarge` |
| `validate_topology_consistency(..).is_empty()` | `:743-754` | `InvalidTopology` |
| hash not already registered | `:763-766` | `TopologyAlreadyRegistered` |

`validate_topology_consistency`
(`crates/quantum-validation/src/validation.rs:104-179`) takes optional allowed
sets for h and j, and `register_topology` passes `None` for both
(`lib.rs:749-750`). What it checks here is therefore purely structural:
duplicate node ids, `h`/`j` array lengths matching `nodes`/`edges`, and every
edge endpoint present in `nodes`. It is never a coefficient-value check on
this path.

It does not reject self-loops or parallel edges. An edge `(u, u)` passes, and
so does a pair repeated as `(u, v)` twice or as `(u, v)` and `(v, u)`
(`validation.rs:115-119`, `:151-154`). Both are registrable today. In the
energy sum a self-loop adds its coupling as a constant, because
`s[pos(u)] * s[pos(u)]` is always 1, and parallel edges each draw their own
`j` and add independently. Whether either is an intended protocol input is not
settled; see the gaps below.

On success the topology is inserted and
`TopologyRegistered { topology_hash, node_count, edge_count }` is emitted
(`lib.rs:788-792`).

The first registration on a chain bootstraps mining. If `DefaultTopology` is
unset, it is set to this hash and the hash is inserted into
`MineableTopologies` as well (`:780-786`), because the default must always be
mineable. Every registration after the first is inert: it publishes a
definition and changes nothing about what the chain mines.

## Bounds

The bounds are `#[pallet::constant]` associated types on `Config`
(`lib.rs:159-172`). The runtime declares their values at
`runtime/src/configs/mod.rs:560-584` and binds them at `:654-670`:

| Bound | Runtime constant | Value |
| --- | --- | --- |
| `MaxNodes` | `QuantumPowMaxNodes` | 5,000 |
| `MaxEdges` | `QuantumPowMaxEdges` | 50,000 |
| `MaxSolutions` | `QuantumPowMaxSolutions` | 32 |
| `MinNodes` | `QuantumPowMinNodes` | 16 |
| `MaxAllowedValues` | `QuantumPowMaxAllowedValues` | 32 |

`MaxNodes` does double duty. It bounds the node array and it bounds the packed
spin byte vector, which is the same `PackedSpinBytesOf<T>` alias at
`lib.rs:36`. That is exactly why a `ContinuousRange` spin spec caps usable
nodes at `MaxNodes / 4`.

A full Zephyr Z(12,4) working graph, at most 4,800 nodes, fits under
`MaxNodes` with an indexed spin spec.

## The three sets

Three storage items hold the topology state, and they answer three different
questions:

| Storage | Line | Shape |
| --- | --- | --- |
| `RegisteredTopologies` | `:201-203` | `StorageMap<Blake2_128Concat, H256, TopologyMeta>`; append-only in practice |
| `DefaultTopology` | `:205-206` | `StorageValue<H256>` |
| `MineableTopologies` | `:227-231` | `StorageMap<Blake2_128Concat, H256, ()>`; set semantics |

The `MineableTopologies` doc comment states its role (`:227-229`):

> Root-controlled whitelist of topologies that may be mined: a topology must
> have an entry here for `submit_proof` to accept its solutions. Steady state
> is `{ DefaultTopology }`.

The invariants over those three are a closed set:

- The default is always mineable. `set_default_topology` (`:805-828`) requires
  the target to be registered and already mineable, erroring
  `TopologyNotRegistered` and `TopologyNotMineable` respectively.
- At most one non-default topology is mineable at a time.
  `add_mineable_topology` (`:1046-1071`) scans for an existing non-default key
  and errors `MineableTopologyConflict` if it finds one. The comment at
  `:1056-1060` names this Model A and gives the reason: `LastProofBlock` is a
  global decay anchor, so a second concurrently mined topology would corrupt
  it. The whitelist is capped at `{ default, one incoming }` during a switch.
- The default cannot be removed from the whitelist. `remove_mineable_topology`
  (`:1073-1092`) refuses with `TopologyIsDefault`.

So `MineableTopologies` is a near-singleton by design, not by current
configuration.

### What registration does and does not obligate

Registration publishes a topology. It obligates nobody.

- Miners mine what they choose. A compute order does not have to correspond to
  any registered topology, and nothing on chain requires it to.
- The active mining set is `MineableTopologies`. It is implemented and it
  gates `submit_proof`, and membership is a precondition of
  `set_default_topology`. It gates nothing else.
- A required-to-be-solvable set -- topologies every miner must be able to
  handle -- is not implemented and is not planned. `register_topology` is root
  only, and the admin gate substitutes for it.
- The registered-only set is `RegisteredTopologies`. It is implemented and
  carries no obligation at all.

The governing principle is that the chain does not enforce topology policy on
miners or on compute-order admission. The root-managed mineable set is the one
chain-enforced topology rule, and it governs only which topologies consensus
PoW accepts proofs against. Topology obligations for miners, and topology
checks on compute orders, are out of scope as a class, not merely something
that has yet to be built. Which topologies a miner is willing to work on is
local policy and belongs in miner software. That software cannot act on the
policy today, because a compute order carries no topology id to filter on; see
the gaps below.

## Where a topology binds, and where it does not

Two pallets consume Ising problems, and only one of them knows topologies
exist.

| Aspect | Consensus PoW | Compute mempool |
| --- | --- | --- |
| pallet | `quantum-pow` | `quantum-compute-mempool` |
| submit path | `submit_proof` | `submit_solution` |
| topology binding | by hash reference | inline, by value |
| registered check | yes (`TopologyNotRegistered`) | none |
| mineable check | yes (`TopologyNotMineable`) | none |
| allowed-value specs | enforced from the registered topology | none |

`quantum-pow` is topology-by-reference. The comment on the lookup in
`submit_proof` states the consequence
(`pallets/quantum-pow/src/lib.rs:947-950`):

> Topology lookup is the source of truth for nodes, edges, and the allowed
> value sets. The proof's `topology_hash` is the only identity claim; there
> are no `proof.nodes`/`proof.edges` to cross-check.

Downstream of that lookup the pallet rebuilds the puzzle deterministically
from the nonce and the registered spec via `generate_ising_model_indexed`
(`:978-989`). The miner supplies a nonce, a salt and spins; every coefficient
comes from state.

The compute mempool is topology-by-value and reads no topology storage at all.
`pallets/quantum-compute-mempool/Cargo.toml` declares no dependency on
`pallet-quantum-pow`, and the crate contains no reference to `quantum_pow`,
`RegisteredTopologies` or `topology_hash`. `propose_job` validates the order's
own inline arrays with `validate_topology_consistency`, again with `None` for
both allowed sets (`pallets/quantum-compute-mempool/src/lib.rs:547-558`).
`submit_solution` rebuilds a `TopologyIndex` from those same stored arrays on
each call (`:656-660`). `JobOrder`
(`pallets/quantum-compute-mempool/src/types.rs:170-183`) has no topology
field; it carries `spec_id`, `proposer` and the inline `ising_params`.

The structural difference is that quantum-pow derives the puzzle from a nonce
plus a registered specification, while the mempool takes the puzzle as data.

A registered-but-non-mineable topology is not something the mempool declines
to accept. It is something the mempool cannot perceive. Mineable is a
consensus concept that gates `submit_proof` and the choice of default, and
because
`MineableTopologies` is a near-singleton by design, gating the mempool on it
would confine every user compute order to whichever single graph block
production happens to be using.

## Non-goals and known gaps

- **No topology id on the wire.** A `JobOrder` carries its graph inline and
  names no registered topology. This is what blocks miner-side filtering by
  registration status, and it is the dependency to close first if that
  filtering is wanted.
- **No `unregister_topology`.** The current runtime API has no path that
  removes a registration. Retiring a topology means pointing the default at
  another topology and then removing it from `MineableTopologies`; the entry
  itself stays.
- **No required-to-be-solvable set and no banned-topology set.** Both are
  withdrawn on principle rather than deferred. Topology obligations on miners
  and topology checks on compute orders are not a planned feature of this
  pallet.
- **Spin magnitudes are inert.** `validate_proof` reduces every decoded spin
  to its sign, so an `allowed_spin_values` spec with magnitudes other than one
  scores exactly like binary spins. The spec still sets the encoded width and
  rejects out-of-range indices.
- **Self-loops and parallel edges are not rejected.** Registration accepts
  them, as described under [Registration](#registration). Whether they are
  intended protocol inputs is an open question, not a settled rule.
- **No pinned nonce derivation.** The interoperability vector starts from a
  nonce. Nothing checked in pins `derive_nonce`
  (`crates/quantum-validation/src/ising.rs:31-37`), which derives that nonce
  from the last proof block hash, the miner and the salt.
- **No permissionless registration.** `register_topology` is root only. There
  is no deposit-backed or governance-voted path, and adding one is not the
  same question as adding topology policy.

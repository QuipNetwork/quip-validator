# PoUW API v3: miner and client upgrade contract

This filename predates deployment of spec 119. It describes the pending !71
upgrade, not the already deployed spec-119 contract. At pinned AGLS block
153726 (`0xb06d9fcbf2dcd1db33540f22b479728c462b8d1e64d61459cad6fca36b158712`),
the chain reports spec 119, transaction version 7, QuantumPowApi v2 and
QuantumPow storage v6. See the [captured evidence](../runtime/tests/fixtures/README.md).
This branch assigns spec **120** to the incompatible upgrade, based on the
[fresh live-version and release-ref audit](release.md#runtime-120-version-assignment).
Recheck that assignment against the live chain and release refs before release.

| Version domain | Pinned deployed state | !71 contract |
| --- | --- | --- |
| Runtime spec | 119 | 120 (recheck before release) |
| Signed transaction | 7 | 8 |
| QuantumPowApi | 2 | 3 |
| QuantumPow storage | 6 | 7, with legacy QBlock values retained |

## Transactions and puzzle generation

`submit_proof` retains call index 4, but `QuantumProof::solutions` becomes one
packed configuration, not a vector of packed configurations. Submit one best
solution. `set_difficulty` takes the one-field `DifficultyConfig`;
`register_topology` gains a trailing `TopologyHardness` argument. These changed
encodings require transaction version 8 and refreshed metadata/signing inputs.
New calls are root `set_topology_hardness` (9), permissionless
`prove_topology_exact` (10) and `prove_topology_planar` (11).

The v7 migration canonicalizes stored node and edge order without changing
topology hashes. If the stored order changes, the same nonce can generate
different positional h/J assignments. Already canonical graphs need not
change: the pinned AGLS graph is one such fixture. Refresh the stored topology,
runtime versions, allowed-value specifications and any edge-indexed witnesses
after an upgrade; invalidate cached work and pending proofs. Generate and pack
against the returned order, not a locally reordered adjacency list. See
[ising-topology.md](ising-topology.md) for the generation contract.

## Admission and work selection

Call `instance_check(topology_hash, miner, salt)` at the same block as the
mining snapshot before scheduling expensive solving. Its result supplies the
nonce, measured frustration/cycle count, raw integer sigma, lower bound,
instance threshold and live topology threshold.

- `InstanceIsGaugeTrivial` rejects zero measured frustration unless both cycle
  count and configured expected frustration are zero. The restriction also
  applies with fields; coupling parity alone does not prove the optimum then.
- `InstanceOutsideFrustrationBand` rejects raw integer sigma outside [-3,3].
  Sigma uses signed truncation; this is not a continuous three-sigma cutoff.
- Re-salt after these draw refusals. Topology eligibility failures require
  refreshing the default/whitelist/retirement state; repeated salt changes do
  not fix a retired or unmineable topology. Transport failures are not draw
  refusals.
- An admitted solution must satisfy `energy_milli <= instance_bar_milli`.
  The live topology threshold alone is insufficient. Pricing includes signed
  truncation, sigma/slope clamps and the symmetric-headroom rule; use the API
  result or a separately verified implementation of the
  [math contract](quantum-pow-difficulty-controller-decisions.md).

`instance_check` does not check miner registration, block proof limits, packed
solution length/content, achieved energy, signatures or payment. It is not a
guarantee that a later transaction succeeds; state may also change before
inclusion.

The winner maximizes quantized normalized depth below its own threshold:
`min((B-E)*10^15/(B-L), 10^15)` for positive room `B-L`; nonpositive room scores
zero. `L` is the lower bound `-(sum|h|+sum|J|)`, not a guaranteed attainable
optimum. Equal quality prefers lower numerical energy; equal quality and energy
retain the incumbent. The old solution-count/diversity gates do not apply to
this PoUW proof format. They must not be removed from unrelated compute-job
contracts.

## Select the API version at the queried block

The API id is `0x17f0e16273cf5f3b` (Blake2b with an eight-byte digest of
`QuantumPowApi`). Pin a block hash `at`, fetch its runtime version, then issue
the API request at that same hash. For generated Rust clients, use
`api.api_version::<dyn QuantumPowApi<...>>(at)`; raw RPC clients inspect the
`apis` array from `state_getRuntimeVersion(at)`. Do not select a decoder from
the latest runtime version when reading an older block.

| Reported API | Client action |
| --- | --- |
| 2 | Select the v2 response types; generated Rust clients use the seven `*_before_version_3` methods. No `instance_check` export is available. |
| 3 | Select current response types and use `instance_check` for new mining work. |
| Missing or another version | Stop or use an explicitly tested adapter for that version; accepting every version >=2 is unsafe. |

The seven changed methods are `mining_snapshot`, `current_difficulty`,
`current_hardness`, `difficulty_for`, `winning_solution`, `qblock_by_id` and
`qblock_by_block`. `#[changed_in(3)]` creates alternate **client** methods; it
does not automatically select a decoder and does not add `before_version`
Wasm exports. Wire calls still use names such as `QuantumPowApi_mining_snapshot`.

Require complete response consumption (`DecodeAll`, or an equivalent explicit
empty-remainder check) wherever the client owns raw SCALE decoding. The pinned
SDK's generated client uses plain `Decode`: calling modern
`current_difficulty` against v2 can silently interpret the first eight of
sixteen bytes as a wrong threshold. Strict decoding supplements version
selection; it does not replace it. Tests in
[quantum_pow_api_versions.rs](../runtime/tests/quantum_pow_api_versions.rs)
reproduce this failure and exercise every legacy method against captured v2
responses. [quantum_pow_wasm.rs](../runtime/tests/quantum_pow_wasm.rs) separately
checks the current exports in native and Wasm engines.

Refresh version/metadata caches on runtime changes and reconnects. A mining
client that supports both generations must select the matching proof encoding,
admission rules and transaction version too; changing only the snapshot decoder
is insufficient. Historical readers may retain v2 support without supporting
new v2 mining work.

## History, indexers and default-topology lifecycle

Current `DifficultyConfig` is one i64. Current QBlocks append measured
`frustration_milli` and `instance_bar_milli` after removing the two old
difficulty fields. Current API reads convert old records, retaining the old
recorded threshold D as the instance bar and using frustration zero to mean
unmeasured. That does not change historical strict `E<D` admission to the new
inclusive rule or establish that the historical draw was unfrustrated.

Raw storage is mixed: for production AccountId32/u128/u32, legacy QBlocks are
180 bytes and current ones 184 bytes. Select by supported bounded length and
fully consume that layout. Do not try permissive current decoding and then
fallback to old decoding. Reject other lengths. Without a format tag, a
truncated value that happens to have another supported length cannot generally
be distinguished from that layout. Metadata alone cannot identify each stored
value's generation. API-version selection and storage-layout selection are
separate decisions; spec numbers alone are insufficient for either.

`ProofAccepted` gains frustration and threshold fields; new event variants are
appended. Decode events using metadata for the block. Follow `DefaultTopology`
state as well as `DefaultProvenTractable`, `DefaultFailedOver` and
`DefaultRetired`, so missed events/reconnects cannot pin stale work. The runtime
starts the grace window in `on_initialize`; proofs for an Exact default stop
being eligible at `DefaultRetiresAt` (`n < retires_at` is the grace condition).
The configured interval is one day in chain blocks, not a wall-clock promise.
A whitelisted Heuristic standby takes over; otherwise mining pauses until a
suitable standby is available.

## Companion coordinator audit and release evidence

Read-only audit on 2026-10-25 of canonical `quip.network/quip-solver` main
commit `4674a34b6c7155aadc60ac3eb7b69db93241c1af` confirmed a pre-existing
!71 rollout dependency. The older `quip-miner` repository URL redirects there;
the local checkout at `bbb3771f5ee94c771db50f47f05c6e0c27456209` is not the
current companion baseline. The following findings describe the newer commit:

- [preflight.rs](https://gitlab.com/quip.network/quip-solver/-/blob/4674a34b6c7155aadc60ac3eb7b69db93241c1af/crates/quip-coordinator/src/chain/preflight.rs)
  accepts every API version >=2, including v3.
- [scale_types.rs](https://gitlab.com/quip.network/quip-solver/-/blob/4674a34b6c7155aadc60ac3eb7b69db93241c1af/crates/quip-coordinator/src/chain/scale_types.rs)
  still has the three-field v2 difficulty type and nested
  `QuantumProof.solutions`.
- [real.rs](https://gitlab.com/quip.network/quip-solver/-/blob/4674a34b6c7155aadc60ac3eb7b69db93241c1af/crates/quip-coordinator/src/chain/real.rs)
  assembles snapshots from pinned storage/API reads, including
  `state_getRuntimeVersion(at)`, but extracts the spec version and decodes
  `difficulty_for` with fixed v2 `DifficultyConfig` through plain `Decode`. It
  does not select that decoder from the reported API version. No
  `instance_check` caller was found in the audited Rust sources.
- [validate.rs](https://gitlab.com/quip.network/quip-solver/-/blob/4674a34b6c7155aadc60ac3eb7b69db93241c1af/crates/quip-coordinator/src/validate.rs)
  still applies strict energy, solution-count and diversity gates.

That revision is not evidence of API3/tx8 mining compatibility. This validator
branch does not modify the companion repository. Record the compatible
coordinator/miner revision and results before release; no blanket compatibility
claim is made for indexers or other clients that have not been audited. This
pre-existing rollout dependency does not prevent a reviewable validator draft;
validator refactor completion and end-to-end rollout readiness are distinct.

Required evidence remains:

1. Supported-version selection at a fixed historical v2 block and at v3, plus
   refusal of unsupported versions and malformed/trailing response bytes.
2. Runtime-upgrade/reconnect cache refresh, canonical graph generation, and
   cancellation of stale work with the actual companion coordinator.
3. A local disposable `--dev` run using API3/tx8: precheck the draw, submit one
   packed solution, observe inclusion and finalization, and reconcile reward,
   winner fields and counters. Use a fixture that satisfies the production
   eligibility rules; synthetic unit-test width claims are not evidence of
   graph hardness.
4. Equality at the instance threshold; ordered draw/topology failures;
   same-block competition and incumbent ties; default retirement/failover;
   historical/current indexer decoding with complete consumption.
5. Companion release identity, validator release identity, fresh live-version
   check, whole-runtime migration rehearsal, signing fixture and release-build
   results recorded in the [release checklist](release.md).

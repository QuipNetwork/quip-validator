# Quantum PoW difficulty controller decisions

This records the controller decisions, including the maintainer's 2026-09-25
approval of the runtime 119 correction. Sections 1, 2 and 4 describe historical
release decisions; section 5 records the later !71 behavior and superseded
release assumption. They are not instructions to merge main into a stacked
clarity branch. Sections 3, 6 and 7 describe the current arithmetic and evidence.

## 1. The !87 controller ships

Runtime 118 introduced `adjust_on_proof` and `apply_decay` from !87.
`adjust_on_proof` computes the next bar after each win.
`apply_decay` is the continuous decay.
The integration decision was for !71 to build on !87, dropping its earlier
stepwise `apply_decay` and retaining `adjust_on_proof`. The single-dial
configuration work could stay on that basis.

!87 argues from measured aglais rounds.
It ships a bounded controller.
A differential test compares the controller with the retired rule.
Two controllers cannot coexist.
The conflicts identified then included `difficulty.rs`, `tests.rs`,
`runtime/src/lib.rs` and the signing fixture; this is not a current conflict list.

## 2. The chain shipped specification version 117

At that decision point, Aglais ran 117 from the v0.3.x tags.
A change must bump `spec_version` when a 117 node would compute a new threshold from the same state.
A change must bump `spec_version` when a 117 node would use a new storage layout on that state.
A change must bump `spec_version` when a 117 node would use a new call encoding on that state.
!87 ships as 118.
!71 must not fold consensus changes into 117.

When `spec_version` changes, regenerate the signing fixture:

    cargo run -p quip-protocol-runtime --example generate_polkadotjs_signing_fixture -- --write

## 3. The team accepts transcendental float math in consensus, with guards

`apply_decay` calls `libm::pow` and `libm::log`.
It rounds the result to an integer.
That integer decides if the chain admits the proof.
This extends the existing precedent.
`adjust_on_proof` calls `libm::round`.
`expected_gse` calls `libm::sqrt`.
`register_topology` runs that call on chain.

The implementation pins dependencies, bounds inputs and tests integer outputs.
These controls provide evidence for the tested paths, not a proof over every
architecture or possible input.

- `pallet-quantum-pow` and `quantum-validation` pin `libm` at `=0.2.16`.
  Pinning the library alone does not prove bit-for-bit native/Wasm agreement.
- The code bounds the inputs.
  Each `u32` operand converts to `f64` exactly; large `i64` energy values may not.
  The cast `as i64` saturates.
  Decay cannot exceed the easy curve bound.
  Hardening cannot exceed the winning energy gap or the per-win cap.
  Each run recomputes from the stored base and the total elapsed blocks.
  Rounding never compounds across blocks.
- `pallets/quantum-pow/src/tests.rs` holds the golden table `apply_decay_and_adjust_on_proof_golden_table`.
  The table pins integer outputs across a swept grid.
  It detects changes at those native test points.
- `runtime/tests/quantum_pow_wasm.rs` executes the difficulty API and seeded
  finalization through both native dispatch and the embedded Wasm runtime.
  Its fixtures compare encoded API results and finalized headers/state exactly,
  with independent integer expectations. They do not cover full signed-proof
  admission, winner selection or every runtime API. Build with the repository
  lockfile, including `WASM_BUILD_WORKSPACE_HINT` for external target directories,
  as documented in that test; a comparison against differently resolved Wasm
  dependencies is not reproducible evidence for the pinned build.

Continuous easing uses a geometric phase followed by a linear floor and rounds
the total easing once. It is not bit-for-bit equivalent to a discrete recurrence
that rounds every epoch. Preserve float operation order, libm calls, casts and
clamps; the integer result is consensus-visible.

## 4. Runtime 119 gates easing and follows winning energy

Runtime 118 eased the live threshold from the first block of each round.
Its post-win rule could then undo that easing in one step.
That behavior did not match the intended 100-block gate.

The threshold now stays fixed through 100 elapsed chain blocks after a win.
At block 101, it receives one block of gradual easing.
No easing from the first 100 blocks carries forward.
The gate uses chain blocks, not wall-clock time.

The overdue rate remains 4.9375% of the gap to the easy bound per epoch.
This is the existing combined rate: `1 - 0.975 * 0.975`.
An epoch is 100 blocks in production.
Near the easy bound, easing uses the one-energy-unit floor per epoch.
It never crosses that bound.

For a positive winning gap, a win hardens from the live threshold toward the
proof's achieved energy. In the multi-configuration rule described here, that
was the lowest validated solution energy in the winning proof; !71 uses the
single configuration's translated depth described in section 5.
The existing elapsed-block rate bands select a fraction of that gap.
The step is at least one unit unless the winning energy gap is smaller.
It cannot exceed 2.5% of the calibrated curve span, or one unit for a smaller span.
This cap applies even below the estimated hard end of the curve.
No later rule overrides it.

A late win preserves the easing needed to clear the threshold.
Its hardening starts from that live threshold, not the previous stored baseline.
The result becomes the next baseline and restarts the 100-block gate.
A win with zero gap leaves the threshold unchanged.
Winner identity cannot make a win ease the threshold.

That correction was assigned `spec_version = 119` while 118 was live, with no
storage-layout, call-encoding or transaction-version change. Its signing fixture
had to match runtime 119. This historical assignment does not reserve 119 for
later !71 changes.

## 5. !71 keeps the !87 controller; its original release assumption is superseded

The earlier plan to fold !71 into an unshipped 119 is superseded by recorded
deployment evidence. At AGLS block 153726
(`0xb06d9fcbf2dcd1db33540f22b479728c462b8d1e64d61459cad6fca36b158712`),
the runtime was spec 119 / transaction version 7, QuantumPowApi v2 and pallet
storage v6. See the pinned [migration fixture](../runtime/tests/fixtures/agls-migration-153726.json)
and [history fixture provenance](../runtime/tests/fixtures/README.md).
This observation is tied to that block, not a claim about the latest chain.
Shipping !71 needs a newer unused spec version, separately verified against
deployment, plus its intended transaction/API/storage changes and matching
signing fixture and miner compatibility. These version numbers describe
different contracts; API v3 is not a blanket name for the storage migration.

!71 keeps `adjust_on_proof` and the continuous `apply_decay` as its controller
functions. Its epoch retarget, stepwise decay and the removal of
`adjust_on_proof` are dropped.

What !71 adds on top:

- `DifficultyConfig` keeps only `max_energy_milli`. A proof carries one
  configuration.
- The live threshold is the curve bar. Each draw's bar is
  `max(curve_bar, lower_bound)`, nudged by an integer-σ frustration handicap
  when sufficient headroom remains. The bound is not necessarily attainable.
- A win hardens the live threshold toward
  `active - (instance_bar - energy)`. That is the proof's depth below its own
  bar, so a looser handicapped bar cannot dodge hardening.
- QuantumPow storage moves 6 → 7. `transaction_version` moves 7 → 8.
  `QuantumPowApi` moves to version 3.

## 6. Decay anchor and starting difficulty

Decay counts from the later of the last win and the last root
`set_difficulty`. A governance bar therefore gets the full 100-block gate
instead of being eased the moment it lands after a long round.
`LastProofBlock` stays the nonce seed and round clock. The anchor stays global;
per-topology anchors wait for concurrent multi-topology mining (qv-s30.4).

A topology with no stored baseline still starts at its curve's knee.

## 7. Bounds, frustration and ranking in !71

For binary spins, `L = -(Σ|h| + Σ|J|)` is a lower energy bound when the sum is
representable. Equality requires all terms to reach their individual minima
together. Zero fields and unfrustrated couplings permit this; frustration or
competing field/coupling preferences can make the bound unattainable. The helper
accumulates unsigned magnitudes with saturation, then pricing clamps the result
to `i64::MAX` before negating, yielding `-i64::MAX` rather than `i64::MIN` for
oversized values. Registration instead uses a checked conversion and rejects
overflow. Neither mean-field curve calibration nor bound clamping proves that
a threshold can be reached.

A gauge transformation of coupling signs relabels binary spins without changing
energy when fields are zero. With fields, their signs must transform too. The
full cycle-parity pattern carries more information than the fraction of
frustrated basis cycles: equal fractions need not imply equivalent instances
or equal solve effort. Independent fair nonzero coupling signs give independent
fair parities for a fixed fundamental cycle basis. The configured expectation
500 is then an expected fraction, not every draw's realized fraction. Arbitrary
coupling distributions need not satisfy this model.

For expected frustration P and realized frustration F in milli, and cycle count
K, `frustration_sigmas` returns zero if P or K is zero. Otherwise it computes
`p = min(P,1000)`, `v = p*(1000-p)*1_000_000/K`, and
`s = max(isqrt(v),1)`, then `(P-F)*1000/s`, truncating signed division toward
zero. Only the variance caps P; P=1000 is not an early-zero case. Admission uses
the raw integer score and rejects magnitudes above 3, while standalone pricing
clamps to [-3,3]. Thus integer buckets ±3 also contain pre-truncation magnitudes
between 3 and 4. Returning zero or a priced threshold does not bypass the other
admission gates.

Pricing clamps the configured slope to [0,1000]. It withholds the whole handicap
when headroom above L is strictly less than the maximum swing; equality permits
the adjustment. For negative thresholds, positive sigma tightens and negative
sigma loosens. Signed division truncates toward zero, reducing either movement's
magnitude by its fractional milli. Zero slope disables this adjustment. These
are absolute energy thresholds, not ratios of realized to expected weight.
The relation between lower frustration and easier solving is a model assumption;
the band and handicap do not establish equal work or a universal bound on
salt-selection profitability.

The earlier inline calibration note attributed results to riff-morph's
`handicap_calibration` on chimera C2/C4, grid 8×8 and mobius M24: correlations
below 0.3, inconsistent signs and implied slopes spanning 4..29 per-mille per
sigma. That report motivated the default slope of 6; this documentation change
does not reproduce the experiment or establish an optimal slope. The runtime's
configured value remains separate from the default constant.

Two existing arithmetic examples explain why the bound and sigma units matter.
The [pure-pricing fixtures](../pallets/quantum-pow/src/tests.rs) supply a curve
threshold of -65,620 milli and a bound magnitude of 64,000 milli; pricing returns
-64,000. This demonstrates the clamp, not that a frustrated graph can attain
that threshold or that the full admission path accepts it. For P=500 and
K=41,065 (the Z(12,4) cycle count cited by the existing fixture), the model's
standard deviation is about 2.467 milli and the integer micro denominator is
2,467. F=498 therefore gives raw score zero after signed truncation, not one
sigma. These examples preserve the useful numerical context without treating
the bound as an optimum or a scalar frustration score as a work estimate.

Ranking uses `room = B-L` and `depth = B-E`. Nonpositive room scores zero;
otherwise quality is `min(depth*10^15/room, 10^15)`, with signed truncation toward
zero and no lower clamp for arbitrary pure-helper inputs. Admitted proofs have
nonnegative depth. Zero quality loses to positive quality but can tie another
zero. Equal quality prefers lower numerical energy; equal quality and energy
retain the incumbent. With room 2,500,000 milli, one milli of improvement adds
400,000,000 quality units. The old 2,500-milli bucket example described a
hypothetical per-mille scale, not the implemented scale.

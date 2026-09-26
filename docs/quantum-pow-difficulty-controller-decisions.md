# Quantum PoW difficulty controller decisions

The maintainer approved the runtime 119 correction on 2026-09-25.
Section 4 replaces the timing and hardening rules from runtime 118.
Sections 1 and 2 record the earlier release decisions for !87 and !71.

## 1. The !87 controller ships

Runtime 118 introduced `adjust_on_proof` and `apply_decay` from !87.
`adjust_on_proof` changes the bar after each win.
`apply_decay` is the continuous decay.
After !87 merges, !71 must rebase onto main.
!71 must drop its controller rewrite.
That rewrite has a stepwise `apply_decay`.
That rewrite also removes `adjust_on_proof`.
The single-dial configuration work in !71 can stay if it builds on the !87 functions.

!87 argues from measured aglais rounds.
It ships a bounded controller.
A differential test compares the controller with the retired rule.
Two controllers cannot coexist.
The !71 branch conflicts with !87 in `difficulty.rs` and `tests.rs`.
The branch also conflicts in `runtime/src/lib.rs` and the signing fixture.

## 2. The chain shipped specification version 117

Aglais runs 117 from the v0.3.x tags.
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

The team accepts this on three conditions.
All three are in place.

- `pallet-quantum-pow` and `quantum-validation` pin `libm` at `=0.2.16`.
  `libm` is pure Rust and has no hardware intrinsics.
  Native builds and wasm builds match bit for bit.
- The code bounds the inputs.
  Each `u32` operand converts to `f64` exactly.
  The cast `as i64` saturates.
  Decay cannot exceed the easy curve bound.
  Hardening cannot exceed the winning energy gap or the per-win cap.
  Each run recomputes from the stored base and the total elapsed blocks.
  Rounding never compounds across blocks.
- `pallets/quantum-pow/src/tests.rs` holds the golden table `apply_decay_and_adjust_on_proof_golden_table`.
  The table pins integer outputs across a swept grid.
  Any change in rounding turns that test red.

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

Each win hardens from the live threshold toward the proof's achieved energy.
The achieved energy is the lowest validated solution energy in the winning proof.
The existing elapsed-block rate bands select a fraction of that gap.
The step is at least one unit unless the winning energy gap is smaller.
It cannot exceed 2.5% of the calibrated curve span, or one unit for a smaller span.
This cap applies even below the estimated hard end of the curve.
No later rule overrides it.

A late win preserves the easing needed to clear the threshold.
Its hardening starts from that live threshold, not the previous stored baseline.
The result becomes the next baseline and restarts the 100-block gate.
A win within the gate always makes the next target harder.
Winner identity cannot make a win ease the threshold.

This consensus change requires `spec_version = 119`, since 118 is live.
The storage layout, call encoding, and transaction version remain unchanged.
The signing fixture must match runtime 119.

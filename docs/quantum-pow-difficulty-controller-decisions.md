# Quantum PoW difficulty controller decisions

The maintainer recorded these decisions on 2026-09-22.
They answer the review of merge request !87.
These decisions bind !87 and !71.

## 1. The !87 controller ships

The chain keeps `adjust_on_proof` and `apply_decay` from !87.
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
`adjust_energy_along_curve` calls `libm::round`.
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
  The code clamps the result to the curve.
  Each run recomputes from the stored base and the total elapsed blocks.
  Rounding never compounds across blocks.
- `pallets/quantum-pow/src/tests.rs` holds the golden table `apply_decay_and_adjust_on_proof_golden_table`.
  The table pins integer outputs across a swept grid.
  Any change in rounding turns that test red.

## 4. Overdue easing replaces dominant-winner easing

The retired rule eased a slow round once one account had won three qblocks in a row.
On aglais one account has won every qblock since about qblock 5,000.
The rule eased the threshold for that account, which is the account the rule targets.
The "waited too long" signal stays, in continuous form.
Past `TARGET_PROOF_BLOCKS`, the live threshold eases at the baseline rate plus `OVERDUE_EASE_RATE_MILLI`.
The ease is the same for every account that ends the round.
Every win hardens the bar.
A win at or under target leaves the stored bar at least one energy unit harder than the round began with.
Past target that floor releases by the overdue easing the round accrued, from zero at the target block.
The stored bar is continuous in the round length.
A miner who waits one more block gains at most one more block of overdue easing.
The baseline decay a round consumed is not applied to the stored bar in one step.

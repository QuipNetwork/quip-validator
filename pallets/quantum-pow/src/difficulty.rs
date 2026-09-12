use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use quantum_validation::{AllowedValueSpec, MilliValue, ValidationError};
use scale_info::TypeInfo;

use crate::types::DifficultyConfig;

// Difficulty policy is block-based.
//
// The earlier timestamp-based approach created a unit mismatch in the pallet,
// because proof decay was evaluated during block execution while the stored
// "last proof" marker was expressed in wall-clock units. We now use elapsed
// blocks everywhere in the difficulty path so:
// - decay aligns directly with `EpochLength`
// - proof adjustment and decay reason over the same time unit
// - validation does not depend on timestamp availability or conversion
//
// These constants are the block-native thresholds the current policy uses,
// measuring elapsed chain blocks between consecutive qblocks (PoW-won
// blocks — formerly referred to as "solution #"/"problem #"). They
// correspond to the earlier 6-second-block translation:
// 360s -> 60 blocks, 600s -> 100 blocks, 1200s -> 200 blocks.
//
// `TARGET_PROOF_BLOCKS` is deliberately co-located with the runtime's
// `QuantumPowEpochLength` (= 100, the decay interval): the first decay
// step, the hardening band's gentle plateau, and the easing rate ramp all
// begin at the same 100-block boundary. A win round is therefore either
// "sub-epoch" (adjusts from the stored difficulty) or "decayed" (adjusts
// gently from the decay-eased base) — never a mix. The two remain separate
// constants on purpose: this one anchors the rate bands, the runtime one
// sets decay cadence. Retune them together.
const FAST_PROOF_BLOCKS: u64 = 60;
const TARGET_PROOF_BLOCKS: u64 = 100;
const SLOW_PROOF_BLOCKS: u64 = 200;

/// Floor on the per-step energy delta — one energy unit (1.0 unit -> 1000
/// milli). Under the geometric model the step shrinks toward zero as the
/// threshold nears a curve bound, so this floor sets the granularity of the
/// tail: the rate at which hardening walks *past* `min_milli` to track a
/// stronger-than-estimated field, and the final settle onto `max_milli` when
/// easing. Hardening and easing share the one floor (the walk-up plan's §3 —
/// it replaces the legacy asymmetric `5.0`/`3.0`-unit floors that let
/// hardening out-pace easing 5:3 at the tail).
const MIN_ENERGY_DELTA_MILLI: i64 = 1000;

/// Decay rate per epoch step: 25 per-mille = 2.5%, half of the typical
/// hardening floor (50 per-mille). Mirrors v0.1 `energy_ease_rate = 0.025`.
const DECAY_RATE_MILLI: u32 = 25;

/// Direction the energy threshold moves under an adjustment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// Make mining harder — push `max_energy_milli` toward `min_milli`
    /// (i.e. more negative).
    Harder,
    /// Make mining easier — push `max_energy_milli` toward `max_milli`
    /// (i.e. less negative).
    Easier,
}

/// The three per-mille empirical `c` values that calibrate an
/// [`EnergyCurve`].
///
/// SCALE-encoded `u32` per-mille because pallet constants must implement
/// `Get<_>` and `f64` does not implement `Encode`. They are divided by 1000
/// before being fed to `expected_gse`. The SCALE/`TypeInfo` derives also let
/// a `CurveC` be stored as a per-topology override and passed to the
/// `set_topology_curve` extrinsic.
#[derive(
    Clone,
    Copy,
    Debug,
    Decode,
    DecodeWithMemTracking,
    Encode,
    Eq,
    MaxEncodedLen,
    PartialEq,
    TypeInfo,
)]
pub struct CurveC {
    /// Easiest (least-negative) end of the curve.
    pub easy_milli: u32,
    /// Knee, where adjustment motion peaks.
    pub knee_milli: u32,
    /// Hardest (most-negative) end of the curve.
    pub hard_milli: u32,
}

/// Topology-derived bounds for the difficulty energy curve.
///
/// The curve is calibrated against a single topology's `(num_nodes,
/// num_edges)` and its allowed h/J value specs, evaluated at three empirical
/// `c` values:
///
/// - `min_milli` = `expected_gse(.., c_hard, ..)` — hardest, most
///   negative.
/// - `knee_milli` = `expected_gse(.., c_knee, ..)` — the mid-curve
///   calibration point between the hard and easy ends.
/// - `max_milli` = `expected_gse(.., c_easy, ..)` — easiest, least
///   negative.
///
/// These are mean-field GSE *estimates*, not hard limits: the actual ground
/// state of a given instance can be more negative than `min_milli`. The
/// difficulty threshold therefore tracks *past* `min_milli` when hardening
/// (to follow a stronger-than-estimated field) but never eases *past*
/// `max_milli` (difficulty stays at or above the easiest calibrated puzzle).
/// See [`adjust_energy_along_curve`].
///
/// All three values are in milli precision. `min_milli < knee_milli <
/// max_milli` (all negative).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnergyCurve {
    pub min_milli: i64,
    pub knee_milli: i64,
    pub max_milli: i64,
}

impl EnergyCurve {
    /// Build a curve from topology size, calibration `c` values, and the
    /// topology's allowed h/J value specs.
    ///
    /// Deriving the field/coupling magnitudes from the specs keeps the curve
    /// aligned with what puzzles actually sample — a zero-field topology
    /// (`allowed_h = Set([0])`) gets a curve with no h contribution instead
    /// of one that credits energy no puzzle can produce.
    ///
    /// Errors when either spec is empty or has inverted bounds (impossible
    /// for topologies that passed `register_topology` validation).
    pub fn new(
        num_nodes: u32,
        num_edges: u32,
        c: CurveC,
        allowed_h: &AllowedValueSpec<&[MilliValue]>,
        allowed_j: &AllowedValueSpec<&[MilliValue]>,
    ) -> Result<Self, ValidationError> {
        let gse = |c_milli: u32| {
            quantum_validation::expected_gse(
                num_nodes,
                num_edges,
                f64::from(c_milli) / 1000.0,
                allowed_h,
                allowed_j,
            )
        };
        Ok(Self {
            min_milli: gse(c.hard_milli)?,
            knee_milli: gse(c.knee_milli)?,
            max_milli: gse(c.easy_milli)?,
        })
    }
}

// Rate bands mirror v0.1 `calculate_adjustment_rate_with_randomness`:
//
// Hardening: <360s (60 blocks) -> 35% ± 30%; >600s (100 blocks) -> 5% ± 4%;
// linear interpolation in between. The graduated band matters because slow
// qblocks won by a non-dominant winner harden too — they need the gentle
// 5% rates, not the fast-qblock 35% ones.
//
// Easing: <600s (100 blocks) -> 2.5% ± 2%; >1200s (200 blocks) -> 15% ± 14%;
// linear interpolation in between.
fn sample_adjustment_milli(mining_time_blocks: u64, harder: bool, seed: &[u8]) -> u32 {
    let (base, variance) = if harder {
        if mining_time_blocks < FAST_PROOF_BLOCKS {
            (350_u32, 300_u32)
        } else if mining_time_blocks > TARGET_PROOF_BLOCKS {
            (50_u32, 40_u32)
        } else {
            let progress = ((mining_time_blocks - FAST_PROOF_BLOCKS) * 1000
                / (TARGET_PROOF_BLOCKS - FAST_PROOF_BLOCKS)) as u32;
            (
                350 - ((350 - 50) * progress / 1000),
                300 - ((300 - 40) * progress / 1000),
            )
        }
    } else if mining_time_blocks > SLOW_PROOF_BLOCKS {
        (150_u32, 140_u32)
    } else if mining_time_blocks < TARGET_PROOF_BLOCKS {
        (25_u32, 20_u32)
    } else {
        let progress = ((mining_time_blocks - TARGET_PROOF_BLOCKS) * 1000
            / (SLOW_PROOF_BLOCKS - TARGET_PROOF_BLOCKS)) as u32;
        (
            25 + ((150 - 25) * progress / 1000),
            20 + ((140 - 20) * progress / 1000),
        )
    };

    let min_rate = base.saturating_sub(variance).max(1);
    let max_rate = base.saturating_add(variance);
    let digest = blake3::hash(&(seed, mining_time_blocks, harder).encode());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    let sample = u64::from_be_bytes(bytes);
    let span = u64::from(max_rate.saturating_sub(min_rate));

    min_rate + (sample % (span + 1)) as u32
}

/// Move `current_milli` toward the curve bound implied by `direction` by a
/// geometric fraction of the distance *remaining* to that bound.
///
/// The step is a geometric fraction `room × rate` of the distance to the
/// curve bound the adjustment moves toward, floored at `min_delta_milli`.
/// Because `rate < 1`, the geometric term is always smaller than `room`, so
/// from anywhere short of the bound the threshold *walks* toward it a fraction
/// of the remaining gap at a time — a single fast win can no longer slam the
/// threshold across the whole range and strand the chain (the slam-and-stall
/// this replaces; see `quantum-pow-difficulty-hardening-walkup-plan.md`).
///
/// The two directions are deliberately asymmetric, because the curve bounds
/// are GSE *estimates*, not hard limits:
///
/// - [`Direction::Harder`] references `min_milli` but is **uncapped**: a
///   stronger-than-calibrated field finds winning energies below the hard
///   estimate, so the threshold must keep tracking past `min_milli`. Once at
///   or past it (`room ≤ 0`) the geometric term vanishes and the floor carries
///   the threshold one further energy step down.
/// - [`Direction::Easier`] references `max_milli` and is **capped** there:
///   difficulty should never ease below the easiest calibrated puzzle, so the
///   step is clamped to the remaining gap and is a no-op once at/past the easy
///   cap. Recovery from a too-hard threshold is fastest at the hard end (where
///   the gap to `max_milli` is largest) and settles gently onto the easy cap.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn adjust_energy_along_curve(
    current_milli: i64,
    rate_milli: u32,
    direction: Direction,
    curve: EnergyCurve,
    min_delta_milli: i64,
) -> i64 {
    // Defensive: a degenerate curve (e.g. zero-node topology) collapses to a
    // single point — there is no bound to reference. Leave `current` alone.
    if curve.max_milli <= curve.min_milli {
        return current_milli;
    }
    let rate = f64::from(rate_milli) / 1000.0;
    // Geometric step toward the target bound, floored so progress never stalls.
    // The `as i64` cast saturates (never UB/panic), and every `room`/result
    // subtraction below saturates too, so the genesis `i64::MAX` sentinel and
    // extreme curves stay overflow-safe.
    let geometric_floored =
        |room: i64| (libm::round(room as f64 * rate) as i64).max(min_delta_milli);

    match direction {
        Direction::Harder => {
            // Uncapped: `room` may be ≤ 0 once the threshold has walked below
            // the hard estimate, where the floor alone advances it one step.
            let room = current_milli.saturating_sub(curve.min_milli);
            current_milli.saturating_sub(geometric_floored(room))
        }
        Direction::Easier => {
            // Capped at the easy cap: never ease past `max_milli`.
            let room = curve.max_milli.saturating_sub(current_milli);
            if room <= 0 {
                return current_milli;
            }
            current_milli.saturating_add(geometric_floored(room).min(room))
        }
    }
}

/// Apply continuous decay easing for `elapsed_blocks` blocks, easing only the
/// energy threshold via the curve. Diversity and solutions are chain-static
/// and never touched here.
///
/// The threshold retains `(1 - rate)^(elapsed / epoch_length)` of its distance
/// to `max_milli`, so at every whole epoch the result equals the retired
/// stepwise rule and the average easing rate is unchanged. Between epochs the
/// threshold eases every block instead of waiting for the boundary. Measured
/// on aglais under the stepwise rule, 44% of rounds ended within ten blocks
/// of a boundary: no miner could clear until the 22,000 milli step landed,
/// then one did at once. A per-block ramp has no such cliff.
///
/// Closed form, so a long stalled round costs the same as a short one. The
/// `MIN_ENERGY_DELTA_MILLI` floor is pro-rated per block.
pub fn apply_decay(
    current: DifficultyConfig,
    elapsed_blocks: u32,
    epoch_length: u32,
    curve: EnergyCurve,
) -> DifficultyConfig {
    let mut difficulty = current;
    difficulty.max_energy_milli = ease_continuous(
        current.max_energy_milli,
        elapsed_blocks,
        epoch_length,
        curve,
    );
    difficulty
}

fn ease_continuous(
    current_milli: i64,
    elapsed_blocks: u32,
    epoch_length: u32,
    curve: EnergyCurve,
) -> i64 {
    if curve.max_milli <= curve.min_milli || elapsed_blocks == 0 || epoch_length == 0 {
        return current_milli;
    }
    let room = curve.max_milli.saturating_sub(current_milli);
    if room <= 0 {
        return current_milli;
    }
    let retained_per_epoch = 1.0 - f64::from(DECAY_RATE_MILLI) / 1000.0;
    let epochs = f64::from(elapsed_blocks) / f64::from(epoch_length);
    let geometric = libm::round(room as f64 * (1.0 - libm::pow(retained_per_epoch, epochs))) as i64;
    let floor =
        MIN_ENERGY_DELTA_MILLI.saturating_mul(i64::from(elapsed_blocks)) / i64::from(epoch_length);
    current_milli.saturating_add(geometric.max(floor).min(room))
}

/// The most a single win may harden the threshold: one decay step measured
/// at the hard estimate, `DECAY_RATE_MILLI` of the full curve span, floored
/// at `MIN_ENERGY_DELTA_MILLI`. Constant for a curve.
///
/// Uncapped, a fast round hardened 35% of the room to the hard estimate. At
/// the aglais operating point that room is 80,000 milli against 895,000 to
/// the easy cap, so a fast round (26,000 milli median, up to 41,000) undid
/// more than one 22,000 milli decay epoch, and every fast round pushed the
/// next one out to the second epoch. The cap is 24,368 milli on that curve.
///
/// The span, not the current threshold, defines the cap: decay from
/// `max_milli` is zero, and a cap read from there would collapse to the
/// floor exactly where a fast win most needs to bite.
pub(crate) fn max_hardening_delta(curve: EnergyCurve) -> i64 {
    let span = curve.max_milli.saturating_sub(curve.min_milli).max(0);
    (libm::round(span as f64 * f64::from(DECAY_RATE_MILLI) / 1000.0) as i64)
        .max(MIN_ENERGY_DELTA_MILLI)
}

/// Adjust difficulty after a winning proof by a non-dominant winner.
/// Mutates only `max_energy_milli`; `min_solutions` and
/// `min_diversity_milli` are chain-static (only the `set_difficulty`
/// extrinsic — `ensure_root` — can change them).
pub fn adjust_on_proof(
    current: DifficultyConfig,
    mining_time_blocks: u64,
    curve: EnergyCurve,
    randomness_seed: &[u8],
) -> DifficultyConfig {
    adjust_on_proof_with_dominance(current, mining_time_blocks, curve, randomness_seed, false)
}

/// Adjust difficulty after a winning proof (a qblock), following the v0.1
/// `compute_next_block_requirements` policy:
///
/// - A fast qblock (under [`FAST_PROOF_BLOCKS`] elapsed chain blocks)
///   ALWAYS hardens — even for a dominant winner.
/// - A slow qblock eases only when the winner dominates (`dominant_winner`,
///   i.e. the same account won at least `ConsecutiveWinnerEasingThreshold`
///   consecutive qblocks); otherwise it hardens gently via the graduated
///   rate band.
/// - Hardening is capped at [`max_hardening_delta`], one decay step measured
///   at the hard estimate, so no single win pushes the next round more than
///   about an epoch out of reach.
///
/// v0.1 keyed dominance on the miner *type* repeating once (streak 2); we
/// key it on the account meeting the configured threshold instead — see
/// QUI-653 for the rationale.
pub fn adjust_on_proof_with_dominance(
    current: DifficultyConfig,
    mining_time_blocks: u64,
    curve: EnergyCurve,
    randomness_seed: &[u8],
    dominant_winner: bool,
) -> DifficultyConfig {
    let harder = mining_time_blocks < FAST_PROOF_BLOCKS || !dominant_winner;
    let rate_milli = sample_adjustment_milli(mining_time_blocks, harder, randomness_seed);
    let direction = if harder {
        Direction::Harder
    } else {
        Direction::Easier
    };
    let stepped = adjust_energy_along_curve(
        current.max_energy_milli,
        rate_milli,
        direction,
        curve,
        MIN_ENERGY_DELTA_MILLI,
    );
    let new_max_energy_milli = match direction {
        Direction::Harder => stepped.max(
            current
                .max_energy_milli
                .saturating_sub(max_hardening_delta(curve)),
        ),
        Direction::Easier => stepped,
    };
    DifficultyConfig {
        max_energy_milli: new_max_energy_milli,
        // Chain-static — never touched here.
        min_solutions: current.min_solutions,
        min_diversity_milli: current.min_diversity_milli,
    }
}

/// Compute the active difficulty for `block_number`, applying continuous
/// decay for every block since the previous winning proof.
///
/// This is the per-block view of difficulty that miners must clear and
/// that `adjust_on_proof` consumes as its baseline. All inputs are
/// explicit — the function reads no storage and is unit-testable in
/// isolation. The pallet wraps this with a method that does the four
/// storage reads (`Difficulty<T>`, `LastProofBlock<T>`, `EpochLength`
/// const, and `DefaultTopology<T>` → `RegisteredTopologies<T>` → curve).
///
/// A `None` curve disables decay — used at genesis (no topology
/// registered) or as a defensive fallback. `last_proof_block == 0` is
/// the genesis sentinel for "no winning proof yet".
pub fn current_difficulty(
    block_number: u32,
    base_difficulty: DifficultyConfig,
    last_proof_block: u32,
    epoch_length: u32,
    curve: Option<EnergyCurve>,
) -> DifficultyConfig {
    if last_proof_block == 0 || epoch_length == 0 {
        return base_difficulty;
    }
    let elapsed = block_number.saturating_sub(last_proof_block);
    if elapsed == 0 {
        return base_difficulty;
    }
    match curve {
        Some(curve) => apply_decay(base_difficulty, elapsed, epoch_length, curve),
        None => base_difficulty,
    }
}

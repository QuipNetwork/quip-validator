use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use quantum_validation::{AllowedValueSpec, MilliValue, ValidationError};
use scale_info::TypeInfo;

use crate::types::DifficultyConfig;

// Difficulty uses elapsed chain blocks, not timestamps. The target gates
// easing; EpochLength sets its rate unit. Production config uses 100 for both.
const FAST_PROOF_BLOCKS: u64 = 60;
pub(crate) const TARGET_PROOF_BLOCKS: u64 = 100;

/// One energy unit per adjustment, limited by the winning energy gap when
/// hardening. Easing uses this floor per epoch once geometric motion is smaller.
const MIN_ENERGY_DELTA_MILLI: i64 = 1000;

/// The hardening cap remains 2.5% of the calibrated curve span.
const DECAY_RATE_MILLI: u32 = 25;

/// Preserve runtime 118's overdue easing rate: the combined retained fraction
/// is 0.975 * 0.975 per epoch. No easing accrues before the 100-block gate.
const OVERDUE_EASE_RATE_MILLI: u32 = 25;

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
/// Hardening instead follows the achieved energy; see [`adjust_on_proof`].
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

// Hardening rate band, mirroring v0.1 `calculate_adjustment_rate_with_randomness`:
// <360s (60 blocks) -> 35% ± 30%; >600s (100 blocks) -> 5% ± 4%; linear
// interpolation in between. Every win hardens, so the graduated band is what
// keeps a slow qblock on the gentle 5% rate instead of the fast-qblock 35%
// one. Easing between wins comes only from decay (`apply_decay`).
fn sample_adjustment_milli(mining_time_blocks: u64, seed: &[u8]) -> u32 {
    let (base, variance) = if mining_time_blocks < FAST_PROOF_BLOCKS {
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
    };

    let min_rate = base.saturating_sub(variance).max(1);
    let max_rate = base.saturating_add(variance);
    let digest = blake3::hash(&(seed, mining_time_blocks).encode());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    let sample = u64::from_be_bytes(bytes);
    let span = u64::from(max_rate.saturating_sub(min_rate));

    min_rate + (sample % (span + 1)) as u32
}

/// Ease the threshold only for blocks strictly past `TARGET_PROOF_BLOCKS`.
/// The first 100 blocks contribute no decay, regardless of `epoch_length`.
/// At block 101, exactly one block of overdue easing accrues. Each read
/// recomputes from the stored baseline, so rounding does not compound.
/// Diversity and minimum solution count remain unchanged.
pub fn apply_decay(
    current: DifficultyConfig,
    elapsed_blocks: u32,
    epoch_length: u32,
    curve: EnergyCurve,
) -> DifficultyConfig {
    let overdue = elapsed_blocks.saturating_sub(TARGET_PROOF_BLOCKS as u32);
    if curve.max_milli <= curve.min_milli || overdue == 0 || epoch_length == 0 {
        return current;
    }
    let room = curve.max_milli.saturating_sub(current.max_energy_milli);
    let retained = (1.0 - f64::from(DECAY_RATE_MILLI) / 1000.0)
        * (1.0 - f64::from(OVERDUE_EASE_RATE_MILLI) / 1000.0);
    let eased = ease_room(room, overdue, epoch_length, 1.0 - retained);
    DifficultyConfig {
        max_energy_milli: current.max_energy_milli.saturating_add(eased),
        ..current
    }
}

/// Ease `room` milli over `blocks` at `rate` per epoch, in closed form,
/// reproducing the retired per-epoch loop. That loop stepped
/// `max(round(room * rate), MIN_ENERGY_DELTA_MILLI)` once per epoch,
/// clamped to the room: geometric while `room * rate` beat the floor,
/// linear at the floor after. The crossover room is `floor / rate`, and the
/// geometric phase lasts `ln(crossover / room) / ln(1 - rate)` epochs.
/// Returns the amount eased, at most `room`.
fn ease_room(room: i64, blocks: u32, epoch_length: u32, rate: f64) -> i64 {
    if room <= 0 || blocks == 0 || epoch_length == 0 || rate <= 0.0 || rate >= 1.0 {
        return 0;
    }
    let floor = MIN_ENERGY_DELTA_MILLI as f64;
    let room_f = room as f64;
    let epochs = f64::from(blocks) / f64::from(epoch_length);
    let crossover = floor / rate;
    let geometric_epochs = if room_f <= crossover {
        0.0
    } else {
        (libm::log(crossover / room_f) / libm::log(1.0 - rate)).min(epochs)
    };
    let room_after_geometric = room_f * libm::pow(1.0 - rate, geometric_epochs);
    let linear = (epochs - geometric_epochs) * floor;
    let eased = libm::round(room_f - room_after_geometric + linear) as i64;
    eased.min(room)
}

/// Maximum hardening per win: 2.5% of the calibrated curve span, with a
/// one-unit minimum. The winning energy gap can impose a smaller limit.
pub(crate) fn max_hardening_delta(curve: EnergyCurve) -> i64 {
    let span = curve.max_milli.saturating_sub(curve.min_milli).max(0);
    (libm::round(span as f64 * f64::from(DECAY_RATE_MILLI) / 1000.0) as i64)
        .max(MIN_ENERGY_DELTA_MILLI)
}

/// Harden from the live threshold toward the validated winning energy.
/// The elapsed-block rate band selects a fraction of that energy gap. The
/// one-unit floor cannot overshoot the winner, and the curve-span cap limits
/// every step, including wins below the curve's estimated hard end.
///
/// A late win retains the easing it needed: adjustment starts at `active`,
/// never at the old round baseline. The caller stores the result and resets
/// the 100-block gate. Only `max_energy_milli` changes.
pub fn adjust_on_proof(
    active: DifficultyConfig,
    winning_energy_milli: i64,
    mining_time_blocks: u64,
    curve: EnergyCurve,
    randomness_seed: &[u8],
) -> DifficultyConfig {
    let gap = active.max_energy_milli.saturating_sub(winning_energy_milli);
    if gap <= 0 {
        return active;
    }
    let rate_milli = sample_adjustment_milli(mining_time_blocks, randomness_seed);
    let geometric = libm::round(gap as f64 * f64::from(rate_milli) / 1000.0) as i64;
    let delta = geometric
        .max(MIN_ENERGY_DELTA_MILLI)
        .min(max_hardening_delta(curve))
        .min(gap);
    DifficultyConfig {
        max_energy_milli: active.max_energy_milli.saturating_sub(delta),
        ..active
    }
}

/// Compute the active difficulty for `block_number`, applying continuous
/// decay only for blocks past the target since the previous winning proof.
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

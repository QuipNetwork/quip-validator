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

/// Clamp an `i128` energy into `i64`, saturating rather than wrapping.
fn saturating_i64(value: i128) -> i64 {
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// Re-exported from `quantum-validation`, where the band predicate and its
/// bound live. Both are pure functions of a draw, so a miner can predict a
/// refusal without running a node.
pub use quantum_validation::{frustration_sigmas, HANDICAP_MAX_SIGMA};

/// Default handicap slope, in per-mille of the active threshold per integer σ.
/// The runtime supplies its configured value separately. This is a model
/// parameter: frustration is a proxy for solve effort, not a measured guarantee
/// of equal work across instances. See `docs/quantum-pow-difficulty-controller-decisions.md`
/// for the calibration provenance and limits of that interpretation.
pub const DEFAULT_HANDICAP_PER_SIGMA_PERMILLE: i64 = 6;

/// Ceiling for `Config::HandicapPerSigmaPermille`, per-mille per σ.
///
/// Shared by `integrity_test` and [`instance_bar_milli`]'s use-site clamp so
/// they cannot drift. The use-site clamp also bounds the signed multiplication.
pub const HANDICAP_SLOPE_MAX_PERMILLE: i64 = 1_000;

/// Normalized ranking resolution: one unit is 10^-15 of the available room.
/// With room 2,500,000 milli, one milli of depth adds 400,000,000 score units.
/// A hypothetical per-mille scale would instead put 2,500 milli in one bucket;
/// that is not the implemented scale. Quantized ties still compare energy,
/// and only equal quality AND equal energy retain the incumbent.
/// Differences of any two i64 energies, multiplied by this scale, fit in i128.
pub const QUALITY_SCALE: i128 = 1_000_000_000_000_000;

/// The three milli energies that place a proof against its own instance.
///
/// A struct for the same reason as [`FrustrationDraw`]: three adjacent `i64`s
/// transpose silently, and BOTH `room` and `depth` change sign under a swap —
/// in the function that decides who gets paid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProofDepth {
    /// The bar this instance had to clear.
    pub instance_bar_milli: i64,
    /// The instance's lower energy bound, `-(sum|h| + sum|J|)`.
    pub anchor_milli: i64,
    /// The energy the proof actually reached.
    pub energy_milli: i64,
}

/// Fraction of its own instance's room a proof actually found, in
/// [`QUALITY_SCALE`] units — `0` at the bar, full scale at the lower bound
/// when room is positive. A frustrated instance need not attain that bound.
///
/// Pure arithmetic over three milli energies, so the ranking is testable
/// without a `T: Config` and a MINER can compute it before paying the fee.
///
/// `room <= 0` scores zero, avoiding division by zero or an automatic full
/// score at the bound. It loses to a positive score, but can tie another zero.
/// For admitted proofs depth is nonnegative. This pure helper also accepts
/// other inputs: negative depth can produce a negative score; only the upper
/// score is capped. Division truncates toward zero.
pub fn proof_quality(d: ProofDepth) -> i128 {
    let room = i128::from(d.instance_bar_milli) - i128::from(d.anchor_milli);
    let depth = i128::from(d.instance_bar_milli) - i128::from(d.energy_milli);
    if room <= 0 {
        0
    } else {
        (depth * QUALITY_SCALE / room).min(QUALITY_SCALE)
    }
}

/// Convert a bound magnitude into a signed lower energy bound in milli.
/// Clamp the unsigned magnitude to i64::MAX BEFORE negating, yielding
/// -i64::MAX for oversized inputs, not i64::MIN. Registration instead uses a
/// checked conversion and rejects overflow: saturating its bound comparison
/// could admit a curve that cannot be represented safely.
pub fn anchor_milli(energy_bound_milli: u64) -> i64 {
    (energy_bound_milli.min(i64::MAX as u64) as i64).saturating_neg()
}

/// One draw's frustration, as the three numbers that always travel together.
///
/// A struct, not three positional arguments: `(realized, expected, cycles)` are
/// adjacent same-typed scalars, and transposing the two `u32`s COMPILES while
/// flipping the sign of the handicap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrustrationDraw {
    /// This instance's realized frustration index, in milli.
    pub realized_milli: u32,
    /// The topology's expected frustration, in milli.
    pub expected_milli: u32,
    /// Fundamental cycles, which set sigma.
    pub cycles: u64,
}

/// [`frustration_sigmas`] of a whole draw.
///
/// Instance validation computes this once and shares the raw score with
/// admission, pricing and API output. Admission rejects abs(score) > 3;
/// pricing clamps the score to [-3, 3].
/// Calling pricing alone therefore does not establish proof admissibility.
pub fn frustration_sigmas_of(draw: FrustrationDraw) -> i64 {
    frustration_sigmas(draw.realized_milli, draw.expected_milli, draw.cycles)
}

/// The energy bar this specific instance must clear, in milli.
///
/// Start at max(curve threshold, lower bound), then apply a bounded handicap
/// in integer sigma buckets. With a negative threshold, positive sigma
/// tightens and negative sigma loosens. The intended relation between lower
/// frustration and lower solve effort is a model assumption.
///
/// Withhold the whole adjustment when room above the lower bound is smaller
/// than the maximum swing; equality permits it. This keeps the adjustment
/// symmetric around the active threshold instead of allowing only loosening
/// at the bound. Clamping to a lower bound does not make the threshold
/// attainable: frustration or conflicting fields can keep the optimum above it.
pub fn instance_bar_milli(
    curve_bar_milli: i64,
    energy_bound_milli: u64,
    draw: FrustrationDraw,
    handicap_per_sigma_permille: i64,
) -> i64 {
    instance_bar_from_sigmas(
        curve_bar_milli,
        energy_bound_milli,
        frustration_sigmas_of(draw),
        handicap_per_sigma_permille,
    )
}

/// Price a draw using its already-computed raw score. This does not enforce
/// admission: callers validating a proof must reject out-of-band scores first.
pub(crate) fn instance_bar_from_sigmas(
    curve_bar_milli: i64,
    energy_bound_milli: u64,
    raw_sigmas: i64,
    handicap_per_sigma_permille: i64,
) -> i64 {
    let lower_bound_milli = anchor_milli(energy_bound_milli);

    let active_threshold_milli = curve_bar_milli.max(lower_bound_milli);

    // The threshold remains an absolute energy; it is not rescaled by the
    // draw's weight. A bound ratio is not an achievable-depth ratio.
    // Zero expectation/cycles produce score zero. Every other zero score also
    // leaves the threshold unchanged, whether headroom withholds or delta is zero.
    if raw_sigmas == 0 {
        return active_threshold_milli;
    }

    // Runtime integrity tests do not execute on chain. Clamp here to prevent
    // negative slopes reversing the adjustment and oversized slopes overflowing
    // the signed multiplication. Zero intentionally disables the handicap.
    let slope_permille = handicap_per_sigma_permille.clamp(0, HANDICAP_SLOPE_MAX_PERMILLE);

    let clamped_sigmas = raw_sigmas.clamp(-HANDICAP_MAX_SIGMA, HANDICAP_MAX_SIGMA);

    // Compare in i128 before applying either sign of adjustment. The bound
    // need not be attainable; this guard only limits the threshold's movement.
    let headroom_milli = i128::from(active_threshold_milli) - i128::from(lower_bound_milli);
    let max_swing_milli = i128::from(active_threshold_milli).abs()
        * i128::from(HANDICAP_MAX_SIGMA)
        * i128::from(slope_permille)
        / 1000;
    if headroom_milli < max_swing_milli {
        return active_threshold_milli;
    }

    // For a negative threshold, positive sigma gives a negative adjustment.
    // Signed division truncates toward zero: both tightening and loosening
    // lose any fractional milli. Pure callers may also supply zero/positive bars.
    let handicap_milli =
        i128::from(active_threshold_milli) * i128::from(clamped_sigmas * slope_permille) / 1000;
    saturating_i64(i128::from(active_threshold_milli) + handicap_milli)
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
    /// Intermediate calibration point; supplies a missing stored baseline.
    pub knee_milli: u32,
    /// Hardest (most-negative) end of the curve.
    pub hard_milli: u32,
}

/// Topology-derived calibration points for the difficulty energy curve.
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
/// All three values are in milli precision. Their intended order is
/// `min_milli < knee_milli < max_milli`; the pure constructor does not validate
/// coefficient order or require negative results.
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
// <60 blocks -> 35% ± 30%; >100 blocks -> 5% ± 4%; integer linear
// interpolation in between. For a positive winning gap, the graduated band
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
    }
}

/// Ease `room` milli in a continuous geometric phase followed by a linear
/// floor of 1000 milli per epoch. The crossover room is `floor / rate`, and
/// the geometric phase lasts `ln(crossover / room) / ln(1 - rate)` epochs.
/// Round the TOTAL easing once, then cap it at room. This is not bit-for-bit
/// equivalent to repeatedly rounding a discrete per-epoch recurrence.
/// Preserve the f64 operation order and libm calls: their integer result is
/// consensus-visible. Runtime integration tests compare native and Wasm outputs.
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
    }
}

/// The energy [`adjust_on_proof`] hardens toward for a per-instance win.
///
/// A proof clears its own [`instance_bar_milli`], not the live threshold, so
/// its raw energy is not on the curve's scale: a looser (hard-draw) bar would
/// let a win sit above the threshold and never harden it. Translating the
/// proof's depth below its own bar onto the live threshold keeps every win at
/// or below it, so `adjust_on_proof` sees the same gap semantics as before.
/// A proof exactly at its bar has zero depth and leaves the threshold alone.
pub fn winning_energy_on_curve(
    active_milli: i64,
    instance_bar_milli: i64,
    energy_milli: i64,
) -> i64 {
    let depth = i128::from(instance_bar_milli) - i128::from(energy_milli);
    saturating_i64(i128::from(active_milli) - depth.max(0))
}

/// Compute the active curve threshold, easing only after the 100-block gate.
/// Despite the parameter name, `last_proof_block` is the decay anchor: the
/// pallet supplies max(last win, last root difficulty update). This pure
/// helper reads no storage. The pallet also supplies the topology's stored
/// baseline (or curve knee when missing) and its configured curve.
///
/// A zero anchor, zero epoch length, missing curve or nonpositive elapsed
/// time disables decay. Instance pricing applies its own lower-bound clamp
/// and handicap to this active curve threshold.
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

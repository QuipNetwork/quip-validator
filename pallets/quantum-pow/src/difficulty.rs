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

/// Handicap per σ of frustration, in per-mille of the curve bar.
///
/// MEASURED, and the slope is **not determinable over the range that matters**:
/// riff-morph's `handicap_calibration` regresses work on frustration for
/// `d(bar)/dφ` over chimera C2/C4, grid 8×8 and mobius M24 and gets
/// correlations under 0.3 with inconsistent signs, the implied constant
/// scattering 4..29 per-mille per σ. Solver variance dominates the natural draw
/// distribution, so grinding only pays far into the tail — bounded by
/// [`HANDICAP_MAX_SIGMA`], not by this. Hence the low end of the scatter: the
/// handicap is symmetric, so too large a slope over-rewards grinding *upward*
/// exactly as much as it over-penalizes an unlucky honest draw.
///
/// A tunable DEFAULT, not the chain's value — it moves as calibration improves
/// and the mock sweeps it. Contrast `HANDICAP_MAX_SIGMA`, which gates
/// `submit_proof` validity and correctly stays a code constant.
pub const DEFAULT_HANDICAP_PER_SIGMA_PERMILLE: i64 = 6;

/// Ceiling for `Config::HandicapPerSigmaPermille`, per-mille per σ.
///
/// Shared by `integrity_test` and [`instance_bar_milli`]'s use-site clamp so
/// they cannot drift. Measured scatter is 4..29; far below the `i64` wrap.
pub const HANDICAP_SLOPE_MAX_PERMILLE: i64 = 1_000;

/// `1e15` puts a unit at 1e-15 of the room while `depth * SCALE` stays around
/// 1e21 for a 1e6-milli depth — four orders inside `i128`.
///
/// The scale is the resolution of the comparison, so it decides who is paid. At
/// per-mille and production energies (a bar near -3.6e6 milli against an anchor
/// near -6.1e6) one unit would span 2500 milli: proofs 2.4 energy units apart
/// tie, and the tie goes to whoever submitted first. Miners cluster within a
/// few units of the bar, so selection would become a latency race, not a work
/// race.
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
    /// The instance's exact optimum, `-(sum|h| + sum|J|)`.
    pub anchor_milli: i64,
    /// The energy the proof actually reached.
    pub energy_milli: i64,
}

/// Fraction of its own instance's room a proof actually found, in
/// [`QUALITY_SCALE`] units — `0` at the bar, full scale at that instance's
/// exact optimum.
///
/// Pure arithmetic over three milli energies, so the ranking is testable
/// without a `T: Config` and a MINER can compute it before paying the fee.
///
/// `room <= 0` scores LOWEST, not highest. `room == 0` means the bar was
/// clamped to the anchor, which happens on the draws with the smallest realized
/// `Σ|h| + Σ|J|` — the cheapest to grind for at `O(n + m)` per salt. Full marks
/// would make the least informative outcome the most rewarded one, and tie
/// every such proof with every other.
pub fn proof_quality(d: ProofDepth) -> i128 {
    let room = i128::from(d.instance_bar_milli) - i128::from(d.anchor_milli);
    let depth = i128::from(d.instance_bar_milli) - i128::from(d.energy_milli);
    if room <= 0 {
        0
    } else {
        (depth * QUALITY_SCALE / room).min(QUALITY_SCALE)
    }
}

/// The instance's exact optimum as a signed milli energy: `-bound`.
///
/// One definition, because there were three in two overflow spellings, two of
/// them on the SAME value in one `submit_proof` call. The anchor is the
/// load-bearing clamp in [`instance_bar_milli`], so its overflow behaviour must
/// not be decided per site.
///
/// Saturating. `ensure_curve_outlasts_the_typical_anchor` must fail closed
/// instead — `i64::MAX` there would make the anchor the most negative value
/// there is and pass the check vacuously — so it keeps its own `try_from`.
pub fn anchor_milli(energy_bound_milli: u64) -> i64 {
    (energy_bound_milli.min(i64::MAX as u64) as i64).saturating_neg()
}

/// One draw's frustration, as the three numbers that always travel together.
///
/// A struct, not three positional arguments: `(realized, expected, cycles)` are
/// adjacent same-typed scalars, and transposing the two `u32`s COMPILES while
/// flipping the sign of the handicap — turning the equalizer into an amplifier
/// that pays for salt-shopping.
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
/// Exists so `submit_proof`'s validity gate and [`instance_bar_milli`]'s
/// pricing are THE SAME z-score by construction: the gate refuses draws *past
/// the handicap's reach*, which only means anything if both measure the same
/// reach. Spelled differently, the gate kept the transposable triple at the one
/// site `FrustrationDraw` exists to protect.
///
/// The positional [`frustration_sigmas`] stays public: tests sweep it, and an
/// off-chain miner has three loose numbers, not a draw.
pub fn frustration_sigmas_of(draw: FrustrationDraw) -> i64 {
    frustration_sigmas(draw.realized_milli, draw.expected_milli, draw.cycles)
}

/// The energy bar this specific instance must clear, in milli.
///
/// The miner picks its salt and therefore its instance, so the bar compensates:
/// an easier-than-typical draw must reach further. This *equalizes expected
/// work across instances*; it does not punish anyone.
///
/// **Scaled in σ of the frustration distribution**, which concentrates because
/// frustration is a mean over `cycles` fundamental cycles: at Z(12,4)'s ~41,000
/// cycles, σ ≈ 2.5 milli. An earlier form interpolated to the `φ = 0` anchor
/// ~44M milli away — ~89,000 milli of bar per milli of frustration, so an
/// ordinary −1σ draw got a bar 19% tighter for reasons unrelated to solver
/// quality, punishing sampling noise and biasing the controller's input. Now
/// `HANDICAP_PER_SIGMA_PERMILLE` per σ, clamped at `±HANDICAP_MAX_SIGMA`.
///
/// **Symmetric**: a harder-than-expected draw earns a proportionally *looser*
/// bar. Tightening below while clamping above would leave grinding upward free
/// — two draws reach `φ ≥ expected`, after which the loosest bar is permanent.
/// Withheld entirely, never one-sidedly, where the anchor clamp leaves no room
/// to tighten. That clamp at `anchor = −(Σ|h| + Σ|J|)` is load-bearing on its
/// own: `expected_gse` is mean-field and on low-degree graphs lands *below* any
/// reachable energy — on a bare cycle it demands −65.6 against a floor of −64,
/// leaving the topology silently unmineable.
pub fn instance_bar_milli(
    curve_bar_milli: i64,
    energy_bound_milli: u64,
    draw: FrustrationDraw,
    handicap_per_sigma_permille: i64,
) -> i64 {
    let anchor = anchor_milli(energy_bound_milli);

    let typical = curve_bar_milli.max(anchor);

    // NOT scaled by this draw's weight, though the asymmetry is real: the bar
    // is an absolute energy while reachable depth grows with the draw's own
    // `Σ|h| + Σ|J|`, and finding a heavy one costs `O(n + m)`.
    //
    // Dividing by `expected_bound_milli` prices that exactly — and
    // over-demands, because the bound is not the achievable optimum and
    // frustration separates the two by a lot (a frustrated triangle reaches
    // about a third of its bound). Measured on the 3-node fixture, NO draw was
    // clearable at any calibrated bar with the ratio applied. The correct
    // denominator is achievable depth, which the σ handicap below already
    // prices — the two must be designed together, not stacked (`riff-hv9.41`).
    // Keep `expected_bound_milli` off `EnergyCurve`: only the registration
    // guard reads it, and every difficulty read would pay for it.
    if draw.expected_milli == 0 || draw.cycles == 0 {
        return typical;
    }

    // CLAMP THE SLOPE AT THE USE SITE. `integrity_test` asserts this range but
    // is `#[cfg(test)]`-only and never runs on a live chain, so a governance
    // `set_code` carrying a wasm built outside this repo is otherwise
    // unguarded.
    //
    // A NEGATIVE slope makes `max_swing` negative, so the withholding test
    // below is false, execution proceeds, and the handicap's SIGN FLIPS: the
    // equalizer becomes the amplifier it exists to prevent, with no panic and
    // no log. An absurd POSITIVE value wraps the `sigmas * slope` `i64`
    // multiply below (release builds do not check overflow) to the same end.
    //
    // Clamp rather than fall back to the default: zero is coherent (it
    // disables the handicap), so no substitute is needed.
    let slope = handicap_per_sigma_permille.clamp(0, HANDICAP_SLOPE_MAX_PERMILLE);

    let sigmas = frustration_sigmas_of(draw).clamp(-HANDICAP_MAX_SIGMA, HANDICAP_MAX_SIGMA);

    // Symmetric only while there is room to move in BOTH directions. Once
    // `typical` is clamped to the attainable floor, tightening is impossible
    // but loosening is not, so applying the handicap would pay for grinding
    // UPWARD and nothing else. Withhold it entirely rather than apply half of
    // it.
    let headroom = i128::from(typical) - i128::from(anchor);
    let max_swing =
        i128::from(typical).abs() * i128::from(HANDICAP_MAX_SIGMA) * i128::from(slope) / 1000;
    if headroom < max_swing {
        return typical;
    }

    // `typical` is negative, so a positive handicap makes the bar more
    // negative, i.e. tighter.
    let handicap = i128::from(typical) * i128::from(sigmas * slope) / 1000;
    saturating_i64(i128::from(typical) + handicap)
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

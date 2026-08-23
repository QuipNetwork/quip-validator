use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use quantum_validation::{AllowedValueSpec, MilliValue, ValidationError};
use scale_info::TypeInfo;

use crate::types::DifficultyConfig;
/// Floor on the per-step energy delta — one energy unit (1000 milli). The
/// geometric step vanishes near a curve bound, so this sets the tail
/// granularity: how fast hardening walks *past* `min_milli` and how easing
/// settles onto `max_milli`. One floor for both directions (walk-up plan §3);
/// the legacy asymmetric 5.0/3.0-unit floors let hardening out-pace easing 5:3.
const MIN_ENERGY_DELTA_MILLI: i64 = 1000;

/// Decay rate per epoch step: 25 per-mille = 2.5%, half of the typical
/// hardening floor (50 per-mille). Mirrors v0.1 `energy_ease_rate = 0.025`.
const DECAY_RATE_MILLI: u32 = 25;

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
/// quality, punishing sampling noise and biasing the retarget's plant. Now
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
    // Clamp rather than fall back to the default as `retarget_bar_milli` does:
    // there a zero step freezes the one difficulty dial, here zero is coherent
    // (it disables the handicap), so no substitute is needed.
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
/// `Get<_>` and `f64` does not `Encode`; divided by 1000 before `expected_gse`.
/// The derives also let a `CurveC` be stored as a per-topology override and
/// passed to `set_topology_curve`.
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
/// `expected_gse` of one topology's `(num_nodes, num_edges)` and h/J specs at
/// `c_hard` (`min_milli`, hardest), `c_knee` and `c_easy` (`max_milli`,
/// easiest). All milli; `min_milli < knee_milli < max_milli`, all negative.
///
/// These are mean-field GSE *estimates*, not hard limits: an instance's actual
/// ground state can be more negative than `min_milli`. So the threshold tracks
/// *past* `min_milli` when hardening but never eases *past* `max_milli`. See
/// [`adjust_energy_along_curve`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnergyCurve {
    pub min_milli: i64,
    pub knee_milli: i64,
    pub max_milli: i64,
}

impl EnergyCurve {
    /// Build a curve from topology size, calibration `c` values, and the
    /// topology's allowed h/J specs. Deriving the magnitudes from the specs
    /// keeps the curve aligned with what puzzles actually sample — a zero-field
    /// topology (`allowed_h = Set([0])`) gets no h contribution instead of
    /// energy no puzzle can produce.
    ///
    /// Errors on an empty or inverted spec (impossible after
    /// `register_topology` validation).
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

/// Move `current_milli` toward the curve bound implied by `direction`, by
/// `room × rate` floored at `min_delta_milli`.
///
/// Because `rate < 1` the step is always smaller than `room`, so the threshold
/// *walks* toward the bound instead of one fast win slamming it across the
/// whole range and stranding the chain (see
/// `quantum-pow-difficulty-hardening-walkup-plan.md`).
///
/// The directions are deliberately asymmetric, because the bounds are GSE
/// *estimates*:
///
/// - [`Direction::Harder`] is **uncapped**: a stronger-than-calibrated field
///   wins below `min_milli`, so the threshold must keep tracking past it. At
///   `room ≤ 0` the geometric term vanishes and the floor alone advances it.
/// - [`Direction::Easier`] is **capped** at `max_milli`: difficulty never eases
///   below the easiest calibrated puzzle. Recovery from a too-hard threshold is
///   fastest at the hard end and settles gently.
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
    // The `as i64` cast saturates, as does every subtraction below, so the
    // genesis `i64::MAX` sentinel and extreme curves stay overflow-safe.
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

/// Apply per-epoch decay easing to `current`, moving only the energy threshold
/// along the curve.
pub fn apply_decay(current: DifficultyConfig, steps: u32, curve: EnergyCurve) -> DifficultyConfig {
    apply_decay_counted(current, steps, curve).0
}

/// [`apply_decay`], also returning how many PRODUCTIVE iterations it ran —
/// the terminating no-op is not counted.
///
/// The count is the point: the early exit leaves the result unchanged, so a
/// test inspecting only the returned difficulty cannot tell whether the bound
/// exists at all. `pub(crate)` because nothing outside the crate should depend
/// on it; [`apply_decay`] is the public entry.
pub(crate) fn apply_decay_counted(
    current: DifficultyConfig,
    steps: u32,
    curve: EnergyCurve,
) -> (DifficultyConfig, u32) {
    let mut difficulty = current;
    let mut ran = 0u32;
    for _ in 0..steps {
        let eased = adjust_energy_along_curve(
            difficulty.max_energy_milli,
            DECAY_RATE_MILLI,
            Direction::Easier,
            curve,
            MIN_ENERGY_DELTA_MILLI,
        );
        // Stop as soon as a step changes nothing: easing is capped at
        // `curve.max_milli`, so every later iteration is a no-op too. `steps`
        // is `elapsed / epoch_length`, bounded only by `u32::MAX`, and a long
        // outage drives it arbitrarily high — unmetered work an attacker could
        // trigger from `submit_proof` for a normal fee.
        if eased == difficulty.max_energy_milli {
            break;
        }
        ran += 1;
        difficulty.max_energy_milli = eased;
    }
    (difficulty, ran)
}

/// Compute the active difficulty for `block_number`, applying decay since the
/// previous winning proof — the per-block view miners must clear and the epoch
/// retarget's baseline.
///
/// All inputs explicit: reads no storage, unit-testable in isolation. The
/// pallet wraps this with the four storage reads. A `None` curve disables decay
/// (genesis, or a defensive fallback); `last_proof_block == 0` is the genesis
/// "no winning proof yet" sentinel.
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
    let steps = elapsed / epoch_length;
    if steps == 0 {
        return base_difficulty;
    }
    match curve {
        Some(curve) => apply_decay(base_difficulty, steps, curve),
        None => base_difficulty,
    }
}

/// Largest fraction of the curve span one epoch's retarget may move the bar, in
/// per-mille. Caps the control loop's gain so a single anomalous epoch — a
/// partition, a miner fleet joining at once — cannot slam the bar across the
/// whole range and strand the chain. A control-loop gain, so a tunable DEFAULT
/// for `Config::RetargetMaxStepPermille` for the same reason as the slope.
pub const DEFAULT_RETARGET_MAX_STEP_PERMILLE: i64 = 250;

/// `ceil(1e6 / step^2)`, factored out of the constant so the rounding is
/// testable at steps other than the one compiled in — the only way to catch the
/// truncation bug, since a `const` is observable at one step.
///
/// `div_ceil` is not const on `i64`, hence the manual rounding. The `< 1` clamp
/// is DEFENSIVE: the numerator is at least `step_sq`, so a non-zero step cannot
/// yield zero. The reachable hazard is `step_permille == 0`, refused by
/// `integrity_test` and by `retarget_bar_milli`'s own `.max(0)`.
pub const fn min_window_for_step(step_permille: i64) -> u32 {
    let step_sq = step_permille * step_permille;
    let bound = (1_000_000 + step_sq - 1) / step_sq;
    if bound < 1 {
        1
    } else {
        bound as u32
    }
}

/// One retarget window's observation.
///
/// `epoch_blocks` and `target_blocks_per_qblock` were adjacent `u64` params,
/// and transposing them COMPILES while inverting the controller's reference: at
/// 100 blocks against a 10-block target, expected qblocks goes from 10 to 0.1,
/// so every window reads as catastrophically fast and the bar tightens by the
/// full clamp each epoch until the chain strands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RetargetWindow {
    /// Qblocks the window actually produced.
    pub qblocks: u32,
    /// Blocks the window spanned.
    pub epoch_blocks: u64,
    /// Blocks per qblock the chain is targeting.
    pub target_blocks_per_qblock: u64,
}

/// Per-epoch error feedback on the energy bar, driving average block time to
/// target. The chain's one difficulty dial.
///
/// The error is **qblock counts**, not blocks per qblock, and the response is
/// proportional and clamped: a slow window eases the bar toward
/// `curve.max_milli`, a fast one tightens it toward `curve.min_milli`.
///
/// Counts rather than the obvious `epoch_blocks / qblocks` because that
/// quotient is integer division on a small denominator, and it collapsed the
/// controller: over a one-target-interval window the only reachable errors
/// were zero qblocks (forced max ease), one (a dead zone) and two or more
/// (always past the clamp). A chain whose true rate was 1.4 per window
/// alternated between holding and slamming a quarter of the curve span.
///
/// So the window must span several target intervals — expected qblocks is
/// `epoch_blocks / target_blocks_per_qblock` and the error is the per-mille
/// shortfall against it, so at `expected = 10` one qblock off moves the bar 10%
/// of the clamp. The step is a fraction of the *curve span*, so the loop is
/// E\*-relative and topology-agnostic.
///
/// No qblocks at all reads as maximally slow and eases by the full clamped
/// step: the recovery path from a bar too tight to clear. Easing stops at
/// `curve.max_milli` while tightening is uncapped, because the bounds are
/// mean-field estimates and a stronger field wins below `min_milli`.
pub fn retarget_bar_milli(
    current_bar_milli: i64,
    curve: EnergyCurve,
    window: RetargetWindow,
    max_step_permille: i64,
) -> i64 {
    let RetargetWindow {
        qblocks,
        epoch_blocks,
        target_blocks_per_qblock,
    } = window;
    if curve.max_milli <= curve.min_milli || target_blocks_per_qblock == 0 || epoch_blocks == 0 {
        return current_bar_milli;
    }
    let target = target_blocks_per_qblock;

    // Qblocks this window should have produced, in milli so a window that is
    // not an exact multiple of the target still carries its fraction.
    let expected_milli = i128::from(epoch_blocks) * 1000 / i128::from(target);
    if expected_milli <= 0 {
        return current_bar_milli;
    }

    // One bound, used by both arms. FALL BACK, do not silently clamp:
    // `max_step_permille.max(0)` turned a non-positive Config value into a zero
    // bound, which zeroes the step on BOTH arms, so `maybe_retarget` writes
    // nothing and emits no event. The one difficulty dial dies with no error,
    // no event and no log — indistinguishable from a chain with no active
    // topologies, and strictly worse than the `Ord::clamp` panic it replaced,
    // which at least named its cause in every node's log. It also killed the
    // `qblocks == 0` arm, the stall-recovery path that MUST ease.
    let bound = if max_step_permille > 0 {
        max_step_permille
    } else {
        log::error!(
            target: "runtime::quantum-pow",
            "RetargetMaxStepPermille = {max_step_permille} is not positive, so the \
             difficulty controller cannot move. Falling back to \
             DEFAULT_RETARGET_MAX_STEP_PERMILLE ({DEFAULT_RETARGET_MAX_STEP_PERMILLE}). \
             Fix the Config value via runtime upgrade.",
        );
        // Deliberately NOT `defensive!`: it panics under `cfg(test)` and
        // try-runtime, which would make this fallback untestable.
        // `integrity_test` is already the CI barrier for a bad Config value;
        // this is the runtime's recovery and has to be exercisable.
        DEFAULT_RETARGET_MAX_STEP_PERMILLE
    };
    let step_permille = if qblocks == 0 {
        bound
    } else {
        // Positive when short of target (too slow) so the bar eases.
        let shortfall = expected_milli - i128::from(qblocks) * 1000;
        let error_permille = saturating_i64(shortfall * 1000 / expected_milli);
        // Not redundant with `integrity_test`, which is `#[cfg(test)]`-only and
        // never runs on a live chain. `Ord::clamp` panics when `min > max`, and
        // this runs inside `on_finalize`, which cannot be refused — a panic
        // here halts block production and import alike. Defend where it would
        // happen.
        error_permille.clamp(-bound, bound)
    };

    let span = i128::from(curve.max_milli) - i128::from(curve.min_milli);
    let delta = span * i128::from(step_permille) / 1000;
    // Saturate, do not truncate. `i64::MAX` is a live sentinel — genesis uses
    // it and `set_difficulty` can set it — so an `as i64` here wraps a
    // slow-epoch ease to about -9.2e18 and stalls the chain permanently, with
    // the `min(max_milli)` clamp powerless because the value is already
    // negative.
    let retargeted = saturating_i64(i128::from(current_bar_milli) + delta);
    retargeted.min(curve.max_milli)
}

#[cfg(test)]
mod window_bound_tests {
    use super::min_window_for_step;

    /// The window bound must round UP, and the squaring must happen before the
    /// division.
    ///
    /// Written as `(1000 / step) * (1000 / step)` it truncates twice and the
    /// error is squared downward, so the guard fails OPEN on a NARROWER clamp —
    /// the direction that raises the requirement. At the current step of 250
    /// both spellings give 16, so a test checking only today's value would pass
    /// against the broken form. These other steps are the mutation check.
    #[test]
    fn the_window_bound_rounds_up_at_every_step() {
        assert_eq!(min_window_for_step(250), 16, "the current clamp");
        // Each of these is strictly greater than the truncating form's answer
        // (12, 9, 7, 3, 2 here vs 9, 4, 4, 1, 1 truncating).
        assert_eq!(min_window_for_step(300), 12);
        assert_eq!(min_window_for_step(334), 9);
        assert_eq!(min_window_for_step(400), 7);
        assert_eq!(min_window_for_step(600), 3);
        assert_eq!(min_window_for_step(750), 2);
        assert_eq!(min_window_for_step(1000), 1);
        // Above 1000 the bound stops meaning anything, but it must clamp to 1,
        // never 0, or `integrity_test` silently checks nothing.
        assert_eq!(min_window_for_step(1500), 1);
    }

    /// `ceil(1e6 / step^2)` is exactly the smallest `w` with
    /// `1/sqrt(w) <= step/1000`, i.e. sigma/expected no worse than the clamp.
    #[test]
    fn the_bound_is_the_smallest_window_where_the_clamp_stops_dominating() {
        for step in [250i64, 300, 334, 400, 600, 750, 1000] {
            let w = i64::from(min_window_for_step(step));
            assert!(
                1_000_000 <= step * step * w,
                "step {step}: window {w} does not reach the clamp"
            );
            assert!(
                w == 1 || 1_000_000 > step * step * (w - 1),
                "step {step}: window {w} is larger than necessary"
            );
        }
    }
}

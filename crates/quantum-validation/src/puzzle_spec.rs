//! Allowed-value sampling specs for nonce-seeded puzzle generation.
//!
//! `AllowedValueSpec` describes how a deterministic RNG picks per-node h
//! fields, per-edge j couplings, and per-spin solution values. The variant
//! determines both the sampling distribution and the on-chain bit-width used
//! when a value (or, for spins, every value in a submitted solution) is
//! encoded in a SCALE-coded payload.

use alloc::vec::Vec;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

use crate::errors::ValidationError;
use crate::fixed::{MilliValue, MILLI_SCALE};

/// Maximum supported bit-width for an indexed encoding (`Set` or
/// `IntegerRange`). Values wider than this fall under `ContinuousRange`, which
/// encodes a raw 32-bit `MilliValue`.
pub const MAX_INDEXED_BITS: u8 = 8;

/// Sampling distribution for a single puzzle parameter (h, j, or spin).
///
/// `Set` values are stored in milli-precision (multiply by 1000 to read as
/// floating point). `IntegerRange` declares whole-integer bounds that are
/// scaled by [`MILLI_SCALE`] when sampled. `ContinuousRange` declares
/// milli-precision bounds and samples any `MilliValue` in the inclusive range.
#[derive(
    Clone, Debug, Encode, Decode, DecodeWithMemTracking, Eq, PartialEq, TypeInfo, MaxEncodedLen,
)]
pub enum AllowedValueSpec<Set> {
    /// Uniform draw from the explicit set; index = bit pattern.
    Set(Set),
    /// Uniform integer draw from `[min, max]` (whole integers), scaled by
    /// [`MILLI_SCALE`] when returned.
    IntegerRange { min: i32, max: i32 },
    /// Uniform milli-precision draw from `[min, max]` in [`MilliValue`] units.
    ContinuousRange { min: MilliValue, max: MilliValue },
}

impl<Set> AllowedValueSpec<Set>
where
    Set: AsRef<[MilliValue]>,
{
    /// Smallest non-zero magnitude this spec can draw, in MILLI, or `0` when
    /// it can draw zero at all.
    ///
    /// THE SCALE ASYMMETRY IS REAL AND CORRECT. `IntegerRange` declares
    /// whole-integer bounds scaled by [`MILLI_SCALE`] when sampled, so its
    /// bounds must be multiplied; `ContinuousRange` already declares milli, so
    /// its must not. Lives HERE, beside the enum doc that states that — the
    /// query used to live in `hardness.rs`, a module away from it.
    pub fn min_abs_milli(&self) -> u64 {
        match self.as_slice() {
            AllowedValueSpec::Set(values) => values
                .iter()
                .map(|v| u64::from(v.unsigned_abs()))
                .min()
                .unwrap_or(0),
            AllowedValueSpec::IntegerRange { min, max } => {
                if min <= 0 && max >= 0 {
                    0
                } else {
                    // Integer bounds -> milli.
                    u64::from(min.unsigned_abs()).min(u64::from(max.unsigned_abs()))
                        * MILLI_SCALE as u64
                }
            }
            AllowedValueSpec::ContinuousRange { min, max } => {
                if min <= 0 && max >= 0 {
                    0
                } else {
                    // Already milli.
                    u64::from(min.unsigned_abs()).min(u64::from(max.unsigned_abs()))
                }
            }
        }
    }

    /// Whether every value this spec can draw is exactly zero.
    ///
    /// Scale-free, so no asymmetry: zero is zero at either scale.
    pub fn samples_only_zero(&self) -> bool {
        match self.as_slice() {
            AllowedValueSpec::Set(values) => !values.is_empty() && values.iter().all(|&v| v == 0),
            AllowedValueSpec::IntegerRange { min, max }
            | AllowedValueSpec::ContinuousRange { min, max } => min == 0 && max == 0,
        }
    }

    /// Whether the spec is symmetric under sign flip, so each fundamental
    /// cycle is an independent fair coin.
    ///
    /// Also scale-free: symmetry is a property of the sign pattern.
    pub fn is_sign_symmetric(&self) -> bool {
        match self.as_slice() {
            AllowedValueSpec::Set(values) => {
                !values.is_empty() && values.iter().all(|v| values.contains(&-v))
            }
            AllowedValueSpec::IntegerRange { min, max }
            | AllowedValueSpec::ContinuousRange { min, max } => min == -max,
        }
    }

    /// View this spec with the inner set borrowed as a slice. Lets math code
    /// be generic over `BoundedVec` / `Vec` / `&[MilliValue]` storage.
    pub fn as_slice(&self) -> AllowedValueSpec<&[MilliValue]> {
        match self {
            Self::Set(values) => AllowedValueSpec::Set(values.as_ref()),
            Self::IntegerRange { min, max } => AllowedValueSpec::IntegerRange {
                min: *min,
                max: *max,
            },
            Self::ContinuousRange { min, max } => AllowedValueSpec::ContinuousRange {
                min: *min,
                max: *max,
            },
        }
    }
}

impl AllowedValueSpec<&[MilliValue]> {
    /// On-chain bit-width per encoded value for this spec.
    ///
    /// - `Set(values)` → `ceil(log2(values.len()))` (minimum 1 bit, 0 bits is
    ///   not a valid encoding even for a single-value set).
    /// - `IntegerRange { min, max }` → `ceil(log2(max - min + 1))` (minimum 1
    ///   bit).
    /// - `ContinuousRange` → 32 bits (raw `MilliValue`).
    ///
    /// Returns an error if the spec is empty/inverted, or if an indexed
    /// variant would need more than [`MAX_INDEXED_BITS`] bits.
    pub fn bits_per_value(&self) -> Result<u8, ValidationError> {
        match *self {
            Self::Set(values) => {
                if values.is_empty() {
                    return Err(ValidationError::EmptyAllowedValues);
                }
                let bits = bits_for_count(values.len() as u64);
                check_indexed_bits(bits)?;
                Ok(bits)
            }
            Self::IntegerRange { min, max } => {
                if max < min {
                    return Err(ValidationError::EmptyAllowedValues);
                }
                // The span is up to (i32::MAX - i32::MIN + 1) == 2^32, which
                // overflows u32. Compute in u64 (which always fits) and let
                // bits_for_count return up to 33, so check_indexed_bits below
                // correctly rejects ranges wider than MAX_INDEXED_BITS instead
                // of silently accepting them as a 1-bit spec.
                let span = (max as i64 - min as i64 + 1) as u64;
                let bits = bits_for_count(span);
                check_indexed_bits(bits)?;
                // decode_value scales the decoded integer by MILLI_SCALE
                // into MilliValue (= i32). Catch range endpoints that
                // overflow that scaling at registration so every proof
                // attempt doesn't later die with ArithmeticOverflow.
                let scaled_min = (min as i64).saturating_mul(MILLI_SCALE);
                let scaled_max = (max as i64).saturating_mul(MILLI_SCALE);
                if scaled_min < MilliValue::MIN as i64 || scaled_max > MilliValue::MAX as i64 {
                    return Err(ValidationError::ArithmeticOverflow);
                }
                Ok(bits)
            }
            Self::ContinuousRange { min, max } => {
                if max < min {
                    return Err(ValidationError::EmptyAllowedValues);
                }
                Ok(32)
            }
        }
    }

    /// Decode a single encoded value to its `MilliValue` interpretation.
    ///
    /// `raw` is the integer read from the bit-packed payload. For `Set` and
    /// `IntegerRange` it is an index/offset; for `ContinuousRange` it is the
    /// full 32-bit value reinterpreted as `MilliValue`.
    pub fn decode_value(&self, raw: u32) -> Result<MilliValue, ValidationError> {
        match *self {
            Self::Set(values) => {
                let index = raw as usize;
                values
                    .get(index)
                    .copied()
                    .ok_or(ValidationError::InvalidEncodedValue { raw })
            }
            Self::IntegerRange { min, max } => {
                // Use u64 so a full-i32-span range computes correctly instead
                // of wrapping to 0 (which would silently mark every encoded
                // value as invalid).
                let span = (max as i64 - min as i64 + 1) as u64;
                if (raw as u64) >= span {
                    return Err(ValidationError::InvalidEncodedValue { raw });
                }
                let value = (min as i64).saturating_add(raw as i64);
                value
                    .saturating_mul(MILLI_SCALE)
                    .try_into()
                    .map_err(|_| ValidationError::ArithmeticOverflow)
            }
            Self::ContinuousRange { min, max } => {
                let value = raw as i32;
                if value < min || value > max {
                    return Err(ValidationError::InvalidEncodedValue { raw });
                }
                Ok(value)
            }
        }
    }

    /// Sample one value from this spec using the provided RNG. Mirrors the
    /// per-variant behavior used by both validator-side puzzle reconstruction
    /// and miner-side packed-payload decoding.
    pub fn sample<R: rand_core::RngCore>(
        &self,
        rng: &mut R,
    ) -> Result<MilliValue, ValidationError> {
        match *self {
            Self::Set(values) => {
                if values.is_empty() {
                    return Err(ValidationError::EmptyAllowedValues);
                }
                let index = (rng.next_u32() as usize) % values.len();
                Ok(values[index])
            }
            Self::IntegerRange { min, max } => {
                if max < min {
                    return Err(ValidationError::EmptyAllowedValues);
                }
                // Compute the span in u64 so a full-i32 range (span == 2^32)
                // does not wrap to 0 and trap on the modulo below. The
                // protocol caps indexed bits at MAX_INDEXED_BITS, so any spec
                // that reaches sample() through register_topology has been
                // validated by bits_per_value() and span fits in u32 — but
                // the helper has to be safe even if a future caller bypasses
                // that validation.
                let span = (max as i64 - min as i64 + 1) as u64;
                let offset = (rng.next_u32() as u64) % span;
                self.decode_value(offset as u32)
            }
            Self::ContinuousRange { min, max } => {
                if max < min {
                    return Err(ValidationError::EmptyAllowedValues);
                }
                let span = (max as i64 - min as i64 + 1) as u64;
                let offset = (rng.next_u32() as u64) % span;
                let value = (min as i64).saturating_add(offset as i64);
                Ok(value as i32)
            }
        }
    }

    /// Canonical byte representation used by `hash_topology` to ensure two
    /// specs that should logically collide hash to the same key (independent
    /// of how `Set` entries were ordered at registration time) and two specs
    /// that should not collide carry distinct discriminant bytes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match *self {
            Self::Set(values) => {
                out.push(0u8);
                if values.windows(2).all(|pair| pair[0] <= pair[1]) {
                    for &value in values {
                        out.extend_from_slice(&value.to_be_bytes());
                    }
                } else {
                    let mut sorted: Vec<MilliValue> = values.to_vec();
                    sorted.sort_unstable();
                    for value in sorted {
                        out.extend_from_slice(&value.to_be_bytes());
                    }
                }
            }
            Self::IntegerRange { min, max } => {
                out.push(1u8);
                out.extend_from_slice(&min.to_be_bytes());
                out.extend_from_slice(&max.to_be_bytes());
            }
            Self::ContinuousRange { min, max } => {
                out.push(2u8);
                out.extend_from_slice(&min.to_be_bytes());
                out.extend_from_slice(&max.to_be_bytes());
            }
        }
        out
    }
}

fn bits_for_count(count: u64) -> u8 {
    match count {
        0 | 1 => 1,
        _ => 64u8 - (count - 1).leading_zeros() as u8,
    }
}

fn check_indexed_bits(bits: u8) -> Result<(), ValidationError> {
    if bits > MAX_INDEXED_BITS {
        Err(ValidationError::EncodingTooWide { bits })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    /// `min_abs_milli` scales `IntegerRange` by `MILLI_SCALE` and does NOT
    /// scale `ContinuousRange`.
    ///
    /// The asymmetry is correct (see the enum's own doc) but was enforced by
    /// nothing while the query lived a module away from that sentence: a spec
    /// query picking the wrong arm had a coin-flip chance of being wrong.
    #[test]
    fn min_abs_milli_scales_integer_bounds_and_not_milli_bounds() {
        let integer: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: 2, max: 5 };
        assert_eq!(
            integer.min_abs_milli(),
            2 * MILLI_SCALE as u64,
            "integer bounds are unit-scaled and must be multiplied"
        );

        let continuous: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::ContinuousRange { min: 2, max: 5 };
        assert_eq!(
            continuous.min_abs_milli(),
            2,
            "milli bounds are already milli and must NOT be multiplied"
        );

        // A range straddling zero can draw zero, at either scale.
        let straddles: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: -1, max: 3 };
        assert_eq!(straddles.min_abs_milli(), 0);

        // A Set is always already in milli.
        let set_vals: [MilliValue; 3] = [-3000, 1000, 2000];
        let set = AllowedValueSpec::Set(&set_vals[..]);
        assert_eq!(set.min_abs_milli(), 1000);
    }

    #[test]
    fn the_scale_free_queries_agree_across_both_range_flavours() {
        for spec in [
            AllowedValueSpec::<&[MilliValue]>::IntegerRange { min: 0, max: 0 },
            AllowedValueSpec::<&[MilliValue]>::ContinuousRange { min: 0, max: 0 },
        ] {
            assert!(spec.samples_only_zero());
            assert!(spec.is_sign_symmetric(), "0 == -0");
        }
        for spec in [
            AllowedValueSpec::<&[MilliValue]>::IntegerRange { min: -4, max: 4 },
            AllowedValueSpec::<&[MilliValue]>::ContinuousRange { min: -4, max: 4 },
        ] {
            assert!(!spec.samples_only_zero());
            assert!(spec.is_sign_symmetric());
        }
        let lopsided: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: -4, max: 5 };
        assert!(!lopsided.is_sign_symmetric());
    }
    use super::*;

    fn set_spec(values: &[MilliValue]) -> AllowedValueSpec<&[MilliValue]> {
        AllowedValueSpec::Set(values)
    }

    #[test]
    fn binary_set_uses_one_bit() {
        let spec = set_spec(&[-1000, 1000]);
        assert_eq!(spec.bits_per_value().unwrap(), 1);
    }

    #[test]
    fn ternary_set_uses_two_bits() {
        let spec = set_spec(&[-6000, 0, 6000]);
        assert_eq!(spec.bits_per_value().unwrap(), 2);
    }

    #[test]
    fn integer_range_uses_minimum_bits() {
        let spec: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: -6, max: 6 };
        // 13 values → 4 bits
        assert_eq!(spec.bits_per_value().unwrap(), 4);
    }

    #[test]
    fn continuous_range_uses_thirty_two_bits() {
        let spec: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::ContinuousRange {
            min: -6000,
            max: 6000,
        };
        assert_eq!(spec.bits_per_value().unwrap(), 32);
    }

    #[test]
    fn decode_set_recovers_values() {
        let spec = set_spec(&[-6000, 0, 6000]);
        assert_eq!(spec.decode_value(0).unwrap(), -6000);
        assert_eq!(spec.decode_value(1).unwrap(), 0);
        assert_eq!(spec.decode_value(2).unwrap(), 6000);
        assert!(matches!(
            spec.decode_value(3),
            Err(ValidationError::InvalidEncodedValue { .. })
        ));
    }

    #[test]
    fn decode_integer_range_scales_by_milli() {
        let spec: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: -6, max: 6 };
        assert_eq!(spec.decode_value(0).unwrap(), -6000);
        assert_eq!(spec.decode_value(6).unwrap(), 0);
        assert_eq!(spec.decode_value(12).unwrap(), 6000);
        assert!(matches!(
            spec.decode_value(13),
            Err(ValidationError::InvalidEncodedValue { .. })
        ));
    }

    #[test]
    fn empty_set_rejected() {
        let spec = set_spec(&[]);
        assert!(matches!(
            spec.bits_per_value(),
            Err(ValidationError::EmptyAllowedValues)
        ));
    }

    #[test]
    fn integer_range_rejected_when_milli_scaling_would_overflow() {
        // bits_per_value caps the *span* at 256 (8 bits), not the absolute
        // value of min/max. A narrow range high in the i32 space can still
        // overflow MILLI_SCALE multiplication at decode_value time. Catch
        // this at bits_per_value so registration fails instead of every
        // proof attempt later dying with ArithmeticOverflow.
        //
        // max = 3_000_000 → 3_000_000 * 1000 = 3 * 10^9 overflows i32::MAX
        // (2.147 * 10^9). Span = 100 (7 bits) — well under MAX_INDEXED_BITS.
        let spec: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::IntegerRange {
            min: 3_000_000,
            max: 3_000_100,
        };
        assert!(matches!(
            spec.bits_per_value(),
            Err(ValidationError::ArithmeticOverflow)
        ));

        // Symmetric check for negative overflow on min.
        let spec: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::IntegerRange {
            min: -3_000_100,
            max: -3_000_000,
        };
        assert!(matches!(
            spec.bits_per_value(),
            Err(ValidationError::ArithmeticOverflow)
        ));

        // Sanity: a narrow, in-range spec is still accepted.
        let spec: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::IntegerRange {
            min: -100,
            max: 100,
        };
        assert!(spec.bits_per_value().is_ok());
    }

    #[test]
    fn full_span_integer_range_rejected_as_too_wide() {
        // i32::MAX - i32::MIN + 1 == 2^32. Prior to the u64 fix the span
        // wrapped to 0 in u32 and bits_for_count(0) returned 1, so the spec
        // was silently accepted as 1-bit; sample() would then panic on
        // `rng.next_u32() % 0`.
        let spec: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::IntegerRange {
            min: i32::MIN,
            max: i32::MAX,
        };
        assert!(matches!(
            spec.bits_per_value(),
            Err(ValidationError::EncodingTooWide { .. })
        ));
    }

    #[test]
    fn canonical_bytes_are_order_independent_for_sets() {
        let a = set_spec(&[6000, -6000, 0]);
        let b = set_spec(&[0, 6000, -6000]);
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn canonical_bytes_distinguish_variants() {
        let s = set_spec(&[0]);
        let ir: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::IntegerRange { min: 0, max: 0 };
        let cr: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::ContinuousRange { min: 0, max: 0 };
        assert_ne!(s.canonical_bytes(), ir.canonical_bytes());
        assert_ne!(s.canonical_bytes(), cr.canonical_bytes());
        assert_ne!(ir.canonical_bytes(), cr.canonical_bytes());
    }
}

//! Bit-packed solution payloads.
//!
//! A submitted Ising solution is a vector of spins. With binary spins the
//! natural wire format is one bit per spin; the encoding supports
//! higher-resolution spin specs (Set, IntegerRange with `MAX_INDEXED_BITS`
//! bits per spin, ContinuousRange with 32 bits) so the wire format can
//! carry magnitudes.
//!
//! Note: the v0.2 `pallet-quantum-pow::validate_proof` pipeline currently
//! collapses every decoded spin to its signum (±1) with [`spin_signs`] before
//! evaluating `energy_of_solution`. That's a v0.2 limitation, not a property
//! of this packing format — when higher-resolution spin magnitudes are wired
//! into the energy calculation, `spin_signs` drops out and the format stays.

use alloc::vec::Vec;

use crate::errors::ValidationError;
use crate::fixed::MilliValue;
use crate::puzzle_spec::AllowedValueSpec;

/// Expected byte length for a packed solution of `num_spins` values under the
/// given spec.
pub fn packed_solution_byte_len(
    num_spins: usize,
    spec: &AllowedValueSpec<&[MilliValue]>,
) -> Result<usize, ValidationError> {
    let bits_per_spin = spec.bits_per_value()? as usize;
    Ok(num_spins.saturating_mul(bits_per_spin).saturating_add(7) / 8)
}

/// Decode a bit-packed solution payload into a vector of `MilliValue` spins.
///
/// Each spin occupies `spec.bits_per_value()` consecutive bits, starting from
/// the least-significant bit of byte 0 and walking upward. The 32-bit
/// `ContinuousRange` variant reads four bytes per spin in big-endian order.
pub fn unpack_solution(
    packed: &[u8],
    num_spins: usize,
    spec: &AllowedValueSpec<&[MilliValue]>,
) -> Result<Vec<MilliValue>, ValidationError> {
    let bits_per_spin = spec.bits_per_value()? as usize;
    let expected = packed_solution_byte_len(num_spins, spec)?;
    if packed.len() != expected {
        return Err(ValidationError::PackedSolutionLengthMismatch {
            expected,
            actual: packed.len(),
        });
    }

    if bits_per_spin == 32 {
        return decode_continuous(packed, num_spins, spec);
    }

    decode_indexed(packed, num_spins, bits_per_spin, spec)
}

fn decode_indexed(
    packed: &[u8],
    num_spins: usize,
    bits_per_spin: usize,
    spec: &AllowedValueSpec<&[MilliValue]>,
) -> Result<Vec<MilliValue>, ValidationError> {
    let mask: u32 = if bits_per_spin == 32 {
        u32::MAX
    } else {
        (1u32 << bits_per_spin) - 1
    };

    let mut out = Vec::with_capacity(num_spins);
    for spin_index in 0..num_spins {
        let bit_offset = spin_index * bits_per_spin;
        let byte_index = bit_offset / 8;
        let intra_byte = bit_offset % 8;

        // bits_per_spin <= MAX_INDEXED_BITS (8) and intra_byte <= 7, so the
        // value never spans more than two bytes. The second byte may be
        // out-of-bounds for the very last spin (when the unused trailing
        // bits are all zero), in which case it's safely defaulted to 0.
        let b0 = u32::from(packed[byte_index]);
        let b1 = u32::from(*packed.get(byte_index + 1).unwrap_or(&0));
        let raw = ((b0 | (b1 << 8)) >> intra_byte) & mask;

        out.push(spec.decode_value(raw)?);
    }
    Ok(out)
}

fn decode_continuous(
    packed: &[u8],
    num_spins: usize,
    spec: &AllowedValueSpec<&[MilliValue]>,
) -> Result<Vec<MilliValue>, ValidationError> {
    let mut out = Vec::with_capacity(num_spins);
    for spin_index in 0..num_spins {
        let offset = spin_index * 4;
        let bytes: [u8; 4] = packed
            .get(offset..offset + 4)
            .and_then(|s| s.try_into().ok())
            .ok_or(ValidationError::PackedSolutionLengthMismatch {
                expected: num_spins * 4,
                actual: packed.len(),
            })?;
        let raw = u32::from_be_bytes(bytes);
        out.push(spec.decode_value(raw)?);
    }
    Ok(out)
}

/// Pack a slice of MilliValue spins into the bit-packed wire format.
///
/// Inverse of [`unpack_solution`]. Mainly used by tests and by miners
/// constructing proofs; the validator path only needs `unpack_solution`.
pub fn pack_solution(
    spins: &[MilliValue],
    spec: &AllowedValueSpec<&[MilliValue]>,
) -> Result<Vec<u8>, ValidationError> {
    let bits_per_spin = spec.bits_per_value()? as usize;
    let byte_len = packed_solution_byte_len(spins.len(), spec)?;
    let mut out = alloc::vec![0u8; byte_len];

    if bits_per_spin == 32 {
        for (i, spin) in spins.iter().enumerate() {
            let bytes = (*spin as u32).to_be_bytes();
            let offset = i * 4;
            out[offset..offset + 4].copy_from_slice(&bytes);
        }
        return Ok(out);
    }

    for (spin_index, spin) in spins.iter().enumerate() {
        let raw = encode_value(*spin, spec)?;
        let bit_offset = spin_index * bits_per_spin;
        let byte_index = bit_offset / 8;
        let intra_byte = bit_offset % 8;

        let shifted = raw << intra_byte;
        out[byte_index] |= (shifted & 0xFF) as u8;
        // bits_per_spin <= MAX_INDEXED_BITS (8) and intra_byte <= 7, so the
        // value never spans more than two bytes — a third-byte spill would
        // need intra_byte + bits_per_spin > 16, which is unreachable.
        if byte_index + 1 < out.len() {
            out[byte_index + 1] |= ((shifted >> 8) & 0xFF) as u8;
        }
    }
    Ok(out)
}

fn encode_value(
    value: MilliValue,
    spec: &AllowedValueSpec<&[MilliValue]>,
) -> Result<u32, ValidationError> {
    match *spec {
        AllowedValueSpec::Set(values) => values
            .iter()
            .position(|&v| v == value)
            .map(|i| i as u32)
            .ok_or(ValidationError::InvalidEncodedValue { raw: value as u32 }),
        AllowedValueSpec::IntegerRange { min, max } => {
            if value % crate::fixed::MILLI_SCALE as i32 != 0 {
                return Err(ValidationError::InvalidEncodedValue { raw: value as u32 });
            }
            let whole = value / crate::fixed::MILLI_SCALE as i32;
            if whole < min || whole > max {
                return Err(ValidationError::InvalidEncodedValue { raw: value as u32 });
            }
            Ok((whole - min) as u32)
        }
        AllowedValueSpec::ContinuousRange { min, max } => {
            if value < min || value > max {
                return Err(ValidationError::InvalidEncodedValue { raw: value as u32 });
            }
            Ok(value as u32)
        }
    }
}

/// Collapse decoded `MilliValue` spins to their signs for energy scoring.
///
/// This is the v0.2 limitation noted in the module docs: only the sign of a
/// decoded spin reaches `energy_of_solution`. A zero has no sign and is
/// rejected.
pub fn spin_signs(milli: &[MilliValue]) -> Result<Vec<i8>, ValidationError> {
    let mut spins = Vec::with_capacity(milli.len());
    for (index, value) in milli.iter().enumerate() {
        let sign = value.signum();
        if sign == 0 {
            return Err(ValidationError::InvalidSpinValue { index, value: 0 });
        }
        spins.push(sign as i8);
    }
    Ok(spins)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle_spec::AllowedValueSpec;

    fn binary_spin_spec() -> AllowedValueSpec<&'static [MilliValue]> {
        AllowedValueSpec::Set(&[-1000, 1000])
    }

    fn ternary_h_spec() -> AllowedValueSpec<&'static [MilliValue]> {
        AllowedValueSpec::Set(&[-6000, 0, 6000])
    }

    #[test]
    fn spin_signs_collapses_magnitudes_and_rejects_zero() {
        assert_eq!(spin_signs(&[3000, -1000, 1, -2]).unwrap(), [1, -1, 1, -1]);
        assert_eq!(
            spin_signs(&[1000, 0]),
            Err(ValidationError::InvalidSpinValue { index: 1, value: 0 })
        );
    }

    #[test]
    fn binary_spins_round_trip() {
        let spec = binary_spin_spec();
        let spins = [-1000, 1000, -1000, 1000, 1000, -1000, -1000, 1000, 1000];
        let packed = pack_solution(&spins, &spec).unwrap();
        // 9 spins * 1 bit = 9 bits → 2 bytes
        assert_eq!(packed.len(), 2);
        let unpacked = unpack_solution(&packed, spins.len(), &spec).unwrap();
        assert_eq!(unpacked, spins);
    }

    #[test]
    fn ternary_h_round_trip() {
        let spec = ternary_h_spec();
        let values = [-6000, 0, 6000, -6000, 0, 6000, -6000];
        let packed = pack_solution(&values, &spec).unwrap();
        // 7 * 2 bits = 14 bits → 2 bytes
        assert_eq!(packed.len(), 2);
        let unpacked = unpack_solution(&packed, values.len(), &spec).unwrap();
        assert_eq!(unpacked, values);
    }

    #[test]
    fn invalid_index_in_ternary_decoded_as_error() {
        let spec = ternary_h_spec();
        // 2 bits per value, but pattern 0b11 == 3 is out of range for a
        // 3-entry set.
        let packed = [0b11_11_11_11];
        assert!(matches!(
            unpack_solution(&packed, 4, &spec),
            Err(ValidationError::InvalidEncodedValue { .. })
        ));
    }

    #[test]
    fn length_mismatch_reported() {
        let spec = binary_spin_spec();
        // 9 spins need 2 bytes; supply 1.
        let packed = [0u8];
        assert!(matches!(
            unpack_solution(&packed, 9, &spec),
            Err(ValidationError::PackedSolutionLengthMismatch { .. })
        ));
    }

    #[test]
    fn integer_range_round_trip() {
        // -2..=2 → 5 values → 3 bits per spin.
        let spec: AllowedValueSpec<&[MilliValue]> =
            AllowedValueSpec::IntegerRange { min: -2, max: 2 };
        let values = [-2000, 0, 2000, 1000, -1000];
        let packed = pack_solution(&values, &spec).unwrap();
        let unpacked = unpack_solution(&packed, values.len(), &spec).unwrap();
        assert_eq!(unpacked, values);
    }

    #[test]
    fn continuous_range_round_trip_uses_32_bits() {
        let spec: AllowedValueSpec<&[MilliValue]> = AllowedValueSpec::ContinuousRange {
            min: -6000,
            max: 6000,
        };
        let values = [-6000, -2500, 0, 2500, 6000];
        let packed = pack_solution(&values, &spec).unwrap();
        assert_eq!(packed.len(), values.len() * 4);
        let unpacked = unpack_solution(&packed, values.len(), &spec).unwrap();
        assert_eq!(unpacked, values);
    }
}

//! Compact difficulty-target encoding (Bitcoin "nBits" format).

use primitive_types::U256;

pub fn bits_to_target(bits: u32) -> Option<U256> {
    let exp = (bits >> 24) as usize;
    let mant = bits & 0x007f_ffff;
    if bits & 0x0080_0000 != 0 || mant == 0 || exp > 34 {
        return None;
    }
    let t = if exp <= 3 {
        U256::from(mant >> (8 * (3 - exp)))
    } else {
        let shift = 8 * (exp - 3);
        if shift > 232 {
            return None; // would overflow 256 bits for a 3-byte mantissa
        }
        U256::from(mant) << shift
    };
    if t.is_zero() {
        None
    } else {
        Some(t)
    }
}

pub fn target_to_bits(target: U256) -> u32 {
    let mut size = target.bits().div_ceil(8);
    let mut compact: u32 = if size <= 3 {
        (target.low_u64() as u32) << (8 * (3 - size))
    } else {
        (target >> (8 * (size - 3))).low_u64() as u32
    };
    if compact & 0x0080_0000 != 0 {
        compact >>= 8;
        size += 1;
    }
    compact | ((size as u32) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_various() {
        for t in [
            U256::from(1u64),
            U256::from(0x7fffffu64),
            U256::from(0x800000u64),
            U256::MAX >> 4,
            U256::MAX >> 24,
            U256::one() << 200,
        ] {
            let bits = target_to_bits(t);
            let back = bits_to_target(bits).unwrap();
            // compact encoding truncates to 3 significant bytes; the
            // round-trip must be <= original and within mantissa precision
            assert!(back <= t);
            assert!(target_to_bits(back) == bits);
        }
    }
}

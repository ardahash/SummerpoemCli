//! ASERT difficulty adjustment (aserti3-2d style, anchored at genesis).
//!
//! next_target = anchor_target * 2^((time_diff - ideal_interval*(height_diff+1)) / tau)
//! computed with a Q16 cubic approximation of 2^x for the fractional part.

use primitive_types::{U256, U512};

fn u512_to_u256_saturating(v: U512, limit: U256) -> U256 {
    if v > U512::from(limit) {
        return limit;
    }
    let bytes: [u8; 64] = v.to_big_endian();
    U256::from_big_endian(&bytes[32..])
}

/// `time_diff` = parent.time - anchor.time (may be negative if clocks skew);
/// `height_diff` = parent.height - anchor.height.
pub fn next_target(
    anchor_target: U256,
    ideal_interval: u64,
    tau: u64,
    time_diff: i64,
    height_diff: u64,
    pow_limit: U256,
) -> U256 {
    let ideal: i128 = (ideal_interval as i128) * (height_diff as i128 + 1);
    let exponent: i128 = (((time_diff as i128) - ideal) << 16) / (tau as i128);

    let mut shifts = exponent >> 16;
    let frac = (exponent - (shifts << 16)) as u128; // 0..65536
    debug_assert!(frac < 65536);

    // 2^(frac/65536) in Q16, cubic approximation (aserti3-2d constants)
    let factor: u64 = 65536
        + ((195_766_423_245_049u128 * frac
            + 971_821_376u128 * frac * frac
            + 5_127u128 * frac * frac * frac
            + (1u128 << 47))
            >> 48) as u64;

    let mut target = (U512::from(anchor_target) * U512::from(factor)) >> 16;

    shifts = shifts.clamp(-256, 256);
    if shifts < 0 {
        target >>= (-shifts) as usize;
    } else if shifts > 0 {
        // shifting left can only make the target easier; anything that would
        // exceed the representable range saturates to pow_limit anyway
        if (target.bits() as i128) + shifts >= 512 {
            return pow_limit;
        }
        target <<= shifts as usize;
    }

    if target.is_zero() {
        return U256::one();
    }
    u512_to_u256_saturating(target, pow_limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 60;
    const TAU: u64 = 86_400;

    fn anchor() -> U256 {
        U256::one() << 224
    }

    #[test]
    fn on_schedule_holds_target() {
        // parent exactly on schedule: time_diff == ideal*(height_diff+1) - T...
        // With time_diff = T*height_diff the exponent is -T/tau (one block worth),
        // so target moves by a factor of 2^(-60/86400) ≈ 0.99952 — essentially flat.
        let t = next_target(anchor(), T, TAU, (T * 1000) as i64, 1000, U256::MAX);
        let ratio_num = (t << 16) / anchor();
        // within ~0.1% of 65536
        assert!(ratio_num > U256::from(65450u64) && ratio_num < U256::from(65600u64));
    }

    #[test]
    fn late_blocks_ease_target() {
        // blocks arriving 2x slow -> target should rise (easier)
        let t = next_target(anchor(), T, TAU, (2 * T * 2880) as i64, 2880, U256::MAX);
        assert!(t > anchor());
    }

    #[test]
    fn fast_blocks_tighten_target() {
        let t = next_target(anchor(), T, TAU, ((T / 2) * 2880) as i64, 2880, U256::MAX);
        assert!(t < anchor());
    }

    #[test]
    fn one_tau_late_doubles() {
        // being exactly tau seconds behind schedule doubles the target
        let height = 100u64;
        let ideal = (T * (height + 1)) as i64;
        let t = next_target(anchor(), T, TAU, ideal + TAU as i64, height, U256::MAX);
        let ratio = t / anchor();
        assert_eq!(ratio, U256::from(2u64));
    }

    #[test]
    fn clamps_to_pow_limit() {
        let limit = U256::one() << 230;
        let t = next_target(anchor(), T, TAU, 10_000_000_000, 10, limit);
        assert_eq!(t, limit);
    }
}

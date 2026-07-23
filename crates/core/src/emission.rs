//! Emission schedule: smooth exponential decay with an exact rational base.
//!
//! R(h) = R0 * b^h with b = (M-1)/M, computed in Q96 fixed point.
//! M is chosen so the half-life M*ln2 ≈ 2,102,573 blocks ≈ 4 years at 60 s.
//! Total issuance = sum R(h) = R0 * M ≤ SUPPLY_CAP by construction (floor
//! rounding only ever undershoots).

use primitive_types::U256;

pub const COIN: u64 = 100_000_000;
pub const SUPPLY_CAP: u64 = 42_000_000 * COIN;

/// Decay divisor: b = (M-1)/M. Half-life ≈ M·ln2 ≈ 2,102,573 blocks.
pub const DECAY_M: u64 = 3_033_415;

const Q: usize = 96;

fn base_q96() -> U256 {
    (U256::from(DECAY_M - 1) << Q) / U256::from(DECAY_M)
}

fn pow_q96(mut base: U256, mut e: u64) -> U256 {
    let mut acc = U256::one() << Q;
    while e > 0 {
        if e & 1 == 1 {
            acc = (acc * base) >> Q;
        }
        base = (base * base) >> Q;
        e >>= 1;
    }
    acc
}

/// Block subsidy in stanzas at the given height. Deterministic, stateless.
pub fn block_reward(height: u64) -> u64 {
    let r0 = SUPPLY_CAP / DECAY_M;
    let f = pow_q96(base_q96(), height);
    ((U256::from(r0) * f) >> Q).as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_reward_close_to_target() {
        // ≈ 13.85 SUMP
        let r0 = block_reward(0);
        assert!(r0 > 13 * COIN && r0 < 14 * COIN, "r0 = {r0}");
    }

    #[test]
    fn monotonically_nonincreasing() {
        let mut prev = block_reward(0);
        for h in [1u64, 10, 1000, 100_000, 2_000_000, 10_000_000, 50_000_000] {
            let r = block_reward(h);
            assert!(r <= prev, "reward increased at {h}");
            prev = r;
        }
    }

    #[test]
    fn half_life_about_four_years() {
        let half_life = 2_102_573u64; // M * ln2
        let r0 = block_reward(0) as f64;
        let rh = block_reward(half_life) as f64;
        let ratio = rh / r0;
        assert!((ratio - 0.5).abs() < 0.001, "ratio = {ratio}");
    }

    #[test]
    fn supply_cap_respected() {
        // Geometric sum bound: sum R(h) <= R0 * M = (CAP/M)*M <= CAP.
        let r0 = block_reward(0) as u128;
        assert!(r0 * DECAY_M as u128 <= SUPPLY_CAP as u128);
    }
}

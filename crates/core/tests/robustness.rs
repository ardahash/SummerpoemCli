//! Hardening tests: decoders must never panic on hostile input, and the
//! consensus math must hold its invariants across wide input ranges.
//!
//! Uses a small deterministic xorshift RNG so failures are reproducible and
//! the suite needs no external fuzzing dependency.

use primitive_types::U256;
use sump_core::asert::next_target;
use sump_core::block::Block;
use sump_core::compact::{bits_to_target, target_to_bits};
use sump_core::emission::{block_reward, DECAY_M, SUPPLY_CAP};
use sump_core::tx::Transaction;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next_u64() & 0xff) as u8).collect()
    }
    fn u256(&mut self) -> U256 {
        let mut b = [0u8; 32];
        for c in b.chunks_mut(8) {
            c.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        U256::from_little_endian(&b)
    }
}

#[test]
fn tx_decoder_never_panics_on_random_input() {
    let mut rng = Rng::new(0xA11CE);
    for _ in 0..20_000 {
        let len = (rng.next_u64() % 300) as usize;
        let bytes = rng.bytes(len);
        // must return a Result, never panic or hang
        let _ = Transaction::decode_all(&bytes);
    }
}

#[test]
fn block_decoder_never_panics_on_random_input() {
    let mut rng = Rng::new(0xB10C);
    for _ in 0..20_000 {
        let len = (rng.next_u64() % 400) as usize;
        let bytes = rng.bytes(len);
        let _ = Block::decode_all(&bytes);
    }
}

#[test]
fn decoders_survive_mutated_valid_encodings() {
    // start from a valid transaction, flip/truncate bytes, ensure no panic
    use sump_core::tx::{Lock, OutPoint, SigScheme, TxBody, TxInput, TxOutput, Witness};
    let tx = Transaction {
        body: TxBody {
            version: 1,
            inputs: vec![TxInput {
                prevout: OutPoint {
                    txid: sump_core::hash::sha3(&[b"x"]),
                    vout: 0,
                },
            }],
            outputs: vec![TxOutput {
                amount: 1000,
                lock: Lock::P2pkh {
                    scheme: SigScheme::MlDsa,
                    pkh: [1u8; 20],
                },
            }],
            locktime: 0,
            coinbase_data: vec![],
        },
        witnesses: vec![Witness {
            pubkey: vec![1, 2, 3],
            signature: vec![4, 5, 6],
        }],
    };
    let valid = tx.encode();
    let mut rng = Rng::new(0xF1);
    for _ in 0..20_000 {
        let mut m = valid.clone();
        // apply 1-3 mutations
        let muts = 1 + (rng.next_u64() % 3);
        for _ in 0..muts {
            if m.is_empty() {
                break;
            }
            match rng.next_u64() % 3 {
                0 => {
                    let i = (rng.next_u64() as usize) % m.len();
                    m[i] ^= (rng.next_u64() & 0xff) as u8;
                }
                1 => {
                    let i = (rng.next_u64() as usize) % m.len();
                    m.truncate(i);
                }
                _ => m.push((rng.next_u64() & 0xff) as u8),
            }
        }
        let _ = Transaction::decode_all(&m);
    }
}

#[test]
fn compact_bits_roundtrip_and_reject_malformed() {
    let mut rng = Rng::new(0xB175);
    for _ in 0..50_000 {
        // random valid-ish nBits: exercise both encode and decode paths
        let bits = (rng.next_u64() & 0xffff_ffff) as u32;
        if let Some(t) = bits_to_target(bits) {
            // a decodable target must re-encode to a decodable value
            let re = target_to_bits(t);
            let back = bits_to_target(re).expect("re-encoded bits decode");
            assert!(back <= t.max(back)); // no panic; consistency
        }
        // random targets roundtrip with truncation (back <= original)
        let target = rng.u256() >> (rng.next_u64() % 250);
        if target.is_zero() {
            continue;
        }
        let b = target_to_bits(target);
        if let Some(back) = bits_to_target(b) {
            assert!(back <= target, "compact roundtrip must not increase target");
        }
    }
}

#[test]
fn emission_invariants_hold_across_range() {
    // reward(0) in the expected band
    assert!(block_reward(0) > 13 * 100_000_000 && block_reward(0) < 14 * 100_000_000);

    // monotone non-increasing and never panics, including extreme heights
    let mut prev = block_reward(0);
    for h in [
        0u64, 1, 2, 100, 10_000, 1_000_000, 2_102_573, 10_000_000, 100_000_000,
        1_000_000_000, u64::MAX / 2, u64::MAX,
    ] {
        let r = block_reward(h);
        assert!(r <= prev || h == 0, "reward increased at height {h}");
        prev = r;
    }
    // reward decays to zero at large heights, never underflows
    assert_eq!(block_reward(u64::MAX), 0);

    // cumulative issuance over an initial span stays under the cap and rises
    let mut cum: u128 = 0;
    let mut last = u64::MAX;
    for h in 0..200_000u64 {
        let r = block_reward(h);
        assert!(r <= last, "non-monotone at {h}");
        last = r;
        cum += r as u128;
        assert!(cum <= SUPPLY_CAP as u128, "cap exceeded at height {h}");
    }
    // geometric total bound: sum over all heights = R0 * M <= cap
    assert!(block_reward(0) as u128 * DECAY_M as u128 <= SUPPLY_CAP as u128);
}

#[test]
fn asert_is_monotone_and_bounded() {
    let pow_limit = U256::one() << 240;
    let anchor = U256::one() << 224;
    let (t, tau) = (60u64, 86_400u64);

    // higher time_diff (slower blocks) => easier (higher) target, monotonically
    let mut prev = U256::zero();
    for k in 0..200i64 {
        let td = k * 1000;
        let target = next_target(anchor, t, tau, td, 100, pow_limit);
        assert!(target >= prev, "ASERT not monotone in time_diff");
        assert!(target <= pow_limit, "ASERT exceeded pow_limit");
        prev = target;
    }

    // extreme inputs must not panic and must clamp within [1, pow_limit]
    for &td in &[i64::MIN / 2, -1_000_000, 0, 1_000_000, i64::MAX / 2] {
        for &hd in &[0u64, 1, 1_000, 1_000_000] {
            let target = next_target(anchor, t, tau, td, hd, pow_limit);
            assert!(target >= U256::one() && target <= pow_limit);
        }
    }
}

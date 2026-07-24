//! The wire message decoder handles untrusted peer bytes: it must never panic
//! or over-allocate, only return Err on bad input.

use sump_net::message::Message;
use sump_core::hash::Hash256;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

#[test]
fn message_decoder_never_panics() {
    let mut rng = Rng(0xDEADBEEF);
    for _ in 0..50_000 {
        let len = (rng.next() % 512) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() & 0xff) as u8).collect();
        let _ = Message::decode(&bytes);
    }
}

#[test]
fn message_decoder_survives_mutated_valid() {
    let valid = Message::Inv {
        blocks: vec![Hash256([1u8; 32]), Hash256([2u8; 32])],
        txs: vec![Hash256([3u8; 32])],
    }
    .encode();
    let mut rng = Rng(0x1234);
    for _ in 0..50_000 {
        let mut m = valid.clone();
        if m.is_empty() {
            continue;
        }
        let i = (rng.next() as usize) % m.len();
        m[i] ^= (rng.next() & 0xff) as u8;
        if rng.next().is_multiple_of(4) {
            m.truncate((rng.next() as usize) % (m.len() + 1));
        }
        let _ = Message::decode(&m);
    }
}

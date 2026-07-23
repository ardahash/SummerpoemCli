//! SHA3-256 / SHAKE-256 wrappers and the 32-byte hash type.

use primitive_types::U256;
use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::{Digest, Sha3_256, Shake256};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash256(pub [u8; 32]);

impl Hash256 {
    pub const ZERO: Hash256 = Hash256([0u8; 32]);

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Option<Hash256> {
        let v = hex::decode(s).ok()?;
        let a: [u8; 32] = v.try_into().ok()?;
        Some(Hash256(a))
    }

    /// Interpret the hash as a big-endian 256-bit integer (for PoW comparison).
    pub fn to_u256(&self) -> U256 {
        U256::from_big_endian(&self.0)
    }
}

impl fmt::Display for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash256({})", self.to_hex())
    }
}

/// SHA3-256 over the concatenation of `parts`.
pub fn sha3(parts: &[&[u8]]) -> Hash256 {
    let mut h = Sha3_256::new();
    for p in parts {
        Digest::update(&mut h, p);
    }
    Hash256(h.finalize().into())
}

/// SHAKE-256 over the concatenation of `parts`, filling `out`.
pub fn shake256(parts: &[&[u8]], out: &mut [u8]) {
    let mut h = Shake256::default();
    for p in parts {
        h.update(p);
    }
    h.finalize_xof().read(out);
}

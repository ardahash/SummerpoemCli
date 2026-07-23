//! ML-DSA-44 signatures (FIPS 204) and bech32m addresses.

use fips204::ml_dsa_44 as mldsa;
use fips204::traits::{SerDes, Signer, Verifier};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sump_core::hash::sha3;

pub const PUBKEY_LEN: usize = mldsa::PK_LEN; // 1312
pub const SIG_LEN: usize = mldsa::SIG_LEN; // 2420

/// Domain context bound into every signature.
const SIG_CTX: &[u8] = b"sump/v1";

pub struct Keypair {
    pub public: Vec<u8>,
    secret: mldsa::PrivateKey,
}

impl Keypair {
    /// Deterministic key generation from a 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> Keypair {
        let mut rng = ChaCha20Rng::from_seed(*seed);
        let (pk, sk) = mldsa::try_keygen_with_rng(&mut rng).expect("ML-DSA keygen");
        Keypair {
            public: pk.into_bytes().to_vec(),
            secret: sk,
        }
    }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        self.secret
            .try_sign(msg, SIG_CTX)
            .expect("ML-DSA signing")
            .to_vec()
    }

    pub fn pubkey_hash(&self) -> [u8; 20] {
        pubkey_hash(&self.public)
    }
}

pub fn verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let Ok(pk_arr): Result<[u8; PUBKEY_LEN], _> = pubkey.try_into() else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; SIG_LEN], _> = sig.try_into() else {
        return false;
    };
    let Ok(pk) = mldsa::PublicKey::try_from_bytes(pk_arr) else {
        return false;
    };
    pk.verify(msg, &sig_arr, SIG_CTX)
}

/// Address payload: first 20 bytes of SHA3-256 over a tagged pubkey.
pub fn pubkey_hash(pubkey: &[u8]) -> [u8; 20] {
    let h = sha3(&[b"sump/pkh/v1", pubkey]);
    h.0[..20].try_into().unwrap()
}

pub mod address {
    use bech32::{Bech32m, Hrp};

    /// Version 0 = ML-DSA-44 P2PKH. Version 1 is reserved for SLH-DSA.
    pub const VERSION_MLDSA: u8 = 0;

    pub fn encode(hrp: &str, version: u8, pkh: &[u8; 20]) -> String {
        let hrp = Hrp::parse(hrp).expect("valid hrp");
        let mut data = Vec::with_capacity(21);
        data.push(version);
        data.extend_from_slice(pkh);
        bech32::encode::<Bech32m>(hrp, &data).expect("bech32m encode")
    }

    pub fn decode(expect_hrp: &str, s: &str) -> Option<(u8, [u8; 20])> {
        let (hrp, data) = bech32::decode(s).ok()?;
        if hrp.as_str() != expect_hrp || data.len() != 21 {
            return None;
        }
        Some((data[0], data[1..].try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_keygen() {
        let a = Keypair::from_seed(&[7u8; 32]);
        let b = Keypair::from_seed(&[7u8; 32]);
        assert_eq!(a.public, b.public);
        let c = Keypair::from_seed(&[8u8; 32]);
        assert_ne!(a.public, c.public);
        assert_eq!(a.public.len(), PUBKEY_LEN);
    }

    #[test]
    fn sign_verify_roundtrip() {
        let kp = Keypair::from_seed(&[1u8; 32]);
        let msg = [42u8; 32];
        let sig = kp.sign(&msg);
        assert_eq!(sig.len(), SIG_LEN);
        assert!(verify(&kp.public, &msg, &sig));
        assert!(!verify(&kp.public, &[43u8; 32], &sig), "wrong message");
        let other = Keypair::from_seed(&[2u8; 32]);
        assert!(!verify(&other.public, &msg, &sig), "wrong key");
        let mut bad = sig.clone();
        bad[0] ^= 1;
        assert!(!verify(&kp.public, &msg, &bad), "corrupted signature");
    }

    #[test]
    fn address_roundtrip() {
        let pkh = [5u8; 20];
        let s = address::encode("sumprt", address::VERSION_MLDSA, &pkh);
        assert!(s.starts_with("sumprt1"));
        let (v, back) = address::decode("sumprt", &s).unwrap();
        assert_eq!(v, address::VERSION_MLDSA);
        assert_eq!(back, pkh);
        assert!(address::decode("sump", &s).is_none(), "wrong hrp rejected");
    }
}

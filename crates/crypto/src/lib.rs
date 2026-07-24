//! Post-quantum signatures and addresses.
//!
//! Two schemes, selected per output by [`SigScheme`]:
//! - **ML-DSA-44** (FIPS 204): the default, small and fast.
//! - **SLH-DSA-SHAKE-128s** (FIPS 205): hash-based "vault" scheme, larger and
//!   slower, but resting only on SHA3/SHAKE. Dormant by policy; opt-in.

use fips204::ml_dsa_44 as mldsa;
use fips204::traits::{SerDes as _, Signer as _, Verifier as _};
use fips205::slh_dsa_shake_128s as slhdsa;
use fips205::traits::{SerDes as _, Signer as _, Verifier as _};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sump_core::hash::sha3;

pub use sump_core::tx::SigScheme;

pub const MLDSA_PUBKEY_LEN: usize = mldsa::PK_LEN; // 1312
pub const MLDSA_SIG_LEN: usize = mldsa::SIG_LEN; // 2420
pub const SLHDSA_PUBKEY_LEN: usize = slhdsa::PK_LEN; // 32
pub const SLHDSA_SIG_LEN: usize = slhdsa::SIG_LEN; // 7856

/// Domain context bound into every signature.
const SIG_CTX: &[u8] = b"sump/v1";

enum SecretKey {
    MlDsa(Box<mldsa::PrivateKey>),
    SlhDsa(Box<slhdsa::PrivateKey>),
}

pub struct Keypair {
    pub scheme: SigScheme,
    pub public: Vec<u8>,
    secret: SecretKey,
}

impl Keypair {
    /// Deterministic key generation from a 32-byte seed, for `scheme`.
    pub fn from_seed(scheme: SigScheme, seed: &[u8; 32]) -> Keypair {
        let mut rng = ChaCha20Rng::from_seed(*seed);
        match scheme {
            SigScheme::MlDsa => {
                let (pk, sk) = mldsa::try_keygen_with_rng(&mut rng).expect("ML-DSA keygen");
                Keypair {
                    scheme,
                    public: pk.into_bytes().to_vec(),
                    secret: SecretKey::MlDsa(Box::new(sk)),
                }
            }
            SigScheme::SlhDsa => {
                let (pk, sk) = slhdsa::try_keygen_with_rng(&mut rng).expect("SLH-DSA keygen");
                Keypair {
                    scheme,
                    public: pk.into_bytes().to_vec(),
                    secret: SecretKey::SlhDsa(Box::new(sk)),
                }
            }
        }
    }

    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        match &self.secret {
            SecretKey::MlDsa(sk) => sk
                .try_sign(msg, SIG_CTX)
                .expect("ML-DSA signing")
                .to_vec(),
            SecretKey::SlhDsa(sk) => sk
                .try_sign(msg, SIG_CTX, true)
                .expect("SLH-DSA signing")
                .to_vec(),
        }
    }

    pub fn pubkey_hash(&self) -> [u8; 20] {
        pubkey_hash(&self.public)
    }
}

/// Verify a signature under the given scheme.
pub fn verify(scheme: SigScheme, pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    match scheme {
        SigScheme::MlDsa => verify_mldsa(pubkey, msg, sig),
        SigScheme::SlhDsa => verify_slhdsa(pubkey, msg, sig),
    }
}

fn verify_mldsa(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let Ok(pk_arr): Result<[u8; MLDSA_PUBKEY_LEN], _> = pubkey.try_into() else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; MLDSA_SIG_LEN], _> = sig.try_into() else {
        return false;
    };
    let Ok(pk) = mldsa::PublicKey::try_from_bytes(pk_arr) else {
        return false;
    };
    pk.verify(msg, &sig_arr, SIG_CTX)
}

fn verify_slhdsa(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let Ok(pk_arr): Result<[u8; SLHDSA_PUBKEY_LEN], _> = pubkey.try_into() else {
        return false;
    };
    let Ok(sig_arr): Result<[u8; SLHDSA_SIG_LEN], _> = sig.try_into() else {
        return false;
    };
    let Ok(pk) = slhdsa::PublicKey::try_from_bytes(&pk_arr) else {
        return false;
    };
    pk.verify(msg, &sig_arr, SIG_CTX)
}

/// Address payload: first 20 bytes of SHA3-256 over a tagged pubkey.
/// Scheme-independent: the output's scheme byte selects the verifier, and the
/// sighash commits to that byte, so a signature cannot be replayed across
/// schemes.
pub fn pubkey_hash(pubkey: &[u8]) -> [u8; 20] {
    let h = sha3(&[b"sump/pkh/v1", pubkey]);
    h.0[..20].try_into().unwrap()
}

pub mod address {
    use sump_core::tx::SigScheme;
    use bech32::{Bech32m, Hrp};

    /// Address version bytes double as scheme selectors.
    pub const VERSION_MLDSA: u8 = 0;
    pub const VERSION_SLHDSA: u8 = 1;

    pub fn version_for(scheme: SigScheme) -> u8 {
        scheme.id()
    }

    pub fn scheme_for_version(version: u8) -> Option<SigScheme> {
        SigScheme::from_id(version)
    }

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
    fn deterministic_keygen_both_schemes() {
        for scheme in [SigScheme::MlDsa, SigScheme::SlhDsa] {
            let a = Keypair::from_seed(scheme, &[7u8; 32]);
            let b = Keypair::from_seed(scheme, &[7u8; 32]);
            assert_eq!(a.public, b.public);
            let c = Keypair::from_seed(scheme, &[8u8; 32]);
            assert_ne!(a.public, c.public);
        }
        assert_eq!(
            Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]).public.len(),
            MLDSA_PUBKEY_LEN
        );
        assert_eq!(
            Keypair::from_seed(SigScheme::SlhDsa, &[1u8; 32]).public.len(),
            SLHDSA_PUBKEY_LEN
        );
    }

    #[test]
    fn sign_verify_roundtrip_mldsa() {
        let kp = Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]);
        let msg = [42u8; 32];
        let sig = kp.sign(&msg);
        assert_eq!(sig.len(), MLDSA_SIG_LEN);
        assert!(verify(SigScheme::MlDsa, &kp.public, &msg, &sig));
        assert!(!verify(SigScheme::MlDsa, &kp.public, &[43u8; 32], &sig));
        let mut bad = sig.clone();
        bad[0] ^= 1;
        assert!(!verify(SigScheme::MlDsa, &kp.public, &msg, &bad));
    }

    #[test]
    fn sign_verify_roundtrip_slhdsa() {
        let kp = Keypair::from_seed(SigScheme::SlhDsa, &[1u8; 32]);
        let msg = [42u8; 32];
        let sig = kp.sign(&msg);
        assert_eq!(sig.len(), SLHDSA_SIG_LEN);
        assert!(verify(SigScheme::SlhDsa, &kp.public, &msg, &sig));
        assert!(!verify(SigScheme::SlhDsa, &kp.public, &[43u8; 32], &sig));
        let mut bad = sig.clone();
        bad[100] ^= 1;
        assert!(!verify(SigScheme::SlhDsa, &kp.public, &msg, &bad));
    }

    #[test]
    fn cross_scheme_verify_fails() {
        // an SLH-DSA signature must not verify as ML-DSA (and vice versa)
        let ml = Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]);
        let slh = Keypair::from_seed(SigScheme::SlhDsa, &[1u8; 32]);
        let msg = [9u8; 32];
        assert!(!verify(SigScheme::MlDsa, &ml.public, &msg, &slh.sign(&msg)));
        assert!(!verify(SigScheme::SlhDsa, &slh.public, &msg, &ml.sign(&msg)));
    }

    #[test]
    fn address_roundtrip_both_versions() {
        for (scheme, ver) in [
            (SigScheme::MlDsa, address::VERSION_MLDSA),
            (SigScheme::SlhDsa, address::VERSION_SLHDSA),
        ] {
            let pkh = [5u8; 20];
            let s = address::encode("sumprt", ver, &pkh);
            let (v, back) = address::decode("sumprt", &s).unwrap();
            assert_eq!(v, ver);
            assert_eq!(back, pkh);
            assert_eq!(address::scheme_for_version(v).unwrap(), scheme);
            assert!(address::decode("sump", &s).is_none());
        }
    }
}

//! Node-agnostic wallet: a master seed, deterministically derived keys in
//! both signature schemes (ML-DSA for everyday addresses, SLH-DSA for
//! vaults), and transaction building/signing from a supplied set of the
//! wallet's own unspent outputs (fetched from a node — this crate never
//! touches chain state directly, so it works for both the bundled node and
//! the standalone light wallet).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use sump_core::hash::sha3;
use sump_core::tx::{
    Lock, OutPoint, SigScheme, Transaction, TxBody, TxInput, TxOutput, Witness,
};
use sump_crypto::{address, Keypair};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalletError {
    #[error("wallet file already exists: {0}")]
    Exists(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("bad wallet file: {0}")]
    Parse(String),
    #[error("insufficient funds: have {have} stanzas spendable, need {need}")]
    Insufficient { have: u64, need: u64 },
    #[error("amount overflow")]
    Overflow,
    #[error("missing key for a selected input")]
    MissingKey,
}

type Result<T> = std::result::Result<T, WalletError>;

pub const SCHEMES: [SigScheme; 2] = [SigScheme::MlDsa, SigScheme::SlhDsa];

#[derive(Serialize, Deserialize)]
pub struct WalletFile {
    pub network: String,
    pub seed_hex: String,
    pub next_index: u32,
}

pub struct Wallet {
    pub file: WalletFile,
    seed: [u8; 32],
}

/// One of the wallet's own unspent outputs, as reported by a node.
#[derive(Clone, Debug)]
pub struct OwnedUtxo {
    pub outpoint: OutPoint,
    pub output: TxOutput,
    pub coinbase: bool,
    pub height: u64,
}

impl Wallet {
    pub fn create(path: &Path, network: &str) -> Result<Wallet> {
        if path.exists() {
            return Err(WalletError::Exists(path.display().to_string()));
        }
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| WalletError::Io(e.to_string()))?;
        let w = Wallet {
            file: WalletFile {
                network: network.to_string(),
                seed_hex: hex::encode(seed),
                next_index: 1,
            },
            seed,
        };
        w.save(path)?;
        Ok(w)
    }

    /// Restore a wallet from a 32-byte seed (hex).
    pub fn restore(path: &Path, network: &str, seed_hex: &str) -> Result<Wallet> {
        if path.exists() {
            return Err(WalletError::Exists(path.display().to_string()));
        }
        let seed: [u8; 32] = hex::decode(seed_hex.trim())
            .map_err(|e| WalletError::Parse(e.to_string()))?
            .try_into()
            .map_err(|_| WalletError::Parse("seed must be 32 bytes".into()))?;
        let w = Wallet {
            file: WalletFile {
                network: network.to_string(),
                seed_hex: hex::encode(seed),
                next_index: 1,
            },
            seed,
        };
        w.save(path)?;
        Ok(w)
    }

    pub fn load(path: &Path) -> Result<Wallet> {
        let raw = std::fs::read_to_string(path).map_err(|e| WalletError::Io(e.to_string()))?;
        let file: WalletFile =
            serde_json::from_str(&raw).map_err(|e| WalletError::Parse(e.to_string()))?;
        let seed: [u8; 32] = hex::decode(&file.seed_hex)
            .map_err(|e| WalletError::Parse(e.to_string()))?
            .try_into()
            .map_err(|_| WalletError::Parse("seed must be 32 bytes".into()))?;
        Ok(Wallet { file, seed })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.file)
            .map_err(|e| WalletError::Parse(e.to_string()))?;
        std::fs::write(path, json).map_err(|e| WalletError::Io(e.to_string()))
    }

    pub fn seed_hex(&self) -> String {
        hex::encode(self.seed)
    }

    fn key_seed(&self, scheme: SigScheme, index: u32) -> [u8; 32] {
        sha3(&[
            b"sump/wallet/key/v1",
            &self.seed,
            &[scheme.id()],
            &index.to_le_bytes(),
        ])
        .0
    }

    pub fn key(&self, scheme: SigScheme, index: u32) -> Keypair {
        Keypair::from_seed(scheme, &self.key_seed(scheme, index))
    }

    /// All owned keys across both schemes and every derived index.
    pub fn keys(&self) -> Vec<Keypair> {
        let mut out = Vec::new();
        for scheme in SCHEMES {
            for i in 0..self.file.next_index {
                out.push(self.key(scheme, i));
            }
        }
        out
    }

    /// Map (scheme id, pkh) -> owning key, for ownership tests and signing.
    pub fn owned(&self) -> HashMap<(u8, [u8; 20]), Keypair> {
        let mut m = HashMap::new();
        for kp in self.keys() {
            m.insert((kp.scheme.id(), kp.pubkey_hash()), kp);
        }
        m
    }

    /// The (scheme, pkh) pairs this wallet controls — send these to a node to
    /// query balances.
    pub fn owned_ids(&self) -> Vec<(u8, [u8; 20])> {
        self.keys()
            .iter()
            .map(|k| (k.scheme.id(), k.pubkey_hash()))
            .collect()
    }

    pub fn address(&self, hrp: &str, index: u32) -> String {
        let kp = self.key(SigScheme::MlDsa, index);
        address::encode(hrp, address::VERSION_MLDSA, &kp.pubkey_hash())
    }

    pub fn vault_address(&self, hrp: &str, index: u32) -> String {
        let kp = self.key(SigScheme::SlhDsa, index);
        address::encode(hrp, address::VERSION_SLHDSA, &kp.pubkey_hash())
    }
}

fn spendable(u: &OwnedUtxo, next_height: u64, maturity: u64) -> bool {
    let mature = !u.coinbase || next_height >= u.height + maturity;
    let unlocked = match u.output.lock {
        Lock::Timelock { height, .. } => next_height >= height,
        Lock::P2pkh { .. } => true,
    };
    mature && unlocked
}

/// (spendable, pending) totals in stanzas over the wallet's owned UTXOs.
pub fn balances(utxos: &[OwnedUtxo], next_height: u64, maturity: u64) -> (u64, u64) {
    let (mut s, mut total) = (0u64, 0u64);
    for u in utxos {
        total += u.output.amount;
        if spendable(u, next_height, maturity) {
            s += u.output.amount;
        }
    }
    (s, total - s)
}

/// Build and sign a payment from the wallet's owned UTXOs. Selects greedily
/// (largest first); change returns to the wallet's index-0 everyday address.
#[allow(clippy::too_many_arguments)]
pub fn build_send(
    wallet: &Wallet,
    utxos: &[OwnedUtxo],
    next_height: u64,
    maturity: u64,
    to_scheme: SigScheme,
    to_pkh: [u8; 20],
    amount: u64,
    fee: u64,
) -> Result<Transaction> {
    let owned = wallet.owned();

    let mut candidates: Vec<&OwnedUtxo> = utxos
        .iter()
        .filter(|u| spendable(u, next_height, maturity))
        .filter(|u| owned.contains_key(&(u.output.lock.scheme().id(), *u.output.lock.pkh())))
        .collect();
    candidates.sort_by_key(|c| std::cmp::Reverse(c.output.amount));

    let need = amount.checked_add(fee).ok_or(WalletError::Overflow)?;
    let mut selected: Vec<&OwnedUtxo> = Vec::new();
    let mut total = 0u64;
    for c in candidates {
        if total >= need {
            break;
        }
        total += c.output.amount;
        selected.push(c);
    }
    if total < need {
        return Err(WalletError::Insufficient { have: total, need });
    }

    let change = total - need;
    let mut outputs = vec![TxOutput {
        amount,
        lock: Lock::P2pkh {
            scheme: to_scheme,
            pkh: to_pkh,
        },
    }];
    if change > 0 {
        outputs.push(TxOutput {
            amount: change,
            lock: Lock::P2pkh {
                scheme: SigScheme::MlDsa,
                pkh: wallet.key(SigScheme::MlDsa, 0).pubkey_hash(),
            },
        });
    }

    let body = TxBody {
        version: 1,
        inputs: selected
            .iter()
            .map(|u| TxInput {
                prevout: u.outpoint,
            })
            .collect(),
        outputs,
        locktime: 0,
        coinbase_data: vec![],
    };

    let mut witnesses = Vec::with_capacity(selected.len());
    for (i, u) in selected.iter().enumerate() {
        let key = owned
            .get(&(u.output.lock.scheme().id(), *u.output.lock.pkh()))
            .ok_or(WalletError::MissingKey)?;
        let sighash = body.sighash(i as u32, &u.output);
        witnesses.push(Witness {
            pubkey: key.public.clone(),
            signature: key.sign(&sighash.0),
        });
    }

    Ok(Transaction { body, witnesses })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_deterministic_and_versioned() {
        // a wallet restored from the same seed yields identical addresses —
        // this is the invariant that lets the standalone wallet and the
        // bundled node agree on a wallet.json
        let dir = std::env::temp_dir();
        let p1 = dir.join(format!("w1-{}.json", std::process::id()));
        let p2 = dir.join(format!("w2-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_file(&p2);
        let seed = "11".repeat(32);
        let a = Wallet::restore(&p1, "mainnet", &seed).unwrap();
        let b = Wallet::restore(&p2, "mainnet", &seed).unwrap();
        assert_eq!(a.address("sump", 0), b.address("sump", 0));
        assert_eq!(a.vault_address("sump", 0), b.vault_address("sump", 0));
        assert!(a.address("sump", 0).starts_with("sump1q"));
        assert!(a.vault_address("sump", 0) != a.address("sump", 0));
        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_file(&p2);
    }

    #[test]
    fn balances_split_mature_and_immature() {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("w3-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let w = Wallet::restore(&p, "regtest", &"22".repeat(32)).unwrap();
        let pkh = w.key(SigScheme::MlDsa, 0).pubkey_hash();
        let mk = |amount, coinbase, height| OwnedUtxo {
            outpoint: OutPoint {
                txid: sha3(&[b"x", &[height as u8]]),
                vout: 0,
            },
            output: TxOutput {
                amount,
                lock: Lock::P2pkh {
                    scheme: SigScheme::MlDsa,
                    pkh,
                },
            },
            coinbase,
            height,
        };
        // next_height 10, maturity 5: coinbase at height 7 is immature (10<12),
        // coinbase at height 3 is mature (10>=8)
        let utxos = vec![mk(100, true, 7), mk(50, true, 3), mk(10, false, 9)];
        let (spend, pending) = balances(&utxos, 10, 5);
        assert_eq!(spend, 60); // 50 mature coinbase + 10 non-coinbase
        assert_eq!(pending, 100); // immature coinbase
        let _ = std::fs::remove_file(&p);
    }
}

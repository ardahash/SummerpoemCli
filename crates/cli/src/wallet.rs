//! File-backed wallet: a master seed, deterministically derived keys in both
//! signature schemes (ML-DSA for everyday addresses, SLH-DSA for vaults).

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use sump_core::hash::sha3;
use sump_core::params::Params;
use sump_core::tx::{Lock, OutPoint, SigScheme, Transaction, TxBody, TxInput, TxOutput, Witness};
use sump_crypto::{address, Keypair};
use sump_node::ChainState;

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

const SCHEMES: [SigScheme; 2] = [SigScheme::MlDsa, SigScheme::SlhDsa];

impl Wallet {
    pub fn create(path: &Path, network: &str) -> Result<Wallet> {
        if path.exists() {
            bail!("wallet file already exists: {}", path.display());
        }
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| anyhow!("rng failure: {e}"))?;
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
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading wallet {}", path.display()))?;
        let file: WalletFile = serde_json::from_str(&raw).context("parsing wallet json")?;
        let seed_vec = hex::decode(&file.seed_hex).context("wallet seed hex")?;
        let seed: [u8; 32] = seed_vec
            .try_into()
            .map_err(|_| anyhow!("wallet seed must be 32 bytes"))?;
        Ok(Wallet { file, seed })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.file)?;
        std::fs::write(path, json)?;
        Ok(())
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

    /// Map (scheme, pkh) -> owning key, for ownership tests and signing.
    fn owned(&self) -> HashMap<(u8, [u8; 20]), Keypair> {
        let mut m = HashMap::new();
        for kp in self.keys() {
            m.insert((kp.scheme.id(), kp.pubkey_hash()), kp);
        }
        m
    }

    /// Everyday receiving address (ML-DSA), index 0.
    pub fn address(&self, params: &Params, index: u32) -> String {
        let kp = self.key(SigScheme::MlDsa, index);
        address::encode(params.address_hrp, address::VERSION_MLDSA, &kp.pubkey_hash())
    }

    /// Vault receiving address (SLH-DSA, hash-based), index 0.
    pub fn vault_address(&self, params: &Params, index: u32) -> String {
        let kp = self.key(SigScheme::SlhDsa, index);
        address::encode(
            params.address_hrp,
            address::VERSION_SLHDSA,
            &kp.pubkey_hash(),
        )
    }

    fn owns(&self, lock: &Lock, owned: &HashMap<(u8, [u8; 20]), Keypair>) -> bool {
        owned.contains_key(&(lock.scheme().id(), *lock.pkh()))
    }

    /// Spendable (mature, non-timelocked) confirmed balance in stanzas.
    pub fn balance(&self, state: &ChainState) -> u64 {
        let owned = self.owned();
        let next_height = state.height() + 1;
        let maturity = state.params().coinbase_maturity;
        state
            .utxos()
            .values()
            .filter(|u| self.owns(&u.output.lock, &owned))
            .filter(|u| !u.coinbase || next_height >= u.height + maturity)
            .filter(|u| match u.output.lock {
                Lock::Timelock { height, .. } => next_height >= height,
                Lock::P2pkh { .. } => true,
            })
            .map(|u| u.output.amount)
            .sum()
    }

    /// Build and sign a payment. Selects UTXOs greedily (largest first). The
    /// output scheme is taken from the recipient address version.
    pub fn build_send(
        &self,
        state: &ChainState,
        to_scheme: SigScheme,
        to_pkh: [u8; 20],
        amount: u64,
        fee: u64,
    ) -> Result<Transaction> {
        let owned = self.owned();
        let next_height = state.height() + 1;
        let maturity = state.params().coinbase_maturity;

        // (outpoint, output) of spendable utxos we own
        let mut candidates: Vec<(OutPoint, TxOutput)> = Vec::new();
        for (op, u) in state.utxos() {
            if u.coinbase && next_height < u.height + maturity {
                continue;
            }
            if let Lock::Timelock { height, .. } = u.output.lock {
                if next_height < height {
                    continue;
                }
            }
            if self.owns(&u.output.lock, &owned) {
                candidates.push((*op, u.output.clone()));
            }
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c.1.amount));

        let need = amount
            .checked_add(fee)
            .ok_or_else(|| anyhow!("amount overflow"))?;
        let mut selected: Vec<(OutPoint, TxOutput)> = Vec::new();
        let mut total = 0u64;
        for c in candidates {
            if total >= need {
                break;
            }
            total += c.1.amount;
            selected.push(c);
        }
        if total < need {
            bail!(
                "insufficient funds: have {} stanzas spendable, need {}",
                total,
                need
            );
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
            // change returns to our everyday (ML-DSA) address
            outputs.push(TxOutput {
                amount: change,
                lock: Lock::P2pkh {
                    scheme: SigScheme::MlDsa,
                    pkh: self.key(SigScheme::MlDsa, 0).pubkey_hash(),
                },
            });
        }

        let body = TxBody {
            version: 1,
            inputs: selected
                .iter()
                .map(|(op, _)| TxInput { prevout: *op })
                .collect(),
            outputs,
            locktime: 0,
            coinbase_data: vec![],
        };

        let mut witnesses = Vec::with_capacity(selected.len());
        for (i, (_, prev_out)) in selected.iter().enumerate() {
            let key = owned
                .get(&(prev_out.lock.scheme().id(), *prev_out.lock.pkh()))
                .ok_or_else(|| anyhow!("missing key for selected input"))?;
            let sighash = body.sighash(i as u32, prev_out);
            witnesses.push(Witness {
                pubkey: key.public.clone(),
                signature: key.sign(&sighash.0),
            });
        }

        Ok(Transaction { body, witnesses })
    }
}

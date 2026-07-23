//! File-backed wallet: a master seed, sequentially derived ML-DSA keys.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use sump_core::hash::sha3;
use sump_core::params::Params;
use sump_core::tx::{Lock, OutPoint, Transaction, TxBody, TxInput, TxOutput, Witness};
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

    fn key_seed(&self, index: u32) -> [u8; 32] {
        sha3(&[b"sump/wallet/key/v1", &self.seed, &index.to_le_bytes()]).0
    }

    pub fn key(&self, index: u32) -> Keypair {
        Keypair::from_seed(&self.key_seed(index))
    }

    pub fn keys(&self) -> Vec<Keypair> {
        (0..self.file.next_index).map(|i| self.key(i)).collect()
    }

    pub fn address(&self, params: &Params, index: u32) -> String {
        address::encode(
            params.address_hrp,
            address::VERSION_MLDSA,
            &self.key(index).pubkey_hash(),
        )
    }

    /// Spendable (mature, non-timelocked) confirmed balance in stanzas.
    pub fn balance(&self, state: &ChainState) -> u64 {
        let ours: Vec<[u8; 20]> = self.keys().iter().map(|k| k.pubkey_hash()).collect();
        let next_height = state.height() + 1;
        let maturity = state.params().coinbase_maturity;
        state
            .utxos()
            .values()
            .filter(|u| ours.contains(u.output.lock.pkh()))
            .filter(|u| !u.coinbase || next_height >= u.height + maturity)
            .filter(|u| match u.output.lock {
                Lock::Timelock { height, .. } => next_height >= height,
                Lock::P2pkh { .. } => true,
            })
            .map(|u| u.output.amount)
            .sum()
    }

    /// Build and sign a payment. Selects UTXOs greedily (largest first).
    pub fn build_send(
        &self,
        state: &ChainState,
        to_pkh: [u8; 20],
        amount: u64,
        fee: u64,
    ) -> Result<Transaction> {
        let keys = self.keys();
        let next_height = state.height() + 1;
        let maturity = state.params().coinbase_maturity;

        // (outpoint, output, key index) of spendable utxos we own
        let mut candidates: Vec<(OutPoint, TxOutput, usize)> = Vec::new();
        for (op, u) in state.utxos() {
            if u.coinbase && next_height < u.height + maturity {
                continue;
            }
            if let Lock::Timelock { height, .. } = u.output.lock {
                if next_height < height {
                    continue;
                }
            }
            if let Some(ki) = keys
                .iter()
                .position(|k| k.pubkey_hash() == *u.output.lock.pkh())
            {
                candidates.push((*op, u.output.clone(), ki));
            }
        }
        candidates.sort_by_key(|c| std::cmp::Reverse(c.1.amount));

        let need = amount
            .checked_add(fee)
            .ok_or_else(|| anyhow!("amount overflow"))?;
        let mut selected: Vec<(OutPoint, TxOutput, usize)> = Vec::new();
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
            lock: Lock::P2pkh { pkh: to_pkh },
        }];
        if change > 0 {
            outputs.push(TxOutput {
                amount: change,
                lock: Lock::P2pkh {
                    pkh: keys[0].pubkey_hash(),
                },
            });
        }

        let body = TxBody {
            version: 1,
            inputs: selected
                .iter()
                .map(|(op, _, _)| TxInput { prevout: *op })
                .collect(),
            outputs,
            locktime: 0,
            coinbase_data: vec![],
        };

        let mut witnesses = Vec::with_capacity(selected.len());
        for (i, (_, prev_out, ki)) in selected.iter().enumerate() {
            let sighash = body.sighash(i as u32, prev_out);
            let key = &keys[*ki];
            witnesses.push(Witness {
                pubkey: key.public.clone(),
                signature: key.sign(&sighash.0),
            });
        }

        Ok(Transaction { body, witnesses })
    }
}

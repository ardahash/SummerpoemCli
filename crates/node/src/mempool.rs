//! In-memory pool of validated, unconfirmed transactions.

use crate::chain::ChainState;
use crate::error::ValidationError;
use std::collections::HashMap;
use sump_core::block::Block;
use sump_core::hash::Hash256;
use sump_core::tx::{OutPoint, Transaction};
use thiserror::Error;

/// Anti-flood limits: a transaction must pay at least 1 stanza per byte
/// (floored), and the pool is capped by total size — free/underpriced
/// transactions cannot exhaust a node's memory.
pub const MIN_RELAY_FEE: u64 = 1_000;
pub const MIN_FEE_PER_BYTE: u64 = 1;
pub const MAX_MEMPOOL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum MempoolError {
    #[error("transaction already in mempool")]
    Duplicate,
    #[error("transaction conflicts with a mempool transaction")]
    Conflict,
    #[error("fee too low (need at least {need} stanzas)")]
    LowFee { need: u64 },
    #[error("mempool is full")]
    Full,
    #[error("invalid transaction: {0}")]
    Invalid(#[from] ValidationError),
}

#[derive(Default)]
pub struct Mempool {
    txs: HashMap<Hash256, Transaction>,
    /// outpoint -> txid of the mempool tx spending it
    spends: HashMap<OutPoint, Hash256>,
    /// running total of encoded transaction sizes
    bytes: usize,
}

impl Mempool {
    pub fn new() -> Mempool {
        Mempool::default()
    }

    pub fn len(&self) -> usize {
        self.txs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    pub fn contains(&self, txid: &Hash256) -> bool {
        self.txs.contains_key(txid)
    }

    pub fn get(&self, txid: &Hash256) -> Option<&Transaction> {
        self.txs.get(txid)
    }

    pub fn transactions(&self) -> Vec<Transaction> {
        self.txs.values().cloned().collect()
    }

    /// Validate against the chain tip and admit. Returns the fee.
    pub fn insert(&mut self, chain: &ChainState, tx: Transaction) -> Result<u64, MempoolError> {
        let txid = tx.txid();
        if self.txs.contains_key(&txid) {
            return Err(MempoolError::Duplicate);
        }
        for input in &tx.body.inputs {
            if self.spends.contains_key(&input.prevout) {
                return Err(MempoolError::Conflict);
            }
        }
        let size = tx.size();
        // anti-flood: enforce a minimum fee before the (cheap) structural
        // checks pass but validation still runs to confirm the fee
        let fee = chain.validate_standalone_tx(&tx)?;
        let need = MIN_RELAY_FEE.max(size as u64 * MIN_FEE_PER_BYTE);
        if fee < need {
            return Err(MempoolError::LowFee { need });
        }
        if self.bytes + size > MAX_MEMPOOL_BYTES {
            return Err(MempoolError::Full);
        }
        for input in &tx.body.inputs {
            self.spends.insert(input.prevout, txid);
        }
        self.bytes += size;
        self.txs.insert(txid, tx);
        Ok(fee)
    }

    fn remove(&mut self, txid: &Hash256) {
        if let Some(tx) = self.txs.remove(txid) {
            self.bytes = self.bytes.saturating_sub(tx.size());
            for input in &tx.body.inputs {
                self.spends.remove(&input.prevout);
            }
        }
    }

    /// Drop transactions included in (or conflicting with) a connected block,
    /// then re-validate what remains against the new tip.
    pub fn update_for_block(&mut self, chain: &ChainState, block: &Block) {
        for tx in &block.transactions {
            self.remove(&tx.txid());
            // conflicts: anything spending an outpoint this block consumed
            for input in &tx.body.inputs {
                if let Some(conflict) = self.spends.get(&input.prevout).cloned() {
                    self.remove(&conflict);
                }
            }
        }
        // re-validate survivors (maturity/timelocks may have changed on reorg)
        let ids: Vec<Hash256> = self.txs.keys().cloned().collect();
        for id in ids {
            let valid = match self.txs.get(&id) {
                Some(tx) => chain.validate_standalone_tx(tx).is_ok(),
                None => true,
            };
            if !valid {
                self.remove(&id);
            }
        }
    }
}

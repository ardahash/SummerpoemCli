//! In-memory pool of validated, unconfirmed transactions.

use crate::chain::ChainState;
use crate::error::ValidationError;
use std::collections::HashMap;
use sump_core::block::Block;
use sump_core::hash::Hash256;
use sump_core::tx::{OutPoint, Transaction};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MempoolError {
    #[error("transaction already in mempool")]
    Duplicate,
    #[error("transaction conflicts with a mempool transaction")]
    Conflict,
    #[error("invalid transaction: {0}")]
    Invalid(#[from] ValidationError),
}

#[derive(Default)]
pub struct Mempool {
    txs: HashMap<Hash256, Transaction>,
    /// outpoint -> txid of the mempool tx spending it
    spends: HashMap<OutPoint, Hash256>,
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
        let fee = chain.validate_standalone_tx(&tx)?;
        for input in &tx.body.inputs {
            self.spends.insert(input.prevout, txid);
        }
        self.txs.insert(txid, tx);
        Ok(fee)
    }

    fn remove(&mut self, txid: &Hash256) {
        if let Some(tx) = self.txs.remove(txid) {
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

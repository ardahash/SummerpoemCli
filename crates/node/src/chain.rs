//! In-memory chain state with full validation.
//!
//! Reorg model (MVP): headers are validated contextually at insertion; when a
//! new best tip appears, the transaction level of the whole active chain is
//! re-applied from genesis. Simple and obviously correct; optimize later.

use crate::error::ValidationError;
use primitive_types::U256;
use std::collections::HashMap;
use std::sync::Arc;
use sump_core::asert;
use sump_core::block::Block;
use sump_core::compact::{bits_to_target, target_to_bits};
use sump_core::emission::block_reward;
use sump_core::hash::Hash256;
use sump_core::params::Params;
use sump_core::tx::{OutPoint, Lock, Transaction, TxOutput};
use sump_pow::{epoch_of_height, meets_target, PowContext};

#[derive(Clone, Debug)]
pub struct Utxo {
    pub output: TxOutput,
    pub height: u64,
    pub coinbase: bool,
}

struct BlockMeta {
    height: u64,
    cum_work: U256,
    time: u64,
    parent: Hash256,
}

pub struct ChainState {
    params: Params,
    all: HashMap<Hash256, Arc<Block>>,
    meta: HashMap<Hash256, BlockMeta>,
    active: Vec<Hash256>,
    utxos: HashMap<OutPoint, Utxo>,
    supply: u64,
    genesis_hash: Hash256,
    pow_light: HashMap<u64, Arc<PowContext>>,
}

fn work_from_target(target: U256) -> U256 {
    // ~2^256 / (target + 1); targets are capped at pow_limit < U256::MAX,
    // so the +1 cannot overflow.
    U256::MAX / (target + 1)
}

impl ChainState {
    pub fn new(params: Params, genesis: Block) -> Result<ChainState, ValidationError> {
        let ghash = genesis.header.hash();
        let mut state = ChainState {
            params,
            all: HashMap::new(),
            meta: HashMap::new(),
            active: Vec::new(),
            utxos: HashMap::new(),
            supply: 0,
            genesis_hash: ghash,
            pow_light: HashMap::new(),
        };
        state.validate_genesis(&genesis)?;
        let target = bits_to_target(genesis.header.bits).ok_or(ValidationError::BadBits)?;
        state.meta.insert(
            ghash,
            BlockMeta {
                height: 0,
                cum_work: work_from_target(target),
                time: genesis.header.time,
                parent: Hash256::ZERO,
            },
        );
        state.all.insert(ghash, Arc::new(genesis));
        state.active.push(ghash);
        let (utxos, supply) = state.apply_chain(&state.active)?;
        state.utxos = utxos;
        state.supply = supply;
        Ok(state)
    }

    fn validate_genesis(&mut self, genesis: &Block) -> Result<(), ValidationError> {
        if genesis.header.prev != Hash256::ZERO {
            return Err(ValidationError::BadGenesis);
        }
        let expected_bits = target_to_bits(self.params.pow_limit);
        if genesis.header.bits != expected_bits {
            return Err(ValidationError::WrongBits {
                expected: expected_bits,
                got: genesis.header.bits,
            });
        }
        self.check_pow(&genesis.header, 0)?;
        check_roots(genesis)?;
        if genesis.size() > self.params.max_block_size {
            return Err(ValidationError::TooLarge);
        }
        Ok(())
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    pub fn genesis_hash(&self) -> Hash256 {
        self.genesis_hash
    }

    pub fn tip_hash(&self) -> Hash256 {
        *self.active.last().unwrap()
    }

    pub fn height(&self) -> u64 {
        (self.active.len() - 1) as u64
    }

    pub fn supply(&self) -> u64 {
        self.supply
    }

    pub fn utxos(&self) -> &HashMap<OutPoint, Utxo> {
        &self.utxos
    }

    pub fn block_at(&self, height: u64) -> Option<Arc<Block>> {
        let h = self.active.get(height as usize)?;
        self.all.get(h).cloned()
    }

    /// Any known block (active or side chain) by hash.
    pub fn block_by_hash(&self, hash: &Hash256) -> Option<Arc<Block>> {
        self.all.get(hash).cloned()
    }

    pub fn contains_block(&self, hash: &Hash256) -> bool {
        self.all.contains_key(hash)
    }

    /// True if `hash` is part of the current active chain.
    pub fn is_active(&self, hash: &Hash256) -> bool {
        match self.meta.get(hash) {
            Some(m) => self.active.get(m.height as usize) == Some(hash),
            None => false,
        }
    }

    /// Bitcoin-style block locator: dense near the tip, exponentially
    /// sparser toward genesis, always ending with genesis.
    pub fn locator(&self) -> Vec<Hash256> {
        let mut out = Vec::new();
        let tip = self.height() as i64;
        let mut step: i64 = 1;
        let mut h = tip;
        while h > 0 {
            out.push(self.active[h as usize]);
            if out.len() >= 10 {
                step *= 2;
            }
            h -= step;
        }
        out.push(self.active[0]);
        out
    }

    /// Active-chain block hashes after the first locator entry we recognize
    /// (fork point), oldest first, up to `max`.
    pub fn hashes_after_locator(&self, locator: &[Hash256], max: usize) -> Vec<Hash256> {
        let mut start_height = 0u64; // default: after genesis
        for h in locator {
            if self.is_active(h) {
                start_height = self.meta[h].height;
                break;
            }
        }
        self.active
            .iter()
            .skip(start_height as usize + 1)
            .take(max)
            .cloned()
            .collect()
    }

    pub fn tip_header_time(&self) -> u64 {
        self.meta[&self.tip_hash()].time
    }

    pub fn active_blocks(&self) -> Vec<Arc<Block>> {
        self.active
            .iter()
            .map(|h| self.all[h].clone())
            .collect()
    }

    fn pow_context(&mut self, height: u64) -> Arc<PowContext> {
        let epoch = epoch_of_height(height, self.params.pow.epoch_length);
        if let Some(ctx) = self.pow_light.get(&epoch) {
            return ctx.clone();
        }
        let ctx = Arc::new(PowContext::new_light(&self.params.pow, epoch));
        self.pow_light.insert(epoch, ctx.clone());
        ctx
    }

    fn check_pow(&mut self, header: &sump_core::block::BlockHeader, height: u64) -> Result<(), ValidationError> {
        let target = bits_to_target(header.bits).ok_or(ValidationError::BadBits)?;
        if target > self.params.pow_limit {
            return Err(ValidationError::BadBits);
        }
        let ctx = self.pow_context(height);
        let h = ctx.compute(&header.pow_message(), header.nonce);
        if !meets_target(&h, target) {
            return Err(ValidationError::BadPow);
        }
        Ok(())
    }

    /// Expected difficulty bits for a block whose parent is `parent_hash`.
    pub fn expected_bits(&self, parent_hash: &Hash256) -> Result<u32, ValidationError> {
        let parent = self
            .meta
            .get(parent_hash)
            .ok_or(ValidationError::UnknownParent)?;
        let genesis = &self.meta[&self.genesis_hash];
        let time_diff = parent.time as i64 - genesis.time as i64;
        let target = asert::next_target(
            self.params.pow_limit,
            self.params.block_interval,
            self.params.asert_tau,
            time_diff,
            parent.height,
            self.params.pow_limit,
        );
        Ok(target_to_bits(target))
    }

    /// Median of the last (up to) 11 block times ending at `from`.
    pub fn median_time_past(&self, from: &Hash256) -> u64 {
        let mut times = Vec::with_capacity(11);
        let mut cur = *from;
        for _ in 0..11 {
            let Some(m) = self.meta.get(&cur) else { break };
            times.push(m.time);
            if m.parent == Hash256::ZERO {
                break;
            }
            cur = m.parent;
        }
        times.sort_unstable();
        times[times.len() / 2]
    }

    /// Insert a block. Returns true if the active tip changed.
    pub fn add_block(&mut self, block: Block) -> Result<bool, ValidationError> {
        let hash = block.header.hash();
        if self.all.contains_key(&hash) {
            return Err(ValidationError::Duplicate);
        }
        let parent = self
            .meta
            .get(&block.header.prev)
            .ok_or(ValidationError::UnknownParent)?;
        let height = parent.height + 1;
        let parent_cum = parent.cum_work;
        let parent_hash = block.header.prev;

        // contextual header checks (against its own parent chain)
        let expected = self.expected_bits(&parent_hash)?;
        if block.header.bits != expected {
            return Err(ValidationError::WrongBits {
                expected,
                got: block.header.bits,
            });
        }
        if block.header.time <= self.median_time_past(&parent_hash) {
            return Err(ValidationError::TimeTooOld);
        }
        if block.size() > self.params.max_block_size {
            return Err(ValidationError::TooLarge);
        }
        check_roots(&block)?;
        self.check_pow(&block.header, height)?;

        let target = bits_to_target(block.header.bits).unwrap();
        let cum_work = parent_cum + work_from_target(target);
        self.meta.insert(
            hash,
            BlockMeta {
                height,
                cum_work,
                time: block.header.time,
                parent: parent_hash,
            },
        );
        self.all.insert(hash, Arc::new(block));

        let tip_hash = self.tip_hash();
        let tip_work = self.meta[&tip_hash].cum_work;
        if cum_work <= tip_work {
            return Ok(false); // side chain, kept for later
        }

        // fast path: the block extends the current tip — apply incrementally
        if parent_hash == tip_hash {
            let block = self.all[&hash].clone();
            let mut utxos = self.utxos.clone();
            match validate_block_txs(&self.params, &mut utxos, &block, height) {
                Ok(minted) => {
                    let supply = self
                        .supply
                        .checked_add(minted)
                        .ok_or(ValidationError::Overflow)?;
                    self.active.push(hash);
                    self.utxos = utxos;
                    self.supply = supply;
                    return Ok(true);
                }
                Err(e) => {
                    self.meta.remove(&hash);
                    self.all.remove(&hash);
                    return Err(e);
                }
            }
        }

        // reorg: walk back to genesis and re-apply the whole candidate chain
        let mut chain = Vec::with_capacity(height as usize + 1);
        let mut cur = hash;
        loop {
            chain.push(cur);
            let m = &self.meta[&cur];
            if m.parent == Hash256::ZERO {
                break;
            }
            cur = m.parent;
        }
        chain.reverse();

        match self.apply_chain(&chain) {
            Ok((utxos, supply)) => {
                self.active = chain;
                self.utxos = utxos;
                self.supply = supply;
                Ok(true)
            }
            Err(e) => {
                self.meta.remove(&hash);
                self.all.remove(&hash);
                Err(e)
            }
        }
    }

    /// Transaction-level validation and application of a full chain.
    fn apply_chain(
        &self,
        chain: &[Hash256],
    ) -> Result<(HashMap<OutPoint, Utxo>, u64), ValidationError> {
        let mut utxos: HashMap<OutPoint, Utxo> = HashMap::new();
        let mut supply: u64 = 0;
        for (height, hash) in chain.iter().enumerate() {
            let block = self.all[hash].clone();
            let minted =
                validate_block_txs(&self.params, &mut utxos, &block, height as u64)?;
            supply = supply.checked_add(minted).ok_or(ValidationError::Overflow)?;
        }
        Ok((utxos, supply))
    }

    /// Validate a transaction against the current tip state (mempool check).
    /// Returns the fee it pays.
    pub fn validate_standalone_tx(&self, tx: &Transaction) -> Result<u64, ValidationError> {
        let next_height = self.height() + 1;
        if tx.body.is_coinbase() {
            return Err(ValidationError::NoInputs);
        }
        let mut spent: Vec<OutPoint> = Vec::new();
        check_tx(
            &self.params,
            tx,
            next_height,
            |op| {
                if spent.contains(op) {
                    return None;
                }
                spent.push(*op);
                self.utxos.get(op).cloned()
            },
        )
    }
}

fn check_roots(block: &Block) -> Result<(), ValidationError> {
    if block.header.tx_root != block.compute_tx_root() {
        return Err(ValidationError::BadMerkleRoot);
    }
    if block.header.witness_root != block.compute_witness_root() {
        return Err(ValidationError::BadWitnessRoot);
    }
    Ok(())
}

/// Validate one non-coinbase transaction given a UTXO lookup; returns its fee.
fn check_tx(
    params: &Params,
    tx: &Transaction,
    height: u64,
    mut lookup: impl FnMut(&OutPoint) -> Option<Utxo>,
) -> Result<u64, ValidationError> {
    if tx.body.inputs.is_empty() {
        return Err(ValidationError::NoInputs);
    }
    if tx.body.outputs.is_empty() {
        return Err(ValidationError::NoOutputs);
    }
    if !tx.body.coinbase_data.is_empty() {
        return Err(ValidationError::BadCoinbaseData);
    }
    if tx.witnesses.len() != tx.body.inputs.len() {
        return Err(ValidationError::WitnessMismatch);
    }
    // no outpoint may be spent twice within one transaction
    for (i, a) in tx.body.inputs.iter().enumerate() {
        if tx.body.inputs[..i].iter().any(|b| b.prevout == a.prevout) {
            return Err(ValidationError::UnknownInput(a.prevout));
        }
    }
    let mut in_total: u64 = 0;
    for (i, input) in tx.body.inputs.iter().enumerate() {
        let utxo = lookup(&input.prevout)
            .ok_or(ValidationError::UnknownInput(input.prevout))?;
        if utxo.coinbase && height < utxo.height + params.coinbase_maturity {
            return Err(ValidationError::ImmatureCoinbase);
        }
        if let Lock::Timelock { height: lock_h, .. } = utxo.output.lock {
            if height < lock_h {
                return Err(ValidationError::Timelocked(lock_h));
            }
        }
        let w = &tx.witnesses[i];
        if sump_crypto::pubkey_hash(&w.pubkey) != *utxo.output.lock.pkh() {
            return Err(ValidationError::WrongPubkey);
        }
        // the output's scheme byte (committed to by the sighash) selects the
        // verifier, so a signature cannot be replayed across schemes
        let sighash = tx.body.sighash(i as u32, &utxo.output);
        if !sump_crypto::verify(utxo.output.lock.scheme(), &w.pubkey, &sighash.0, &w.signature) {
            return Err(ValidationError::BadSignature);
        }
        in_total = in_total
            .checked_add(utxo.output.amount)
            .ok_or(ValidationError::Overflow)?;
    }
    let mut out_total: u64 = 0;
    for o in &tx.body.outputs {
        if o.amount == 0 {
            return Err(ValidationError::ZeroOutput);
        }
        out_total = out_total
            .checked_add(o.amount)
            .ok_or(ValidationError::Overflow)?;
    }
    if in_total < out_total {
        return Err(ValidationError::InsufficientInputs);
    }
    Ok(in_total - out_total)
}

/// Validate all transactions of a block and apply them to `utxos`.
/// Returns the amount actually minted by the coinbase.
fn validate_block_txs(
    params: &Params,
    utxos: &mut HashMap<OutPoint, Utxo>,
    block: &Block,
    height: u64,
) -> Result<u64, ValidationError> {
    let txs = &block.transactions;
    if txs.is_empty() || !txs[0].body.is_coinbase() {
        return Err(ValidationError::MissingCoinbase);
    }
    // coinbase structure
    let cb = &txs[0];
    if !cb.witnesses.is_empty() {
        return Err(ValidationError::WitnessMismatch);
    }
    if cb.body.outputs.is_empty() {
        return Err(ValidationError::NoOutputs);
    }
    let data = &cb.body.coinbase_data;
    if data.len() < 8
        || data.len() > 8 + sump_core::tx::MAX_COINBASE_EXTRA
        || u64::from_le_bytes(data[..8].try_into().unwrap()) != height
    {
        return Err(ValidationError::BadCoinbaseData);
    }

    // duplicate-tx guard
    let mut seen = std::collections::HashSet::new();
    for tx in txs {
        if !seen.insert(tx.txid()) {
            return Err(ValidationError::DuplicateTx);
        }
    }

    let mut fees: u64 = 0;
    for tx in &txs[1..] {
        if tx.body.is_coinbase() {
            return Err(ValidationError::ExtraCoinbase);
        }
        let fee = check_tx(params, tx, height, |op| utxos.get(op).cloned())?;
        fees = fees.checked_add(fee).ok_or(ValidationError::Overflow)?;
        // apply: spend inputs, add outputs
        for input in &tx.body.inputs {
            utxos.remove(&input.prevout);
        }
        let txid = tx.txid();
        for (vout, o) in tx.body.outputs.iter().enumerate() {
            utxos.insert(
                OutPoint {
                    txid,
                    vout: vout as u32,
                },
                Utxo {
                    output: o.clone(),
                    height,
                    coinbase: false,
                },
            );
        }
    }

    // coinbase amount check + apply
    let mut cb_total: u64 = 0;
    for o in &cb.body.outputs {
        if o.amount == 0 {
            return Err(ValidationError::ZeroOutput);
        }
        cb_total = cb_total.checked_add(o.amount).ok_or(ValidationError::Overflow)?;
    }
    let allowed = block_reward(height)
        .checked_add(fees)
        .ok_or(ValidationError::Overflow)?;
    if cb_total > allowed {
        return Err(ValidationError::CoinbaseOverpay);
    }
    let cb_txid = cb.txid();
    for (vout, o) in cb.body.outputs.iter().enumerate() {
        utxos.insert(
            OutPoint {
                txid: cb_txid,
                vout: vout as u32,
            },
            Utxo {
                output: o.clone(),
                height,
                coinbase: true,
            },
        );
    }
    // minted = coinbase outputs minus redistributed fees
    Ok(cb_total.saturating_sub(fees))
}

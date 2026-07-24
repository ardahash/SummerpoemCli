//! Adversarial consensus tests: every structural defect a peer could send in
//! a block must be rejected with the right error, and reorgs must roll the
//! UTXO set and mempool back correctly.

use sump_core::block::Block;
use sump_core::compact::bits_to_target;
use sump_core::hash::Hash256;
use sump_core::params::Params;
use sump_core::tx::{
    Lock, OutPoint, SigScheme, Transaction, TxBody, TxInput, TxOutput, Witness,
};
use sump_crypto::Keypair;
use sump_node::chain::ChainState;
use sump_node::genesis::build_genesis;
use sump_node::mempool::Mempool;
use sump_node::miner::{build_block_template, mine_block};
use sump_node::ValidationError;
use sump_pow::{meets_target, PowContext};

fn ml_key(seed: u8) -> Keypair {
    Keypair::from_seed(SigScheme::MlDsa, &[seed; 32])
}

struct H {
    state: ChainState,
    ctx: PowContext,
    key: Keypair,
    clock: u64,
}

impl H {
    fn new() -> H {
        let params = Params::regtest();
        let ctx = PowContext::new_full(&params.pow, 0);
        let genesis = build_genesis(&params, &ctx);
        let state = ChainState::new(params.clone(), genesis).unwrap();
        H {
            clock: params.genesis_time,
            state,
            ctx,
            key: ml_key(1),
        }
    }

    fn payout(&self) -> [u8; 20] {
        self.key.pubkey_hash()
    }

    /// A valid, mined block extending the tip (roots + PoW correct).
    fn valid_next(&mut self, txs: &[Transaction]) -> Block {
        self.clock += 60;
        let mut b = build_block_template(&self.state, txs, self.payout(), self.clock);
        assert!(mine_block(&self.ctx, &mut b, u64::MAX));
        b
    }

    fn mine(&mut self, txs: &[Transaction]) {
        let b = self.valid_next(txs);
        self.state.add_block(b).expect("valid block connects");
    }

    /// Re-compute roots and mine to a valid nonce after a mutation.
    fn refinalize(&self, b: &mut Block) {
        b.header.tx_root = b.compute_tx_root();
        b.header.witness_root = b.compute_witness_root();
        assert!(mine_block(&self.ctx, b, u64::MAX));
    }

    /// A nonce whose PoW does NOT meet the target (regtest target is easy, so
    /// most nonces fail; find one deterministically).
    fn failing_nonce(&self, b: &Block) -> u64 {
        let target = bits_to_target(b.header.bits).unwrap();
        let msg = b.header.pow_message();
        for n in 0..10_000u64 {
            if !meets_target(&self.ctx.compute(&msg, n), target) {
                return n;
            }
        }
        panic!("no failing nonce found");
    }

    fn coinbase_outpoint(&self, height: u64) -> (OutPoint, TxOutput) {
        let block = self.state.block_at(height).unwrap();
        let cb = &block.transactions[0];
        (
            OutPoint {
                txid: cb.txid(),
                vout: 0,
            },
            cb.body.outputs[0].clone(),
        )
    }

    fn spend(
        &self,
        from_height: u64,
        to_pkh: [u8; 20],
        amount: u64,
        fee: u64,
    ) -> Transaction {
        let (op, prev) = self.coinbase_outpoint(from_height);
        let mut outputs = vec![TxOutput {
            amount,
            lock: Lock::P2pkh {
                scheme: SigScheme::MlDsa,
                pkh: to_pkh,
            },
        }];
        let change = prev.amount - amount - fee;
        if change > 0 {
            outputs.push(TxOutput {
                amount: change,
                lock: Lock::P2pkh {
                    scheme: SigScheme::MlDsa,
                    pkh: self.payout(),
                },
            });
        }
        let body = TxBody {
            version: 1,
            inputs: vec![TxInput { prevout: op }],
            outputs,
            locktime: 0,
            coinbase_data: vec![],
        };
        let sig = self.key.sign(&body.sighash(0, &prev).0);
        Transaction {
            witnesses: vec![Witness {
                pubkey: self.key.public.clone(),
                signature: sig,
            }],
            body,
        }
    }
}

#[test]
fn rejects_bad_pow() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    b.header.nonce = h.failing_nonce(&b);
    assert!(matches!(h.state.add_block(b), Err(ValidationError::BadPow)));
}

#[test]
fn rejects_wrong_bits() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    // a valid but harder (different) target than the expected one
    let expected = bits_to_target(b.header.bits).unwrap();
    b.header.bits = sump_core::compact::target_to_bits(expected >> 1);
    h.refinalize(&mut b);
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::WrongBits { .. })
    ));
}

#[test]
fn rejects_bad_merkle_root() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    b.header.tx_root = Hash256([0xAB; 32]);
    // mine so PoW passes; merkle check (earlier) must still fail
    assert!(mine_block(&h.ctx, &mut b, u64::MAX));
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::BadMerkleRoot)
    ));
}

#[test]
fn rejects_bad_witness_root() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    b.header.witness_root = Hash256([0xCD; 32]);
    assert!(mine_block(&h.ctx, &mut b, u64::MAX));
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::BadWitnessRoot)
    ));
}

#[test]
fn rejects_timestamp_at_or_before_mtp() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    b.header.time = h.state.tip_header_time(); // == MTP at height 0
    h.refinalize(&mut b);
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::TimeTooOld)
    ));
}

#[test]
fn rejects_unknown_parent() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    b.header.prev = Hash256([0x99; 32]);
    h.refinalize(&mut b);
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::UnknownParent)
    ));
}

#[test]
fn rejects_duplicate_block() {
    let mut h = H::new();
    let b = h.valid_next(&[]);
    assert!(h.state.add_block(b.clone()).unwrap());
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::Duplicate)
    ));
}

#[test]
fn rejects_wrong_coinbase_height() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    // corrupt the LE64 height prefix in the coinbase data
    b.transactions[0].body.coinbase_data[0] ^= 0xff;
    h.refinalize(&mut b);
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::BadCoinbaseData)
    ));
}

#[test]
fn rejects_extra_coinbase() {
    let mut h = H::new();
    let mut b = h.valid_next(&[]);
    // a second input-less (coinbase-like) transaction is illegal
    b.transactions.push(Transaction {
        body: TxBody {
            version: 1,
            inputs: vec![],
            outputs: vec![TxOutput {
                amount: 1,
                lock: Lock::P2pkh {
                    scheme: SigScheme::MlDsa,
                    pkh: [2u8; 20],
                },
            }],
            locktime: 0,
            coinbase_data: vec![0u8; 8],
        },
        witnesses: vec![],
    });
    h.refinalize(&mut b);
    assert!(matches!(
        h.state.add_block(b),
        Err(ValidationError::ExtraCoinbase)
    ));
}

#[test]
fn reorg_rolls_back_utxos_and_conflicting_spend() {
    // Branch A spends block-1's coinbase; a heavier branch B does not. After
    // reorg to B, the coinbase is unspent again and A's payee output is gone.
    let mut h = H::new();
    for _ in 0..6 {
        h.mine(&[]); // heights 1..=6, block-1 coinbase mature
    }
    let recipient = ml_key(2).pubkey_hash();
    let amount = 3 * sump_core::emission::COIN;
    let spend = h.spend(1, recipient, amount, 100_000);
    let (cb1_op, _) = h.coinbase_outpoint(1);

    // branch A: block 7 including the spend
    h.mine(std::slice::from_ref(&spend));
    assert_eq!(h.state.height(), 7);
    assert!(!h.state.utxos().contains_key(&cb1_op), "coinbase spent on A");
    assert!(h
        .state
        .utxos()
        .values()
        .any(|u| *u.output.lock.pkh() == recipient));

    // build heavier branch B (2 blocks) from block 6 in a fork state
    let params = h.state.params().clone();
    let g = h.state.block_at(0).unwrap();
    let mut fork = ChainState::new(params.clone(), (*g).clone()).unwrap();
    let ctx = PowContext::new_full(&params.pow, 0);
    let other = ml_key(7).pubkey_hash();
    let t0 = params.genesis_time;
    for i in 1..=6u64 {
        // replay the same first 6 blocks so fork shares them
        let b = h.state.block_at(i).unwrap();
        fork.add_block((*b).clone()).unwrap();
    }
    let b7 =
        sump_node::miner::mine_and_connect(&mut fork, &ctx, &[], other, t0 + 7 * 60 + 1).unwrap();
    let b8 =
        sump_node::miner::mine_and_connect(&mut fork, &ctx, &[], other, t0 + 8 * 60 + 1).unwrap();

    assert!(!h.state.add_block(b7).unwrap(), "B7 is a side block");
    assert!(h.state.add_block(b8).unwrap(), "B8 triggers reorg");
    assert_eq!(h.state.height(), 8);

    // after reorg: coinbase-1 unspent again, recipient output gone
    assert!(
        h.state.utxos().contains_key(&cb1_op),
        "coinbase restored after reorg"
    );
    assert!(
        !h.state.utxos().values().any(|u| *u.output.lock.pkh() == recipient),
        "A-branch payee output removed after reorg"
    );
}

#[test]
fn mempool_drops_conflicting_tx_after_block() {
    let mut h = H::new();
    for _ in 0..6 {
        h.mine(&[]);
    }
    let mut pool = Mempool::new();
    let tx = h.spend(1, ml_key(2).pubkey_hash(), 2 * sump_core::emission::COIN, 100_000);
    pool.insert(&h.state, tx).expect("valid tx admitted");
    assert_eq!(pool.len(), 1);

    // a block spends the same coinbase via a different transaction
    let conflicting = h.spend(1, ml_key(3).pubkey_hash(), sump_core::emission::COIN, 100_000);
    let block = h.valid_next(std::slice::from_ref(&conflicting));
    h.state.add_block(block.clone()).unwrap();
    pool.update_for_block(&h.state, &block);
    assert_eq!(pool.len(), 0, "conflicting mempool tx evicted");
}

#[test]
fn mempool_rejects_underpriced_tx() {
    // a transaction paying below the minimum relay fee is rejected, so free
    // transactions cannot flood the pool
    let mut h = H::new();
    for _ in 0..6 {
        h.mine(&[]);
    }
    let mut pool = Mempool::new();
    // fee of 1 stanza is far below the size-based minimum (~4 KB tx)
    let cheap = h.spend(1, ml_key(2).pubkey_hash(), sump_core::emission::COIN, 1);
    assert!(matches!(
        pool.insert(&h.state, cheap),
        Err(sump_node::mempool::MempoolError::LowFee { .. })
    ));
    assert_eq!(pool.len(), 0);
    // a properly-paying transaction is admitted
    let ok = h.spend(1, ml_key(2).pubkey_hash(), sump_core::emission::COIN, 100_000);
    assert!(pool.insert(&h.state, ok).is_ok());
    assert_eq!(pool.len(), 1);
}

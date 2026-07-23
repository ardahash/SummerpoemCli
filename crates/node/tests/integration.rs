//! End-to-end consensus test on regtest: genesis, mining, spending,
//! maturity, double spends, bad signatures, and a simple reorg.

use sump_core::emission::block_reward;
use sump_core::params::Params;
use sump_core::tx::{Lock, OutPoint, Transaction, TxBody, TxInput, TxOutput, Witness};
use sump_crypto::Keypair;
use sump_node::chain::ChainState;
use sump_node::genesis::build_genesis;
use sump_node::miner::{build_block_template, mine_and_connect, mine_block};
use sump_node::ValidationError;
use sump_pow::PowContext;

struct Harness {
    state: ChainState,
    ctx: PowContext,
    miner_key: Keypair,
    clock: u64,
}

impl Harness {
    fn new() -> Harness {
        let params = Params::regtest();
        let ctx = PowContext::new_full(&params.pow, 0);
        let genesis = build_genesis(&params, &ctx);
        let state = ChainState::new(params.clone(), genesis).expect("valid genesis");
        Harness {
            clock: params.genesis_time + 60,
            state,
            ctx,
            miner_key: Keypair::from_seed(&[1u8; 32]),
        }
    }

    fn mine(&mut self, txs: &[Transaction]) {
        self.clock += 60;
        mine_and_connect(
            &mut self.state,
            &self.ctx,
            txs,
            self.miner_key.pubkey_hash(),
            self.clock,
        )
        .expect("mined block should connect");
    }

    /// The miner's coinbase output of the block at `height`.
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

    fn signed_spend(
        &self,
        from_height: u64,
        key: &Keypair,
        to_pkh: [u8; 20],
        amount: u64,
        fee: u64,
    ) -> Transaction {
        let (op, prev_out) = self.coinbase_outpoint(from_height);
        let change = prev_out.amount - amount - fee;
        let mut outputs = vec![TxOutput {
            amount,
            lock: Lock::P2pkh { pkh: to_pkh },
        }];
        if change > 0 {
            outputs.push(TxOutput {
                amount: change,
                lock: Lock::P2pkh {
                    pkh: key.pubkey_hash(),
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
        let sighash = body.sighash(0, &prev_out);
        let sig = key.sign(&sighash.0);
        Transaction {
            witnesses: vec![Witness {
                pubkey: key.public.clone(),
                signature: sig,
            }],
            body,
        }
    }
}

#[test]
fn full_lifecycle() {
    let mut h = Harness::new();
    assert_eq!(h.state.height(), 0);
    assert_eq!(h.state.supply(), block_reward(0));

    // mine 6 blocks so block 1's coinbase matures (regtest maturity = 5)
    for _ in 0..6 {
        h.mine(&[]);
    }
    assert_eq!(h.state.height(), 6);
    let expected_supply: u64 = (0..=6).map(block_reward).sum();
    assert_eq!(h.state.supply(), expected_supply);

    // spend the block-1 coinbase to a recipient
    let recipient = Keypair::from_seed(&[2u8; 32]);
    let amount = 3 * sump_core::emission::COIN;
    let fee = 100_000;
    let spend = h.signed_spend(1, &Keypair::from_seed(&[1u8; 32]), recipient.pubkey_hash(), amount, fee);
    let got_fee = h.state.validate_standalone_tx(&spend).expect("valid spend");
    assert_eq!(got_fee, fee);

    h.mine(std::slice::from_ref(&spend));
    assert_eq!(h.state.height(), 7);
    let block7 = h.state.block_at(7).unwrap();
    assert_eq!(block7.transactions.len(), 2, "spend included");
    // miner collected the fee
    assert_eq!(
        block7.transactions[0].body.outputs[0].amount,
        block_reward(7) + fee
    );

    // recipient's utxo exists
    let found = h
        .state
        .utxos()
        .values()
        .any(|u| u.output.amount == amount && *u.output.lock.pkh() == recipient.pubkey_hash());
    assert!(found, "recipient owns the new output");

    // double spend of the same coinbase must now fail
    let double = h.signed_spend(1, &Keypair::from_seed(&[1u8; 32]), recipient.pubkey_hash(), 1_000, fee);
    assert!(matches!(
        h.state.validate_standalone_tx(&double),
        Err(ValidationError::UnknownInput(_))
    ));
}

#[test]
fn immature_coinbase_rejected() {
    let mut h = Harness::new();
    h.mine(&[]); // height 1
    h.mine(&[]); // height 2: block 1's coinbase is only 1 deep, maturity is 5
    let spend = h.signed_spend(2, &Keypair::from_seed(&[1u8; 32]), [9u8; 20], 1_000, 1_000);
    assert!(matches!(
        h.state.validate_standalone_tx(&spend),
        Err(ValidationError::ImmatureCoinbase)
    ));
}

#[test]
fn bad_signature_rejected() {
    let mut h = Harness::new();
    for _ in 0..6 {
        h.mine(&[]);
    }
    let mut spend = h.signed_spend(1, &Keypair::from_seed(&[1u8; 32]), [9u8; 20], 1_000, 1_000);
    // corrupt the signature
    spend.witnesses[0].signature[100] ^= 0xff;
    assert!(matches!(
        h.state.validate_standalone_tx(&spend),
        Err(ValidationError::BadSignature)
    ));
    // wrong key entirely
    let mut spend2 = h.signed_spend(1, &Keypair::from_seed(&[1u8; 32]), [9u8; 20], 1_000, 1_000);
    spend2.witnesses[0].pubkey = Keypair::from_seed(&[3u8; 32]).public.clone();
    assert!(matches!(
        h.state.validate_standalone_tx(&spend2),
        Err(ValidationError::WrongPubkey)
    ));
}

#[test]
fn overpaying_coinbase_rejected() {
    let mut h = Harness::new();
    let mut block = build_block_template(
        &h.state,
        &[],
        h.miner_key.pubkey_hash(),
        h.state.params().genesis_time + 120,
    );
    block.transactions[0].body.outputs[0].amount = block_reward(1) + 1;
    block.header.tx_root = block.compute_tx_root();
    block.header.witness_root = block.compute_witness_root();
    assert!(mine_block(&h.ctx, &mut block, u64::MAX));
    assert!(matches!(
        h.state.add_block(block),
        Err(ValidationError::CoinbaseOverpay)
    ));
    // clean template still works
    h.mine(&[]);
    assert_eq!(h.state.height(), 1);
}

#[test]
fn simple_reorg_wins_by_work() {
    let mut h = Harness::new();
    h.mine(&[]); // A1 on top of genesis; tip height 1
    let a1 = h.state.tip_hash();

    // build a competing branch B1, B2 from genesis
    let genesis_hash = h.state.genesis_hash();
    let params = h.state.params().clone();
    let other_pkh = Keypair::from_seed(&[7u8; 32]).pubkey_hash();

    // fork state to build B-branch blocks (fresh state over same genesis)
    let genesis_block = h.state.block_at(0).unwrap();
    let mut fork = ChainState::new(params.clone(), (*genesis_block).clone()).unwrap();
    let ctx = PowContext::new_full(&params.pow, 0);
    let t0 = params.genesis_time;
    let b1 = mine_and_connect(&mut fork, &ctx, &[], other_pkh, t0 + 61).unwrap();
    let b2 = mine_and_connect(&mut fork, &ctx, &[], other_pkh, t0 + 122).unwrap();

    // feed the B branch into the main state: B1 is a side block, B2 reorgs
    assert!(!h.state.add_block(b1).unwrap(), "side block");
    assert_eq!(h.state.tip_hash(), a1, "tip unchanged");
    assert!(h.state.add_block(b2).unwrap(), "reorg to longer chain");
    assert_eq!(h.state.height(), 2);
    assert_ne!(h.state.block_at(1).unwrap().header.hash(), a1);
    assert_eq!(h.state.block_at(0).unwrap().header.hash(), genesis_hash);
}

//! Two real nodes over localhost: encrypted handshake, chain sync,
//! transaction relay, and block propagation.

use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};
use sump_core::params::Params;
use sump_core::tx::{Lock, OutPoint, SigScheme, Transaction, TxBody, TxInput, TxOutput, Witness};
use sump_crypto::Keypair;
use sump_net::transport;
use sump_net::NetNode;
use sump_node::chain::ChainState;
use sump_node::genesis::build_genesis;
use sump_node::miner::mine_and_connect;
use sump_pow::PowContext;

fn wait_until(deadline_secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(deadline_secs);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn make_chain(ctx: &PowContext, params: &Params) -> ChainState {
    let genesis = build_genesis(params, ctx);
    ChainState::new(params.clone(), genesis).expect("valid genesis")
}

#[test]
fn transport_handshake_and_tamper_detection() {
    use std::io::Write;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let (mut r, mut w) = transport::respond(stream, 1).unwrap();
        assert_eq!(r.recv().unwrap(), b"hello from initiator");
        w.send(b"hello from responder").unwrap();
        // the next frame is forged (not AEAD-authenticated): must be rejected
        assert!(r.recv().is_err(), "forged frame accepted");
    });

    let stream = TcpStream::connect(addr).unwrap();
    // keep a raw handle on the same socket so we can inject bytes that
    // bypass the encryption layer, simulating an in-path tamperer
    let mut raw = stream.try_clone().unwrap();
    let (mut r, mut w) = transport::initiate(stream, 1).unwrap();
    w.send(b"hello from initiator").unwrap();
    assert_eq!(r.recv().unwrap(), b"hello from responder");

    // well-formed length prefix, garbage ciphertext
    raw.write_all(&(32u32).to_le_bytes()).unwrap();
    raw.write_all(&[0u8; 32]).unwrap();
    raw.flush().unwrap();

    server.join().unwrap();
}

#[test]
fn network_mismatch_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        assert!(transport::respond(stream, 0).is_err(), "mainnet responder");
    });
    let stream = TcpStream::connect(addr).unwrap();
    assert!(transport::initiate(stream, 1).is_err(), "regtest initiator");
    server.join().unwrap();
}

#[test]
fn sync_and_relay_between_two_nodes() {
    let params = Params::regtest();
    let ctx = PowContext::new_full(&params.pow, 0);
    let miner_key = Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]);
    let payout = miner_key.pubkey_hash();
    let t0 = params.genesis_time;

    // node A: mine 8 blocks before any networking
    let mut chain_a = make_chain(&ctx, &params);
    for i in 0..8u64 {
        mine_and_connect(&mut chain_a, &ctx, &[], payout, t0 + 60 * (i + 1)).unwrap();
    }
    // node B: same genesis, empty chain
    let chain_b = make_chain(&ctx, &params);
    assert_eq!(
        chain_a.block_at(0).unwrap().header.hash(),
        chain_b.block_at(0).unwrap().header.hash(),
        "deterministic genesis"
    );

    let node_a = NetNode::new(chain_a, None, true);
    let node_b = NetNode::new(chain_b, None, true);
    let addr_a = node_a.listen("127.0.0.1:0").unwrap();
    node_b.connect(&addr_a.to_string()).unwrap();

    // B must sync all 8 blocks from A
    assert!(
        wait_until(30, || node_b.height() == 8 && node_b.tip_hash() == node_a.tip_hash()),
        "node B failed to sync (height {})",
        node_b.height()
    );

    // build a spend of A's block-1 coinbase (mature: height 8 >= 1+5+...)
    let (prev_op, prev_out) = {
        let chain = node_a.shared().chain.lock().unwrap();
        let block1 = chain.block_at(1).unwrap();
        let cb = &block1.transactions[0];
        (
            OutPoint {
                txid: cb.txid(),
                vout: 0,
            },
            cb.body.outputs[0].clone(),
        )
    };
    let fee = 50_000u64;
    let body = TxBody {
        version: 1,
        inputs: vec![TxInput { prevout: prev_op }],
        outputs: vec![TxOutput {
            amount: prev_out.amount - fee,
            lock: Lock::P2pkh {
                scheme: SigScheme::MlDsa,
                pkh: [9u8; 20],
            },
        }],
        locktime: 0,
        coinbase_data: vec![],
    };
    let sighash = body.sighash(0, &prev_out);
    let tx = Transaction {
        witnesses: vec![Witness {
            pubkey: miner_key.public.clone(),
            signature: miner_key.sign(&sighash.0),
        }],
        body,
    };
    let txid = tx.txid();

    // submit to B; it must relay to A's mempool
    node_b.submit_tx(tx).expect("valid tx");
    assert!(
        wait_until(15, || node_a.mempool_len() == 1),
        "tx did not relay to node A"
    );

    // A mines a block including the tx; B must receive it via gossip
    let block = {
        let chain = node_a.shared().chain.lock().unwrap();
        let txs = node_a.shared().mempool.lock().unwrap().transactions();
        let mut template = sump_node::miner::build_block_template(
            &chain,
            &txs,
            payout,
            t0 + 60 * 20,
        );
        assert!(sump_node::miner::mine_block(&ctx, &mut template, u64::MAX));
        template
    };
    assert!(node_a.submit_block(block).expect("block connects"), "new tip");
    // ensure B converges on height 9 with the tx confirmed
    assert!(
        wait_until(30, || node_b.height() == 9),
        "block did not propagate to node B (height {})",
        node_b.height()
    );
    let confirmed = {
        let chain = node_b.shared().chain.lock().unwrap();
        let blk = chain.block_at(9).unwrap();
        blk.transactions.iter().any(|t| t.txid() == txid)
    };
    assert!(confirmed, "relayed tx not in synced block");
    // both mempools drained
    assert!(wait_until(10, || node_a.mempool_len() == 0 && node_b.mempool_len() == 0));
}

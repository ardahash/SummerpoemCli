//! Multi-node network hardening: transitive sync through a relay, and
//! reconvergence of two independently-mined chains after they connect.

use std::thread;
use std::time::{Duration, Instant};
use sump_core::params::Params;
use sump_crypto::{Keypair, SigScheme};
use sump_net::NetNode;
use sump_node::chain::ChainState;
use sump_node::genesis::build_genesis;
use sump_node::miner::mine_and_connect;
use sump_pow::PowContext;

fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn fresh_chain(ctx: &PowContext, params: &Params) -> ChainState {
    ChainState::new(params.clone(), build_genesis(params, ctx)).unwrap()
}

#[test]
fn self_connection_is_dropped() {
    // a node that dials its own address (as can happen when discovery gossips
    // it back) must detect the loop via the startup nonce and drop it — no
    // stable self-peer should remain.
    let params = Params::regtest();
    let ctx = PowContext::new_full(&params.pow, 0);
    let node = NetNode::new(fresh_chain(&ctx, &params), None, true);
    let addr = node.listen("127.0.0.1:0").unwrap();
    node.connect(&addr.to_string()).unwrap();
    // give it time to connect, exchange Hello, detect self, and tear down
    assert!(
        wait_until(5, || node.peer_count() == 0),
        "self-connection was not dropped (peers still {})",
        node.peer_count()
    );
    // a real peer still connects fine (sanity: the drop is self-specific)
    let other = NetNode::new(fresh_chain(&ctx, &params), None, true);
    other.connect(&addr.to_string()).unwrap();
    assert!(
        wait_until(10, || node.peer_count() >= 1 && other.peer_count() >= 1),
        "a genuine peer failed to connect"
    );
}

#[test]
fn three_node_transitive_sync() {
    let params = Params::regtest();
    let ctx = PowContext::new_full(&params.pow, 0);
    let payout = Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]).pubkey_hash();
    let t0 = params.genesis_time;

    // A starts with 10 blocks; B and C start empty on the same genesis.
    let mut chain_a = fresh_chain(&ctx, &params);
    for i in 0..10u64 {
        mine_and_connect(&mut chain_a, &ctx, &[], payout, t0 + 60 * (i + 1)).unwrap();
    }
    let node_a = NetNode::new(chain_a, None, true);
    let node_b = NetNode::new(fresh_chain(&ctx, &params), None, true);
    let node_c = NetNode::new(fresh_chain(&ctx, &params), None, true);

    // topology: C -> B -> A (C only knows B). Sync must flow transitively.
    let addr_a = node_a.listen("127.0.0.1:0").unwrap();
    let addr_b = node_b.listen("127.0.0.1:0").unwrap();
    node_b.connect(&addr_a.to_string()).unwrap();
    node_c.connect(&addr_b.to_string()).unwrap();

    assert!(
        wait_until(30, || node_b.height() == 10 && node_c.height() == 10),
        "B={} C={}",
        node_b.height(),
        node_c.height()
    );
    assert_eq!(node_c.tip_hash(), node_a.tip_hash());

    // A mines another block; it must propagate A -> B -> C by gossip.
    let mut block = {
        let shared = node_a.shared();
        let chain = shared.chain.lock().unwrap();
        sump_node::miner::build_block_template(&chain, &[], payout, t0 + 60 * 11)
    };
    assert!(sump_node::miner::mine_block(&ctx, &mut block, u64::MAX));
    assert!(node_a.submit_block(block).unwrap(), "A connects its new block");
    assert!(
        wait_until(30, || node_c.height() == 11),
        "gossip did not reach C (height {})",
        node_c.height()
    );
}

#[test]
fn two_chains_reconverge_on_higher_work() {
    let params = Params::regtest();
    let ctx = PowContext::new_full(&params.pow, 0);
    let payout = Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]).pubkey_hash();
    let t0 = params.genesis_time;

    // A mines 3 blocks; B independently mines 6 (heavier), same genesis.
    let mut chain_a = fresh_chain(&ctx, &params);
    for i in 0..3u64 {
        mine_and_connect(&mut chain_a, &ctx, &[], payout, t0 + 60 * (i + 1)).unwrap();
    }
    let mut chain_b = fresh_chain(&ctx, &params);
    for i in 0..6u64 {
        mine_and_connect(&mut chain_b, &ctx, &[], payout, t0 + 60 * (i + 1)).unwrap();
    }

    let node_a = NetNode::new(chain_a, None, true);
    let node_b = NetNode::new(chain_b, None, true);
    let b_tip = node_b.tip_hash();

    let addr_a = node_a.listen("127.0.0.1:0").unwrap();
    node_b.connect(&addr_a.to_string()).unwrap();

    // A must abandon its 3-block chain for B's heavier 6-block chain.
    assert!(
        wait_until(30, || node_a.height() == 6 && node_a.tip_hash() == b_tip),
        "A failed to reconverge (height {})",
        node_a.height()
    );
    // B keeps its own chain (already heaviest)
    assert_eq!(node_b.height(), 6);
    assert_eq!(node_b.tip_hash(), b_tip);
}

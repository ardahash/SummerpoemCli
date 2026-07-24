//! Peer discovery: nodes that share only a common seed must learn about each
//! other through address gossip and form direct connections.

use std::thread;
use std::time::{Duration, Instant};
use sump_core::params::Params;
use sump_crypto::{Keypair, SigScheme};
use sump_net::NetNode;
use sump_node::chain::ChainState;
use sump_node::genesis::build_genesis;
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

fn fresh(ctx: &PowContext, params: &Params) -> ChainState {
    ChainState::new(params.clone(), build_genesis(params, ctx)).unwrap()
}

#[test]
fn nodes_discover_each_other_through_a_seed() {
    let params = Params::regtest();
    let ctx = PowContext::new_full(&params.pow, 0);
    let _ = Keypair::from_seed(SigScheme::MlDsa, &[1u8; 32]); // (schemes wired)

    // Seed node S: listens, runs discovery with no upstream seeds.
    let seed = NetNode::new(fresh(&ctx, &params), None, true);
    let seed_addr = seed.listen("127.0.0.1:0").unwrap();
    seed.start_discovery(&[]);

    // B and C each know only S. With discovery on, they should learn about
    // each other (S gossips their addresses) and connect directly.
    let node_b = NetNode::new(fresh(&ctx, &params), None, true);
    let b_addr = node_b.listen("127.0.0.1:0").unwrap();
    node_b.start_discovery(&[seed_addr.to_string()]);

    let node_c = NetNode::new(fresh(&ctx, &params), None, true);
    let c_addr = node_c.listen("127.0.0.1:0").unwrap();
    node_c.start_discovery(&[seed_addr.to_string()]);

    // each of B and C should end up with at least 2 peers (the seed + the
    // other), proving a direct B<->C link formed via discovery
    let ok = wait_until(40, || node_b.peer_count() >= 2 && node_c.peer_count() >= 2);
    assert!(
        ok,
        "discovery failed: B peers={}, C peers={} (books B={}, C={})",
        node_b.peer_count(),
        node_c.peer_count(),
        node_b.known_addr_count(),
        node_c.known_addr_count(),
    );

    // both learned the full set of addresses (seed, B, C minus self)
    assert!(node_b.known_addr_count() >= 2);
    assert!(node_c.known_addr_count() >= 2);
    // keep addresses referenced so the bindings are considered used
    let _ = (seed_addr, b_addr, c_addr);
}

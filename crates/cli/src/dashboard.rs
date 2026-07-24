//! Local web dashboard for a running node: a self-contained HTML page plus a
//! JSON status endpoint, served over plain HTTP on localhost. Opened in the
//! operator's browser — a lightweight "mining GUI" with no native toolkit.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use sump_core::emission::COIN;
use sump_core::tx::Lock;
use sump_net::NetNode;

/// Shared mining telemetry, updated by the miner loop and read by the server.
pub struct MinerStats {
    pub mining: AtomicBool,
    pub gpu: AtomicBool,
    pub hashes: AtomicU64,
    start: Instant,
}

impl MinerStats {
    pub fn new() -> Arc<MinerStats> {
        Arc::new(MinerStats {
            mining: AtomicBool::new(false),
            gpu: AtomicBool::new(false),
            hashes: AtomicU64::new(0),
            start: Instant::now(),
        })
    }

    pub fn add_hashes(&self, n: u64) {
        self.hashes.fetch_add(n, Ordering::Relaxed);
    }
}

/// Spendable balance (stanzas) for a precomputed set of owned (scheme, pkh).
fn balance_for(node: &NetNode, owned: &[(u8, [u8; 20])]) -> u64 {
    let shared = node.shared();
    let chain = shared.chain.lock().unwrap();
    let next_height = chain.height() + 1;
    let maturity = chain.params().coinbase_maturity;
    chain
        .utxos()
        .values()
        .filter(|u| owned.contains(&(u.output.lock.scheme().id(), *u.output.lock.pkh())))
        .filter(|u| !u.coinbase || next_height >= u.height + maturity)
        .filter(|u| match u.output.lock {
            Lock::Timelock { height, .. } => next_height >= height,
            Lock::P2pkh { .. } => true,
        })
        .map(|u| u.output.amount)
        .sum()
}

fn json_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c => vec![c],
        })
        .collect()
}

fn status_json(
    node: &NetNode,
    stats: &MinerStats,
    owned: &[(u8, [u8; 20])],
    address: &str,
    vault: &str,
    network: &str,
) -> String {
    let shared = node.shared();
    let (height, tip, supply, mempool) = {
        let chain = shared.chain.lock().unwrap();
        (
            chain.height(),
            chain.tip_hash().to_hex(),
            chain.supply(),
            0,
        )
    };
    let mempool = shared.mempool.lock().unwrap().len().max(mempool);
    let peers = node.peer_count();
    let known = node.known_addr_count();
    let balance = balance_for(node, owned);
    let hashes = stats.hashes.load(Ordering::Relaxed);
    let secs = stats.start.elapsed().as_secs_f64().max(1.0);
    let hps = hashes as f64 / secs;

    format!(
        "{{\"network\":\"{}\",\"height\":{},\"tip\":\"{}\",\"supply\":{:.8},\
         \"peers\":{},\"known_addrs\":{},\"mempool\":{},\"mining\":{},\
         \"gpu\":{},\"hashrate\":{:.0},\"balance\":{:.8},\
         \"address\":\"{}\",\"vault\":\"{}\"}}",
        json_escape(network),
        height,
        tip,
        supply as f64 / COIN as f64,
        peers,
        known,
        mempool,
        stats.mining.load(Ordering::Relaxed),
        stats.gpu.load(Ordering::Relaxed),
        hps,
        balance as f64 / COIN as f64,
        json_escape(address),
        json_escape(vault),
    )
}

fn respond(mut stream: TcpStream, content_type: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{}",
        content_type,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

/// Start the dashboard HTTP server. Returns the bound address.
#[allow(clippy::too_many_arguments)]
pub fn serve(
    bind: &str,
    node: NetNode,
    stats: Arc<MinerStats>,
    owned: Vec<(u8, [u8; 20])>,
    address: String,
    vault: String,
    network: String,
) -> std::io::Result<std::net::SocketAddr> {
    let listener = TcpListener::bind(bind)?;
    let local = listener.local_addr()?;
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            if path.starts_with("/status") {
                let body =
                    status_json(&node, &stats, &owned, &address, &vault, &network);
                respond(stream, "application/json", &body);
            } else {
                respond(stream, "text/html; charset=utf-8", PAGE);
            }
        }
    });
    Ok(local)
}

const PAGE: &str = include_str!("dashboard.html");

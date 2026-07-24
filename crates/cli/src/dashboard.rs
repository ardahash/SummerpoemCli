//! Local web dashboard for a running node: a self-contained HTML page plus a
//! JSON status endpoint, served over plain HTTP on localhost. Opened in the
//! operator's browser — a lightweight "mining GUI" with no native toolkit.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use sump_core::emission::COIN;
use sump_core::tx::{Lock, Transaction};
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

/// (spendable, pending) balance in stanzas for a precomputed owned set.
/// Pending = owned coins not yet spendable (immature coinbase or timelocked).
fn balances_for(node: &NetNode, owned: &[(u8, [u8; 20])]) -> (u64, u64) {
    let shared = node.shared();
    let chain = shared.chain.lock().unwrap();
    let next_height = chain.height() + 1;
    let maturity = chain.params().coinbase_maturity;
    let (mut spendable, mut total) = (0u64, 0u64);
    for u in chain.utxos().values() {
        if !owned.contains(&(u.output.lock.scheme().id(), *u.output.lock.pkh())) {
            continue;
        }
        total += u.output.amount;
        let mature = !u.coinbase || next_height >= u.height + maturity;
        let unlocked = match u.output.lock {
            Lock::Timelock { height, .. } => next_height >= height,
            Lock::P2pkh { .. } => true,
        };
        if mature && unlocked {
            spendable += u.output.amount;
        }
    }
    (spendable, total - spendable)
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
    let (balance, pending) = balances_for(node, owned);
    let hashes = stats.hashes.load(Ordering::Relaxed);
    let secs = stats.start.elapsed().as_secs_f64().max(1.0);
    let hps = hashes as f64 / secs;

    format!(
        "{{\"network\":\"{}\",\"height\":{},\"tip\":\"{}\",\"supply\":{:.8},\
         \"peers\":{},\"known_addrs\":{},\"mempool\":{},\"mining\":{},\
         \"gpu\":{},\"hashrate\":{:.0},\"balance\":{:.8},\"pending\":{:.8},\
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
        pending as f64 / COIN as f64,
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

/// Operator-dashboard context (present when `--gui` is used).
pub struct DashboardCtx {
    pub stats: Arc<MinerStats>,
    pub owned: Vec<(u8, [u8; 20])>,
    pub address: String,
    pub vault: String,
    pub network: String,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read one HTTP request: (method, path, body). Handles Content-Length bodies
/// (POST) up to a sane cap.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    const CAP: usize = 8 * 1024 * 1024;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = find(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let mut content_length = 0usize;
    for l in lines {
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0).min(CAP);
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Some((method, path, body))
}

/// RPC: owned UTXOs for a set of (scheme, pkh) keys, plus height + maturity.
fn rpc_utxos(node: &NetNode, body: &[u8]) -> String {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return format!("{{\"error\":\"bad request: {}\"}}", json_escape(&e.to_string())),
    };
    let mut want: HashSet<(u8, [u8; 20])> = HashSet::new();
    if let Some(keys) = v.get("keys").and_then(|k| k.as_array()) {
        for k in keys {
            let scheme = k.get("scheme").and_then(|s| s.as_u64()).unwrap_or(255) as u8;
            let pkh_hex = k.get("pkh").and_then(|p| p.as_str()).unwrap_or("");
            if let Ok(bytes) = hex::decode(pkh_hex) {
                if let Ok(pkh) = <[u8; 20]>::try_from(bytes.as_slice()) {
                    want.insert((scheme, pkh));
                }
            }
        }
    }
    let shared = node.shared();
    let chain = shared.chain.lock().unwrap();
    let height = chain.height();
    let maturity = chain.params().coinbase_maturity;
    let mut items = Vec::new();
    for (op, u) in chain.utxos() {
        let key = (u.output.lock.scheme().id(), *u.output.lock.pkh());
        if !want.contains(&key) {
            continue;
        }
        items.push(format!(
            "{{\"txid\":\"{}\",\"vout\":{},\"amount\":{},\"scheme\":{},\
             \"pkh\":\"{}\",\"coinbase\":{},\"height\":{}}}",
            op.txid.to_hex(),
            op.vout,
            u.output.amount,
            key.0,
            hex::encode(key.1),
            u.coinbase,
            u.height,
        ));
    }
    format!(
        "{{\"height\":{},\"maturity\":{},\"utxos\":[{}]}}",
        height,
        maturity,
        items.join(",")
    )
}

/// RPC: submit a hex-encoded transaction; validate and broadcast it.
fn rpc_submit(node: &NetNode, body: &[u8]) -> String {
    let hexstr = String::from_utf8_lossy(body);
    let bytes = match hex::decode(hexstr.trim()) {
        Ok(b) => b,
        Err(_) => return "{\"error\":\"body must be hex-encoded transaction\"}".into(),
    };
    let tx = match Transaction::decode_all(&bytes) {
        Ok(t) => t,
        Err(e) => return format!("{{\"error\":\"decode: {}\"}}", json_escape(&e.to_string())),
    };
    let txid = tx.txid().to_hex();
    match node.submit_tx(tx) {
        Ok(_) => format!("{{\"txid\":\"{txid}\"}}"),
        Err(e) => format!("{{\"error\":\"{}\"}}", json_escape(&e.to_string())),
    }
}

/// Start the node's HTTP server. `dash` present ⇒ also serve the operator
/// dashboard page and `/status`; the `/api/*` RPC is always served.
pub fn serve(
    bind: &str,
    node: NetNode,
    dash: Option<DashboardCtx>,
) -> std::io::Result<std::net::SocketAddr> {
    let listener = TcpListener::bind(bind)?;
    let local = listener.local_addr()?;
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            // bound each request so a slow client cannot stall this
            // single-threaded server (anti-slowloris)
            let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
            let Some((method, path, body)) = read_request(&mut stream) else {
                continue;
            };
            let route = path.split('?').next().unwrap_or("/");
            match (method.as_str(), route) {
                ("POST", "/api/utxos") => {
                    respond(stream, "application/json", &rpc_utxos(&node, &body));
                }
                ("POST", "/api/submit") => {
                    respond(stream, "application/json", &rpc_submit(&node, &body));
                }
                (_, "/status") => match &dash {
                    Some(d) => {
                        let b = status_json(
                            &node, &d.stats, &d.owned, &d.address, &d.vault, &d.network,
                        );
                        respond(stream, "application/json", &b);
                    }
                    None => respond(stream, "application/json", "{\"rpc\":true}"),
                },
                _ => match &dash {
                    Some(_) => respond(stream, "text/html; charset=utf-8", PAGE),
                    None => respond(
                        stream,
                        "text/plain; charset=utf-8",
                        "Summerpoem node RPC. Endpoints: POST /api/utxos, POST /api/submit.",
                    ),
                },
            }
        }
    });
    Ok(local)
}

const PAGE: &str = include_str!("dashboard.html");

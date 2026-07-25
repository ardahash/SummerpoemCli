//! Networked node: peer management, chain sync, block/tx gossip.
//!
//! Threading model: one accept-loop thread, plus a reader thread and a
//! writer thread per peer. Shared state (chain, mempool, peer table) lives
//! behind mutexes; lock order is always chain -> mempool -> peers.

use crate::message::{Message, BLOCKS_PER_ROUND, MAX_ADDRS, MAX_INV, PROTOCOL_VERSION};
use crate::transport;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use sump_core::block::Block;
use sump_core::hash::Hash256;
use sump_core::tx::Transaction;
use sump_node::{ChainState, Mempool, MempoolError, ValidationError};

pub struct PeerHandle {
    pub id: u64,
    pub addr: SocketAddr,
    pub outbound: bool,
    sender: Sender<Message>,
}

/// Target number of outbound connections the discovery dialer maintains.
const TARGET_PEERS: usize = 8;
const ADDR_SAMPLE: usize = 200;
/// DoS limits (Bitcoin-style): cap inbound connections and per-IP connections
/// to bound resource use; ban an IP once its misbehavior score crosses the
/// threshold (an invalid block/tx/message scores the full amount → instant
/// ban), so an attacker cannot cheaply reconnect and re-flood.
const MAX_INBOUND: usize = 117;
const MAX_PER_IP: usize = 5;
const BAN_SCORE: i32 = 100;
const BAN_DURATION: Duration = Duration::from_secs(24 * 3600);

pub struct Shared {
    pub network_id: u8,
    pub chain: Mutex<ChainState>,
    pub mempool: Mutex<Mempool>,
    peers: Mutex<HashMap<u64, PeerHandle>>,
    next_peer_id: AtomicU64,
    chain_path: Option<PathBuf>,
    pub quiet: bool,
    /// Our own listen port (0 = not listening), advertised in Hello.
    listen_port: AtomicU16,
    /// Highest peer height observed in handshakes. Miners use this to avoid
    /// starting before the local chain has caught up to known public peers.
    best_peer_height: AtomicU64,
    /// Known reachable peer addresses (the address book).
    book: Mutex<HashSet<SocketAddr>>,
    /// Addresses we are currently connected to or dialing (dedup dials).
    connected: Mutex<HashSet<SocketAddr>>,
    /// Live inbound connection counts per source IP.
    inbound: Mutex<HashMap<IpAddr, usize>>,
    /// Misbehavior score per IP; crossing BAN_SCORE bans the IP.
    scores: Mutex<HashMap<IpAddr, i32>>,
    /// Banned IPs and the time their ban expires.
    banned: Mutex<HashMap<IpAddr, Instant>>,
}

fn is_banned(shared: &Shared, ip: IpAddr) -> bool {
    let mut banned = shared.banned.lock().unwrap();
    match banned.get(&ip) {
        Some(&until) if Instant::now() < until => true,
        Some(_) => {
            banned.remove(&ip);
            false
        }
        None => false,
    }
}

/// Record misbehavior for `ip`; ban it once the score crosses the threshold.
fn misbehave(shared: &Shared, ip: IpAddr, points: i32) {
    let mut scores = shared.scores.lock().unwrap();
    let s = scores.entry(ip).or_insert(0);
    *s += points;
    if *s >= BAN_SCORE {
        scores.remove(&ip);
        drop(scores);
        shared
            .banned
            .lock()
            .unwrap()
            .insert(ip, Instant::now() + BAN_DURATION);
        log(shared, &format!("\x1b[31mbanned {ip}\x1b[0m (misbehavior)"));
    }
}

fn dec_inbound(shared: &Shared, ip: IpAddr) {
    let mut inbound = shared.inbound.lock().unwrap();
    if let Some(c) = inbound.get_mut(&ip) {
        *c = c.saturating_sub(1);
        if *c == 0 {
            inbound.remove(&ip);
        }
    }
}

#[derive(Clone)]
pub struct NetNode {
    shared: Arc<Shared>,
}

fn log(shared: &Shared, msg: &str) {
    if !shared.quiet {
        // dim gold [net] prefix; the message may carry its own color codes
        eprintln!("\x1b[2;33m[net]\x1b[0m {msg}");
    }
}

impl NetNode {
    pub fn new(chain: ChainState, chain_path: Option<PathBuf>, quiet: bool) -> NetNode {
        let network_id = chain.params().network.id();
        let height = chain.height();
        NetNode {
            shared: Arc::new(Shared {
                network_id,
                chain: Mutex::new(chain),
                mempool: Mutex::new(Mempool::new()),
                peers: Mutex::new(HashMap::new()),
                next_peer_id: AtomicU64::new(1),
                chain_path,
                quiet,
                listen_port: AtomicU16::new(0),
                best_peer_height: AtomicU64::new(height),
                book: Mutex::new(HashSet::new()),
                connected: Mutex::new(HashSet::new()),
                inbound: Mutex::new(HashMap::new()),
                scores: Mutex::new(HashMap::new()),
                banned: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    pub fn height(&self) -> u64 {
        self.shared.chain.lock().unwrap().height()
    }

    pub fn tip_hash(&self) -> Hash256 {
        self.shared.chain.lock().unwrap().tip_hash()
    }

    pub fn peer_count(&self) -> usize {
        self.shared.peers.lock().unwrap().len()
    }

    pub fn best_peer_height(&self) -> u64 {
        self.shared.best_peer_height.load(Ordering::SeqCst)
    }

    pub fn mempool_len(&self) -> usize {
        self.shared.mempool.lock().unwrap().len()
    }

    /// Start listening; returns the bound address (useful with port 0).
    pub fn listen(&self, addr: &str) -> std::io::Result<SocketAddr> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        self.shared.listen_port.store(local.port(), Ordering::SeqCst);
        let shared = self.shared.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let peer_addr = stream
                    .peer_addr()
                    .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
                let ip = peer_addr.ip();

                // DoS gate: drop banned IPs and enforce inbound / per-IP caps
                // before spending a handshake on the connection.
                if is_banned(&shared, ip) {
                    continue;
                }
                {
                    let mut inbound = shared.inbound.lock().unwrap();
                    let total: usize = inbound.values().sum();
                    let per_ip = *inbound.get(&ip).unwrap_or(&0);
                    if total >= MAX_INBOUND || per_ip >= MAX_PER_IP {
                        continue; // silently drop; the socket closes
                    }
                    *inbound.entry(ip).or_insert(0) += 1;
                }

                let shared = shared.clone();
                thread::spawn(move || {
                    match transport::respond(stream, shared.network_id) {
                        Ok((r, w)) => run_peer(shared.clone(), r, w, peer_addr, false),
                        Err(e) => log(&shared, &format!("handshake with {peer_addr} failed: {e}")),
                    }
                    dec_inbound(&shared, ip);
                });
            }
        });
        Ok(local)
    }

    /// Connect out to a peer (by host:port string).
    pub fn connect(&self, addr: &str) -> std::io::Result<()> {
        let target: SocketAddr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::other("unresolvable"))?;
        dial(&self.shared, target);
        Ok(())
    }

    /// Seed the address book and start the discovery dialer thread, which
    /// maintains up to TARGET_PEERS outbound connections from known addresses.
    pub fn start_discovery(&self, seeds: &[String]) {
        {
            let mut book = self.shared.book.lock().unwrap();
            for s in seeds {
                match s.to_socket_addrs() {
                    Ok(it) => {
                        for a in it {
                            if a.port() != 0 && !a.ip().is_unspecified() {
                                book.insert(a);
                            }
                        }
                    }
                    Err(e) => {
                        log(&self.shared, &format!("seed {s} did not resolve: {e}"));
                    }
                }
            }
        }
        let shared = self.shared.clone();
        thread::spawn(move || loop {
            let have = shared.peers.lock().unwrap().len();
            if have < TARGET_PEERS {
                // dial book addresses we are not already connected to / dialing
                let candidates: Vec<SocketAddr> = {
                    let connected = shared.connected.lock().unwrap();
                    shared
                        .book
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|a| !connected.contains(a))
                        .take(TARGET_PEERS - have)
                        .copied()
                        .collect()
                };
                for a in candidates {
                    dial(&shared, a);
                }
            }
            thread::sleep(Duration::from_secs(5));
        });
    }

    pub fn known_addr_count(&self) -> usize {
        self.shared.book.lock().unwrap().len()
    }

    /// Validate and admit a locally submitted transaction, then announce it.
    pub fn submit_tx(&self, tx: Transaction) -> Result<u64, MempoolError> {
        let txid = tx.txid();
        let fee = {
            let chain = self.shared.chain.lock().unwrap();
            self.shared.mempool.lock().unwrap().insert(&chain, tx)?
        };
        broadcast(
            &self.shared,
            &Message::Inv {
                blocks: vec![],
                txs: vec![txid],
            },
            None,
        );
        Ok(fee)
    }

    /// Connect a locally mined (or otherwise obtained) block, then announce it.
    pub fn submit_block(&self, block: Block) -> Result<bool, ValidationError> {
        accept_block(&self.shared, block, None)
    }
}

/// Add a block to the chain; on success update mempool, persist, announce.
/// `from_peer` suppresses re-announcing to the peer that sent it.
fn accept_block(
    shared: &Arc<Shared>,
    block: Block,
    from_peer: Option<u64>,
) -> Result<bool, ValidationError> {
    let hash = block.header.hash();
    let new_tip = {
        let mut chain = shared.chain.lock().unwrap();
        let new_tip = chain.add_block(block.clone())?;
        if new_tip {
            let mut mempool = shared.mempool.lock().unwrap();
            mempool.update_for_block(&chain, &block);
            // persist periodically, not per block: rewriting the whole file
            // every block is O(n^2) I/O during fast sync. Unsaved recent
            // blocks are simply re-synced from peers after a crash.
            if chain.height().is_multiple_of(32) {
                if let Some(path) = &shared.chain_path {
                    if let Err(e) = sump_node::store::save(
                        path,
                        chain.params().network.id(),
                        &chain.active_blocks(),
                    ) {
                        log(shared, &format!("chain save failed: {e}"));
                    }
                }
            }
            log(
                shared,
                &format!(
                    "\x1b[32m✓ new tip height {}\x1b[0m {} ({} txs)",
                    chain.height(),
                    hash,
                    block.transactions.len()
                ),
            );
        }
        new_tip
    };
    if new_tip {
        broadcast(
            shared,
            &Message::Inv {
                blocks: vec![hash],
                txs: vec![],
            },
            from_peer,
        );
    }
    Ok(new_tip)
}

fn broadcast(shared: &Arc<Shared>, msg: &Message, except: Option<u64>) {
    let peers = shared.peers.lock().unwrap();
    for (id, p) in peers.iter() {
        if Some(*id) != except {
            let _ = p.sender.send(msg.clone());
        }
    }
}

/// Dial an outbound peer, deduplicating against in-flight/established dials.
fn dial(shared: &Arc<Shared>, target: SocketAddr) {
    if is_banned(shared, target.ip()) {
        return;
    }
    {
        let mut connected = shared.connected.lock().unwrap();
        if !connected.insert(target) {
            return; // already connected or dialing
        }
    }
    let shared = shared.clone();
    thread::spawn(move || {
        let fail = |shared: &Arc<Shared>, e: String| {
            log(shared, &format!("handshake with {target} failed: {e}"));
            shared.connected.lock().unwrap().remove(&target);
        };
        match TcpStream::connect(target) {
            Ok(stream) => match transport::initiate(stream, shared.network_id) {
                Ok((r, w)) => run_peer(shared, r, w, target, true),
                Err(e) => fail(&shared, e.to_string()),
            },
            Err(e) => fail(&shared, e.to_string()),
        }
    });
}

fn run_peer(
    shared: Arc<Shared>,
    mut reader: transport::SecureReader,
    mut writer: transport::SecureWriter,
    addr: SocketAddr,
    outbound: bool,
) {
    let id = shared.next_peer_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = channel::<Message>();

    // writer thread: drain the outbox
    let writer_handle = thread::spawn(move || {
        while let Ok(msg) = rx.recv() {
            if writer.send(&msg.encode()).is_err() {
                break;
            }
        }
    });

    {
        let mut peers = shared.peers.lock().unwrap();
        peers.insert(
            id,
            PeerHandle {
                id,
                addr,
                outbound,
                sender: tx.clone(),
            },
        );
    }
    log(
        &shared,
        &format!("peer #{id} {} ({addr})", if outbound { "connected" } else { "accepted" }),
    );

    // greet, advertising our own listen port for discovery
    {
        let chain = shared.chain.lock().unwrap();
        let _ = tx.send(Message::Hello {
            version: PROTOCOL_VERSION,
            height: chain.height(),
            tip: chain.tip_hash(),
            listen_port: shared.listen_port.load(Ordering::SeqCst),
        });
    }

    // reader loop
    let mut peer_state = PeerState {
        requested_blocks: HashSet::new(),
        addr,
        outbound,
        advertised: outbound.then_some(addr),
    };
    loop {
        let frame = match reader.recv() {
            Ok(f) => f,
            Err(e) => {
                log(&shared, &format!("peer #{id} disconnected: {e}"));
                break;
            }
        };
        let msg = match Message::decode(&frame) {
            Ok(m) => m,
            Err(e) => {
                // an undecodable message is a protocol violation → ban
                log(&shared, &format!("peer #{id} sent bad message: {e}"));
                misbehave(&shared, addr.ip(), BAN_SCORE);
                break;
            }
        };
        if let Err(e) = handle_message(&shared, id, msg, &tx, &mut peer_state) {
            // invalid consensus data (bad block/tx, oversized request) → ban
            log(&shared, &format!("peer #{id} error: {e}"));
            misbehave(&shared, addr.ip(), BAN_SCORE);
            break;
        }
    }

    shared.peers.lock().unwrap().remove(&id);
    // release this peer's reachable address so the dialer may reuse it
    if let Some(a) = peer_state.advertised {
        shared.connected.lock().unwrap().remove(&a);
    }
    drop(tx);
    let _ = writer_handle.join();
}

/// Per-connection sync state, owned by the reader thread.
struct PeerState {
    /// Blocks we have asked this peer for and not yet received.
    requested_blocks: HashSet<Hash256>,
    /// The transport-level peer address (ephemeral source port if inbound).
    addr: SocketAddr,
    outbound: bool,
    /// The peer's reachable (listen) address once learned, for book/dedup.
    advertised: Option<SocketAddr>,
}

fn handle_message(
    shared: &Arc<Shared>,
    peer_id: u64,
    msg: Message,
    out: &Sender<Message>,
    peer: &mut PeerState,
) -> Result<(), String> {
    match msg {
        Message::Hello {
            height,
            tip,
            listen_port,
            ..
        } => {
            shared.best_peer_height.fetch_max(height, Ordering::SeqCst);
            // learn the peer's reachable address: for inbound peers, combine
            // the observed source IP with the advertised listen port
            if listen_port != 0 {
                let reachable = if peer.outbound {
                    peer.addr
                } else {
                    SocketAddr::new(peer.addr.ip(), listen_port)
                };
                peer.advertised = Some(reachable);
                let mut book = shared.book.lock().unwrap();
                book.insert(reachable);
                shared.connected.lock().unwrap().insert(reachable);
            }
            // ask the peer for more addresses to grow the book
            let _ = out.send(Message::GetAddr);
            let (our_height, known, locator) = {
                let chain = shared.chain.lock().unwrap();
                (chain.height(), chain.contains_block(&tip), chain.locator())
            };
            if height > our_height && !known {
                let _ = out.send(Message::GetBlocks { locator });
            }
        }
        Message::GetAddr => {
            let sample: Vec<SocketAddr> = shared
                .book
                .lock()
                .unwrap()
                .iter()
                .take(ADDR_SAMPLE)
                .copied()
                .collect();
            if !sample.is_empty() {
                let _ = out.send(Message::Addr(sample));
            }
        }
        Message::Addr(addrs) => {
            let mut fresh = Vec::new();
            {
                let mut book = shared.book.lock().unwrap();
                for a in addrs.into_iter().take(MAX_ADDRS) {
                    if a.port() != 0 && !a.ip().is_unspecified() && book.insert(a) {
                        fresh.push(a);
                    }
                }
            }
            // relay newly-learned addresses to other peers
            if !fresh.is_empty() {
                broadcast(shared, &Message::Addr(fresh), Some(peer_id));
            }
        }
        Message::Ping(n) => {
            let _ = out.send(Message::Pong(n));
        }
        Message::Pong(_) => {}
        Message::Inv { blocks, txs } => {
            let (want_blocks, want_txs) = {
                let chain = shared.chain.lock().unwrap();
                let mempool = shared.mempool.lock().unwrap();
                let wb: Vec<Hash256> = blocks
                    .into_iter()
                    .filter(|h| !chain.contains_block(h) && !peer.requested_blocks.contains(h))
                    .collect();
                let wt: Vec<Hash256> = txs
                    .into_iter()
                    .filter(|t| !mempool.contains(t))
                    .collect();
                (wb, wt)
            };
            if !want_blocks.is_empty() || !want_txs.is_empty() {
                for h in &want_blocks {
                    peer.requested_blocks.insert(*h);
                }
                let _ = out.send(Message::GetData {
                    blocks: want_blocks,
                    txs: want_txs,
                });
            }
        }
        Message::GetData { blocks, txs } => {
            if blocks.len() > MAX_INV || txs.len() > MAX_INV {
                return Err("oversized getdata".into());
            }
            for h in blocks {
                let block = shared.chain.lock().unwrap().block_by_hash(&h);
                if let Some(b) = block {
                    let _ = out.send(Message::Block(Box::new((*b).clone())));
                }
            }
            for t in txs {
                let tx = shared.mempool.lock().unwrap().get(&t).cloned();
                if let Some(tx) = tx {
                    let _ = out.send(Message::Tx(Box::new(tx)));
                }
            }
        }
        Message::GetBlocks { locator } => {
            let hashes = {
                let chain = shared.chain.lock().unwrap();
                chain.hashes_after_locator(&locator, BLOCKS_PER_ROUND)
            };
            if !hashes.is_empty() {
                let _ = out.send(Message::Inv {
                    blocks: hashes,
                    txs: vec![],
                });
            }
        }
        Message::Block(block) => {
            let hash = block.header.hash();
            peer.requested_blocks.remove(&hash);
            let batch_done = peer.requested_blocks.is_empty();
            match accept_block(shared, *block, Some(peer_id)) {
                Ok(_) => {
                    // ask for the next batch only once this one is complete —
                    // per-block GetBlocks floods the peer with redundant work
                    if batch_done {
                        let locator = shared.chain.lock().unwrap().locator();
                        let _ = out.send(Message::GetBlocks { locator });
                    }
                }
                Err(ValidationError::UnknownParent) => {
                    // orphan: restart sync from our fork point
                    peer.requested_blocks.clear();
                    let locator = shared.chain.lock().unwrap().locator();
                    let _ = out.send(Message::GetBlocks { locator });
                }
                Err(ValidationError::Duplicate) => {}
                Err(e) => return Err(format!("invalid block: {e}")),
            }
        }
        Message::Tx(tx) => {
            let txid = tx.txid();
            let inserted = {
                let chain = shared.chain.lock().unwrap();
                let mut mempool = shared.mempool.lock().unwrap();
                mempool.insert(&chain, *tx)
            };
            match inserted {
                Ok(_) => {
                    broadcast(
                        shared,
                        &Message::Inv {
                            blocks: vec![],
                            txs: vec![txid],
                        },
                        Some(peer_id),
                    );
                }
                // benign: already have it, conflicts, underpriced, or pool
                // full — drop without penalizing the peer
                Err(MempoolError::Duplicate)
                | Err(MempoolError::Conflict)
                | Err(MempoolError::LowFee { .. })
                | Err(MempoolError::Full) => {}
                // relaying a consensus-invalid tx is misbehavior → ban
                Err(MempoolError::Invalid(e)) => {
                    return Err(format!("invalid tx relayed: {e}"));
                }
            }
        }
    }
    Ok(())
}

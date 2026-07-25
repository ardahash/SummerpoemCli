//! `sump` — Summerpoem reference CLI: genesis builder, chain inspection,
//! wallet, and CPU reference miner.

mod dashboard;
mod mining;
mod wallet;

use mining::Rig;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use sump_core::emission::COIN;
use sump_core::params::{Network, Params};
use sump_core::tx::Transaction;
use sump_crypto::address;
use sump_node::chain::ChainState;
use sump_node::{genesis, miner, store};
use sump_pow::{epoch_of_height, PowContext};
use wallet::Wallet;

#[derive(Parser)]
#[command(name = "sump", version, about = "Summerpoem (SUMP) reference node & tools")]
struct Cli {
    /// Network: regtest or mainnet
    #[arg(long, global = true, default_value = "mainnet")]
    network: String,
    /// Data directory holding chain.dat and the mempool
    #[arg(long, global = true, default_value = "./sumpchain")]
    chain_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a regtest chain directory (mainnet genesis is built in)
    Genesis,
    /// Chain inspection and maintenance
    Node {
        #[command(subcommand)]
        cmd: NodeCmd,
    },
    /// Key and coin management
    Wallet {
        #[command(subcommand)]
        cmd: WalletCmd,
    },
    /// Standalone miner (GPU or CPU)
    Miner {
        #[command(subcommand)]
        cmd: MinerCmd,
    },
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Show chain status
    Info,
    /// Show the software-version distribution of recent blocks (upgrade adoption)
    Versions {
        /// How many recent blocks to sample
        #[arg(long, default_value_t = 200)]
        blocks: u64,
    },
    /// Re-validate the whole chain from genesis
    Validate,
    /// Validate a transaction file and place it in the mempool
    Submit { tx_file: PathBuf },
    /// Run a networked node (P2P over ML-KEM encrypted transport)
    Run {
        /// Address to listen on for peers (0.0.0.0 = reachable by other
        /// machines; use 127.0.0.1 to stay local-only)
        #[arg(long, default_value = "0.0.0.0:8776")]
        listen: String,
        /// Peer address(es) to connect to (repeatable)
        #[arg(long)]
        connect: Vec<String>,
        /// Do not dial built-in seed nodes. Use only for public seed/bootnode
        /// operators that are themselves the bootstrap address.
        #[arg(long)]
        no_default_seeds: bool,
        /// Also mine blocks while running
        #[arg(long)]
        mine: bool,
        /// Use the GPU (CUDA) miner, falling back to CPU if unavailable
        #[arg(long)]
        gpu: bool,
        /// Wallet receiving block rewards when --mine is set
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
        /// Serve a local web dashboard (GUI) at this address
        #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:8787")]
        gui: Option<String>,
        /// Expose the wallet RPC (for standalone wallets) at this address.
        /// Use 0.0.0.0:PORT to let other machines' wallets connect.
        #[arg(long, num_args = 0..=1, default_missing_value = "127.0.0.1:8788")]
        rpc: Option<String>,
    },
}

#[derive(Subcommand)]
enum WalletCmd {
    /// Create a new wallet file
    New {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Show the wallet's everyday receiving address (ML-DSA, index 0)
    Address {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Show the wallet's vault address (SLH-DSA, hash-based cold storage)
    VaultAddress {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Show confirmed spendable balance
    Balance {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Build and sign a payment, writing it to the mempool
    Send {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
        /// Recipient address
        #[arg(long)]
        to: String,
        /// Amount in SUMP (decimal, e.g. 1.5)
        #[arg(long)]
        amount: String,
        /// Fee in stanzas
        #[arg(long, default_value_t = 100_000)]
        fee: u64,
    },
}

#[derive(Subcommand)]
enum MinerCmd {
    /// Mine N blocks, paying rewards to the wallet, including mempool txs
    Mine {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
        #[arg(long, default_value_t = 1)]
        blocks: u64,
        /// Use the GPU (CUDA) miner, falling back to CPU if unavailable
        #[arg(long)]
        gpu: bool,
    },
}

fn parse_network(s: &str) -> Result<Network> {
    match s {
        "mainnet" => Ok(Network::Mainnet),
        "regtest" => Ok(Network::Regtest),
        _ => bail!("unknown network '{s}' (expected mainnet or regtest)"),
    }
}

fn chain_file(dir: &Path) -> PathBuf {
    dir.join("chain.dat")
}

fn mempool_dir(dir: &Path) -> PathBuf {
    dir.join("mempool")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn format_sump(stanzas: u64) -> String {
    format!("{}.{:08}", stanzas / COIN, stanzas % COIN)
}

fn parse_sump(s: &str) -> Result<u64> {
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if frac.len() > 8 {
        bail!("at most 8 decimal places (stanzas)");
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().context("bad amount")?
    };
    let mut frac_val: u64 = 0;
    if !frac.is_empty() {
        frac_val = format!("{frac:0<8}").parse().context("bad amount")?;
    }
    whole
        .checked_mul(COIN)
        .and_then(|w| w.checked_add(frac_val))
        .ok_or_else(|| anyhow!("amount overflow"))
}

/// Persist the fixed network genesis as the first block in a fresh chain
/// database. Mainnet uses a baked-in nonce; regtest may mine an easy local
/// genesis for isolated testing.
fn write_initial_chain(
    params: &Params,
    dir: &Path,
    ctx: &PowContext,
) -> Result<sump_core::Hash256> {
    std::fs::create_dir_all(mempool_dir(dir))?;
    let block = genesis::build_genesis(params, ctx);
    let hash = block.header.hash();
    let state = ChainState::new(params.clone(), block)
        .map_err(|e| anyhow!("initial genesis failed validation: {e}"))?;
    save_state(&state, dir)?;
    Ok(hash)
}

fn init_chain_from_genesis(params: &Params, dir: &Path) -> Result<sump_core::Hash256> {
    if params.network == Network::Regtest {
        eprintln!("creating regtest genesis block...");
    } else {
        eprintln!("initializing chain from built-in mainnet genesis...");
    }
    // A light context suffices: a hardcoded-nonce genesis is only verified
    // (not mined), and regtest's easy target resolves in a few light hashes.
    let ctx = PowContext::new_light(&params.pow, 0);
    write_initial_chain(params, dir, &ctx)
}

fn load_state(params: &Params, dir: &Path) -> Result<ChainState> {
    let path = chain_file(dir);
    if !path.exists() {
        let hint = if params.network == Network::Regtest {
            "run `sump --network regtest genesis` first"
        } else {
            "run `sump node run` to initialize and sync"
        };
        bail!(
            "no chain at {} - {hint}",
            path.display()
        );
    }
    let (net_id, blocks) = store::load(&path)?;
    if net_id != params.network.id() {
        bail!("chain file belongs to a different network");
    }
    let mut iter = blocks.into_iter();
    let genesis_block = iter.next().ok_or_else(|| anyhow!("empty chain file"))?;
    let mut state = ChainState::new(params.clone(), genesis_block)
        .map_err(|e| anyhow!("invalid genesis in chain file: {e}"))?;
    for b in iter {
        state
            .add_block(b)
            .map_err(|e| anyhow!("invalid block in chain file: {e}"))?;
    }
    Ok(state)
}

fn save_state(state: &ChainState, dir: &Path) -> Result<()> {
    store::save(
        &chain_file(dir),
        state.params().network.id(),
        &state.active_blocks(),
    )?;
    Ok(())
}

fn load_mempool(dir: &Path) -> Result<Vec<(PathBuf, Transaction)>> {
    let mp = mempool_dir(dir);
    let mut out = Vec::new();
    if !mp.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&mp)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("tx") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)?;
        let bytes = hex::decode(raw.trim()).context("mempool tx hex")?;
        match Transaction::decode_all(&bytes) {
            Ok(tx) => out.push((path, tx)),
            Err(e) => eprintln!("skipping {}: {e}", path.display()),
        }
    }
    Ok(out)
}

fn mainnet_mining_wait_reason(params: &Params, node: &sump_net::NetNode) -> Option<String> {
    if params.network != Network::Mainnet {
        return None;
    }
    let peers = node.peer_count();
    if peers == 0 {
        return Some("waiting for public peers before mining".into());
    }
    let local_height = node.height();
    let best_peer_height = node.best_peer_height();
    if local_height < best_peer_height {
        return Some(format!(
            "syncing before mining: local height {local_height}, peer height {best_peer_height}"
        ));
    }
    None
}

/// Enable ANSI color escape processing on the Windows console so colored log
/// output renders in cmd.exe/conhost as well as Windows Terminal.
#[cfg(windows)]
fn enable_ansi() {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        for h in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = GetStdHandle(h);
            let mut mode = 0u32;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}
#[cfg(not(windows))]
fn enable_ansi() {}

fn main() -> Result<()> {
    enable_ansi();
    let cli = Cli::parse();
    let network = parse_network(&cli.network)?;
    let params = Params::for_network(network);
    let dir = cli.chain_dir.clone();

    match cli.command {
        Command::Genesis => {
            if params.network != Network::Regtest {
                bail!(
                    "`sump genesis` is regtest-only; mainnet genesis is built \
                     into this release. Run `sump node run` to initialize and sync."
                );
            }
            let path = chain_file(&dir);
            if path.exists() {
                bail!("chain already exists at {}", path.display());
            }
            let hash = init_chain_from_genesis(&params, &dir)?;
            println!("regtest genesis created: {hash}");
            println!("chain dir: {}", dir.display());
        }

        Command::Node { cmd } => {
            match cmd {
                NodeCmd::Info => {
                    let state = load_state(&params, &dir)?;
                    println!("software: v{}", env!("CARGO_PKG_VERSION"));
                    println!("network:  {:?}", params.network);
                    println!("height:   {}", state.height());
                    println!("tip:      {}", state.tip_hash());
                    println!("supply:   {} SUMP", format_sump(state.supply()));
                    println!("utxos:    {}", state.utxos().len());
                }
                NodeCmd::Versions { blocks } => {
                    let state = load_state(&params, &dir)?;
                    let tip = state.height();
                    let start = tip.saturating_sub(blocks.saturating_sub(1));
                    let mut counts: std::collections::BTreeMap<String, u64> =
                        std::collections::BTreeMap::new();
                    let mut sampled = 0u64;
                    for h in start..=tip {
                        if let Some(block) = state.block_at(h) {
                            let cb = &block.transactions[0].body.coinbase_data;
                            let label = match sump_core::tx::coinbase_version(cb) {
                                Some((a, b, c)) => format!("v{a}.{b}.{c}"),
                                None => "pre-0.5.7 / unknown".to_string(),
                            };
                            *counts.entry(label).or_insert(0) += 1;
                            sampled += 1;
                        }
                    }
                    println!("software versions of the last {sampled} block(s):");
                    let mut rows: Vec<_> = counts.into_iter().collect();
                    rows.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
                    for (label, n) in rows {
                        let pct = n.checked_mul(100).unwrap_or(0) / sampled.max(1);
                        println!("  {label:<22} {n:>6}  ({pct}%)");
                    }
                }
                NodeCmd::Validate => {
                    // load_state fully re-validates from genesis
                    let state = load_state(&params, &dir)?;
                    println!(
                        "chain valid: {} blocks, tip {}",
                        state.height() + 1,
                        state.tip_hash()
                    );
                }
                NodeCmd::Run {
                    listen,
                    connect,
                    no_default_seeds,
                    mine,
                    gpu,
                    wallet,
                    gui,
                    rpc,
                } => {
                    // First launch: initialize the local database from the
                    // fixed network genesis, then sync from peers.
                    if !chain_file(&dir).exists() {
                        let hash = init_chain_from_genesis(&params, &dir)?;
                        println!("chain initialized from genesis: {hash}");
                    }
                    let state = load_state(&params, &dir)?;
                    // load the wallet if we mine or serve a GUI (for payout/balance)
                    let wallet_obj = if mine || gui.is_some() {
                        Some(Wallet::load(&wallet)?)
                    } else {
                        None
                    };
                    let payout = if mine {
                        Some(
                            wallet_obj
                                .as_ref()
                                .unwrap()
                                .key(sump_core::tx::SigScheme::MlDsa, 0)
                                .pubkey_hash(),
                        )
                    } else {
                        None
                    };
                    let node = sump_net::NetNode::new(state, Some(chain_file(&dir)), false);
                    let bound = node.listen(&listen)?;
                    println!(
                        "Summerpoem v{} — listening on {bound} (network {:?})",
                        env!("CARGO_PKG_VERSION"),
                        params.network
                    );
                    for peer in &connect {
                        match node.connect(peer) {
                            Ok(()) => println!("connecting to {peer}..."),
                            Err(e) => eprintln!("cannot connect to {peer}: {e}"),
                        }
                    }
                    // peer discovery: seed the address book (explicit peers +
                    // network seed nodes) and maintain outbound connections
                    let mut seeds: Vec<String> = if no_default_seeds {
                        Vec::new()
                    } else {
                        params.seeds.iter().map(|s| s.to_string()).collect()
                    };
                    seeds.extend(connect.iter().cloned());
                    if params.network == Network::Mainnet
                        && seeds.is_empty()
                        && !no_default_seeds
                    {
                        bail!(
                            "mainnet has no seed or --connect peer configured; \
                             refusing to start an isolated node"
                        );
                    }
                    node.start_discovery(&seeds);
                    if no_default_seeds {
                        println!("discovery: built-in seed dialing disabled");
                    } else if !params.seeds.is_empty() {
                        println!("discovery: {} seed node(s)", params.seeds.len());
                    }

                    let stats = dashboard::MinerStats::new();

                    // dashboard GUI server (also serves the wallet RPC)
                    if let Some(gui_addr) = &gui {
                        let w = wallet_obj.as_ref().unwrap();
                        let owned: Vec<(u8, [u8; 20])> = w
                            .keys()
                            .iter()
                            .map(|k| (k.scheme.id(), k.pubkey_hash()))
                            .collect();
                        let ctx = dashboard::DashboardCtx {
                            stats: stats.clone(),
                            owned,
                            address: wallet::address(w, &params, 0),
                            vault: wallet::vault_address(w, &params, 0),
                            network: format!("{:?}", params.network),
                        };
                        match dashboard::serve(gui_addr, node.clone(), Some(ctx)) {
                            Ok(addr) => println!("dashboard: http://{addr}"),
                            Err(e) => eprintln!("could not start dashboard: {e}"),
                        }
                    }

                    // wallet RPC server (API only; for standalone wallets)
                    if let Some(rpc_addr) = &rpc {
                        match dashboard::serve(rpc_addr, node.clone(), None) {
                            Ok(addr) => println!("wallet RPC: http://{addr} (POST /api/utxos, /api/submit)"),
                            Err(e) => eprintln!("could not start RPC: {e}"),
                        }
                    }

                    if let Some(payout) = payout {
                        let miner_node = node.clone();
                        let miner_params = params.clone();
                        let miner_stats = stats.clone();
                        miner_stats
                            .mining
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        miner_stats
                            .gpu
                            .store(gpu, std::sync::atomic::Ordering::Relaxed);
                        std::thread::spawn(move || {
                            use std::sync::atomic::Ordering;
                            let mut ctx_epoch = u64::MAX;
                            let mut ctx: Option<PowContext> = None;
                            let mut rig: Option<Rig> = None;
                            // GPU launches hash a whole chunk before returning;
                            // keep it small enough that the template refreshes
                            // often for new transactions.
                            let gpu_chunk = 1_048_576u32;
                            let mut wait_reason = String::new();
                            loop {
                                if let Some(reason) =
                                    mainnet_mining_wait_reason(&miner_params, &miner_node)
                                {
                                    miner_stats
                                        .mining
                                        .store(false, Ordering::Relaxed);
                                    if reason != wait_reason {
                                        eprintln!("\x1b[36m[miner]\x1b[0m {reason}");
                                        wait_reason = reason;
                                    }
                                    std::thread::sleep(std::time::Duration::from_secs(2));
                                    continue;
                                }
                                if !wait_reason.is_empty() {
                                    eprintln!(
                                        "\x1b[36m[miner]\x1b[0m sync complete; mining starting"
                                    );
                                    wait_reason.clear();
                                }
                                miner_stats
                                    .mining
                                    .store(true, Ordering::Relaxed);
                                let (mut template, height) = {
                                    let shared = miner_node.shared();
                                    let chain = shared.chain.lock().unwrap();
                                    let txs = shared.mempool.lock().unwrap().transactions();
                                    let height = chain.height() + 1;
                                    (
                                        sump_node::miner::build_block_template(
                                            &chain, &txs, payout, now(),
                                        ),
                                        height,
                                    )
                                };
                                let epoch =
                                    epoch_of_height(height, miner_params.pow.epoch_length);
                                if epoch != ctx_epoch {
                                    eprintln!("\x1b[36m[miner]\x1b[0m preparing to mine epoch {epoch}");
                                    let c = PowContext::new_full(&miner_params.pow, epoch);
                                    let selected = Rig::select(&c, gpu);
                                    miner_stats.gpu.store(selected.is_gpu(), Ordering::Relaxed);
                                    rig = Some(selected);
                                    ctx = Some(c);
                                    ctx_epoch = epoch;
                                }
                                // one bounded attempt, then loop to refresh the
                                // template against new txs / a new tip
                                let found = rig.as_ref().unwrap().try_mine(
                                    ctx.as_ref().unwrap(),
                                    &mut template,
                                    gpu_chunk,
                                );
                                miner_stats.add_hashes(gpu_chunk as u64);
                                if found {
                                    match miner_node.submit_block(template) {
                                        Ok(true) => {}
                                        Ok(false) => {}
                                        Err(e) => eprintln!("[miner] block rejected: {e}"),
                                    }
                                }
                            }
                        });
                        if params.network == Network::Mainnet {
                            println!(
                                "mining enabled ({}; waits for peers and sync)",
                                if gpu { "GPU requested" } else { "CPU" }
                            );
                        } else {
                            println!(
                                "mining enabled ({})",
                                if gpu { "GPU requested" } else { "CPU" }
                            );
                        }
                    }

                    // main loop: bridge wallet tx files from the mempool dir
                    let mp = mempool_dir(&dir);
                    std::fs::create_dir_all(&mp)?;
                    println!("watching {} for transactions; ctrl-c to stop", mp.display());
                    loop {
                        for (path, tx) in load_mempool(&dir)? {
                            match node.submit_tx(tx) {
                                Ok(_) => {
                                    let _ = std::fs::remove_file(&path);
                                    println!("accepted tx from {}", path.display());
                                }
                                Err(sump_node::MempoolError::Duplicate) => {
                                    let _ = std::fs::remove_file(&path);
                                }
                                Err(sump_node::MempoolError::Conflict) => {}
                                Err(sump_node::MempoolError::LowFee { need }) => {
                                    let _ = std::fs::remove_file(&path);
                                    eprintln!(
                                        "rejected {}: fee too low (need at least {need} stanzas)",
                                        path.display()
                                    );
                                }
                                Err(sump_node::MempoolError::Full) => {
                                    // retry later once peers mine or relay blocks
                                }
                                Err(sump_node::MempoolError::Invalid(_)) => {
                                    // likely not yet valid (immature); retry later
                                }
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                }
                NodeCmd::Submit { tx_file } => {
                    let state = load_state(&params, &dir)?;
                    let raw = std::fs::read_to_string(&tx_file)?;
                    let bytes = hex::decode(raw.trim()).context("tx hex")?;
                    let tx = Transaction::decode_all(&bytes)
                        .map_err(|e| anyhow!("tx decode: {e}"))?;
                    let fee = state
                        .validate_standalone_tx(&tx)
                        .map_err(|e| anyhow!("tx invalid: {e}"))?;
                    let mp = mempool_dir(&dir);
                    std::fs::create_dir_all(&mp)?;
                    let dest = mp.join(format!("{}.tx", tx.txid()));
                    std::fs::write(&dest, hex::encode(tx.encode()))?;
                    println!("accepted {} (fee {} stanzas)", tx.txid(), fee);
                }
            }
        }

        Command::Wallet { cmd } => match cmd {
            WalletCmd::New { wallet } => {
                let w = Wallet::create(&wallet, &cli.network)?;
                println!("wallet created: {}", wallet.display());
                println!("address:       {}", wallet::address(&w, &params, 0));
                println!("vault address: {}", wallet::vault_address(&w, &params, 0));
            }
            WalletCmd::Address { wallet } => {
                let w = Wallet::load(&wallet)?;
                println!("{}", wallet::address(&w, &params, 0));
            }
            WalletCmd::VaultAddress { wallet } => {
                let w = Wallet::load(&wallet)?;
                println!("{}", wallet::vault_address(&w, &params, 0));
            }
            WalletCmd::Balance { wallet } => {
                let w = Wallet::load(&wallet)?;
                let state = load_state(&params, &dir)?;
                println!("{} SUMP", format_sump(wallet::balance(&w, &state)));
            }
            WalletCmd::Send {
                wallet,
                to,
                amount,
                fee,
            } => {
                let w = Wallet::load(&wallet)?;
                let state = load_state(&params, &dir)?;
                let (version, pkh) = address::decode(params.address_hrp, &to)
                    .ok_or_else(|| anyhow!("invalid address for this network"))?;
                let scheme = address::scheme_for_version(version)
                    .ok_or_else(|| anyhow!("unsupported address version {version}"))?;
                let amount = parse_sump(&amount)?;
                let tx = wallet::send(&w, &state, scheme, pkh, amount, fee)?;
                state
                    .validate_standalone_tx(&tx)
                    .map_err(|e| anyhow!("built tx failed validation: {e}"))?;
                let mp = mempool_dir(&dir);
                std::fs::create_dir_all(&mp)?;
                let dest = mp.join(format!("{}.tx", tx.txid()));
                std::fs::write(&dest, hex::encode(tx.encode()))?;
                println!("queued {} ({} SUMP + {} fee)", tx.txid(), format_sump(amount), fee);
                println!("it will be included by the next mined block");
            }
        },

        Command::Miner { cmd } => match cmd {
            MinerCmd::Mine {
                wallet,
                blocks,
                gpu,
            } => {
                if params.network == Network::Mainnet {
                    bail!(
                        "`sump miner mine` is disabled on mainnet because it \
                         mines without peer sync; use `sump node run --mine --gpu --gui`."
                    );
                }
                let w = Wallet::load(&wallet)?;
                let mut state = load_state(&params, &dir)?;
                let payout = w.key(sump_core::tx::SigScheme::MlDsa, 0).pubkey_hash();
                let mut ctx_epoch = u64::MAX;
                let mut ctx: Option<PowContext> = None;
                let mut rig: Option<Rig> = None;
                for _ in 0..blocks {
                    let height = state.height() + 1;
                    let epoch = epoch_of_height(height, params.pow.epoch_length);
                    if epoch != ctx_epoch {
                        eprintln!("generating PoW dataset for epoch {epoch}...");
                        let c = PowContext::new_full(&params.pow, epoch);
                        rig = Some(Rig::select(&c, gpu));
                        ctx = Some(c);
                        ctx_epoch = epoch;
                    }
                    let mempool = load_mempool(&dir)?;
                    let txs: Vec<Transaction> =
                        mempool.iter().map(|(_, t)| t.clone()).collect();
                    let ctx_ref = ctx.as_ref().unwrap();
                    let mut block =
                        miner::build_block_template(&state, &txs, payout, now());
                    rig.as_ref().unwrap().mine(ctx_ref, &mut block);
                    state
                        .add_block(block.clone())
                        .map_err(|e| anyhow!("mined block rejected: {e}"))?;
                    // clear included txs from the mempool
                    let included: Vec<_> =
                        block.transactions[1..].iter().map(|t| t.txid()).collect();
                    for (path, tx) in &mempool {
                        if included.contains(&tx.txid()) {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    println!(
                        "mined block {} ({} txs) {}",
                        state.height(),
                        block.transactions.len(),
                        block.header.hash()
                    );
                }
                save_state(&state, &dir)?;
            }
        },
    }
    Ok(())
}

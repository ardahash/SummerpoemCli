//! `sump` — Summerpoem reference CLI: genesis builder, chain inspection,
//! wallet, and CPU reference miner.

mod wallet;

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
    #[arg(long, global = true, default_value = "regtest")]
    network: String,
    /// Data directory holding chain.dat and the mempool
    #[arg(long, global = true, default_value = "./sumpchain")]
    chain_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the genesis block and initialize the chain directory
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
    /// CPU reference miner
    Miner {
        #[command(subcommand)]
        cmd: MinerCmd,
    },
}

#[derive(Subcommand)]
enum NodeCmd {
    /// Show chain status
    Info,
    /// Re-validate the whole chain from genesis
    Validate,
    /// Validate a transaction file and place it in the mempool
    Submit { tx_file: PathBuf },
}

#[derive(Subcommand)]
enum WalletCmd {
    /// Create a new wallet file
    New {
        #[arg(long, default_value = "wallet.json")]
        wallet: PathBuf,
    },
    /// Show the wallet's receiving address (index 0)
    Address {
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

fn load_state(params: &Params, dir: &Path) -> Result<ChainState> {
    let path = chain_file(dir);
    if !path.exists() {
        bail!(
            "no chain at {} — run `sump genesis` first",
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let network = parse_network(&cli.network)?;
    let params = Params::for_network(network);
    let dir = cli.chain_dir.clone();

    match cli.command {
        Command::Genesis => {
            let path = chain_file(&dir);
            if path.exists() {
                bail!("chain already exists at {}", path.display());
            }
            std::fs::create_dir_all(mempool_dir(&dir))?;
            if network == Network::Mainnet {
                eprintln!(
                    "note: building the mainnet dataset ({} MiB cache) and mining genesis \
                     on CPU may take a long time",
                    params.pow.cache_bytes >> 20
                );
            }
            eprintln!("generating epoch-0 PoW dataset...");
            let ctx = PowContext::new_full(&params.pow, 0);
            eprintln!("mining genesis block...");
            let block = genesis::build_genesis(&params, &ctx);
            let hash = block.header.hash();
            let state = ChainState::new(params.clone(), block)
                .map_err(|e| anyhow!("genesis failed validation: {e}"))?;
            save_state(&state, &dir)?;
            println!("genesis created: {hash}");
            println!("chain dir: {}", dir.display());
        }

        Command::Node { cmd } => {
            match cmd {
                NodeCmd::Info => {
                    let state = load_state(&params, &dir)?;
                    println!("network:  {:?}", params.network);
                    println!("height:   {}", state.height());
                    println!("tip:      {}", state.tip_hash());
                    println!("supply:   {} SUMP", format_sump(state.supply()));
                    println!("utxos:    {}", state.utxos().len());
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
                println!("address: {}", w.address(&params, 0));
            }
            WalletCmd::Address { wallet } => {
                let w = Wallet::load(&wallet)?;
                println!("{}", w.address(&params, 0));
            }
            WalletCmd::Balance { wallet } => {
                let w = Wallet::load(&wallet)?;
                let state = load_state(&params, &dir)?;
                println!("{} SUMP", format_sump(w.balance(&state)));
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
                if version != address::VERSION_MLDSA {
                    bail!("unsupported address version {version}");
                }
                let amount = parse_sump(&amount)?;
                let tx = w.build_send(&state, pkh, amount, fee)?;
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
            MinerCmd::Mine { wallet, blocks } => {
                let w = Wallet::load(&wallet)?;
                let mut state = load_state(&params, &dir)?;
                let payout = w.key(0).pubkey_hash();
                let mut ctx_epoch = u64::MAX;
                let mut ctx: Option<PowContext> = None;
                for _ in 0..blocks {
                    let height = state.height() + 1;
                    let epoch = epoch_of_height(height, params.pow.epoch_length);
                    if epoch != ctx_epoch {
                        eprintln!("generating PoW dataset for epoch {epoch}...");
                        ctx = Some(PowContext::new_full(&params.pow, epoch));
                        ctx_epoch = epoch;
                    }
                    let mempool = load_mempool(&dir)?;
                    let txs: Vec<Transaction> =
                        mempool.iter().map(|(_, t)| t.clone()).collect();
                    let block = miner::mine_and_connect(
                        &mut state,
                        ctx.as_ref().unwrap(),
                        &txs,
                        payout,
                        now(),
                    )
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

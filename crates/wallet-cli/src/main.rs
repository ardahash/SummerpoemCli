//! `sump-wallet` — a standalone light wallet. Holds keys locally and talks to
//! a Summerpoem node's RPC (`--rpc` on the node) to read balances and submit
//! payments. Runs as a CLI or a local web GUI.

mod rpc;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use sump_core::emission::COIN;
use sump_core::params::Params;
use sump_crypto::address;
use sump_wallet::{balances, build_send, Wallet};

#[derive(Parser)]
#[command(name = "sump-wallet", version, about = "Summerpoem standalone light wallet")]
struct Cli {
    #[arg(long, global = true, default_value = "wallet.json")]
    wallet: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new wallet
    New {
        #[arg(long, default_value = "mainnet")]
        network: String,
    },
    /// Restore a wallet from a 32-byte seed (hex)
    Restore {
        #[arg(long)]
        seed: String,
        #[arg(long, default_value = "mainnet")]
        network: String,
    },
    /// Show the everyday receiving address (ML-DSA)
    Address,
    /// Show the vault address (SLH-DSA, cold storage)
    VaultAddress,
    /// Show the master seed (for backup) — keep it secret
    ExportSeed,
    /// Show balance (queries a node)
    Balance {
        #[arg(long)]
        node: String,
    },
    /// Send SUMP (builds, signs, and submits via a node)
    Send {
        #[arg(long)]
        node: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        amount: String,
        #[arg(long, default_value_t = 100_000)]
        fee: u64,
    },
    /// Serve the local web wallet GUI
    Gui {
        #[arg(long)]
        node: String,
        #[arg(long, default_value = "127.0.0.1:8799")]
        bind: String,
    },
}

fn hrp_for(network: &str) -> Result<&'static str> {
    match network {
        "mainnet" => Ok(Params::mainnet().address_hrp),
        "regtest" => Ok(Params::regtest().address_hrp),
        other => bail!("unknown network '{other}'"),
    }
}

fn network_of(w: &Wallet) -> String {
    w.file.network.clone()
}

fn format_sump(stanzas: u64) -> String {
    format!("{}.{:08}", stanzas / COIN, stanzas % COIN)
}

fn parse_sump(s: &str) -> Result<u64> {
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    if frac.len() > 8 {
        bail!("at most 8 decimal places");
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().context("bad amount")?
    };
    let frac_val: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<8}").parse().context("bad amount")?
    };
    whole
        .checked_mul(COIN)
        .and_then(|w| w.checked_add(frac_val))
        .ok_or_else(|| anyhow!("amount overflow"))
}

/// Fetch UTXOs and return (spendable, pending, height).
fn query_balance(w: &Wallet, node: &str) -> Result<(u64, u64, u64)> {
    let reply = rpc::fetch_utxos(node, &w.owned_ids())?;
    let (spendable, pending) = balances(&reply.utxos, reply.height + 1, reply.maturity);
    Ok((spendable, pending, reply.height))
}

/// Build, sign, and submit a payment. Returns the txid.
fn do_send(w: &Wallet, node: &str, to: &str, amount: u64, fee: u64) -> Result<String> {
    let hrp = hrp_for(&network_of(w))?;
    let (version, pkh) =
        address::decode(hrp, to).ok_or_else(|| anyhow!("invalid address for this network"))?;
    let scheme = address::scheme_for_version(version)
        .ok_or_else(|| anyhow!("unsupported address version {version}"))?;
    let reply = rpc::fetch_utxos(node, &w.owned_ids())?;
    let tx = build_send(
        w,
        &reply.utxos,
        reply.height + 1,
        reply.maturity,
        scheme,
        pkh,
        amount,
        fee,
    )?;
    rpc::submit_tx(node, &hex::encode(tx.encode()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New { network } => {
            let w = Wallet::create(&cli.wallet, &network)?;
            let hrp = hrp_for(&network)?;
            println!("wallet created: {}", cli.wallet.display());
            println!("address:       {}", w.address(hrp, 0));
            println!("vault address: {}", w.vault_address(hrp, 0));
            println!("\nBack up {} — it is the only copy of your keys.", cli.wallet.display());
        }
        Command::Restore { seed, network } => {
            let w = Wallet::restore(&cli.wallet, &network, &seed)?;
            let hrp = hrp_for(&network)?;
            println!("wallet restored: {}", cli.wallet.display());
            println!("address: {}", w.address(hrp, 0));
        }
        Command::Address => {
            let w = Wallet::load(&cli.wallet)?;
            println!("{}", w.address(hrp_for(&network_of(&w))?, 0));
        }
        Command::VaultAddress => {
            let w = Wallet::load(&cli.wallet)?;
            println!("{}", w.vault_address(hrp_for(&network_of(&w))?, 0));
        }
        Command::ExportSeed => {
            let w = Wallet::load(&cli.wallet)?;
            println!("{}", w.seed_hex());
        }
        Command::Balance { node } => {
            let w = Wallet::load(&cli.wallet)?;
            let (spendable, pending, height) = query_balance(&w, &node)?;
            println!("height:    {height}");
            println!("spendable: {} SUMP", format_sump(spendable));
            println!("pending:   {} SUMP (maturing)", format_sump(pending));
        }
        Command::Send {
            node,
            to,
            amount,
            fee,
        } => {
            let w = Wallet::load(&cli.wallet)?;
            let amount = parse_sump(&amount)?;
            let txid = do_send(&w, &node, &to, amount, fee)?;
            println!("sent {} SUMP (+{} fee)", format_sump(amount), fee);
            println!("txid: {txid}");
        }
        Command::Gui { node, bind } => {
            let w = Arc::new(Wallet::load(&cli.wallet)?);
            let addr = serve_gui(&bind, w, node)?;
            println!("wallet GUI: http://{addr}");
            println!("press ctrl-c to stop");
            loop {
                thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// web GUI server
// ---------------------------------------------------------------------------

const PAGE: &str = include_str!("wallet.html");

fn json_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            c => vec![c],
        })
        .collect()
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

fn read_request(stream: &mut TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
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
    let mut clen = 0usize;
    for l in lines {
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            clen = v.trim().parse().unwrap_or(0).min(1 << 20);
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < clen {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(clen);
    Some((method, path, body))
}

fn status_json(w: &Wallet, node: &str) -> String {
    let hrp = hrp_for(&network_of(w)).unwrap_or("sump");
    let address = w.address(hrp, 0);
    let vault = w.vault_address(hrp, 0);
    match query_balance(w, node) {
        Ok((spendable, pending, height)) => format!(
            "{{\"connected\":true,\"network\":\"{}\",\"height\":{},\"balance\":{:.8},\
             \"pending\":{:.8},\"address\":\"{}\",\"vault\":\"{}\",\"node\":\"{}\"}}",
            json_escape(&network_of(w)),
            height,
            spendable as f64 / COIN as f64,
            pending as f64 / COIN as f64,
            json_escape(&address),
            json_escape(&vault),
            json_escape(node),
        ),
        Err(e) => format!(
            "{{\"connected\":false,\"error\":\"{}\",\"address\":\"{}\",\"vault\":\"{}\",\"node\":\"{}\"}}",
            json_escape(&e.to_string()),
            json_escape(&address),
            json_escape(&vault),
            json_escape(node),
        ),
    }
}

fn send_json(w: &Wallet, node: &str, body: &[u8]) -> String {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return format!("{{\"error\":\"bad request: {}\"}}", json_escape(&e.to_string())),
    };
    let to = v.get("to").and_then(|t| t.as_str()).unwrap_or("");
    let amount_str = v.get("amount").and_then(|t| t.as_str()).unwrap_or("");
    let fee = v.get("fee").and_then(|t| t.as_u64()).unwrap_or(100_000);
    let amount = match parse_sump(amount_str) {
        Ok(a) => a,
        Err(e) => return format!("{{\"error\":\"{}\"}}", json_escape(&e.to_string())),
    };
    match do_send(w, node, to, amount, fee) {
        Ok(txid) => format!("{{\"txid\":\"{}\"}}", json_escape(&txid)),
        Err(e) => format!("{{\"error\":\"{}\"}}", json_escape(&e.to_string())),
    }
}

fn serve_gui(bind: &str, wallet: Arc<Wallet>, node: String) -> Result<std::net::SocketAddr> {
    let listener = TcpListener::bind(bind).with_context(|| format!("binding {bind}"))?;
    let local = listener.local_addr()?;
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(15)));
            let Some((method, path, body)) = read_request(&mut stream) else {
                continue;
            };
            let wallet = wallet.clone();
            let node = node.clone();
            thread::spawn(move || {
                let route = path.split('?').next().unwrap_or("/");
                match (method.as_str(), route) {
                    (_, "/status") => {
                        respond(stream, "application/json", &status_json(&wallet, &node))
                    }
                    ("POST", "/send") => {
                        respond(stream, "application/json", &send_json(&wallet, &node, &body))
                    }
                    _ => respond(stream, "text/html; charset=utf-8", PAGE),
                }
            });
        }
    });
    Ok(local)
}

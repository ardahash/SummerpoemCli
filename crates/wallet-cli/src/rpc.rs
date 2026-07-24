//! Minimal HTTP client to a node's wallet RPC, plus response parsing.

use anyhow::{anyhow, bail, Result};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use sump_core::tx::{Lock, OutPoint, SigScheme, TxOutput};
use sump_wallet::OwnedUtxo;

/// Normalize "http://host:port" / "host:port" to "host:port".
fn host_port(node: &str) -> String {
    node.trim()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

fn http_post(node: &str, path: &str, body: &[u8]) -> Result<Vec<u8>> {
    let addr = host_port(node);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| anyhow!("cannot reach node at {addr}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(req.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp)?;
    let pos = resp
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response from node"))?;
    Ok(resp[pos + 4..].to_vec())
}

pub struct UtxoReply {
    pub height: u64,
    pub maturity: u64,
    pub utxos: Vec<OwnedUtxo>,
}

/// Query the node for the wallet's owned UTXOs.
pub fn fetch_utxos(node: &str, owned_ids: &[(u8, [u8; 20])]) -> Result<UtxoReply> {
    let keys: Vec<String> = owned_ids
        .iter()
        .map(|(s, pkh)| format!("{{\"scheme\":{},\"pkh\":\"{}\"}}", s, hex::encode(pkh)))
        .collect();
    let body = format!("{{\"keys\":[{}]}}", keys.join(","));
    let raw = http_post(node, "/api/utxos", body.as_bytes())?;
    let v: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|e| anyhow!("bad node reply: {e}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        bail!("node error: {err}");
    }
    let height = v.get("height").and_then(|h| h.as_u64()).unwrap_or(0);
    let maturity = v.get("maturity").and_then(|m| m.as_u64()).unwrap_or(100);
    let mut utxos = Vec::new();
    if let Some(arr) = v.get("utxos").and_then(|u| u.as_array()) {
        for it in arr {
            let txid_hex = it.get("txid").and_then(|t| t.as_str()).unwrap_or("");
            let vout = it.get("vout").and_then(|t| t.as_u64()).unwrap_or(0) as u32;
            let amount = it.get("amount").and_then(|t| t.as_u64()).unwrap_or(0);
            let scheme_id = it.get("scheme").and_then(|t| t.as_u64()).unwrap_or(0) as u8;
            let pkh_hex = it.get("pkh").and_then(|t| t.as_str()).unwrap_or("");
            let coinbase = it.get("coinbase").and_then(|t| t.as_bool()).unwrap_or(false);
            let uheight = it.get("height").and_then(|t| t.as_u64()).unwrap_or(0);
            let (Some(txid), Ok(pkh_bytes)) = (
                sump_core::hash::Hash256::from_hex(txid_hex),
                hex::decode(pkh_hex),
            ) else {
                continue;
            };
            let Ok(pkh) = <[u8; 20]>::try_from(pkh_bytes.as_slice()) else {
                continue;
            };
            let scheme = match SigScheme::from_id(scheme_id) {
                Some(s) => s,
                None => continue,
            };
            utxos.push(OwnedUtxo {
                outpoint: OutPoint { txid, vout },
                output: TxOutput {
                    amount,
                    lock: Lock::P2pkh { scheme, pkh },
                },
                coinbase,
                height: uheight,
            });
        }
    }
    Ok(UtxoReply {
        height,
        maturity,
        utxos,
    })
}

/// Submit a signed transaction (hex). Returns the txid on success.
pub fn submit_tx(node: &str, tx_hex: &str) -> Result<String> {
    let raw = http_post(node, "/api/submit", tx_hex.as_bytes())?;
    let v: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|e| anyhow!("bad node reply: {e}"))?;
    if let Some(txid) = v.get("txid").and_then(|t| t.as_str()) {
        Ok(txid.to_string())
    } else if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        bail!("node rejected transaction: {err}")
    } else {
        bail!("unexpected node reply")
    }
}

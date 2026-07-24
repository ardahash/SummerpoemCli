//! Wire protocol messages, encoded with the canonical encoding.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use sump_core::block::Block;
use sump_core::encode::{put_u32, put_u64, put_u8, DecodeError, Reader};
use sump_core::hash::Hash256;
use sump_core::tx::Transaction;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_INV: usize = 2_000;
pub const MAX_LOCATOR: usize = 64;
pub const MAX_ADDRS: usize = 1_000;
/// Blocks returned per GetBlocks round (requester re-requests until synced).
pub const BLOCKS_PER_ROUND: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Hello {
        version: u32,
        height: u64,
        tip: Hash256,
        /// The port this node accepts peer connections on (0 = not listening,
        /// do not gossip). Combined with the observed source IP to form the
        /// peer's reachable address, avoiding self-IP detection.
        listen_port: u16,
    },
    Ping(u64),
    Pong(u64),
    /// Announcement of known blocks / transactions.
    Inv {
        blocks: Vec<Hash256>,
        txs: Vec<Hash256>,
    },
    /// Request for full data of announced items.
    GetData {
        blocks: Vec<Hash256>,
        txs: Vec<Hash256>,
    },
    /// Request block announcements after the fork point in `locator`.
    GetBlocks { locator: Vec<Hash256> },
    Block(Box<Block>),
    Tx(Box<Transaction>),
    /// Request known peer addresses.
    GetAddr,
    /// Share known peer addresses (for discovery).
    Addr(Vec<SocketAddr>),
}

fn put_addr(out: &mut Vec<u8>, addr: &SocketAddr) {
    let v6 = match addr.ip() {
        IpAddr::V4(a) => a.to_ipv6_mapped(),
        IpAddr::V6(a) => a,
    };
    out.extend_from_slice(&v6.octets());
    out.extend_from_slice(&addr.port().to_le_bytes());
}

fn read_addr(r: &mut Reader) -> Result<SocketAddr, DecodeError> {
    let octets = r.read_array::<16>()?;
    let port = u16::from_le_bytes(r.read_array::<2>()?);
    let v6 = Ipv6Addr::from(octets);
    // unmap IPv4 so the address dials correctly on IPv4-only stacks
    let ip = match v6.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(v6),
    };
    Ok(SocketAddr::new(ip, port))
}

fn put_hashes(out: &mut Vec<u8>, hashes: &[Hash256]) {
    put_u32(out, hashes.len() as u32);
    for h in hashes {
        out.extend_from_slice(&h.0);
    }
}

fn read_hashes(r: &mut Reader, max: usize) -> Result<Vec<Hash256>, DecodeError> {
    let n = r.read_count(max)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(Hash256(r.read_array()?));
    }
    Ok(v)
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Message::Hello {
                version,
                height,
                tip,
                listen_port,
            } => {
                put_u8(&mut out, 1);
                put_u32(&mut out, *version);
                put_u64(&mut out, *height);
                out.extend_from_slice(&tip.0);
                out.extend_from_slice(&listen_port.to_le_bytes());
            }
            Message::Ping(n) => {
                put_u8(&mut out, 2);
                put_u64(&mut out, *n);
            }
            Message::Pong(n) => {
                put_u8(&mut out, 3);
                put_u64(&mut out, *n);
            }
            Message::Inv { blocks, txs } => {
                put_u8(&mut out, 4);
                put_hashes(&mut out, blocks);
                put_hashes(&mut out, txs);
            }
            Message::GetData { blocks, txs } => {
                put_u8(&mut out, 5);
                put_hashes(&mut out, blocks);
                put_hashes(&mut out, txs);
            }
            Message::GetBlocks { locator } => {
                put_u8(&mut out, 6);
                put_hashes(&mut out, locator);
            }
            Message::Block(b) => {
                put_u8(&mut out, 7);
                out.extend_from_slice(&b.encode());
            }
            Message::Tx(t) => {
                put_u8(&mut out, 8);
                out.extend_from_slice(&t.encode());
            }
            Message::GetAddr => {
                put_u8(&mut out, 9);
            }
            Message::Addr(addrs) => {
                put_u8(&mut out, 10);
                put_u32(&mut out, addrs.len() as u32);
                for a in addrs {
                    put_addr(&mut out, a);
                }
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Message, DecodeError> {
        let mut r = Reader::new(bytes);
        let msg = match r.read_u8()? {
            1 => Message::Hello {
                version: r.read_u32()?,
                height: r.read_u64()?,
                tip: Hash256(r.read_array()?),
                listen_port: u16::from_le_bytes(r.read_array::<2>()?),
            },
            2 => Message::Ping(r.read_u64()?),
            3 => Message::Pong(r.read_u64()?),
            4 => Message::Inv {
                blocks: read_hashes(&mut r, MAX_INV)?,
                txs: read_hashes(&mut r, MAX_INV)?,
            },
            5 => Message::GetData {
                blocks: read_hashes(&mut r, MAX_INV)?,
                txs: read_hashes(&mut r, MAX_INV)?,
            },
            6 => Message::GetBlocks {
                locator: read_hashes(&mut r, MAX_LOCATOR)?,
            },
            7 => Message::Block(Box::new(Block::decode(&mut r)?)),
            8 => Message::Tx(Box::new(Transaction::decode(&mut r)?)),
            9 => Message::GetAddr,
            10 => {
                let n = r.read_count(MAX_ADDRS)?;
                let mut addrs = Vec::with_capacity(n);
                for _ in 0..n {
                    addrs.push(read_addr(&mut r)?);
                }
                Message::Addr(addrs)
            }
            _ => return Err(DecodeError::Invalid("unknown message tag")),
        };
        r.finish()?;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msgs = vec![
            Message::Hello {
                version: 1,
                height: 42,
                tip: Hash256([7u8; 32]),
                listen_port: 8776,
            },
            Message::Ping(9),
            Message::Pong(9),
            Message::Inv {
                blocks: vec![Hash256([1u8; 32])],
                txs: vec![Hash256([2u8; 32]), Hash256([3u8; 32])],
            },
            Message::GetData {
                blocks: vec![],
                txs: vec![Hash256([4u8; 32])],
            },
            Message::GetBlocks {
                locator: vec![Hash256([5u8; 32]); 3],
            },
            Message::GetAddr,
            Message::Addr(vec![
                "127.0.0.1:8776".parse().unwrap(),
                "[::1]:9000".parse().unwrap(),
            ]),
        ];
        for m in msgs {
            assert_eq!(Message::decode(&m.encode()).unwrap(), m);
        }
    }
}

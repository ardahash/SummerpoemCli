//! Flat-file chain persistence: magic, network id, then length-prefixed
//! encoded blocks of the active chain.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use sump_core::block::Block;
use sump_core::encode::{put_u32, Reader};

const MAGIC: &[u8; 8] = b"SUMPCHN1";

pub fn save(path: &Path, network_id: u8, blocks: &[Arc<Block>]) -> io::Result<()> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(network_id);
    put_u32(&mut out, blocks.len() as u32);
    for b in blocks {
        let enc = b.encode();
        put_u32(&mut out, enc.len() as u32);
        out.extend_from_slice(&enc);
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &out)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load(path: &Path) -> io::Result<(u8, Vec<Block>)> {
    let bytes = fs::read(path)?;
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    if bytes.len() < 13 || &bytes[..8] != MAGIC {
        return Err(bad("not a summerpoem chain file"));
    }
    let network_id = bytes[8];
    let mut r = Reader::new(&bytes[9..]);
    let count = r.read_u32().map_err(|e| bad(&e.to_string()))? as usize;
    let mut blocks = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let len = r.read_u32().map_err(|e| bad(&e.to_string()))? as usize;
        let raw = r.read_bytes(len).map_err(|e| bad(&e.to_string()))?;
        blocks.push(Block::decode_all(raw).map_err(|e| bad(&e.to_string()))?);
    }
    Ok((network_id, blocks))
}

//! Blocks and headers.

use crate::encode::{put_u32, put_u64, DecodeError, Reader};
use crate::hash::{sha3, Hash256};
use crate::merkle::merkle_root;
use crate::tx::Transaction;

pub const HEADER_TAG: &[u8] = b"sump/header/v1";
pub const POW_MSG_TAG: &[u8] = b"sump/powmsg/v1";
pub const MAX_BLOCK_TXS: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockHeader {
    pub version: u32,
    pub prev: Hash256,
    pub tx_root: Hash256,
    pub witness_root: Hash256,
    pub time: u64,
    pub bits: u32,
    pub nonce: u64,
}

impl BlockHeader {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(120);
        put_u32(&mut out, self.version);
        out.extend_from_slice(&self.prev.0);
        out.extend_from_slice(&self.tx_root.0);
        out.extend_from_slice(&self.witness_root.0);
        put_u64(&mut out, self.time);
        put_u32(&mut out, self.bits);
        put_u64(&mut out, self.nonce); // nonce is last: pow message excludes it
        out
    }

    pub fn decode(r: &mut Reader) -> Result<BlockHeader, DecodeError> {
        Ok(BlockHeader {
            version: r.read_u32()?,
            prev: Hash256(r.read_array()?),
            tx_root: Hash256(r.read_array()?),
            witness_root: Hash256(r.read_array()?),
            time: r.read_u64()?,
            bits: r.read_u32()?,
            nonce: r.read_u64()?,
        })
    }

    /// Block identity (includes the nonce).
    pub fn hash(&self) -> Hash256 {
        sha3(&[HEADER_TAG, &self.encode()])
    }

    /// The message SumpHash mines over: the header without its nonce.
    pub fn pow_message(&self) -> Hash256 {
        let mut bytes = self.encode();
        bytes.truncate(bytes.len() - 8);
        sha3(&[POW_MSG_TAG, &bytes])
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
}

impl Block {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.header.encode();
        put_u32(&mut out, self.transactions.len() as u32);
        for tx in &self.transactions {
            out.extend_from_slice(&tx.encode());
        }
        out
    }

    pub fn decode(r: &mut Reader) -> Result<Block, DecodeError> {
        let header = BlockHeader::decode(r)?;
        let n = r.read_count(MAX_BLOCK_TXS)?;
        let mut transactions = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            transactions.push(Transaction::decode(r)?);
        }
        Ok(Block {
            header,
            transactions,
        })
    }

    pub fn decode_all(bytes: &[u8]) -> Result<Block, DecodeError> {
        let mut r = Reader::new(bytes);
        let b = Block::decode(&mut r)?;
        r.finish()?;
        Ok(b)
    }

    pub fn compute_tx_root(&self) -> Hash256 {
        merkle_root(self.transactions.iter().map(|t| t.txid()).collect())
    }

    pub fn compute_witness_root(&self) -> Hash256 {
        merkle_root(self.transactions.iter().map(|t| t.witness_hash()).collect())
    }

    pub fn size(&self) -> usize {
        self.encode().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::TxBody;

    #[test]
    fn header_roundtrip_and_pow_message_excludes_nonce() {
        let mut h = BlockHeader {
            version: 1,
            prev: sha3(&[b"p"]),
            tx_root: sha3(&[b"t"]),
            witness_root: sha3(&[b"w"]),
            time: 12345,
            bits: 0x207fffff,
            nonce: 7,
        };
        let bytes = h.encode();
        let mut r = Reader::new(&bytes);
        assert_eq!(BlockHeader::decode(&mut r).unwrap(), h);

        let pm = h.pow_message();
        let id = h.hash();
        h.nonce = 8;
        assert_eq!(h.pow_message(), pm, "pow message must not depend on nonce");
        assert_ne!(h.hash(), id, "block id must depend on nonce");
    }

    #[test]
    fn block_roundtrip() {
        let b = Block {
            header: BlockHeader {
                version: 1,
                prev: Hash256::ZERO,
                tx_root: Hash256::ZERO,
                witness_root: Hash256::ZERO,
                time: 0,
                bits: 0x207fffff,
                nonce: 0,
            },
            transactions: vec![Transaction {
                body: TxBody {
                    version: 1,
                    inputs: vec![],
                    outputs: vec![],
                    locktime: 0,
                    coinbase_data: vec![0; 8],
                },
                witnesses: vec![],
            }],
        };
        assert_eq!(Block::decode_all(&b.encode()).unwrap(), b);
    }
}

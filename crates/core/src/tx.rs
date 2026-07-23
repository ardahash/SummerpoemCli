//! Transactions: body (committed to by txid) and witnesses (segregated).

use crate::encode::{put_u32, put_u64, put_u8, put_vec, DecodeError, Reader};
use crate::hash::{sha3, Hash256};

pub const TXID_TAG: &[u8] = b"sump/txid/v1";
pub const SIGHASH_TAG: &[u8] = b"sump/sighash/v1";
pub const WITNESS_TAG: &[u8] = b"sump/witness/v1";

/// Coinbase data: 8-byte height prefix plus up to this much free-form data.
pub const MAX_COINBASE_EXTRA: usize = 100;
pub const MAX_TX_IO: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OutPoint {
    pub txid: Hash256,
    pub vout: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lock {
    /// Pay to SHA3-256(public key), truncated to 20 bytes.
    P2pkh { pkh: [u8; 20] },
    /// Same, but unspendable before `height`.
    Timelock { pkh: [u8; 20], height: u64 },
}

impl Lock {
    pub fn pkh(&self) -> &[u8; 20] {
        match self {
            Lock::P2pkh { pkh } => pkh,
            Lock::Timelock { pkh, .. } => pkh,
        }
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Lock::P2pkh { pkh } => {
                put_u8(out, 0);
                out.extend_from_slice(pkh);
            }
            Lock::Timelock { pkh, height } => {
                put_u8(out, 1);
                out.extend_from_slice(pkh);
                put_u64(out, *height);
            }
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Lock, DecodeError> {
        match r.read_u8()? {
            0 => Ok(Lock::P2pkh {
                pkh: r.read_array()?,
            }),
            1 => Ok(Lock::Timelock {
                pkh: r.read_array()?,
                height: r.read_u64()?,
            }),
            _ => Err(DecodeError::Invalid("unknown lock type")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOutput {
    pub amount: u64,
    pub lock: Lock,
}

impl TxOutput {
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        put_u64(out, self.amount);
        self.lock.encode_into(out);
    }

    pub fn decode(r: &mut Reader) -> Result<TxOutput, DecodeError> {
        Ok(TxOutput {
            amount: r.read_u64()?,
            lock: Lock::decode(r)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxInput {
    pub prevout: OutPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxBody {
    pub version: u32,
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
    pub locktime: u64,
    /// Only the coinbase may carry data here; must start with the LE64 height.
    pub coinbase_data: Vec<u8>,
}

impl TxBody {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u32(&mut out, self.version);
        put_u32(&mut out, self.inputs.len() as u32);
        for i in &self.inputs {
            out.extend_from_slice(&i.prevout.txid.0);
            put_u32(&mut out, i.prevout.vout);
        }
        put_u32(&mut out, self.outputs.len() as u32);
        for o in &self.outputs {
            o.encode_into(&mut out);
        }
        put_u64(&mut out, self.locktime);
        put_vec(&mut out, &self.coinbase_data);
        out
    }

    pub fn decode(r: &mut Reader) -> Result<TxBody, DecodeError> {
        let version = r.read_u32()?;
        let n_in = r.read_count(MAX_TX_IO)?;
        let mut inputs = Vec::with_capacity(n_in);
        for _ in 0..n_in {
            inputs.push(TxInput {
                prevout: OutPoint {
                    txid: Hash256(r.read_array()?),
                    vout: r.read_u32()?,
                },
            });
        }
        let n_out = r.read_count(MAX_TX_IO)?;
        let mut outputs = Vec::with_capacity(n_out);
        for _ in 0..n_out {
            outputs.push(TxOutput::decode(r)?);
        }
        let locktime = r.read_u64()?;
        let coinbase_data = r.read_vec()?;
        Ok(TxBody {
            version,
            inputs,
            outputs,
            locktime,
            coinbase_data,
        })
    }

    pub fn txid(&self) -> Hash256 {
        sha3(&[TXID_TAG, &self.encode()])
    }

    pub fn is_coinbase(&self) -> bool {
        self.inputs.is_empty()
    }

    /// The message signed for a given input. Commits to the entire body,
    /// the input index, and the exact output being spent (amount + lock),
    /// in the spirit of BIP-143.
    pub fn sighash(&self, input_index: u32, prev_output: &TxOutput) -> Hash256 {
        let mut extra = Vec::new();
        put_u32(&mut extra, input_index);
        prev_output.encode_into(&mut extra);
        sha3(&[SIGHASH_TAG, &self.encode(), &extra])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Witness {
    pub pubkey: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub body: TxBody,
    /// One witness per input; empty for the coinbase.
    pub witnesses: Vec<Witness>,
}

impl Transaction {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_vec(&mut out, &self.body.encode());
        out.extend_from_slice(&self.encode_witness_section());
        out
    }

    fn encode_witness_section(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u32(&mut out, self.witnesses.len() as u32);
        for w in &self.witnesses {
            put_vec(&mut out, &w.pubkey);
            put_vec(&mut out, &w.signature);
        }
        out
    }

    pub fn decode(r: &mut Reader) -> Result<Transaction, DecodeError> {
        let body_bytes = r.read_vec()?;
        let mut br = Reader::new(&body_bytes);
        let body = TxBody::decode(&mut br)?;
        br.finish()?;
        let n_wit = r.read_count(MAX_TX_IO)?;
        let mut witnesses = Vec::with_capacity(n_wit);
        for _ in 0..n_wit {
            witnesses.push(Witness {
                pubkey: r.read_vec()?,
                signature: r.read_vec()?,
            });
        }
        Ok(Transaction { body, witnesses })
    }

    pub fn decode_all(bytes: &[u8]) -> Result<Transaction, DecodeError> {
        let mut r = Reader::new(bytes);
        let tx = Transaction::decode(&mut r)?;
        r.finish()?;
        Ok(tx)
    }

    pub fn txid(&self) -> Hash256 {
        self.body.txid()
    }

    /// Hash of this transaction's witness section (leaf of the witness tree).
    pub fn witness_hash(&self) -> Hash256 {
        sha3(&[WITNESS_TAG, &self.encode_witness_section()])
    }

    pub fn size(&self) -> usize {
        self.encode().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tx() -> Transaction {
        Transaction {
            body: TxBody {
                version: 1,
                inputs: vec![TxInput {
                    prevout: OutPoint {
                        txid: sha3(&[b"prev"]),
                        vout: 3,
                    },
                }],
                outputs: vec![
                    TxOutput {
                        amount: 5000,
                        lock: Lock::P2pkh { pkh: [7u8; 20] },
                    },
                    TxOutput {
                        amount: 100,
                        lock: Lock::Timelock {
                            pkh: [9u8; 20],
                            height: 42,
                        },
                    },
                ],
                locktime: 0,
                coinbase_data: vec![],
            },
            witnesses: vec![Witness {
                pubkey: vec![1, 2, 3],
                signature: vec![4, 5, 6],
            }],
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let tx = sample_tx();
        let bytes = tx.encode();
        let back = Transaction::decode_all(&bytes).unwrap();
        assert_eq!(tx, back);
    }

    #[test]
    fn txid_ignores_witness() {
        let mut tx = sample_tx();
        let id1 = tx.txid();
        tx.witnesses[0].signature = vec![99; 2420];
        assert_eq!(tx.txid(), id1);
        assert_ne!(tx.witness_hash(), sample_tx().witness_hash());
    }

    #[test]
    fn sighash_commits_to_prevout_and_index() {
        let tx = sample_tx();
        let out_a = TxOutput {
            amount: 1,
            lock: Lock::P2pkh { pkh: [0u8; 20] },
        };
        let out_b = TxOutput {
            amount: 2,
            lock: Lock::P2pkh { pkh: [0u8; 20] },
        };
        assert_ne!(tx.body.sighash(0, &out_a), tx.body.sighash(0, &out_b));
        assert_ne!(tx.body.sighash(0, &out_a), tx.body.sighash(1, &out_a));
    }
}

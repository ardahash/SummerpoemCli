//! Minimal canonical binary encoding. All integers are little-endian.
//! Variable-length byte strings carry a u32 length prefix.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("unexpected end of input")]
    Eof,
    #[error("invalid encoding: {0}")]
    Invalid(&'static str),
    #[error("trailing bytes after decode")]
    Trailing,
}

/// Upper bound for any single length-prefixed field; prevents allocation bombs.
pub const MAX_FIELD_LEN: usize = 8 * 1024 * 1024;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Eof);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        Ok(self.read_bytes(N)?.try_into().unwrap())
    }

    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    pub fn read_u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.read_array::<8>()?))
    }

    pub fn read_vec(&mut self) -> Result<Vec<u8>, DecodeError> {
        let n = self.read_u32()? as usize;
        if n > MAX_FIELD_LEN {
            return Err(DecodeError::Invalid("field length exceeds maximum"));
        }
        Ok(self.read_bytes(n)?.to_vec())
    }

    /// Read a count prefix for a repeated structure, bounded for sanity.
    pub fn read_count(&mut self, max: usize) -> Result<usize, DecodeError> {
        let n = self.read_u32()? as usize;
        if n > max {
            return Err(DecodeError::Invalid("count exceeds maximum"));
        }
        Ok(n)
    }

    pub fn finish(self) -> Result<(), DecodeError> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(DecodeError::Trailing)
        }
    }
}

pub fn put_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn put_vec(out: &mut Vec<u8>, v: &[u8]) {
    put_u32(out, v.len() as u32);
    out.extend_from_slice(v);
}

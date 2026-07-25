//! Post-quantum encrypted transport ("sump/transport/v1").
//!
//! Handshake: the responder (listener) generates an ephemeral ML-KEM-768
//! keypair and sends its encapsulation key; the initiator encapsulates and
//! returns the ciphertext; both sides derive per-direction ChaCha20-Poly1305
//! keys from the shared secret via SHAKE-256. All subsequent traffic is
//! length-prefixed AEAD frames with counter nonces.
//!
//! Like BIP-324, this is opportunistic encryption without peer identity:
//! consensus data is self-certifying (PoW + signatures); the transport layer
//! provides confidentiality and tamper evidence, not authentication.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use fips203::ml_kem_768 as kem;
use fips203::traits::{Decaps, Encaps, KeyGen, SerDes};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use sump_core::hash::shake256;

const MAGIC: &[u8; 8] = b"SUMPNET1";
// v2 (0.5.6): self-connect nonce + forward-compatible message framing. Bumped
// so 0.5.5 and 0.5.6 nodes reject each other cleanly at the header check
// (no ban, no wasted handshake) rather than mismatching at the message layer.
const TRANSPORT_VERSION: u8 = 2;
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub const EK_LEN: usize = 1184; // ML-KEM-768 encapsulation key
pub const CT_LEN: usize = 1088; // ML-KEM-768 ciphertext

fn other(msg: &str) -> io::Error {
    io::Error::other(msg.to_string())
}

fn write_header(stream: &mut TcpStream, network_id: u8) -> io::Result<()> {
    stream.write_all(MAGIC)?;
    stream.write_all(&[TRANSPORT_VERSION, network_id])?;
    Ok(())
}

fn read_and_check_header(stream: &mut TcpStream, network_id: u8) -> io::Result<()> {
    let mut hdr = [0u8; 10];
    stream.read_exact(&mut hdr)?;
    if &hdr[..8] != MAGIC {
        return Err(other("bad magic"));
    }
    if hdr[8] != TRANSPORT_VERSION {
        return Err(other("transport version mismatch"));
    }
    if hdr[9] != network_id {
        return Err(other("peer is on a different network"));
    }
    Ok(())
}

fn derive_key(ss: &[u8; 32], label: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 32];
    shake256(&[b"sump/aead/v1/", label, ss], &mut k);
    k
}

pub struct SecureReader {
    stream: TcpStream,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

pub struct SecureWriter {
    stream: TcpStream,
    cipher: ChaCha20Poly1305,
    counter: u64,
}

fn nonce_for(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&counter.to_le_bytes());
    Nonce::from(n)
}

impl SecureWriter {
    pub fn send(&mut self, plaintext: &[u8]) -> io::Result<()> {
        let ct = self
            .cipher
            .encrypt(&nonce_for(self.counter), plaintext)
            .map_err(|_| other("encryption failure"))?;
        self.counter += 1;
        self.stream.write_all(&(ct.len() as u32).to_le_bytes())?;
        self.stream.write_all(&ct)?;
        self.stream.flush()
    }
}

impl SecureReader {
    pub fn recv(&mut self) -> io::Result<Vec<u8>> {
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes)?;
        let len = u32::from_le_bytes(len_bytes) as usize;
        if !(16..=MAX_FRAME + 16).contains(&len) {
            return Err(other("invalid frame length"));
        }
        let mut ct = vec![0u8; len];
        self.stream.read_exact(&mut ct)?;
        let pt = self
            .cipher
            .decrypt(&nonce_for(self.counter), ct.as_ref())
            .map_err(|_| other("frame authentication failed"))?;
        self.counter += 1;
        Ok(pt)
    }
}

fn split(
    stream: TcpStream,
    send_key: [u8; 32],
    recv_key: [u8; 32],
) -> io::Result<(SecureReader, SecureWriter)> {
    let read_stream = stream.try_clone()?;
    stream.set_read_timeout(None)?;
    read_stream.set_read_timeout(None)?;
    Ok((
        SecureReader {
            stream: read_stream,
            cipher: ChaCha20Poly1305::new(Key::from_slice(&recv_key)),
            counter: 0,
        },
        SecureWriter {
            stream,
            cipher: ChaCha20Poly1305::new(Key::from_slice(&send_key)),
            counter: 0,
        },
    ))
}

/// Client side of the handshake.
pub fn initiate(
    mut stream: TcpStream,
    network_id: u8,
) -> io::Result<(SecureReader, SecureWriter)> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    write_header(&mut stream, network_id)?;
    read_and_check_header(&mut stream, network_id)?;

    let mut ek_bytes = [0u8; EK_LEN];
    stream.read_exact(&mut ek_bytes)?;
    let ek = kem::EncapsKey::try_from_bytes(ek_bytes)
        .map_err(|_| other("invalid ML-KEM encapsulation key"))?;
    let (ssk, ct) = ek.try_encaps().map_err(|_| other("ML-KEM encapsulation failed"))?;
    stream.write_all(&ct.into_bytes())?;
    stream.flush()?;

    let ss: [u8; 32] = ssk.into_bytes();
    // initiator sends with i2r, receives with r2i
    split(stream, derive_key(&ss, b"i2r"), derive_key(&ss, b"r2i"))
}

/// Server side of the handshake.
pub fn respond(
    mut stream: TcpStream,
    network_id: u8,
) -> io::Result<(SecureReader, SecureWriter)> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    read_and_check_header(&mut stream, network_id)?;
    write_header(&mut stream, network_id)?;

    let (ek, dk) = kem::KG::try_keygen().map_err(|_| other("ML-KEM keygen failed"))?;
    stream.write_all(&ek.into_bytes())?;
    stream.flush()?;

    let mut ct_bytes = [0u8; CT_LEN];
    stream.read_exact(&mut ct_bytes)?;
    let ct = kem::CipherText::try_from_bytes(ct_bytes)
        .map_err(|_| other("invalid ML-KEM ciphertext"))?;
    let ssk = dk
        .try_decaps(&ct)
        .map_err(|_| other("ML-KEM decapsulation failed"))?;

    let ss: [u8; 32] = ssk.into_bytes();
    // responder sends with r2i, receives with i2r
    split(stream, derive_key(&ss, b"r2i"), derive_key(&ss, b"i2r"))
}

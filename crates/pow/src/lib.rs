//! SumpHash v1: Ethash-style memory-hard proof-of-work on SHA3/SHAKE.
//!
//! A per-epoch dataset is generated from a 64 MiB (mainnet) light cache.
//! Mining requires random 128-byte reads across the full dataset (DRAM
//! bandwidth bound); verification recomputes only the accessed items from
//! the cache and needs no dataset.

use primitive_types::U256;
use sump_core::hash::{sha3, shake256, Hash256};
use sump_core::params::PowParams;

pub const ITEM: usize = 64;
pub const PAGE: usize = 128;
const CACHE_ROUNDS: usize = 2;
pub const POW_TAG: &[u8] = b"sump/pow/v1";

pub fn epoch_of_height(height: u64, epoch_length: u64) -> u64 {
    height / epoch_length
}

/// Epoch seeds form a hash chain from zero, independent of chain content
/// (as in Ethash), so datasets can be computed ahead of time.
pub fn seed_for_epoch(epoch: u64) -> Hash256 {
    let mut s = Hash256::ZERO;
    for _ in 0..=epoch {
        s = sha3(&[b"sump/epochseed", &s.0]);
    }
    s
}

#[inline(always)]
fn fnv(a: u32, b: u32) -> u32 {
    a.wrapping_mul(0x0100_0193) ^ b
}

/// Per-u32-lane FNV fold of `other` into `mix` (lengths must match, /4).
fn fnv_mix(mix: &mut [u8], other: &[u8]) {
    debug_assert_eq!(mix.len(), other.len());
    for k in 0..mix.len() / 4 {
        let a = u32::from_le_bytes(mix[4 * k..4 * k + 4].try_into().unwrap());
        let b = u32::from_le_bytes(other[4 * k..4 * k + 4].try_into().unwrap());
        mix[4 * k..4 * k + 4].copy_from_slice(&fnv(a, b).to_le_bytes());
    }
}

pub struct PowContext {
    pub params: PowParams,
    pub epoch: u64,
    pub seed: Hash256,
    cache: Vec<u8>,
    dataset: Option<Vec<u8>>,
}

impl PowContext {
    /// Verification context: cache only.
    pub fn new_light(params: &PowParams, epoch: u64) -> Self {
        let seed = seed_for_epoch(epoch);
        let cache = Self::gen_cache(params, &seed);
        PowContext {
            params: params.clone(),
            epoch,
            seed,
            cache,
            dataset: None,
        }
    }

    /// Mining context: cache plus full dataset.
    pub fn new_full(params: &PowParams, epoch: u64) -> Self {
        let mut ctx = Self::new_light(params, epoch);
        ctx.gen_dataset();
        ctx
    }

    fn cache_items(&self) -> usize {
        self.params.cache_bytes / ITEM
    }

    fn dataset_pages(&self) -> usize {
        self.params.dataset_bytes / PAGE
    }

    fn gen_cache(params: &PowParams, seed: &Hash256) -> Vec<u8> {
        let n = params.cache_bytes / ITEM;
        assert!(n >= 2, "cache too small");
        let mut cache = vec![0u8; n * ITEM];
        let mut item = [0u8; ITEM];
        shake256(&[b"sump/cache", &seed.0], &mut item);
        cache[..ITEM].copy_from_slice(&item);
        for i in 1..n {
            let prev: [u8; ITEM] = cache[(i - 1) * ITEM..i * ITEM].try_into().unwrap();
            shake256(&[&prev], &mut item);
            cache[i * ITEM..(i + 1) * ITEM].copy_from_slice(&item);
        }
        // RandMemoHash-style strengthening rounds
        for _ in 0..CACHE_ROUNDS {
            for i in 0..n {
                let v = (u32::from_le_bytes(cache[i * ITEM..i * ITEM + 4].try_into().unwrap())
                    as usize)
                    % n;
                let j = (i + n - 1) % n;
                let mut buf = [0u8; ITEM];
                for k in 0..ITEM {
                    buf[k] = cache[j * ITEM + k] ^ cache[v * ITEM + k];
                }
                shake256(&[&buf], &mut item);
                cache[i * ITEM..(i + 1) * ITEM].copy_from_slice(&item);
            }
        }
        cache
    }

    /// Compute dataset item `i` from the cache (the "light" path).
    pub fn dataset_item(&self, i: u64) -> [u8; ITEM] {
        let n = self.cache_items();
        let mut mix = [0u8; ITEM];
        mix.copy_from_slice(&self.cache[(i as usize % n) * ITEM..][..ITEM]);
        let ib = i.to_le_bytes();
        for k in 0..8 {
            mix[k] ^= ib[k];
        }
        let mut out = [0u8; ITEM];
        shake256(&[&mix], &mut out);
        mix = out;
        for p in 0..self.params.parents as u32 {
            let lane =
                u32::from_le_bytes(mix[(p as usize % 16) * 4..][..4].try_into().unwrap());
            let idx = (fnv((i as u32) ^ p, lane) as usize) % n;
            fnv_mix(&mut mix, &self.cache[idx * ITEM..(idx + 1) * ITEM]);
        }
        shake256(&[&mix], &mut out);
        out
    }

    /// Materialize the full dataset (miners only).
    pub fn gen_dataset(&mut self) {
        let items = self.params.dataset_bytes / ITEM;
        let mut ds = vec![0u8; items * ITEM];
        for i in 0..items {
            let item = self.dataset_item(i as u64);
            ds[i * ITEM..(i + 1) * ITEM].copy_from_slice(&item);
        }
        self.dataset = Some(ds);
    }

    pub fn has_dataset(&self) -> bool {
        self.dataset.is_some()
    }

    /// The full materialized dataset (miners only), for uploading to a GPU.
    pub fn dataset_bytes(&self) -> Option<&[u8]> {
        self.dataset.as_deref()
    }

    /// Number of 128-byte pages in the dataset.
    pub fn pages(&self) -> usize {
        self.dataset_pages()
    }

    pub fn accesses(&self) -> usize {
        self.params.accesses
    }

    fn page(&self, idx: usize, buf: &mut [u8; PAGE]) {
        if let Some(ds) = &self.dataset {
            buf.copy_from_slice(&ds[idx * PAGE..(idx + 1) * PAGE]);
        } else {
            buf[..ITEM].copy_from_slice(&self.dataset_item(2 * idx as u64));
            buf[ITEM..].copy_from_slice(&self.dataset_item(2 * idx as u64 + 1));
        }
    }

    /// SumpHash of (pow_message, nonce).
    pub fn compute(&self, pow_message: &Hash256, nonce: u64) -> Hash256 {
        let mut seed64 = [0u8; 64];
        shake256(
            &[b"sump/mixseed", &pow_message.0, &nonce.to_le_bytes()],
            &mut seed64,
        );
        let mut mix = [0u8; PAGE];
        mix[..ITEM].copy_from_slice(&seed64);
        mix[ITEM..].copy_from_slice(&seed64);
        let pages = self.dataset_pages();
        let s0 = u32::from_le_bytes(seed64[..4].try_into().unwrap());
        let mut pagebuf = [0u8; PAGE];
        for a in 0..self.params.accesses as u32 {
            let lane =
                u32::from_le_bytes(mix[(a as usize % 32) * 4..][..4].try_into().unwrap());
            let idx = (fnv(a ^ s0, lane) as usize) % pages;
            self.page(idx, &mut pagebuf);
            fnv_mix(&mut mix, &pagebuf);
        }
        // compress 128 -> 32 bytes, 4:1 FNV fold
        let mut cmix = [0u8; 32];
        for k in 0..8 {
            let m = |j: usize| {
                u32::from_le_bytes(mix[4 * (4 * k + j)..][..4].try_into().unwrap())
            };
            let c = fnv(fnv(fnv(m(0), m(1)), m(2)), m(3));
            cmix[4 * k..4 * k + 4].copy_from_slice(&c.to_le_bytes());
        }
        sha3(&[POW_TAG, &seed64, &cmix])
    }
}

pub fn meets_target(hash: &Hash256, target: U256) -> bool {
    hash.to_u256() <= target
}

/// Search nonces in `[start, start+max_iters)`; returns the first that meets
/// the target.
pub fn mine(
    ctx: &PowContext,
    pow_message: &Hash256,
    target: U256,
    start: u64,
    max_iters: u64,
) -> Option<u64> {
    for i in 0..max_iters {
        let nonce = start.wrapping_add(i);
        if meets_target(&ctx.compute(pow_message, nonce), target) {
            return Some(nonce);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use sump_core::params::Params;

    fn test_params() -> PowParams {
        Params::regtest().pow
    }

    #[test]
    fn cache_and_dataset_deterministic() {
        let p = test_params();
        let a = PowContext::new_light(&p, 0);
        let b = PowContext::new_light(&p, 0);
        assert_eq!(a.cache, b.cache);
        assert_eq!(a.dataset_item(7), b.dataset_item(7));
        let c = PowContext::new_light(&p, 1);
        assert_ne!(a.cache, c.cache, "different epochs, different cache");
    }

    #[test]
    fn light_equals_full() {
        let p = test_params();
        let light = PowContext::new_light(&p, 0);
        let full = PowContext::new_full(&p, 0);
        let msg = sha3(&[b"header"]);
        for nonce in 0..8u64 {
            assert_eq!(light.compute(&msg, nonce), full.compute(&msg, nonce));
        }
    }

    #[test]
    fn nonce_changes_result() {
        let p = test_params();
        let ctx = PowContext::new_light(&p, 0);
        let msg = sha3(&[b"header"]);
        assert_ne!(ctx.compute(&msg, 0), ctx.compute(&msg, 1));
    }

    #[test]
    fn mining_finds_easy_target() {
        let p = test_params();
        let ctx = PowContext::new_full(&p, 0);
        let msg = sha3(&[b"header"]);
        let target = U256::MAX >> 4;
        let nonce = mine(&ctx, &msg, target, 0, 10_000).expect("should find");
        assert!(meets_target(&ctx.compute(&msg, nonce), target));
    }
}

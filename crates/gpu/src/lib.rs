//! CUDA GPU miner for SumpHash v1.
//!
//! The kernel (compiled to PTX by build.rs via nvcc) is loaded at runtime
//! through the CUDA driver API. The per-epoch dataset is generated on the CPU
//! (reusing `PowContext`) and uploaded to GPU global memory once; the kernel
//! then performs the random DRAM reads that make SumpHash bandwidth-bound.
//!
//! If no CUDA toolkit was present at build time, or no CUDA device is present
//! at runtime, `GpuMiner::new` returns `GpuError::Unavailable` and callers
//! fall back to the CPU miner.

use cudarc::driver::{CudaDevice, CudaSlice, DeviceSlice, LaunchAsync, LaunchConfig};
use cudarc::nvrtc::Ptx;

/// Threads per block. The kernel keeps a full Keccak state plus mix/seed
/// buffers in local memory, so it is register-heavy; a small block avoids
/// CUDA_ERROR_LAUNCH_OUT_OF_RESOURCES while still saturating memory bandwidth.
const BLOCK: u32 = 64;

fn launch_cfg(count: u32) -> LaunchConfig {
    LaunchConfig {
        grid_dim: (count.div_ceil(BLOCK), 1, 1),
        block_dim: (BLOCK, 1, 1),
        shared_mem_bytes: 0,
    }
}
use primitive_types::U256;
use std::sync::Arc;
use sump_core::hash::Hash256;
use sump_pow::PowContext;
use thiserror::Error;

const PTX: &str = include_str!(concat!(env!("OUT_DIR"), "/sumphash.ptx"));

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("GPU mining unavailable: {0}")]
    Unavailable(String),
    #[error("CUDA error: {0}")]
    Cuda(String),
}

impl From<cudarc::driver::DriverError> for GpuError {
    fn from(e: cudarc::driver::DriverError) -> Self {
        GpuError::Cuda(e.to_string())
    }
}

pub struct GpuMiner {
    dev: Arc<CudaDevice>,
    dataset: CudaSlice<u8>,
    pages: u32,
    accesses: u32,
}

impl GpuMiner {
    /// Build a miner for the given (full) PoW context, uploading its dataset.
    pub fn new(ctx: &PowContext) -> Result<GpuMiner, GpuError> {
        if PTX.trim().is_empty() {
            return Err(GpuError::Unavailable(
                "built without a CUDA toolkit (no PTX)".into(),
            ));
        }
        let dataset = ctx
            .dataset_bytes()
            .ok_or_else(|| GpuError::Unavailable("PoW context has no dataset".into()))?;

        let dev = CudaDevice::new(0).map_err(|e| {
            GpuError::Unavailable(format!("no CUDA device: {e}"))
        })?;
        dev.load_ptx(
            Ptx::from_src(PTX),
            "sumphash",
            &["sumphash_search", "sumphash_hash"],
        )?;

        let d_dataset = dev.htod_sync_copy(dataset)?;
        Ok(GpuMiner {
            dev,
            dataset: d_dataset,
            pages: ctx.pages() as u32,
            accesses: ctx.accesses() as u32,
        })
    }

    pub fn device_name(&self) -> String {
        self.dev.name().unwrap_or_else(|_| "unknown".into())
    }

    /// Search nonces `[start, start+count)` for one meeting `target`.
    /// Returns the smallest such nonce, or None.
    pub fn search(
        &self,
        pow_message: &Hash256,
        target: U256,
        start: u64,
        count: u32,
    ) -> Result<Option<u64>, GpuError> {
        let target_be = target.to_big_endian();
        let d_msg = self.dev.htod_sync_copy(&pow_message.0)?;
        let d_target = self.dev.htod_sync_copy(&target_be)?;
        let d_found = self.dev.htod_sync_copy(&[u64::MAX])?;

        let func = self
            .dev
            .get_func("sumphash", "sumphash_search")
            .ok_or_else(|| GpuError::Cuda("kernel sumphash_search not found".into()))?;
        let cfg = launch_cfg(count);
        unsafe {
            func.launch(
                cfg,
                (
                    &self.dataset,
                    self.pages,
                    self.accesses,
                    &d_msg,
                    start,
                    count,
                    &d_target,
                    &d_found,
                ),
            )?;
        }
        let found = self.dev.dtoh_sync_copy(&d_found)?;
        let n = found[0];
        if n != u64::MAX && n >= start && n < start.wrapping_add(count as u64) {
            Ok(Some(n))
        } else {
            Ok(None)
        }
    }

    /// Compute raw 32-byte hashes for nonces `[start, start+count)`.
    /// Used to verify GPU output matches the CPU reference bit-for-bit.
    pub fn hash_batch(
        &self,
        pow_message: &Hash256,
        start: u64,
        count: u32,
    ) -> Result<Vec<[u8; 32]>, GpuError> {
        let d_msg = self.dev.htod_sync_copy(&pow_message.0)?;
        let mut d_out = self.dev.alloc_zeros::<u8>(count as usize * 32)?;

        let func = self
            .dev
            .get_func("sumphash", "sumphash_hash")
            .ok_or_else(|| GpuError::Cuda("kernel sumphash_hash not found".into()))?;
        let cfg = launch_cfg(count);
        unsafe {
            func.launch(
                cfg,
                (
                    &self.dataset,
                    self.pages,
                    self.accesses,
                    &d_msg,
                    start,
                    count,
                    &mut d_out,
                ),
            )?;
        }
        let flat = self.dev.dtoh_sync_copy(&d_out)?;
        Ok(flat
            .chunks_exact(32)
            .map(|c| c.try_into().unwrap())
            .collect())
    }

    pub fn dataset_bytes(&self) -> usize {
        self.dataset.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sump_core::hash::sha3;
    use sump_core::params::Params;

    fn gpu_or_skip(ctx: &PowContext) -> Option<GpuMiner> {
        match GpuMiner::new(ctx) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("skipping GPU test: {e}");
                None
            }
        }
    }

    #[test]
    fn gpu_matches_cpu_hashes() {
        let params = Params::regtest();
        let ctx = PowContext::new_full(&params.pow, 0);
        let Some(gpu) = gpu_or_skip(&ctx) else { return };

        let msg = sha3(&[b"gpu-vs-cpu"]);
        let count = 256u32;
        let gpu_hashes = gpu.hash_batch(&msg, 0, count).unwrap();
        for nonce in 0..count as u64 {
            let cpu = ctx.compute(&msg, nonce);
            assert_eq!(
                gpu_hashes[nonce as usize], cpu.0,
                "mismatch at nonce {nonce}"
            );
        }
    }

    #[test]
    fn gpu_search_finds_valid_nonce() {
        let params = Params::regtest();
        let ctx = PowContext::new_full(&params.pow, 0);
        let Some(gpu) = gpu_or_skip(&ctx) else { return };

        let msg = sha3(&[b"search"]);
        let target = U256::MAX >> 8;
        let found = gpu.search(&msg, target, 0, 100_000).unwrap();
        let nonce = found.expect("should find a nonce at this easy target");
        // verify against CPU
        let h = ctx.compute(&msg, nonce);
        assert!(sump_pow::meets_target(&h, target));
    }
}

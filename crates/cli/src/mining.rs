//! Mining rig abstraction: GPU (CUDA) when available, else CPU reference.

use sump_core::block::Block;
use sump_core::compact::bits_to_target;
use sump_gpu::GpuMiner;
use sump_pow::PowContext;

/// Nonces per GPU search launch.
const GPU_CHUNK: u32 = 8_388_608;
/// Nonces per CPU search attempt.
const CPU_CHUNK: u64 = 1_000_000;

pub enum Rig {
    Gpu(GpuMiner),
    Cpu,
}

impl Rig {
    /// Build a rig for this epoch's context. Falls back to CPU (with a note)
    /// if `want_gpu` is set but no CUDA device/toolkit is available.
    pub fn select(ctx: &PowContext, want_gpu: bool) -> Rig {
        if !want_gpu {
            return Rig::Cpu;
        }
        match GpuMiner::new(ctx) {
            Ok(g) => {
                eprintln!("[miner] GPU: {}", g.device_name());
                Rig::Gpu(g)
            }
            Err(e) => {
                eprintln!("[miner] GPU unavailable ({e}); using CPU");
                Rig::Cpu
            }
        }
    }

    pub fn is_gpu(&self) -> bool {
        matches!(self, Rig::Gpu(_))
    }

    /// Mine `block` in place until a valid nonce is found. Blocks the thread.
    /// Returns false only if the (bounded) CPU attempt is exhausted without a
    /// solution when `bounded` is set.
    pub fn mine(&self, ctx: &PowContext, block: &mut Block) -> bool {
        let target = bits_to_target(block.header.bits).expect("valid bits");
        let msg = block.header.pow_message();
        match self {
            Rig::Gpu(gpu) => {
                let mut start = block.header.nonce;
                loop {
                    match gpu.search(&msg, target, start, GPU_CHUNK) {
                        Ok(Some(nonce)) => {
                            block.header.nonce = nonce;
                            return true;
                        }
                        Ok(None) => start = start.wrapping_add(GPU_CHUNK as u64),
                        Err(e) => {
                            eprintln!("[miner] GPU search error: {e}; giving up block");
                            return false;
                        }
                    }
                }
            }
            Rig::Cpu => {
                let mut start = block.header.nonce;
                loop {
                    if let Some(nonce) =
                        sump_pow::mine(ctx, &msg, target, start, CPU_CHUNK)
                    {
                        block.header.nonce = nonce;
                        return true;
                    }
                    start = start.wrapping_add(CPU_CHUNK);
                }
            }
        }
    }

    /// One bounded attempt (for the live node miner, so it can refresh the
    /// template between tries). Returns true if a nonce was found.
    pub fn try_mine(&self, ctx: &PowContext, block: &mut Block, gpu_chunk: u32) -> bool {
        let target = bits_to_target(block.header.bits).expect("valid bits");
        let msg = block.header.pow_message();
        match self {
            Rig::Gpu(gpu) => match gpu.search(&msg, target, block.header.nonce, gpu_chunk) {
                Ok(Some(nonce)) => {
                    block.header.nonce = nonce;
                    true
                }
                Ok(None) => {
                    block.header.nonce = block.header.nonce.wrapping_add(gpu_chunk as u64);
                    false
                }
                Err(e) => {
                    eprintln!("[miner] GPU search error: {e}");
                    false
                }
            },
            Rig::Cpu => {
                if let Some(nonce) =
                    sump_pow::mine(ctx, &msg, target, block.header.nonce, CPU_CHUNK)
                {
                    block.header.nonce = nonce;
                    true
                } else {
                    block.header.nonce = block.header.nonce.wrapping_add(CPU_CHUNK);
                    false
                }
            }
        }
    }
}

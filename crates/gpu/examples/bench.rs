//! CPU vs GPU SumpHash throughput benchmark.
//!
//! Run with the CUDA toolkit's bin\x64 on PATH:
//!   cargo run --release -p sump-gpu --example bench

use std::time::Instant;
use sump_core::hash::sha3;
use sump_core::params::PowParams;
use sump_gpu::GpuMiner;
use sump_pow::PowContext;

fn main() {
    // A 256 MiB dataset with mainnet-like access count: large enough that
    // random reads miss cache and hit DRAM, which is the property SumpHash
    // is built around.
    let pow = PowParams {
        cache_bytes: 16 << 20,
        dataset_bytes: 256 << 20,
        epoch_length: 4096,
        accesses: 64,
        parents: 64,
    };
    println!(
        "generating {} MiB dataset (one-time, CPU)...",
        pow.dataset_bytes >> 20
    );
    let t = Instant::now();
    let ctx = PowContext::new_full(&pow, 0);
    println!("  dataset ready in {:.1}s", t.elapsed().as_secs_f64());

    let msg = sha3(&[b"benchmark-header"]);

    // ---- CPU ----
    let cpu_iters = 2_000u64;
    let t = Instant::now();
    let mut sink = 0u8;
    for nonce in 0..cpu_iters {
        sink ^= ctx.compute(&msg, nonce).0[0];
    }
    let cpu_secs = t.elapsed().as_secs_f64();
    let cpu_hps = cpu_iters as f64 / cpu_secs;
    println!(
        "\nCPU (1 thread): {:>10.0} H/s   ({} hashes in {:.2}s)  [{}]",
        cpu_hps, cpu_iters, cpu_secs, sink
    );

    // ---- GPU ----
    let gpu = match GpuMiner::new(&ctx) {
        Ok(g) => g,
        Err(e) => {
            println!("\nGPU unavailable: {e}");
            return;
        }
    };
    println!("GPU: {} ({} MiB dataset resident)", gpu.device_name(), gpu.dataset_bytes() >> 20);

    // Target of zero never matches, so every nonce is fully hashed.
    let zero = primitive_types::U256::zero();
    let gpu_iters: u32 = 8_388_608; // 8 Mi nonces
    // warm up (JIT + first launch)
    let _ = gpu.search(&msg, zero, 0, 65_536).unwrap();

    let t = Instant::now();
    let _ = gpu.search(&msg, zero, 0, gpu_iters).unwrap();
    let gpu_secs = t.elapsed().as_secs_f64();
    let gpu_hps = gpu_iters as f64 / gpu_secs;
    println!(
        "GPU:            {:>10.0} H/s   ({} hashes in {:.2}s)",
        gpu_hps, gpu_iters, gpu_secs
    );

    println!("\nspeedup: {:.0}x", gpu_hps / cpu_hps);
}

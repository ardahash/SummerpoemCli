# Summerpoem (SUMP)

A quantum-resistant proof-of-work cryptocurrency. See [WHITEPAPER.md](WHITEPAPER.md)
for the full design.

- **Signatures:** ML-DSA-44 (FIPS 204) — no elliptic curves anywhere.
  Optional SLH-DSA-128s (FIPS 205) "vault" addresses for hash-only security.
- **Addresses:** bech32m over SHA3-256 public-key hashes (`sump1...`); the
  version byte selects the signature scheme.
- **PoW:** SumpHash v1 — Ethash-style memory-hard SHA3/SHAKE algorithm,
  fixed 2 GiB dataset, 64 MiB light-verification cache.
- **Difficulty:** per-block ASERT. **Emission:** smooth exponential decay,
  42 M SUMP cap, ~4-year half-life. Base unit: the *stanza* (10⁻⁸ SUMP).

## Workspace layout

| Crate | Contents |
|---|---|
| `crates/core` | Canonical encoding, hashing, tx/block structures, merkle, emission, ASERT, compact bits, network params |
| `crates/pow` | SumpHash v1 (cache/dataset generation, light + full compute, CPU miner loop) |
| `crates/crypto` | ML-DSA-44 keys/signing/verification, bech32m addresses |
| `crates/node` | Chain state, full validation, reorgs, mempool, block templates, genesis builder, flat-file store |
| `crates/net` | P2P: ML-KEM-768 encrypted transport, block/tx gossip, chain sync |
| `crates/gpu` | CUDA SumpHash miner (nvcc→PTX, driver-API launch), CPU-identical |
| `crates/cli` | The `sump` binary: `genesis`, `node`, `wallet`, `miner` subcommands |

## Build & test

```
cargo build --release
cargo test
```

## Regtest walkthrough

```
# create a chain and two wallets
cargo run -- genesis
cargo run -- wallet new --wallet alice.json
cargo run -- wallet new --wallet bob.json

# mine 6 blocks to alice (regtest coinbase maturity is 5)
cargo run -- miner mine --wallet alice.json --blocks 6
cargo run -- wallet balance --wallet alice.json

# pay bob 5.5 SUMP; the tx waits in the mempool until mined
cargo run -- wallet send --wallet alice.json --to <bob-address> --amount 5.5
cargo run -- miner mine --wallet alice.json --blocks 1
cargo run -- wallet balance --wallet bob.json

# hash-based vault addresses (SLH-DSA) for cold storage
cargo run -- wallet vault-address --wallet alice.json   # sump1... (version 1)
cargo run -- wallet send --wallet alice.json --to <vault-address> --amount 100

# inspect / fully re-validate the chain from disk
cargo run -- node info
cargo run -- node validate
```

## Running a networked node

```
# terminal 1: listen and mine
cargo run -- node run --listen 127.0.0.1:8776 --mine --wallet alice.json

# terminal 2 (separate --chain-dir): connect and sync
cargo run -- --chain-dir ./sumpchain2 node run --listen 127.0.0.1:8777 --connect 127.0.0.1:8776
```

Peer connections are encrypted end-to-end with an ML-KEM-768 (FIPS 203)
handshake and ChaCha20-Poly1305 frames. A running node picks up transaction
files that `wallet send` drops into `<chain-dir>/mempool/`, relays them, and
(with `--mine`) includes them in blocks.

## Mining with the dashboard (GUI)

```
# one command: auto-creates genesis on first run, mines, serves a GUI
cargo run --release -- node run --mine --gpu --gui
```

Open the printed `http://127.0.0.1:8787` in a browser for a live dashboard
(height, hashrate, balance, peers, mempool, addresses). Drop `--gpu` to mine
on CPU; drop `--gui` for terminal-only.

A prebuilt single-executable package is produced under `dist/`
(`summerpoem-v<version>-windows-x64.zip`: `sump.exe` + QUICKSTART).

## GPU mining

```
# mine on the GPU (CUDA); falls back to CPU if unavailable
cargo run --release -- miner mine --gpu --blocks 10
cargo run --release -- node run --mine --gpu
```

Requires an NVIDIA CUDA toolkit at build time (nvcc compiles the kernel to
PTX) and a CUDA-capable GPU at runtime. Without them the crate still builds
and mining transparently uses the CPU. GPU output is bit-identical to the CPU
reference — GPU-mined blocks verify under the standard rules.

Benchmark CPU vs GPU throughput:

```
cargo run --release -p sump-gpu --example bench
```

Defaults: `--network regtest`, `--chain-dir ./sumpchain`. Mainnet parameters
exist (`--network mainnet`) but mainnet genesis has not been cut.

## Status

Pre-release, v0.4, pre-mainnet hardening complete (58 tests). Implemented and
tested: consensus core (validation, reorgs, emission, difficulty, PoW), P2P
networking (ML-KEM encrypted transport, gossip, chain sync, mempool relay),
the CUDA GPU miner (bit-identical to the CPU reference, ~238× a single CPU
thread on an RTX 5070 Ti), and SLH-DSA vault addresses. The hardening pass
adds decoder fuzzing (no panics on hostile input), property tests on the
consensus math, an adversarial block-rejection matrix, reorg UTXO/mempool
rollback, and multi-node convergence (in-process and live 3-process).

Peer discovery is implemented (seed list + address gossip; a dialer keeps up
to 8 outbound peers). Remaining before mainnet is not engineering: the
one-time fair-launch parameters (`genesis_time`, `genesis_message`, `seeds`
in `Params::mainnet()`) and the operational deployment of public seed nodes.
The phase-2 STARK witness compression described in the whitepaper remains
future work.

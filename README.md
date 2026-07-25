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
# create a local regtest chain and two wallets
cargo run -- --network regtest genesis
cargo run -- --network regtest wallet new --wallet alice.json
cargo run -- --network regtest wallet new --wallet bob.json

# mine 6 blocks to alice (regtest coinbase maturity is 5)
cargo run -- --network regtest miner mine --wallet alice.json --blocks 6
cargo run -- --network regtest wallet balance --wallet alice.json

# pay bob 5.5 SUMP; the tx waits in the mempool until mined
cargo run -- --network regtest wallet send --wallet alice.json --to <bob-address> --amount 5.5
cargo run -- --network regtest miner mine --wallet alice.json --blocks 1
cargo run -- --network regtest wallet balance --wallet bob.json

# hash-based vault addresses (SLH-DSA) for cold storage
cargo run -- --network regtest wallet vault-address --wallet alice.json
cargo run -- --network regtest wallet send --wallet alice.json --to <vault-address> --amount 100

# inspect / fully re-validate the chain from disk
cargo run -- --network regtest node info
cargo run -- --network regtest node validate
```

## Running a networked node

```
# terminal 1: listen and mine
cargo run -- --network regtest node run --listen 127.0.0.1:8776 --mine --wallet alice.json

# terminal 2 (separate --chain-dir): connect and sync
cargo run -- --network regtest --chain-dir ./sumpchain2 node run --listen 127.0.0.1:8777 --connect 127.0.0.1:8776
```

Peer connections are encrypted end-to-end with an ML-KEM-768 (FIPS 203)
handshake and ChaCha20-Poly1305 frames. A running node picks up transaction
files that `wallet send` drops into `<chain-dir>/mempool/`, relays them, and
(with `--mine`) includes them in blocks.

## Mining with the dashboard (GUI)

```
# one command: initializes mainnet, connects to seeds, syncs, then mines
cargo run --release -- node run --mine --gpu --gui
```

Open the printed `http://127.0.0.1:8787` in a browser for a live dashboard
(height, hashrate, balance, peers, mempool, addresses). Drop `--gpu` to mine
on CPU; drop `--gui` for terminal-only.

A prebuilt single-executable package is produced under `dist/`
(`summerpoem-v<version>-windows-x64.zip`: `sump.exe` + QUICKSTART).

## GPU mining

```
# regtest-only standalone mining
cargo run --release -- --network regtest miner mine --gpu --blocks 10

# public mainnet mining stays connected and synced
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

Defaults: `--network mainnet`, `--chain-dir ./sumpchain`. Mainnet has a fixed
genesis and public bootstrap seed list in `Params::mainnet().seeds`; regtest
must be selected explicitly with `--network regtest`.

## Status

v0.5.4 mainnet hardening complete (58 tests). Implemented and
tested: consensus core (validation, reorgs, emission, difficulty, PoW), P2P
networking (ML-KEM encrypted transport, gossip, chain sync, mempool relay),
the CUDA GPU miner (bit-identical to the CPU reference, ~238× a single CPU
thread on an RTX 5070 Ti), and SLH-DSA vault addresses. The hardening pass
adds decoder fuzzing (no panics on hostile input), property tests on the
consensus math, an adversarial block-rejection matrix, reorg UTXO/mempool
rollback, and multi-node convergence (in-process and live 3-process).

Peer discovery is implemented (public seed list + address gossip; a dialer keeps
up to 8 outbound peers). Mainnet miners wait for peers and catch-up sync before
hashing so a double-click miner does not start on an isolated local network.
The phase-2 STARK witness compression described in the whitepaper remains
future work.

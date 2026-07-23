# Summerpoem (SUMP)

A quantum-resistant proof-of-work cryptocurrency. See [WHITEPAPER.md](WHITEPAPER.md)
for the full design.

- **Signatures:** ML-DSA-44 (FIPS 204) — no elliptic curves anywhere.
- **Addresses:** bech32m over SHA3-256 public-key hashes (`sump1...`).
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
| `crates/node` | Chain state, full validation, reorgs, block templates, genesis builder, flat-file store |
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

# inspect / fully re-validate the chain from disk
cargo run -- node info
cargo run -- node validate
```

Defaults: `--network regtest`, `--chain-dir ./sumpchain`. Mainnet parameters
exist (`--network mainnet`) but mainnet genesis has not been cut.

## Status

Pre-release. Consensus core (validation, reorgs, emission, difficulty, PoW)
is implemented and covered by unit + integration tests. Not yet implemented:
P2P networking (ML-KEM transport), GPU miner kernels, SLH-DSA dormant output
type, and the phase-2 STARK witness compression described in the whitepaper.

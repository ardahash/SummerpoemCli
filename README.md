# Summerpoem (SUMP)

Summerpoem is a quantum-resistant proof-of-work cryptocurrency.

- Coin name: **Summerpoem**
- Symbol: **SUMP**
- Base unit: **stanza** (`1 SUMP = 100,000,000 stanzas`)
- Genesis message: `What if life was meant to be lived`
- Mainnet genesis hash:
  `60235b421eb3478072192851a1ea05eeb221dd8821aeaacb3fcd361abb21ca0d`

Summerpoem uses post-quantum signatures, a memory-hard GPU-friendly proof of
work, and a Bitcoin-style UTXO ledger.

For the full technical design, read [WHITEPAPER.md](WHITEPAPER.md) and
[DESIGN.md](DESIGN.md).

## Download

Download the latest Windows package from:

https://github.com/ardahash/SummerpoemCli/releases/latest

The release zip contains:

| File | Purpose |
|---|---|
| `sump.exe` | Full node, miner, local wallet commands, dashboard |
| `sump-wallet.exe` | Standalone light wallet |
| `Start Mining.bat` | Double-click launcher for node + GPU miner + dashboard |
| `Open Wallet.bat` | Double-click launcher for standalone wallet GUI |
| `Check Balance.bat` | Double-click balance/address helper |
| `QUICKSTART.txt` | Short offline instructions |

No installer is required. Unzip the folder somewhere permanent and keep the
files together.

Windows may show SmartScreen because the binary is not code-signed yet. Choose
**More info -> Run anyway** if you trust the release and checksums.

## First: back up your wallet

The file `wallet.json` is your key file. Anyone with it can spend your SUMP.
If you lose it, no one can recover your coins.

After creating a wallet, copy `wallet.json` to a safe backup location.

## 1. Miner + Wallet

This is the normal setup for someone who wants to mine SUMP.

### Easiest path

1. Unzip the release package.
2. Double-click **`Start Mining.bat`**.
3. On first run it creates `wallet.json`.
4. Back up `wallet.json`.
5. Leave the miner window open.
6. Open the dashboard in your browser:

```text
http://127.0.0.1:8787
```

The first mining launch prepares a 2 GiB SumpHash dataset. This can take a few
minutes. The miner shows progress while it prepares the dataset.

The dashboard shows:

- block height
- mining status
- GPU/CPU mode
- hashrate
- spendable balance
- pending immature mining rewards
- peer count
- receive address and vault address

### Open the wallet GUI

With `Start Mining.bat` still running, double-click:

```text
Open Wallet.bat
```

Then open:

```text
http://127.0.0.1:8799
```

This wallet GUI talks to your local node over RPC. `Start Mining.bat` already
starts the node with RPC enabled.

### Terminal path

Open PowerShell in the unzipped folder and run:

```powershell
.\sump.exe wallet new
```

```powershell
.\sump.exe node run --mine --gpu --gui --rpc
```

Useful options:

```powershell
# CPU mining instead of GPU
.\sump.exe node run --mine --gui --rpc
```

```powershell
# Terminal-only mining, no dashboard
.\sump.exe node run --mine --gpu --rpc
```

```powershell
# Mine while connecting to a specific peer
.\sump.exe node run --mine --gpu --gui --rpc --connect seed.summerpoem.org:8776
```

GPU mining uses CUDA when available and falls back to CPU if unavailable.

### Mining rewards are not spendable immediately

Mining rewards are coinbase outputs. They mature after **100 blocks** on
mainnet. The dashboard separates:

- **Balance**: spendable SUMP
- **Pending**: mined rewards still maturing

## 2. Non-Mining Full Node

Run this if you want to support the network, relay blocks/transactions, or
serve wallet RPC without mining.

### Simple full node

```powershell
.\sump.exe node run
```

This:

- creates the mainnet genesis locally if needed
- connects to built-in seeds
- syncs the chain
- listens on `0.0.0.0:8776`
- relays blocks, transactions, and peer addresses

### Full node with dashboard

```powershell
.\sump.exe node run --gui
```

Dashboard:

```text
http://127.0.0.1:8787
```

### Full node with wallet RPC

If you want a standalone wallet to connect to this node:

```powershell
.\sump.exe node run --rpc
```

Local wallet RPC default:

```text
127.0.0.1:8788
```

To let other machines connect to your wallet RPC:

```powershell
.\sump.exe node run --rpc 0.0.0.0:8788
```

Only expose RPC publicly if you understand the DoS tradeoff. The RPC holds no
private keys, but it still consumes node resources.

### Firewall / router

For a publicly reachable node, allow inbound TCP:

```text
8776
```

If the node is behind a home router, forward TCP `8776` to the node machine.

Seed nodes:

- `seed.summerpoem.org:8776`
- `seed2.summerpoem.org:8776`

Seed operators should also read [deploy/SEED-SETUP.md](deploy/SEED-SETUP.md).

## 3. Just Wallet

Use this if you only want to hold, receive, and send SUMP without mining or
running a full node.

The standalone wallet keeps your keys locally and connects to a node RPC for
balances and transaction broadcast.

You need a node RPC address, for example:

```text
127.0.0.1:8788
```

for your own local node, or a public/community RPC node if one is available.

### Create a wallet

```powershell
.\sump-wallet.exe new
```

Back up `wallet.json`.

### Show addresses

```powershell
.\sump-wallet.exe address
```

```powershell
.\sump-wallet.exe vault-address
```

The regular address uses ML-DSA. The vault address uses SLH-DSA, a larger
hash-based signature scheme intended for cold storage.

### Open wallet GUI

```powershell
.\sump-wallet.exe gui --node 127.0.0.1:8788
```

Then open:

```text
http://127.0.0.1:8799
```

If using a remote node:

```powershell
.\sump-wallet.exe gui --node example-node.example.com:8788
```

### Check balance

```powershell
.\sump-wallet.exe balance --node 127.0.0.1:8788
```

### Send SUMP

```powershell
.\sump-wallet.exe send --node 127.0.0.1:8788 --to <address> --amount 1.5
```

The wallet signs locally. The node only receives the signed transaction.

## Useful commands

```powershell
# chain status
.\sump.exe node info
```

```powershell
# fully re-validate chain.dat from genesis
.\sump.exe node validate
```

```powershell
# recent miner software versions, useful before upgrades
.\sump.exe node versions
```

```powershell
# private local test chain
.\sump.exe --network regtest node run --mine --gui
```

## Troubleshooting

### I double-clicked `sump.exe` and nothing happened

`sump.exe` is a command-line program. Use the `.bat` launchers, or run it from
PowerShell with a command such as:

```powershell
.\sump.exe node run --mine --gpu --gui --rpc
```

### Miner says it is waiting for peers

Mainnet mining waits until the node has a peer and is synced. This prevents
accidentally mining an isolated fork.

Check:

- internet connection
- firewall/router
- inbound TCP `8776` if you want others to connect to you
- seed reachability:
  - `seed.summerpoem.org:8776`
  - `seed2.summerpoem.org:8776`

### GPU mining falls back to CPU

Install/update the NVIDIA driver and CUDA runtime. CPU mining still works, but
is slower.

### Balance is zero after mining

Freshly mined rewards mature after 100 blocks. Check the dashboard's
**Pending** tile.

### I am upgrading from an older version

Stop the node/miner, replace the executables, and restart. Keep:

- `wallet.json`
- `sumpchain/` or your chosen `--chain-dir`

Do not delete `chain.dat`.

## Technical summary

| Area | Summerpoem choice |
|---|---|
| Signatures | ML-DSA-44 by default; SLH-DSA vault addresses |
| Address format | bech32m, SHA3-256 public-key hash |
| Proof of work | SumpHash v1, fixed 2 GiB dataset, SHA3/SHAKE |
| P2P transport | ML-KEM-768 handshake + ChaCha20-Poly1305 frames |
| Difficulty | per-block ASERT |
| Emission | 42M SUMP cap, smooth geometric decay |
| Ledger | UTXO, segregated witnesses |

## Developer build

Install Rust, then:

```powershell
cargo build --release
cargo test
```

For release packaging on Windows, the project builds static executables and
places release assets in `dist/`.

## Operator docs

- Seed node setup: [deploy/SEED-SETUP.md](deploy/SEED-SETUP.md)
- Upgrade policy: [deploy/UPGRADES.md](deploy/UPGRADES.md)
- systemd service: [deploy/sump-seed.service](deploy/sump-seed.service)


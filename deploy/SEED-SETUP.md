# Running a Summerpoem seed node (Linux)

A seed node is a plain node that stays up on a public address so other nodes
can find the network through it. It doesn't need to mine (no GPU required).

## 1. Build

```bash
# Rust toolchain (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone https://github.com/ardahash/SummerpoemCli.git
cd SummerpoemCli
cargo build --release          # GPU kernel is skipped automatically (no CUDA needed)
sudo cp target/release/sump /usr/local/bin/sump
```

## 2. Create a dedicated user and data dir

```bash
sudo useradd --system --home /var/lib/sump --shell /usr/sbin/nologin sump
sudo mkdir -p /var/lib/sump
sudo chown sump:sump /var/lib/sump
```

## 3. Install the service

```bash
sudo cp deploy/sump-seed.service /etc/systemd/system/sump-seed.service
sudo systemctl daemon-reload
sudo systemctl enable --now sump-seed
sudo systemctl status sump-seed        # should be "active (running)"
journalctl -u sump-seed -f             # follow the log
```

On first start it builds the deterministic genesis (a few seconds) and begins
listening on `0.0.0.0:8776`.

## 4. Make it reachable

```bash
sudo ufw allow 8776/tcp                # if ufw is enabled
```

- On a **cloud VPS**: also open TCP 8776 in the provider's firewall/security group.
- **Behind a home router**: forward TCP 8776 to this machine's LAN IP, and give
  that machine a static/reserved DHCP lease.

Verify from *outside* the network (e.g. phone on cellular, or canyouseeme.org)
that TCP `8776` is reachable on your public address, then point DNS
(`seed.summerpoem.org`) at it.

## Optional: also serve the wallet RPC

To let standalone light wallets query balances / submit sends against this
node, append `--rpc 0.0.0.0:8788` to `ExecStart` and open TCP 8788. The RPC is
unauthenticated (like the P2P layer) and only exposes public-chain data plus
transaction broadcast; it has request timeouts, but treat it as a public
service and watch load.

## Upgrading (IMPORTANT: protocol changes are flag-days)

A version that changes the wire protocol (e.g. the 0.5.5 → 0.5.6 transport
bump) is **not** backward-compatible: old and new nodes reject each other. So
**upgrade all of your nodes together**, don't stagger them.

```bash
sudo systemctl stop sump-seed
cd SummerpoemCli && git pull && cargo build --release
sudo cp target/release/sump /usr/local/bin/sump
sudo systemctl start sump-seed
```

Your `/var/lib/sump/chain.dat` persists across the restart, and any blocks
produced during the brief downtime are re-synced from peers automatically.
From 0.5.6 onward, purely *additive* protocol changes are non-breaking
(older nodes ignore fields/messages they don't understand), so most future
upgrades won't require a coordinated flag-day — only changes that bump the
transport version do.

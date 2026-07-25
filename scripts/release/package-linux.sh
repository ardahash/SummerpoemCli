#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

VERSION="${VERSION:-$(awk -F '"' '/^version = / { print $2; exit }' Cargo.toml)}"
SKIP_BUILD="${SKIP_BUILD:-0}"

if [[ "$SKIP_BUILD" != "1" ]]; then
  cargo build --release -p sump
fi

BIN="$ROOT/target/release/sump"
if [[ ! -x "$BIN" ]]; then
  echo "Missing $BIN. Build first, or rerun with SKIP_BUILD=0." >&2
  exit 1
fi

DIST_ROOT="$ROOT/dist"
PKG_NAME="summerpoem-v${VERSION}-linux-x64"
PKG="$DIST_ROOT/$PKG_NAME"
ARCHIVE="$DIST_ROOT/${PKG_NAME}.tar.gz"

rm -rf "$PKG" "$ARCHIVE"
mkdir -p "$PKG"

cp "$BIN" "$PKG/sump"

cat > "$PKG/start-mining.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

if [[ ! -f "$DIR/wallet.json" ]]; then
  "$DIR/sump" wallet new --wallet "$DIR/wallet.json"
fi

"$DIR/sump" --chain-dir "$DIR/sumpchain" node run --mine --gpu --gui --wallet "$DIR/wallet.json" --connect seed.summerpoem.org:8776
EOF

cat > "$PKG/start-seed-node.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

CHAIN_DIR="${SUMMERPOEM_CHAIN_DIR:-/var/lib/summerpoem}"
"$DIR/sump" --chain-dir "$CHAIN_DIR" node run --listen 0.0.0.0:8776 --no-default-seeds
EOF

cat > "$PKG/check-balance.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"$DIR/sump" --chain-dir "$DIR/sumpchain" wallet balance --wallet "$DIR/wallet.json"
EOF

chmod +x "$PKG/sump" "$PKG/start-mining.sh" "$PKG/start-seed-node.sh" "$PKG/check-balance.sh"

cat > "$PKG/QUICKSTART.txt" <<EOF
Summerpoem v${VERSION} Linux x64

Start mining:
  ./start-mining.sh

Run a public seed node:
  sudo mkdir -p /var/lib/summerpoem
  sudo chown "\$USER":"\$USER" /var/lib/summerpoem
  ./start-seed-node.sh

Check your balance:
  ./check-balance.sh

Important:
- Keep wallet.json private and backed up.
- GPU mining requires an NVIDIA CUDA-capable GPU and driver. If GPU mining is unavailable, the node falls back to CPU.
- A seed node does not need to mine; it only needs to stay online and listen on TCP 8776.

Mainnet genesis:
60235b421eb3478072192851a1ea05eeb221dd8821aeaacb3fcd361abb21ca0d

Public bootstrap seed:
seed.summerpoem.org:8776
EOF

tar -C "$PKG" -czf "$ARCHIVE" .

echo "Created $ARCHIVE"
sha256sum "$ARCHIVE"

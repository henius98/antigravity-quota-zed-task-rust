#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${HOME}/.local/bin"

command -v cargo >/dev/null 2>&1 || {
  echo "error: cargo is required to build the Rust binary" >&2
  exit 1
}

cd "$ROOT"
cargo build --release

mkdir -p "$BIN_DIR"
install -m 0755 \
  "$ROOT/target/release/antigravity-quota" \
  "$BIN_DIR/antigravity-quota"

echo
echo "Installed:"
echo "  $BIN_DIR/antigravity-quota"
echo
echo "Now merge:"
echo "  $ROOT/zed/tasks.json -> ~/.config/zed/tasks.json"
echo
echo "Optional keyboard shortcut:"
echo "  merge $ROOT/zed/keymap.json -> ~/.config/zed/keymap.json"
echo
echo "Test:"
echo "  $BIN_DIR/antigravity-quota"

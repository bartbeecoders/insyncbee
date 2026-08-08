#!/usr/bin/env bash
# Build release binaries for CLI and GUI
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== InSyncBee Release Build ==="

cd "$PROJECT_DIR"

# Build CLI
echo ""
echo "--- Building CLI (insyncbee-daemon) ---"
cargo build --release --package insyncbee-daemon
echo "CLI binary: target/release/insyncbee"

# Build GUI
echo ""
echo "--- Building GUI (Tauri app) ---"
cargo tauri build
echo ""
echo "=== Build complete ==="
# One target dir for the whole workspace, so bundles are NOT under src-tauri/.
echo "Bundles are in: target/release/bundle/"

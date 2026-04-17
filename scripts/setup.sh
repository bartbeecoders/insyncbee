#!/usr/bin/env bash
# Install all dependencies (Rust + frontend)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== InSyncBee Setup ==="

# Check required tools
for cmd in cargo pnpm; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "Error: '$cmd' is not installed."
        exit 1
    fi
done

# Install frontend dependencies
echo ""
echo "--- Installing frontend dependencies ---"
cd "$PROJECT_DIR/ui"
pnpm install

# Build Rust workspace (check only, to download + compile deps)
echo ""
echo "--- Building Rust workspace ---"
cd "$PROJECT_DIR"
cargo build

echo ""
echo "=== Setup complete ==="
echo ""
echo "Next steps:"
echo "  1. Set OAuth credentials:  export INSYNCBEE_CLIENT_ID=... INSYNCBEE_CLIENT_SECRET=..."
echo "  2. Run the GUI:            ./scripts/dev-gui.sh"
echo "  3. Or run the CLI:         ./scripts/dev-cli.sh login"

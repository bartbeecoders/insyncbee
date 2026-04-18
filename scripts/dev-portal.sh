#!/usr/bin/env bash
# Run the InSyncBee marketing portal in dev mode (Vite HMR).
# Extra args are forwarded to `pnpm dev`, e.g.:
#   ./scripts/dev-portal.sh --host         # expose to LAN
#   ./scripts/dev-portal.sh --port 5174    # override port
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORTAL_DIR="$(dirname "$SCRIPT_DIR")/insyncbee.portal"

if ! command -v pnpm >/dev/null 2>&1; then
    echo "Error: pnpm not found. Install it with: npm install -g pnpm" >&2
    exit 1
fi

cd "$PORTAL_DIR"

if [ ! -d node_modules ]; then
    echo "Installing portal dependencies (first run)..."
    pnpm install
fi

pnpm dev "$@"

#!/usr/bin/env bash
# Run the Tauri desktop app in development mode (hot-reload)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [ -z "${INSYNCBEE_CLIENT_ID:-}" ] || [ -z "${INSYNCBEE_CLIENT_SECRET:-}" ]; then
    echo "Warning: INSYNCBEE_CLIENT_ID and/or INSYNCBEE_CLIENT_SECRET not set."
    echo "Google login will not work. See docs/USAGE.md for setup instructions."
    echo ""
fi

cd "$PROJECT_DIR"
export WEBKIT_DISABLE_DMABUF_RENDERER=1
cargo tauri dev

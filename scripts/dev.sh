#!/usr/bin/env bash
# Start the backend daemon and the Tauri GUI in development mode.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [ -z "${INSYNCBEE_CLIENT_ID:-}" ] || [ -z "${INSYNCBEE_CLIENT_SECRET:-}" ]; then
    echo "Warning: INSYNCBEE_CLIENT_ID and/or INSYNCBEE_CLIENT_SECRET not set."
    echo "Google login will not work. See docs/USAGE.md for setup instructions."
    echo ""
fi

cd "$PROJECT_DIR"

# Start the backend daemon in the background
cargo run --package insyncbee-daemon -- daemon &
DAEMON_PID=$!
echo "Backend daemon started (PID $DAEMON_PID)"

# Ensure the daemon is killed when this script exits
trap 'echo "Stopping backend daemon (PID $DAEMON_PID)..."; kill "$DAEMON_PID" 2>/dev/null; wait "$DAEMON_PID" 2>/dev/null' EXIT

# Start the GUI (foreground — exits when the window is closed)
export WEBKIT_DISABLE_DMABUF_RENDERER=1
cargo tauri dev

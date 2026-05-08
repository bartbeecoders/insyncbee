#!/usr/bin/env bash
# Start the backend daemon and the Tauri GUI in development mode.
#
# On every invocation:
#   1. Any leftover insyncbee daemon from a prior run (crashed terminal,
#      hard-killed shell, etc.) is stopped before we start.
#   2. The daemon is rebuilt from source.
#   3. The freshly-built binary is launched in the background.
#   4. The Tauri GUI runs in the foreground; closing it (or Ctrl-C) shuts
#      the daemon down via the cleanup trap.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [ -z "${INSYNCBEE_CLIENT_ID:-}" ] || [ -z "${INSYNCBEE_CLIENT_SECRET:-}" ]; then
    echo "Warning: INSYNCBEE_CLIENT_ID and/or INSYNCBEE_CLIENT_SECRET not set."
    echo "Google login will not work. See docs/USAGE.md for setup instructions."
    echo ""
fi

cd "$PROJECT_DIR"

# ── 1. Stop any leftover daemon from a previous dev run ─────────────
# Match the built binary path precisely so we never touch unrelated processes.
DAEMON_BIN="$PROJECT_DIR/target/debug/insyncbee"
DAEMON_PATTERN="$DAEMON_BIN daemon"
if pgrep -f "$DAEMON_PATTERN" >/dev/null 2>&1; then
    echo "Stopping previous daemon..."
    pkill -TERM -f "$DAEMON_PATTERN" 2>/dev/null || true
    # Up to 5s for graceful shutdown, then SIGKILL stragglers.
    for _ in 1 2 3 4 5; do
        pgrep -f "$DAEMON_PATTERN" >/dev/null 2>&1 || break
        sleep 1
    done
    if pgrep -f "$DAEMON_PATTERN" >/dev/null 2>&1; then
        echo "Daemon didn't exit cleanly — sending SIGKILL."
        pkill -KILL -f "$DAEMON_PATTERN" 2>/dev/null || true
    fi
fi

# ── 2. Build first so we know it succeeded before launching ─────────
echo "Building daemon..."
cargo build --package insyncbee-daemon

# ── 3. Launch the fresh binary in the background ────────────────────
"$DAEMON_BIN" daemon &
DAEMON_PID=$!
echo "Backend daemon started (PID $DAEMON_PID)"

cleanup() {
    if kill -0 "$DAEMON_PID" 2>/dev/null; then
        echo "Stopping backend daemon (PID $DAEMON_PID)..."
        kill -TERM "$DAEMON_PID" 2>/dev/null || true
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
}
# EXIT covers normal exits; INT/TERM/HUP catch Ctrl-C and terminal close.
trap cleanup EXIT INT TERM HUP

# ── 4. Tauri GUI in the foreground ──────────────────────────────────
export WEBKIT_DISABLE_DMABUF_RENDERER=1
cargo tauri dev

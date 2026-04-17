#!/usr/bin/env bash
# Run the CLI (insyncbee-daemon) in development mode.
# All arguments are forwarded, e.g.:
#   ./scripts/dev-cli.sh login
#   ./scripts/dev-cli.sh add --name "Docs" --local ~/Documents --remote-id root --account <id>
#   ./scripts/dev-cli.sh sync
#   ./scripts/dev-cli.sh daemon
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [ -z "${INSYNCBEE_CLIENT_ID:-}" ] || [ -z "${INSYNCBEE_CLIENT_SECRET:-}" ]; then
    echo "Warning: INSYNCBEE_CLIENT_ID and/or INSYNCBEE_CLIENT_SECRET not set."
    echo "Google login will not work. See docs/USAGE.md for setup instructions."
    echo ""
fi

cd "$PROJECT_DIR"
cargo run --package insyncbee-daemon -- "$@"

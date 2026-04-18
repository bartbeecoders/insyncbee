#!/bin/bash
set -euo pipefail

#===============================================================================
# InSyncBee - Upload release binaries to VPS
#===============================================================================
# Rsyncs artifacts from ./releases/<version>/ into the VPS hostPath mounted by
# the insyncbee-portal pod at /srv/insyncbee/releases.
#
# Usage:
#   ./scripts/upload-release.sh [version]
#
# Environment overrides:
#   VPS_IP, VPS_USER
#===============================================================================

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

VPS_IP="${VPS_IP:-212.47.77.32}"
VPS_USER="${VPS_USER:-bart}"
VERSION="${1:-$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' releases.json | head -1 | sed 's/.*"\([^"]*\)"$/\1/')}"

LOCAL_DIR="releases/$VERSION"
REMOTE_DIR="/srv/insyncbee/releases"

if [[ ! -d "$LOCAL_DIR" ]]; then
  echo "No artifacts directory found at $LOCAL_DIR"
  echo "Drop your built binaries there (matching filenames in releases.json), then re-run."
  exit 1
fi

echo "Uploading artifacts from $LOCAL_DIR to $VPS_USER@$VPS_IP:$REMOTE_DIR ..."

# Ensure the remote dir exists and is writable (needs sudo on most hosts)
ssh -o StrictHostKeyChecking=accept-new "$VPS_USER@$VPS_IP" \
  "sudo mkdir -p $REMOTE_DIR && sudo chown -R $VPS_USER:$VPS_USER $REMOTE_DIR"

rsync -avh --progress --delete-after \
  "$LOCAL_DIR/" \
  "$VPS_USER@$VPS_IP:$REMOTE_DIR/"

echo ""
echo "✓ Uploaded version $VERSION"
echo "  Binaries are now served at https://insyncbee.dev/releases/"

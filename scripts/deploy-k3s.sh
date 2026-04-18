#!/bin/bash
set -euo pipefail

#===============================================================================
# InSyncBee Portal - K3S Deployment Script (Podman)
#===============================================================================
# Builds and pushes the insyncbee.dev marketing site, then deploys to K3S on VPS.
# Architecture: static Nginx container serving the Vite build, plus a hostPath
#               mount at /srv/insyncbee/releases for the download binaries.
# Namespace: insyncbee
# Registry:  beecodersregistry.azurecr.io (override with REGISTRY=...)
#
# Usage:
#   ./scripts/deploy-k3s.sh [all|build|push|deploy|status|upload-releases|ingress]
#
# Required tools locally:
#   podman, ssh, scp
#===============================================================================

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

COMMAND="${1:-all}"

REGISTRY="${REGISTRY:-beecodersregistry.azurecr.io}"
NAMESPACE="insyncbee"
IMAGE_NAME="insyncbee-portal"
IMAGE="$REGISTRY/$IMAGE_NAME"
PORTAL_DIR="$ROOT_DIR/insyncbee.portal"

# VPS target (override via environment variables)
VPS_IP="${VPS_IP:-212.47.77.32}"
VPS_USER="${VPS_USER:-bart}"

VPS_BASE_DIR="${VPS_BASE_DIR:-~/insyncbee}"
VPS_K8S_DIR="$VPS_BASE_DIR/k8s"

# Version from package.json
APP_VERSION=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$PORTAL_DIR/package.json" | head -1 | sed 's/.*"\([^"]*\)"$/\1/')

ssh_vps() {
  local cmd="$1"
  ssh -o StrictHostKeyChecking=accept-new "$VPS_USER@$VPS_IP" "bash -lc $(printf %q "$cmd")"
}

kubectl_vps() {
  local args="$1"
  ssh_vps "if command -v kubectl >/dev/null 2>&1; then kubectl $args; else sudo k3s kubectl $args; fi"
}

check_deps() {
  command -v podman >/dev/null 2>&1 || { echo "podman not found"; exit 1; }
  command -v ssh >/dev/null 2>&1 || { echo "ssh not found"; exit 1; }
  command -v scp >/dev/null 2>&1 || { echo "scp not found"; exit 1; }
}

build_image() {
  echo "Building $IMAGE_NAME image v$APP_VERSION..."
  # Copy the top-level releases.json into the build context so the Vite build
  # can bundle the current manifest. (Optional — the app ships with a default.)
  if [[ -f "$ROOT_DIR/releases.json" ]]; then
    cp "$ROOT_DIR/releases.json" "$PORTAL_DIR/releases.json"
  fi
  podman build \
    -t "$IMAGE:latest" \
    -t "$IMAGE:$APP_VERSION" \
    -f "$PORTAL_DIR/Dockerfile" \
    "$PORTAL_DIR"
}

push_image() {
  echo "Logging into registry $REGISTRY..."
  if [[ -n "${REGISTRY_USER:-}" && -n "${REGISTRY_PASSWORD:-}" ]]; then
    podman login -u "$REGISTRY_USER" -p "$REGISTRY_PASSWORD" "$REGISTRY"
  else
    podman login "$REGISTRY"
  fi

  echo "Pushing $IMAGE_NAME image..."
  podman push "$IMAGE:latest"
  podman push "$IMAGE:$APP_VERSION"
}

deploy_manifests() {
  echo "Deploying to $VPS_USER@$VPS_IP (namespace: $NAMESPACE)..."

  # Ensure working directory on VPS (no sudo over non-TTY ssh — fall back only if needed)
  ssh_vps "mkdir -p $VPS_K8S_DIR"

  # The hostPath releases dir (/srv/insyncbee/releases) is created by the
  # kubelet via DirectoryOrCreate the first time the pod starts. If you want
  # the CI pipeline / scp to write there as $VPS_USER, run ONCE on the VPS:
  #   sudo mkdir -p /srv/insyncbee/releases
  #   sudo chown -R $USER:$USER /srv/insyncbee
  # We don't do that here because non-TTY ssh has no way to enter a sudo
  # password.

  # Copy manifests
  scp -o StrictHostKeyChecking=accept-new -r "$ROOT_DIR/k8s/portal" "$VPS_USER@$VPS_IP:$VPS_K8S_DIR/"

  # Apply
  kubectl_vps "apply -f $VPS_K8S_DIR/portal/namespace.yaml"

  # Check for image pull secret
  if ! kubectl_vps "-n $NAMESPACE get secret acr-secret >/dev/null 2>&1"; then
    echo ""
    echo "⚠ Missing secret '$NAMESPACE/acr-secret' (imagePullSecret)."
    echo "  Create it so K3S can pull images from $REGISTRY. Example:"
    echo ""
    echo "    kubectl -n $NAMESPACE create secret docker-registry acr-secret \\"
    echo "      --docker-server=$REGISTRY \\"
    echo "      --docker-username=<SP_APP_ID> \\"
    echo "      --docker-password=<SP_PASSWORD>"
    echo ""
  fi

  kubectl_vps "apply -f $VPS_K8S_DIR/portal/deployment.yaml"

  # Restart to pick up new image
  kubectl_vps "-n $NAMESPACE rollout restart deployment insyncbee-portal"
  kubectl_vps "-n $NAMESPACE rollout status deployment insyncbee-portal --timeout=90s"

  echo ""
  echo "✓ Deployed insyncbee-portal v$APP_VERSION"
  echo "  • NodePort: http://$VPS_IP:32081"
  echo "  • Ingress:  run ./scripts/deploy-k3s.sh ingress to apply the Ingress rule"
  echo ""
  echo "NOTE: To publish download binaries, run:"
  echo "  ./scripts/upload-release.sh"
}

deploy_ingress() {
  echo "Applying Ingress (requires cert-manager + ClusterIssuer letsencrypt-prod)..."
  scp -o StrictHostKeyChecking=accept-new "$ROOT_DIR/k8s/portal/ingress.yaml" "$VPS_USER@$VPS_IP:$VPS_K8S_DIR/portal/"
  kubectl_vps "apply -f $VPS_K8S_DIR/portal/ingress.yaml"
  kubectl_vps "-n $NAMESPACE get ingress"
}

upload_releases() {
  "$ROOT_DIR/scripts/upload-release.sh"
}

status() {
  kubectl_vps "-n $NAMESPACE get pods,svc,ingress"
}

main() {
  check_deps

  case "$COMMAND" in
    all)
      echo "Deploying insyncbee-portal v$APP_VERSION"
      build_image
      push_image
      deploy_manifests
      status
      ;;
    build)           build_image ;;
    push)            push_image ;;
    deploy)          deploy_manifests ;;
    ingress)         deploy_ingress ;;
    status)          status ;;
    upload-releases) upload_releases ;;
    *)
      echo "Unknown command: $COMMAND"
      echo "Usage: $0 [all|build|push|deploy|ingress|status|upload-releases]"
      exit 1
      ;;
  esac
}

main

# insyncbee.portal

Marketing + download site for [InSyncBee](../).

See [PLAN.md](./PLAN.md) for the full architecture and deployment guide.

## Quick start

```bash
pnpm install
pnpm dev       # http://localhost:5173
pnpm build     # → dist/
```

## Deploy

```bash
# From the repo root:
./scripts/deploy-k3s.sh              # build + push + apply
./scripts/deploy-k3s.sh ingress      # apply TLS ingress (once)
./scripts/upload-release.sh 0.1.0    # publish binaries under /releases/
```

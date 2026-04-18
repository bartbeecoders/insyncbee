# InSyncBee Portal — Plan

The marketing / download site for InSyncBee. Lives at `insyncbee.dev` (TBD),
served by an nginx container on the existing K3S VPS alongside the `sqail` site.

---

## 1. Goals

1. Explain the product in under 10 seconds above the fold.
2. Let visitors download the right binary in ≤ 2 clicks (OS auto-detection).
3. Make the "why us, not Insync" case explicit and defensible.
4. Be trivially cheap to host — single static container, no backend.
5. Ship fast: a `pnpm build` → `podman push` → `kubectl apply` deploys in < 2 min.

Non-goals for v1: blog, i18n, newsletter, telemetry, docs portal (docs live
in the GitHub repo).

---

## 2. Stack

| Layer          | Choice                   | Why                                             |
|----------------|--------------------------|--------------------------------------------------|
| Framework      | **Vite + React 18 + TS** | Familiar, fast dev HMR, tiny prod bundle.        |
| Styling        | **Vanilla CSS**          | No runtime tax, zero dependencies, easy to read. |
| Icons / branding| **Inline SVG**          | Scales perfectly, no image pipeline.             |
| Runtime        | **nginx:alpine**         | ~20 MB image, bulletproof static serving.        |
| Orchestration  | **K3S (existing VPS)**   | Reuses the infra that hosts sqail.dev.           |
| Registry       | Azure Container Registry | Matches template; override via `REGISTRY` env.   |

Fonts: Inter (UI) + JetBrains Mono (code accents), loaded from Google Fonts
with `preconnect`. Total critical path: ~30 KB gzipped CSS+JS, fonts async.

---

## 3. Information architecture

```
┌─ Nav (sticky) ────────────────────────────────────────────────┐
│  Logo · Features · How · Compare · FAQ · GitHub · [Download]  │
└───────────────────────────────────────────────────────────────┘

┌─ Hero ────────────────────────────────────────────────────────┐
│  H1: "Fast Google Drive sync for every desktop."              │
│  Tagline, [Download] [GitHub], honeycomb+bee illustration     │
└───────────────────────────────────────────────────────────────┘

┌─ Features (6-card grid) ──────────────────────────────────────┐
│  Block-level delta · Conflict resolution · Placeholders       │
│  Data safety · Bandwidth control · Google Docs handling       │
└───────────────────────────────────────────────────────────────┘

┌─ How it works (3 numbered steps) ─────────────────────────────┐
│  1. Sign in · 2. Pair a folder · 3. Sync and forget           │
└───────────────────────────────────────────────────────────────┘

┌─ Download ────────────────────────────────────────────────────┐
│  Recommended-for-you card (UA detection)                       │
│  Linux · macOS · Windows cards with .deb/.rpm/.AppImage,       │
│  .dmg (arm64+intel), .msi and portable .zip                    │
└───────────────────────────────────────────────────────────────┘

┌─ Compare table ───────────────────────────────────────────────┐
│  Drive desktop vs Insync vs rclone vs InSyncBee, 9 rows        │
└───────────────────────────────────────────────────────────────┘

┌─ FAQ (7 questions) ───────────────────────────────────────────┐
│  Pricing, Insync differences, data storage, multi-account,    │
│  CLI, Shared Drives, contributing                              │
└───────────────────────────────────────────────────────────────┘

┌─ Footer ─ copyright, GitHub, Issues, FAQ, License ────────────┘
```

All sections are anchor-linked from the nav; scroll is smooth.

---

## 4. Design

- **Palette:** same amber (`#f5a623`) as the app, dark slate background
  (`#0f1115`). Single accent colour — everything else is neutral grey.
- **Motif:** honeycomb hexagons, echoed in the hero SVG, the logo, and the
  floating badge overlays.
- **Type:** Inter, four weights. Generous line-height in body copy, tight
  `letter-spacing: -0.02em` on headings.
- **Motion:** subtle only — a gentle float on the hero badges and a
  transform/border colour hover on cards. No scroll-jacking, no parallax.
- **Responsive:** two breakpoints (900px, 620px). Features grid collapses
  3→2→1, download grid 3→1.

**Illustrations:** the hero bee-on-honeycomb is hand-built SVG. To upgrade to
raster/AI-generated hero art, replace `src/assets/Logo.tsx#Honeycomb` or drop
a PNG into `public/` and swap the `<Honeycomb />` component for an `<img>`.

> The task asked for the xAI-image MCP for branding. That MCP is not available
> in this environment, so v1 ships with hand-rolled SVG. When images are
> produced, add them under `public/brand/` and reference them from Hero.tsx.

---

## 5. Downloads flow

The site offers binaries for Linux, macOS, Windows. The manifest lives in
**two** places:

1. `/releases.json` at the repo root — source of truth, edited for each
   release. `scripts/deploy-k3s.sh build` copies it into the portal's build
   context.
2. `src/data/releases.ts` — a TypeScript default baked into the bundle so the
   page works even if `releases.json` is missing.

At runtime the UI auto-detects the OS from `navigator.platform` /
`navigator.userAgent` and surfaces the most likely installer at the top.

The actual binaries live on the VPS at **`/srv/insyncbee/releases/`** and are
mounted into the pod as a read-only `hostPath`. nginx serves them under
`/releases/<filename>` with `Cache-Control: no-store` and
`Content-Disposition: attachment`. Upload via:

```bash
./scripts/upload-release.sh 0.1.0
```

which rsyncs `./releases/<version>/*` to the VPS over SSH.

---

## 6. Deployment architecture

```
┌─── dev machine ───┐         ┌──── Azure CR ────┐       ┌───── VPS (K3S) ─────┐
│ pnpm build        │──image──►                   │──pull─►  insyncbee-portal   │
│ podman build      │         │ insyncbee-portal │       │  (nginx + static)    │
│ podman push       │         └──────────────────┘       │  NodePort 32081      │
│                   │                                    │  Ingress insyncbee.dev│
│ rsync releases/*  │─────────SSH──────────────────────► │  hostPath:           │
└───────────────────┘                                    │  /srv/insyncbee/rel..│
                                                         └──────────────────────┘
```

- **Namespace:** `insyncbee`
- **Image:** `beecodersregistry.azurecr.io/insyncbee-portal`
- **NodePort:** `32081` (sqail uses `32080`)
- **Ingress host:** `insyncbee.dev` (placeholder, update the manifest)
- **TLS:** cert-manager via ClusterIssuer `letsencrypt-prod`
- **Image pull secret:** `insyncbee/acr-secret` — created manually once
- **Release artifacts volume:** `hostPath: /srv/insyncbee/releases` (read-only)

### Commands

```bash
# End-to-end:
./scripts/deploy-k3s.sh              # build → push → deploy → status
./scripts/deploy-k3s.sh ingress      # apply TLS ingress (once)
./scripts/upload-release.sh 0.1.0    # publish binaries

# Piecemeal:
./scripts/deploy-k3s.sh build
./scripts/deploy-k3s.sh push
./scripts/deploy-k3s.sh deploy
./scripts/deploy-k3s.sh status
```

---

## 7. File layout

```
insyncbee.portal/
├── PLAN.md                 ← this file
├── package.json
├── tsconfig.json
├── vite.config.ts
├── index.html
├── Dockerfile
├── nginx.conf
├── .dockerignore
├── .gitignore
├── public/
│   ├── favicon.svg
│   └── og-image.svg
└── src/
    ├── main.tsx
    ├── App.tsx
    ├── index.css
    ├── assets/Logo.tsx
    ├── data/releases.ts
    └── components/
        ├── Nav.tsx
        ├── Hero.tsx
        ├── Features.tsx
        ├── HowItWorks.tsx
        ├── Download.tsx
        ├── Compare.tsx
        ├── FAQ.tsx
        └── Footer.tsx

# Repo-level additions
releases.json                         ← source-of-truth release manifest
k8s/portal/
├── namespace.yaml
├── deployment.yaml                   ← Deployment + Service (NodePort 32081)
└── ingress.yaml                      ← Traefik Ingress + buffering middleware
scripts/
├── deploy-k3s.sh                     ← build / push / deploy / status
└── upload-release.sh                 ← rsync binaries to VPS
```

---

## 8. Open items / flagged placeholders

Things to confirm before going live:

1. **Domain** — `insyncbee.dev` is a placeholder. Update
   `k8s/portal/ingress.yaml` once the real domain is registered, and add the
   DNS A record `insyncbee.dev → 212.47.77.32`.
2. **GitHub URLs** — Nav, Hero, Footer link to
   `https://github.com/bartroelant/InSyncBee`. Update if the repo moves.
3. **Logos/brand images** — currently inline SVG. Swap in AI-generated art
   (xAI / image-gen) when available.
4. **Real release binaries** — `releases.json` lists filenames but the actual
   artifacts don't exist yet. Hook this up to the Tauri bundle pipeline once
   Linux/Mac/Windows builds are producing.
5. **Registry credentials** — create the image-pull secret on the VPS:

   ```bash
   kubectl -n insyncbee create secret docker-registry acr-secret \
     --docker-server=beecodersregistry.azurecr.io \
     --docker-username=<SP_APP_ID> \
     --docker-password=<SP_PASSWORD>
   ```

6. **Analytics** — none in v1. If added later, stick to privacy-friendly
   (Plausible / simple self-hosted) to stay on-brand with "no telemetry".

---

## 9. Local development

```bash
cd insyncbee.portal
pnpm install
pnpm dev       # http://localhost:5173
pnpm build     # outputs dist/
pnpm preview   # serves dist/
```

Docker:

```bash
podman build -t insyncbee-portal:local -f insyncbee.portal/Dockerfile insyncbee.portal
podman run --rm -p 8080:80 insyncbee-portal:local
```

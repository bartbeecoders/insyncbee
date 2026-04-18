# ── Build stage ──────────────────────────────────────────────────
FROM node:20-alpine AS build

WORKDIR /app

# Enable corepack so we can use pnpm from the repo
RUN corepack enable

COPY package.json pnpm-lock.yaml* package-lock.json* ./
RUN if [ -f pnpm-lock.yaml ]; then \
      pnpm install --frozen-lockfile; \
    elif [ -f package-lock.json ]; then \
      npm ci; \
    else \
      npm install; \
    fi

COPY . .

# Version + releasedAt are sed'd into src/data/releases.ts by the
# deploy-portal job before this build runs, so the bundled SPA matches the
# tag being released.
RUN npm run build

# ── Runtime stage ────────────────────────────────────────────────
FROM nginx:1.27-alpine

COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html

# /releases is mounted as a hostPath (or emptyDir) so the pod can serve
# binaries uploaded out-of-band by upload-release.sh.
RUN mkdir -p /usr/share/nginx/html/releases

EXPOSE 80

HEALTHCHECK --interval=30s --timeout=3s CMD wget -qO- http://localhost/healthz || exit 1

CMD ["nginx", "-g", "daemon off;"]

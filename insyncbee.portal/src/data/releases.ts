export type OsKey = "linux" | "mac" | "windows";

// `osLabel` is the segment that appears in filenames produced by the release
// pipeline (e.g. "linux-x86_64", "macos-aarch64", "windows-x86_64").
//
// `product` is the filename stem, and it is per-artifact rather than
// per-release because one release ships two products: the headless
// db-service and the desktop app.
export interface Artifact {
  kind: "tar.gz" | "zip" | "deb" | "rpm" | "AppImage"; // filename extension
  product: string;        // e.g. "insyncbee-db-service", "insyncbee-desktop"
  label: string;          // human-readable label shown on the card
  arch: string;           // e.g. "x86_64", "aarch64"
  osLabel: string;        // matrix label produced by .github/workflows/release.yml
  size?: string;
  sha256?: string;
}

export interface PlatformRelease {
  os: OsKey;
  displayName: string;
  requirement: string;
  artifacts: Artifact[];
}

export type ProductId = "desktop" | "db-service";

export interface ProductRelease {
  id: ProductId;
  displayName: string;
  tagline: string;
  platforms: PlatformRelease[];
}

export interface Releases {
  version: string;     // sed-replaced by CI (deploy-portal step)
  channel: "stable" | "beta" | "dev";
  releasedAt: string;  // sed-replaced by CI (deploy-portal step)
  repo: string;
  products: ProductRelease[];
}

// Single source of truth. CI rewrites only `version` and `releasedAt` before
// the portal Docker build — every filename is derived from `version` so a
// tag bump propagates everywhere automatically. Keep exactly one
// `version: "…"` and one `releasedAt: "…"` in this file: the deploy-portal
// sed is unanchored and would rewrite any others too.
export const DEFAULT_RELEASES: Releases = {
  version: "0.2.5",
  channel: "dev",
  releasedAt: "2026-08-08",
  repo: "bartbeecoders/insyncbee",
  products: [
    {
      id: "desktop",
      displayName: "Desktop app",
      tagline:
        "The full app: window, tray icon, conflict resolution, start on login.",
      platforms: [
        {
          os: "linux",
          displayName: "Linux",
          requirement:
            "GTK 3 + WebKit2GTK 4.1. Tray support is bundled — nothing extra to install.",
          artifacts: [
            {
              kind: "AppImage",
              product: "insyncbee-desktop",
              label: "AppImage (portable)",
              arch: "x86_64",
              osLabel: "linux-x86_64",
            },
            {
              kind: "deb",
              product: "insyncbee-desktop",
              label: "Debian / Ubuntu (.deb)",
              arch: "x86_64",
              osLabel: "linux-x86_64",
            },
            {
              kind: "rpm",
              product: "insyncbee-desktop",
              label: "Fedora / openSUSE (.rpm)",
              arch: "x86_64",
              osLabel: "linux-x86_64",
            },
          ],
        },
      ],
    },
    {
      id: "db-service",
      displayName: "db-service",
      tagline:
        "The headless background sync service — CLI and daemon, no window, no tray.",
      platforms: [
        {
          os: "linux",
          displayName: "Linux",
          requirement: "glibc 2.35+ (Ubuntu 22.04+, Fedora 38+, Arch)",
          artifacts: [
            {
              kind: "tar.gz",
              product: "insyncbee-db-service",
              label: "Linux x86_64 (tar.gz)",
              arch: "x86_64",
              osLabel: "linux-x86_64",
            },
          ],
        },
        {
          os: "mac",
          displayName: "macOS",
          requirement: "macOS 12 Monterey or later (Apple Silicon)",
          artifacts: [
            {
              kind: "tar.gz",
              product: "insyncbee-db-service",
              label: "Apple Silicon (tar.gz)",
              arch: "aarch64",
              osLabel: "macos-aarch64",
            },
          ],
        },
        {
          os: "windows",
          displayName: "Windows",
          requirement: "Windows 10 1809 or later",
          artifacts: [
            {
              kind: "zip",
              product: "insyncbee-db-service",
              label: "Windows x86_64 (zip)",
              arch: "x86_64",
              osLabel: "windows-x86_64",
            },
          ],
        },
      ],
    },
  ],
};

export function productById(
  id: ProductId,
  release: Releases = DEFAULT_RELEASES,
): ProductRelease | undefined {
  return release.products.find((p) => p.id === id);
}

export function artifactFilename(
  artifact: Artifact,
  release: Releases = DEFAULT_RELEASES,
): string {
  return `${artifact.product}-${release.version}-${artifact.osLabel}.${artifact.kind}`;
}

export function detectOs(): OsKey | null {
  if (typeof navigator === "undefined") return null;
  const ua = navigator.userAgent.toLowerCase();
  const p = navigator.platform?.toLowerCase() ?? "";
  if (p.includes("mac") || ua.includes("mac os")) return "mac";
  if (p.includes("win") || ua.includes("windows")) return "windows";
  if (p.includes("linux") || ua.includes("linux")) return "linux";
  return null;
}

// Binaries are served by the portal pod from a hostPath volume populated by
// the release pipeline (scp into /srv/insyncbee/releases on the VPS). The
// nginx config maps /releases/* to that mount.
export function downloadUrl(
  artifact: Artifact,
  release: Releases = DEFAULT_RELEASES,
): string {
  return `/releases/${artifactFilename(artifact, release)}`;
}

export function checksumUrl(
  artifact: Artifact,
  release: Releases = DEFAULT_RELEASES,
): string {
  return `${downloadUrl(artifact, release)}.sha256`;
}

export function githubReleaseUrl(release: Releases = DEFAULT_RELEASES): string {
  return `https://github.com/${release.repo}/releases/tag/v${release.version}`;
}

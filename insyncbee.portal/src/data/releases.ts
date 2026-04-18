export type OsKey = "linux" | "mac" | "windows";

export interface Artifact {
  kind: string;        // e.g. "tar.gz", "zip"
  label: string;       // human-readable label shown on the card
  arch: string;        // e.g. "x86_64", "aarch64"
  filename: string;    // matches the GitHub Release asset filename
  size?: string;       // e.g. "9.2 MB"
  sha256?: string;     // optional
}

export interface PlatformRelease {
  os: OsKey;
  displayName: string;
  requirement: string;
  artifacts: Artifact[];
}

export interface Releases {
  version: string;
  channel: "stable" | "beta" | "dev";
  releasedAt: string; // ISO
  product: string;    // e.g. "insyncbee-db-service"
  repo: string;       // "<owner>/<name>" — drives the GitHub release URL
  platforms: PlatformRelease[];
}

// Default baked-in manifest. The Docker build copies the top-level
// `releases.json` over this on `deploy-portal.yml` so the portal always
// reflects the latest published GitHub Release.
export const DEFAULT_RELEASES: Releases = {
  version: "0.1.0",
  channel: "dev",
  releasedAt: "2026-04-18",
  product: "insyncbee-db-service",
  repo: "bartbeecoders/insyncbee",
  platforms: [
    {
      os: "linux",
      displayName: "Linux",
      requirement: "glibc 2.35+ (Ubuntu 22.04+, Fedora 38+, Arch)",
      artifacts: [
        {
          kind: "tar.gz",
          label: "Linux x86_64 (tar.gz)",
          arch: "x86_64",
          filename: "insyncbee-db-service-0.1.0-linux-x86_64.tar.gz",
        },
      ],
    },
    {
      os: "mac",
      displayName: "macOS",
      requirement: "macOS 12 Monterey or later",
      artifacts: [
        {
          kind: "tar.gz",
          label: "Apple Silicon (tar.gz)",
          arch: "aarch64",
          filename: "insyncbee-db-service-0.1.0-macos-aarch64.tar.gz",
        },
        {
          kind: "tar.gz",
          label: "Intel (tar.gz)",
          arch: "x86_64",
          filename: "insyncbee-db-service-0.1.0-macos-x86_64.tar.gz",
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
          label: "Windows x86_64 (zip)",
          arch: "x86_64",
          filename: "insyncbee-db-service-0.1.0-windows-x86_64.zip",
        },
      ],
    },
  ],
};

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
export function downloadUrl(filename: string): string {
  return `/releases/${filename}`;
}

export function checksumUrl(filename: string): string {
  return `${downloadUrl(filename)}.sha256`;
}

// Direct GitHub Release URL — used by the "view on GitHub" footnote.
export function githubReleaseUrl(release: Releases = DEFAULT_RELEASES): string {
  return `https://github.com/${release.repo}/releases/tag/v${release.version}`;
}

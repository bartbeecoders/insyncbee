export type OsKey = "linux" | "mac" | "windows";

// `osLabel` is the segment that appears in archive filenames produced by the
// release pipeline (e.g. "linux-x86_64", "macos-aarch64", "windows-x86_64").
export interface Artifact {
  kind: "tar.gz" | "zip"; // archive extension
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

export interface Releases {
  version: string;     // sed-replaced by CI (deploy-portal step)
  channel: "stable" | "beta" | "dev";
  releasedAt: string;  // sed-replaced by CI (deploy-portal step)
  product: string;
  repo: string;
  platforms: PlatformRelease[];
}

// Single source of truth. CI rewrites only `version` and `releasedAt` before
// the portal Docker build — every filename is derived from `version` so a
// tag bump propagates everywhere automatically.
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
        { kind: "tar.gz", label: "Linux x86_64 (tar.gz)", arch: "x86_64", osLabel: "linux-x86_64" },
      ],
    },
    {
      os: "mac",
      displayName: "macOS",
      requirement: "macOS 12 Monterey or later (Apple Silicon)",
      artifacts: [
        { kind: "tar.gz", label: "Apple Silicon (tar.gz)", arch: "aarch64", osLabel: "macos-aarch64" },
      ],
    },
    {
      os: "windows",
      displayName: "Windows",
      requirement: "Windows 10 1809 or later",
      artifacts: [
        { kind: "zip", label: "Windows x86_64 (zip)", arch: "x86_64", osLabel: "windows-x86_64" },
      ],
    },
  ],
};

export function artifactFilename(
  artifact: Artifact,
  release: Releases = DEFAULT_RELEASES,
): string {
  return `${release.product}-${release.version}-${artifact.osLabel}.${artifact.kind}`;
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

import { describe, expect, it } from "vitest";
import {
  artifactFilename,
  checksumUrl,
  DEFAULT_RELEASES,
  detectOs,
  downloadUrl,
  githubReleaseUrl,
  productById,
  type Artifact,
  type Releases,
} from "./releases";

const sample: Artifact = {
  kind: "tar.gz",
  product: "insyncbee-db-service",
  label: "Linux x86_64 (tar.gz)",
  arch: "x86_64",
  osLabel: "linux-x86_64",
};

const allArtifacts = () =>
  DEFAULT_RELEASES.products.flatMap((p) =>
    p.platforms.flatMap((pl) => pl.artifacts),
  );

describe("artifactFilename", () => {
  it("composes <product>-<version>-<osLabel>.<kind>", () => {
    expect(artifactFilename(sample)).toBe(
      `insyncbee-db-service-${DEFAULT_RELEASES.version}-linux-x86_64.tar.gz`,
    );
  });

  it("uses the supplied release for version interpolation", () => {
    const release: Releases = { ...DEFAULT_RELEASES, version: "9.9.9" };
    expect(artifactFilename(sample, release)).toBe(
      "insyncbee-db-service-9.9.9-linux-x86_64.tar.gz",
    );
  });

  it("keeps the two products in separate filename namespaces", () => {
    const desktop: Artifact = { ...sample, product: "insyncbee-desktop", kind: "AppImage" };
    expect(artifactFilename(desktop)).toBe(
      `insyncbee-desktop-${DEFAULT_RELEASES.version}-linux-x86_64.AppImage`,
    );
  });

  it.each([
    ["linux-x86_64", "tar.gz"] as const,
    ["macos-aarch64", "tar.gz"] as const,
    ["windows-x86_64", "zip"] as const,
    ["linux-x86_64", "AppImage"] as const,
    ["linux-x86_64", "deb"] as const,
    ["linux-x86_64", "rpm"] as const,
  ])("matches the CI matrix label %s with extension .%s", (osLabel, kind) => {
    const a: Artifact = { osLabel, kind, product: "insyncbee-x", arch: "x86_64", label: "" };
    expect(artifactFilename(a)).toMatch(new RegExp(`-${osLabel}\\.${kind}$`));
  });
});

describe("downloadUrl", () => {
  it("returns a relative /releases/* path served by the portal nginx", () => {
    const u = downloadUrl(sample);
    expect(u.startsWith("/releases/")).toBe(true);
    expect(u.endsWith(".tar.gz")).toBe(true);
  });

  it("does NOT escape upward (no ..)", () => {
    for (const a of allArtifacts()) {
      expect(downloadUrl(a)).not.toContain("..");
    }
  });
});

describe("checksumUrl", () => {
  it("appends .sha256 to the download url", () => {
    expect(checksumUrl(sample)).toBe(`${downloadUrl(sample)}.sha256`);
  });
});

describe("githubReleaseUrl", () => {
  it("points at the v<version> release page on the configured repo", () => {
    expect(githubReleaseUrl()).toBe(
      `https://github.com/${DEFAULT_RELEASES.repo}/releases/tag/v${DEFAULT_RELEASES.version}`,
    );
  });
});

describe("productById", () => {
  it("finds both shipped products", () => {
    expect(productById("desktop")?.displayName).toBe("Desktop app");
    expect(productById("db-service")?.displayName).toBe("db-service");
  });
});

describe("detectOs", () => {
  const setNav = (platform: string, ua: string) => {
    Object.defineProperty(globalThis, "navigator", {
      configurable: true,
      value: { platform, userAgent: ua },
    });
  };

  it("returns 'mac' for macOS user agents", () => {
    setNav("MacIntel", "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)");
    expect(detectOs()).toBe("mac");
  });

  it("returns 'windows' for Windows user agents", () => {
    setNav("Win32", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)");
    expect(detectOs()).toBe("windows");
  });

  it("returns 'linux' for Linux user agents", () => {
    setNav("Linux x86_64", "Mozilla/5.0 (X11; Linux x86_64)");
    expect(detectOs()).toBe("linux");
  });

  it("returns null for unrecognised platforms", () => {
    setNav("Plan9", "Mozilla/5.0 (Plan9)");
    expect(detectOs()).toBeNull();
  });
});

describe("DEFAULT_RELEASES manifest invariants", () => {
  it("ships the desktop app and the db-service", () => {
    expect(DEFAULT_RELEASES.products.map((p) => p.id).sort()).toEqual([
      "db-service",
      "desktop",
    ]);
  });

  it("covers all three target platforms with the db-service", () => {
    const oss = productById("db-service")!.platforms.map((p) => p.os).sort();
    expect(oss).toEqual(["linux", "mac", "windows"]);
  });

  // The desktop bundles are Linux-only until macOS/Windows signing exists —
  // publishing an unsigned .app or .msi would be worse than not shipping one.
  it("ships desktop bundles for Linux only", () => {
    expect(productById("desktop")!.platforms.map((p) => p.os)).toEqual(["linux"]);
  });

  it("every artifact has an osLabel and product produced by the CI matrix", () => {
    const labels = new Set(["linux-x86_64", "macos-aarch64", "windows-x86_64"]);
    const products = new Set(["insyncbee-db-service", "insyncbee-desktop"]);
    for (const a of allArtifacts()) {
      expect(labels.has(a.osLabel)).toBe(true);
      expect(products.has(a.product)).toBe(true);
    }
  });

  it("version is a valid semver-ish string", () => {
    expect(DEFAULT_RELEASES.version).toMatch(/^\d+\.\d+\.\d+/);
  });
});

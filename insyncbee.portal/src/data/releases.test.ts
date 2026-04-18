import { describe, expect, it } from "vitest";
import {
  artifactFilename,
  checksumUrl,
  DEFAULT_RELEASES,
  detectOs,
  downloadUrl,
  githubReleaseUrl,
  type Artifact,
  type Releases,
} from "./releases";

const sample: Artifact = {
  kind: "tar.gz",
  label: "Linux x86_64 (tar.gz)",
  arch: "x86_64",
  osLabel: "linux-x86_64",
};

describe("artifactFilename", () => {
  it("composes <product>-<version>-<osLabel>.<kind>", () => {
    expect(artifactFilename(sample)).toBe(
      `${DEFAULT_RELEASES.product}-${DEFAULT_RELEASES.version}-linux-x86_64.tar.gz`,
    );
  });

  it("uses the supplied release for version interpolation", () => {
    const release: Releases = { ...DEFAULT_RELEASES, version: "9.9.9" };
    expect(artifactFilename(sample, release)).toBe(
      `${release.product}-9.9.9-linux-x86_64.tar.gz`,
    );
  });

  it.each([
    ["linux-x86_64", "tar.gz"] as const,
    ["macos-aarch64", "tar.gz"] as const,
    ["windows-x86_64", "zip"] as const,
  ])("matches the CI matrix label %s with extension .%s", (osLabel, kind) => {
    const a: Artifact = { osLabel, kind, arch: "x86_64", label: "" };
    const fn = artifactFilename(a);
    expect(fn).toMatch(new RegExp(`-${osLabel}\\.${kind}$`));
  });
});

describe("downloadUrl", () => {
  it("returns a relative /releases/* path served by the portal nginx", () => {
    const u = downloadUrl(sample);
    expect(u.startsWith("/releases/")).toBe(true);
    expect(u.endsWith(".tar.gz")).toBe(true);
  });

  it("does NOT escape upward (no ..)", () => {
    expect(downloadUrl(sample)).not.toContain("..");
  });
});

describe("checksumUrl", () => {
  it("appends .sha256 to the download url", () => {
    expect(checksumUrl(sample)).toBe(`${downloadUrl(sample)}.sha256`);
  });
});

describe("githubReleaseUrl", () => {
  it("points at the v<version> release page on the configured repo", () => {
    const u = githubReleaseUrl();
    expect(u).toBe(
      `https://github.com/${DEFAULT_RELEASES.repo}/releases/tag/v${DEFAULT_RELEASES.version}`,
    );
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
  it("covers all three target platforms", () => {
    const oss = DEFAULT_RELEASES.platforms.map((p) => p.os).sort();
    expect(oss).toEqual(["linux", "mac", "windows"]);
  });

  it("every artifact has a matching osLabel produced by the CI matrix", () => {
    const allowed = new Set(["linux-x86_64", "macos-aarch64", "windows-x86_64"]);
    for (const p of DEFAULT_RELEASES.platforms) {
      for (const a of p.artifacts) {
        expect(allowed.has(a.osLabel)).toBe(true);
      }
    }
  });

  it("version is a valid semver-ish string", () => {
    expect(DEFAULT_RELEASES.version).toMatch(/^\d+\.\d+\.\d+/);
  });
});

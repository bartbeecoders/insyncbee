import { useMemo } from "react";
import {
  DEFAULT_RELEASES,
  detectOs,
  downloadUrl,
  githubReleaseUrl,
  type OsKey,
  type PlatformRelease,
} from "../data/releases";

const OS_ICONS: Record<OsKey, React.ReactNode> = {
  linux: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 3c-2 0-3 2-3 4 0 1.5.5 2.5 1 3-1 1-2 3-2 5 0 3 2 6 4 6s4-3 4-6c0-2-1-4-2-5 .5-.5 1-1.5 1-3 0-2-1-4-3-4z" />
      <circle cx="10.5" cy="7.2" r="0.6" fill="currentColor" />
      <circle cx="13.5" cy="7.2" r="0.6" fill="currentColor" />
    </svg>
  ),
  mac: (
    <svg viewBox="0 0 24 24" fill="currentColor">
      <path d="M16.4 13.2c0-2.7 2.2-4 2.3-4-.2-1.8-2-3.3-4-3.3-1.8 0-2.5 1-3.8 1-1.4 0-2.5-1-4-1-2 0-4 1.5-4.2 3.5-.2 2.4.6 6 2.4 8 .9 1 1.8 2 3 2 1.1 0 1.6-.7 3-.7s1.8.7 3 .7c1.3 0 2.2-1 3-2 .7-.9 1-1.5 1.4-2.5-1.8-.7-3-2.2-3-3.7zM13.5 3.7c.8-.9 1.3-2 1.1-3.1-1 .1-2.1.6-2.8 1.5-.6.8-1.2 2-1 3 1.1 0 2-.5 2.7-1.4z" />
    </svg>
  ),
  windows: (
    <svg viewBox="0 0 24 24" fill="currentColor">
      <path d="M3 5.5l7.5-1v7.3H3V5.5zm0 7.7h7.5v7.3L3 19.5v-6.3zm8.5-8.8L21 3v9.5h-9.5V4.4zm0 8.8H21v9.5l-9.5-1.3v-8.2z" />
    </svg>
  ),
};

function PlatformCard({ platform }: { platform: PlatformRelease }) {
  return (
    <div className="download-card">
      <div className="os-icon" aria-hidden="true">{OS_ICONS[platform.os]}</div>
      <div style={{ flex: 1 }}>
        <h3>{platform.displayName}</h3>
        <div className="os-meta">{platform.requirement}</div>
        <div className="artifact-list">
          {platform.artifacts.map((a) => (
            <a
              key={a.osLabel}
              href={downloadUrl(a)}
              download
            >
              {a.label} <code>{a.arch}</code>
            </a>
          ))}
        </div>
      </div>
    </div>
  );
}

export default function Download() {
  const detected: OsKey | null = useMemo(() => detectOs(), []);
  const recommended = useMemo(
    () =>
      detected
        ? DEFAULT_RELEASES.platforms.find((p) => p.os === detected)
        : undefined,
    [detected],
  );

  const topArtifact = recommended?.artifacts[0];

  return (
    <section id="download">
      <div className="container">
        <span className="eyebrow">Download · db-service</span>
        <h2>Get the InSyncBee db-service.</h2>
        <p className="section-intro">
          The headless background sync service. Version{" "}
          <span className="text-accent">{DEFAULT_RELEASES.version}</span>{" "}
          · Released {DEFAULT_RELEASES.releasedAt} · Channel: {DEFAULT_RELEASES.channel}
        </p>

        {recommended && topArtifact && (
          <div className="download-recommended">
            <div className="rec-text">
              <h3>We detected <span className="text-accent">{recommended.displayName}</span></h3>
              <p>Recommended: {topArtifact.label} · {topArtifact.arch}</p>
            </div>
            <a
              className="btn btn-primary btn-lg"
              href={downloadUrl(topArtifact)}
              download
            >
              ↓ Download {DEFAULT_RELEASES.version}
            </a>
          </div>
        )}

        <div className="download-grid">
          {DEFAULT_RELEASES.platforms.map((p) => (
            <PlatformCard key={p.os} platform={p} />
          ))}
        </div>

        <p className="download-footnote">
          Each archive ships with a <code>.sha256</code> checksum next to it on
          the{" "}
          <a className="text-accent" href={githubReleaseUrl()}>
            v{DEFAULT_RELEASES.version} GitHub Release
          </a>
          . All builds are produced by GitHub Actions from{" "}
          <a className="text-accent" href={`https://github.com/${DEFAULT_RELEASES.repo}`}>
            source on GitHub
          </a>
          .
        </p>
      </div>
    </section>
  );
}

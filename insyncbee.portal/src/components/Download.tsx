import { useMemo } from "react";
import {
  DEFAULT_RELEASES,
  detectOs,
  downloadUrl,
  githubReleaseUrl,
  productById,
  type Artifact,
  type OsKey,
  type PlatformRelease,
  type ProductRelease,
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
        <h4>{platform.displayName}</h4>
        <div className="os-meta">{platform.requirement}</div>
        <div className="artifact-list">
          {platform.artifacts.map((a) => (
            <a key={`${a.product}-${a.osLabel}-${a.kind}`} href={downloadUrl(a)} download>
              {a.label} <code>{a.arch}</code>
            </a>
          ))}
        </div>
      </div>
    </div>
  );
}

function ProductBlock({ product }: { product: ProductRelease }) {
  return (
    <div className="product-block" id={`download-${product.id}`}>
      <div className="product-head">
        <h3>{product.displayName}</h3>
        <p className="product-tagline">{product.tagline}</p>
      </div>
      <div className="download-grid">
        {product.platforms.map((p) => (
          <PlatformCard key={p.os} platform={p} />
        ))}
      </div>
    </div>
  );
}

// What the big button offers: the desktop app for the visitor's OS when we
// build one for it, otherwise the db-service. Never nothing — an unrecognised
// OS still gets the Linux desktop build offered below.
function recommend(
  os: OsKey | null,
): { product: ProductRelease; platform: PlatformRelease; artifact: Artifact } | undefined {
  if (!os) return undefined;
  for (const id of ["desktop", "db-service"] as const) {
    const product = productById(id);
    const platform = product?.platforms.find((p) => p.os === os);
    const artifact = platform?.artifacts[0];
    if (product && platform && artifact) return { product, platform, artifact };
  }
  return undefined;
}

export default function Download() {
  const detected = useMemo(() => detectOs(), []);
  const rec = useMemo(() => recommend(detected), [detected]);

  return (
    <section id="download">
      <div className="container">
        <span className="eyebrow">Download</span>
        <h2>Get InSyncBee.</h2>
        <p className="section-intro">
          Version <span className="text-accent">{DEFAULT_RELEASES.version}</span>{" "}
          · Released {DEFAULT_RELEASES.releasedAt} · Channel:{" "}
          {DEFAULT_RELEASES.channel}
        </p>

        {rec && (
          <div className="download-recommended">
            <div className="rec-text">
              <h3>
                We detected{" "}
                <span className="text-accent">{rec.platform.displayName}</span>
              </h3>
              <p>
                Recommended: {rec.product.displayName} · {rec.artifact.label}
              </p>
            </div>
            <a
              className="btn btn-primary btn-lg"
              href={downloadUrl(rec.artifact)}
              download
            >
              ↓ Download {DEFAULT_RELEASES.version}
            </a>
          </div>
        )}

        {DEFAULT_RELEASES.products.map((p) => (
          <ProductBlock key={p.id} product={p} />
        ))}

        <p className="download-footnote">
          The desktop app is the one with the window and the tray icon; the
          db-service is the same sync engine with no UI, for servers and
          headless boxes. Each file ships with a <code>.sha256</code> checksum
          next to it on the{" "}
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

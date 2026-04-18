import { Honeycomb } from "../assets/Logo";

export default function Hero() {
  return (
    <section id="top" className="hero">
      <div className="honeycomb-bg" />
      <div className="container hero-grid">
        <div>
          <span className="eyebrow">Open-source · Rust-native</span>
          <h1>
            Fast Google Drive sync for{" "}
            <span className="hero-highlight">every desktop.</span>
          </h1>
          <p className="hero-tagline">
            InSyncBee syncs your Drive like it should: block-level deltas, real
            conflict resolution, zero data loss. Linux, macOS, Windows — one
            honest binary, no subscriptions.
          </p>
          <div className="hero-cta-row">
            <a className="btn btn-primary btn-lg" href="#download">
              ↓ Download InSyncBee
            </a>
            <a
              className="btn btn-lg"
              href="https://github.com/bartroelant/InSyncBee"
              target="_blank"
              rel="noreferrer"
            >
              View on GitHub
            </a>
          </div>
          <div className="hero-meta">
            <span>● MIT licensed</span>
            <span>● ~8 MB binary</span>
            <span>● No telemetry</span>
          </div>
        </div>

        <div className="hero-art" aria-hidden="true">
          <Honeycomb />
          <div className="hex-float hex-1">⚡ Delta</div>
          <div className="hex-float hex-2">☁️ Drive</div>
          <div className="hex-float hex-3">🔒 Safe</div>
        </div>
      </div>
    </section>
  );
}

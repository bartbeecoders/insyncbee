import { DEFAULT_RELEASES, githubReleaseUrl } from "../data/releases";

const HIGHLIGHTS = [
  {
    label: "Tray + autostart",
    body: "Lives in the tray, optional start-on-login, headless --tray flag.",
  },
  {
    label: "Per-pair encryption",
    body: "Client-side encryption with keys in the OS keyring.",
  },
  {
    label: "Folder delete propagation",
    body: "Deleting a folder locally now removes it from Drive instead of resurrecting it.",
  },
  {
    label: "Nested upload fix",
    body: "Parent IDs resolved at execute time so children land in the new folder.",
  },
];

export default function WhatsNew() {
  return (
    <section id="whats-new" className="whats-new">
      <div className="container">
        <div className="whats-new-head">
          <div>
            <span className="eyebrow">
              What's new · v{DEFAULT_RELEASES.version}
            </span>
            <h2>Released {DEFAULT_RELEASES.releasedAt}.</h2>
          </div>
          <a
            className="btn"
            href={githubReleaseUrl()}
            target="_blank"
            rel="noreferrer"
          >
            Full release notes →
          </a>
        </div>
        <div className="whats-new-grid">
          {HIGHLIGHTS.map((h) => (
            <div key={h.label} className="whats-new-card">
              <div className="whats-new-label">{h.label}</div>
              <div className="whats-new-body">{h.body}</div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

import { DEFAULT_RELEASES, githubReleaseUrl } from "../data/releases";

const HIGHLIGHTS = [
  {
    label: "Conflicts converge",
    body: "Resolutions are recorded, so Keep Both stops making a new copy every cycle.",
  },
  {
    label: "Adopt existing folders",
    body: "Files already on both sides are matched by checksum instead of conflicting.",
  },
  {
    label: "Hidden files stay hidden",
    body: "Remote dot-files are ignored instead of being read as a local delete.",
  },
  {
    label: "Live e2e suite",
    body: "Scenario catalogue tested against a real Drive account before every release.",
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

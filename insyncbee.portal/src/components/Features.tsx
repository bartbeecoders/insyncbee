const FEATURES = [
  {
    title: "Block-level delta sync",
    body:
      "Only changed blocks travel — powered by Rust rsync + content-defined chunking. Edit a 2 GB file, upload a few KB.",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
        <path d="M3 12h4l3-8 4 16 3-8h4" />
      </svg>
    ),
  },
  {
    title: "Real conflict resolution",
    body:
      "Three-way compare, side-by-side preview, keep-both / keep-local / keep-remote — no silent overwrites, no surprises.",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
        <path d="M6 3v12" /><path d="M18 9v12" />
        <circle cx="6" cy="18" r="3" /><circle cx="18" cy="6" r="3" />
        <path d="M18 9a9 9 0 01-9 9" /><path d="M6 15a9 9 0 009-9" />
      </svg>
    ),
  },
  {
    title: "On-demand placeholders",
    body:
      "See your whole Drive in Finder/Explorer, fetch files only when you open them. Terabytes without the disk cost.",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
        <path d="M21 15V7a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2h9" />
        <polyline points="8 10 12 14 16 10" /><path d="M19 21v-6" /><circle cx="19" cy="18" r="1" />
      </svg>
    ),
  },
  {
    title: "Rock-solid safety",
    body:
      "Base-state journaling, atomic writes, cryptographic hashes. We never delete what we didn't verify first.",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
        <path d="M12 2l9 4v6c0 5-4 9-9 10-5-1-9-5-9-10V6l9-4z" />
        <path d="M9 12l2 2 4-4" />
      </svg>
    ),
  },
  {
    title: "Bandwidth control",
    body:
      "Upload and download caps per hour, schedule, or connection type. Stop Insync from eating your tethered laptop.",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="10" />
        <path d="M12 6v6l4 2" />
      </svg>
    ),
  },
  {
    title: "Google Docs, done right",
    body:
      "Keep native .gdoc / .gsheet shortcuts, or auto-convert to docx/xlsx on download. You choose, per sync pair.",
    icon: (
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
        <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z" />
        <polyline points="14 2 14 8 20 8" />
        <line x1="9" y1="13" x2="15" y2="13" /><line x1="9" y1="17" x2="13" y2="17" />
      </svg>
    ),
  },
];

export default function Features() {
  return (
    <section id="features">
      <div className="container">
        <span className="eyebrow">Features</span>
        <h2>Everything Insync should be.</h2>
        <p className="section-intro">
          Written in Rust from scratch, InSyncBee inherits all the sync primitives
          you expect — and adds the ones you've been missing.
        </p>

        <div className="features-grid">
          {FEATURES.map((f) => (
            <div key={f.title} className="feature-card">
              <div className="feature-icon">{f.icon}</div>
              <h3>{f.title}</h3>
              <p>{f.body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

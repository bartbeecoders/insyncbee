const STEPS = [
  {
    n: "1",
    title: "Sign in with Google",
    body:
      "Standard OAuth flow in your browser. Tokens live in your OS keyring — never on our servers, because we don't have any.",
  },
  {
    n: "2",
    title: "Pair a folder",
    body:
      "Pick a local directory and a Drive folder. Two-way, upload-only, or download-only — per pair. Add as many as you like.",
  },
  {
    n: "3",
    title: "Sync and forget",
    body:
      "File watcher + debounced diff + delta upload. Your changes appear on Drive in seconds, and theirs appear on disk just as fast.",
  },
];

export default function HowItWorks() {
  return (
    <section id="how">
      <div className="container">
        <span className="eyebrow">How it works</span>
        <h2>Up and running in under two minutes.</h2>
        <p className="section-intro">
          No server-side account. No daemon-in-the-cloud. Just your desktop
          talking to Drive — fast.
        </p>

        <div className="steps">
          {STEPS.map((s) => (
            <div key={s.n} className="step">
              <div className="step-num">{s.n}</div>
              <h3>{s.title}</h3>
              <p>{s.body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

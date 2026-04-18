const QS = [
  {
    q: "Is InSyncBee really free? What's the catch?",
    a: "It's MIT-licensed and free. We don't run servers or collect data — there's nothing to monetise. If you want to fund development, star the repo or contribute a PR.",
  },
  {
    q: "How is it different from Insync?",
    a: "Three things: (1) block-level delta sync so you don't re-upload a whole file for a small edit, (2) real three-way conflict detection with a GUI resolver, and (3) a Rust implementation that won't leak memory over a month of uptime.",
  },
  {
    q: "Where is my data stored?",
    a: "On your disk and in your Google Drive. Nowhere else. OAuth tokens live in the OS keyring (Keychain on macOS, Credential Manager on Windows, Secret Service / kwallet on Linux).",
  },
  {
    q: "Does it work with multiple Google accounts?",
    a: "Yes — add as many as you like. Each account can have multiple sync pairs (local folder ↔ Drive folder), each with its own direction and conflict policy.",
  },
  {
    q: "Is there a CLI?",
    a: "The daemon runs headless and has a small CLI for scripted setups. The desktop GUI and CLI share the same state DB, so you can mix them freely.",
  },
  {
    q: "What about Shared Drives / Team Drives?",
    a: "Supported. You'll see them alongside 'My Drive' in the folder picker when you add a sync pair.",
  },
  {
    q: "Can I contribute?",
    a: "Absolutely — the full codebase is on GitHub. Check the open issues tagged 'good first issue', or drop us a PR.",
  },
];

export default function FAQ() {
  return (
    <section id="faq">
      <div className="container">
        <span className="eyebrow">FAQ</span>
        <h2>Answers up front.</h2>
        <div className="faq-list">
          {QS.map((item) => (
            <details key={item.q} className="faq-item">
              <summary>{item.q}</summary>
              <p>{item.a}</p>
            </details>
          ))}
        </div>
      </div>
    </section>
  );
}

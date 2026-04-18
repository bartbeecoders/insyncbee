import { LogoMark } from "../assets/Logo";

export default function Nav() {
  return (
    <header className="nav">
      <div className="container nav-inner">
        <a href="#top" className="brand">
          <LogoMark className="brand-mark" size={30} />
          <span>InSyncBee</span>
        </a>
        <nav className="nav-links" aria-label="Primary">
          <a className="nav-link" href="#features">Features</a>
          <a className="nav-link" href="#how">How it works</a>
          <a className="nav-link" href="#compare">Compare</a>
          <a className="nav-link" href="#faq">FAQ</a>
          <a
            className="nav-link"
            href="https://github.com/bartroelant/InSyncBee"
            target="_blank"
            rel="noreferrer"
          >
            GitHub
          </a>
          <a className="btn btn-primary nav-cta" href="#download">
            Download
          </a>
        </nav>
      </div>
    </header>
  );
}

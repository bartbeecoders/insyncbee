import { LogoMark } from "../assets/Logo";

export default function Footer() {
  return (
    <footer className="footer">
      <div className="container footer-inner">
        <div className="brand">
          <LogoMark size={22} />
          <span>© {new Date().getFullYear()} InSyncBee</span>
        </div>
        <div className="footer-links">
          <a href="https://github.com/bartroelant/InSyncBee" target="_blank" rel="noreferrer">
            GitHub
          </a>
          <a href="https://github.com/bartroelant/InSyncBee/issues" target="_blank" rel="noreferrer">
            Issues
          </a>
          <a href="#faq">FAQ</a>
          <a href="https://github.com/bartroelant/InSyncBee/blob/main/LICENSE" target="_blank" rel="noreferrer">
            MIT License
          </a>
        </div>
      </div>
    </footer>
  );
}

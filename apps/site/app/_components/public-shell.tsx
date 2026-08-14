import type { ReactNode } from "react";

/**
 * The mark ships in two colourways rather than one recoloured file, and CSS
 * picks between them, so an explicit `data-theme` toggle works the same way it
 * does for the colour tokens. A `<picture>` element would only follow the
 * media query.
 */
function Mark() {
  return (
    <>
      <img className="site-mark site-mark--light" src="/brand/aex-icon-black.png" alt="" width={24} height={24} />
      <img className="site-mark site-mark--dark" src="/brand/aex-icon-white.png" alt="" width={24} height={24} />
    </>
  );
}

export function PublicShell({ children, loginHref }: Readonly<{ children: ReactNode; loginHref: string }>) {
  return (
    <>
      <a className="aex-skip" href="#main">Skip to content</a>
      <header className="site-header">
        <div className="aex-container site-header__inner">
          <a className="site-wordmark" href="/" aria-label="Aex home">
            <Mark />
            <span>aex</span>
          </a>
          <nav className="site-nav" aria-label="Primary">
            <a className="site-nav__link" href="/docs">Docs</a>
            <a className="site-nav__link site-nav__github" href="https://github.com/aexhq/aex">GitHub</a>
            <a className="site-nav__link" href={loginHref}>Log in</a>
          </nav>
        </div>
      </header>
      <main id="main" className="site-main">{children}</main>
      <footer className="site-footer">
        <div className="aex-container site-footer__inner">
          <p>Open source under Apache-2.0. <a href="https://github.com/aexhq/aex">GitHub</a></p>
          <p>For partnerships and enquiries, contact <a href="mailto:support@aex.dev">support@aex.dev</a>.</p>
        </div>
      </footer>
    </>
  );
}

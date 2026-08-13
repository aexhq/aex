import type { ReactNode } from "react";

export function PublicShell({ children, loginHref }: Readonly<{ children: ReactNode; loginHref: string }>) {
  return (
    <>
      <a className="aex-skip" href="#main">Skip to content</a>
      <header className="site-header">
        <div className="aex-container site-header__inner">
          <a className="site-wordmark" href="/" aria-label="AEX home">aex</a>
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
          <p>Open source under Apache-2.0.</p>
          <p><a href="mailto:support@aex.dev">support@aex.dev</a></p>
        </div>
      </footer>
    </>
  );
}

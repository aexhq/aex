import type { ReactNode } from "react";

import "../design/index.css";
import "./site.css";

export default function Layout({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <body>
        <a className="aex-skip" href="#main">
          Skip to content
        </a>
        <header className="site-header">
          <div className="aex-container site-header__inner">
            <a className="site-wordmark" href="/">
              AEX
            </a>
            <nav aria-label="Primary">
              <a className="site-nav-link" href="/docs">
                Docs
              </a>
            </nav>
          </div>
        </header>
        <main id="main">{children}</main>
        <footer className="site-footer">
          <div className="aex-container site-footer__inner">
            <p>
              Support: <a href="mailto:support@aex.dev">support@aex.dev</a>
            </p>
            <p>
              Open source under Apache-2.0 at <a href="https://github.com/aexhq/aex">github.com/aexhq/aex</a>
            </p>
          </div>
        </footer>
      </body>
    </html>
  );
}

import type { ReactElement } from "react";

import { loadMarketingPage } from "../_lib/marketing";
import { Inline } from "./inline";

export const marketingPage = loadMarketingPage();

export function LandingPage(): ReactElement {
  const { heading, body, links } = marketingPage;
  return (
    <article className="site-essay" aria-labelledby="landing-heading">
      <div className="aex-container site-essay__inner">
        <h1 id="landing-heading">{heading}</h1>
        <div className="site-essay__body">
          {body.map((paragraph, position) => (
            <p key={`paragraph-${position}`}><Inline paragraph={paragraph} /></p>
          ))}
        </div>
        <nav className="site-essay__links" aria-label="Get started">
          {links.map((link) => (
            <a href={link.href} key={link.href}>{link.label}</a>
          ))}
        </nav>
      </div>
    </article>
  );
}

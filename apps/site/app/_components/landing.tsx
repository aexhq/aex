import type { ReactElement } from "react";

import { loadMarketingPage } from "../_lib/marketing";
import { Inline } from "./inline";

export const marketingPage = loadMarketingPage();

export function LandingPage(): ReactElement {
  const { intro, links, features, note } = marketingPage;
  return (
    <div className="site-overview">
      <div className="aex-container site-overview__inner">
        <div className="site-overview__intro">
          {intro.map((paragraph, position) => (
            <p key={`intro-${position}`}><Inline paragraph={paragraph} /></p>
          ))}
        </div>
        <nav className="site-overview__links" aria-label="Get started">
          {links.map((link) => (
            <a href={link.href} key={link.href}>{link.label}</a>
          ))}
        </nav>
        <dl className="site-feature-list">
          {features.map((feature) => (
            <div className="site-feature" key={feature.title}>
              <dt>{feature.title}</dt>
              <dd>
                {feature.body.map((paragraph, position) => (
                  <p key={`${feature.title}-${position}`}><Inline paragraph={paragraph} /></p>
                ))}
              </dd>
            </div>
          ))}
        </dl>
        <p className="site-overview__note"><Inline paragraph={note} /></p>
      </div>
    </div>
  );
}

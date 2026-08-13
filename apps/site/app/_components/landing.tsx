import type { ReactElement } from "react";

import { loadMarketingPage, type Paragraph, type Section } from "../_lib/marketing";
import { Inline } from "./inline";

export const marketingPage = loadMarketingPage();

export function LandingPage(): ReactElement {
  const { hero, sections } = marketingPage;
  return (
    <>
      <section className="site-hero" aria-labelledby="hero-heading">
        <div className="aex-container site-hero__inner">
          <p className="site-kicker"># agent infrastructure</p>
          <h1 id="hero-heading">{hero.heading}</h1>
          {hero.body.map((paragraph, position) => (
            <p className="site-hero__lede" key={`lede-${position}`}><Inline paragraph={paragraph} /></p>
          ))}
          <div className="site-hero__actions">
            {hero.links.map((link, position) => (
              <a
                className={`aex-button ${position === 0 ? "aex-button--primary" : "aex-button--secondary"}`}
                href={link.href}
                key={link.href}
              >
                {link.label}
              </a>
            ))}
          </div>
          <p className="site-note"><Inline paragraph={hero.note} /></p>
        </div>
      </section>
      {sections.map((section) => <SectionBlock key={section.slug} section={section} />)}
    </>
  );
}

function SectionBlock({ section }: Readonly<{ section: Section }>): ReactElement {
  return (
    <section className="site-section" id={section.slug} aria-labelledby={`${section.slug}-heading`}>
      <div className="aex-container site-section__layout">
        <div className="site-section__head">
          <p className="site-kicker">## {section.slug}</p>
          <h2 id={`${section.slug}-heading`}>{section.title}</h2>
          {section.lede.map((paragraph, position) => (
            <p className="site-section__lede" key={`lede-${position}`}><Inline paragraph={paragraph} /></p>
          ))}
        </div>
        <div className="site-entry-list">
          {section.entries.map((entry) => (
            <article className="site-entry" key={entry.slug}>
              <h3>{entry.title}</h3>
              <Body paragraphs={entry.body} />
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}

function Body({ paragraphs }: Readonly<{ paragraphs: readonly Paragraph[] }>): ReactElement {
  return <>{paragraphs.map((paragraph, position) => (
    <p className="site-entry__body" key={`body-${position}`}><Inline paragraph={paragraph} /></p>
  ))}</>;
}

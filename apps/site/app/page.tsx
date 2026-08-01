import type { Metadata } from "next";
import type { ReactElement } from "react";

import { Inline } from "./_components/inline.js";
import { loadMarketingPage, type Paragraph, type Section } from "./_lib/marketing.js";

const page = loadMarketingPage();

export const metadata: Metadata = {
  title: page.title,
  description: page.description,
};

/**
 * The landing page. Every string below comes from
 * `content/marketing/index.mdx`; this file owns layout only.
 *
 * The first section renders as a card grid and the rest render as a ruled
 * definition list. That is a presentation choice keyed on position, so adding a
 * claim never requires touching this file.
 */
export default function Home(): ReactElement {
  const { hero, sections } = page;
  return (
    <>
      <section className="hero" aria-labelledby="hero-heading">
        <div className="aex-container hero__inner">
          <h1 id="hero-heading">{hero.heading}</h1>
          {hero.body.map((paragraph, position) => (
            <p className="hero__lede" key={`lede-${position}`}>
              <Inline paragraph={paragraph} />
            </p>
          ))}
          <div className="hero__actions">
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
          <p className="aex-note hero__note">
            <Inline paragraph={hero.note} />
          </p>
        </div>
      </section>
      {sections.map((section, position) => (
        <SectionBlock key={section.slug} section={section} variant={position === 0 ? "cards" : "list"} />
      ))}
    </>
  );
}

function SectionBlock({
  section,
  variant,
}: Readonly<{ section: Section; variant: "cards" | "list" }>): ReactElement {
  const headingId = `${section.slug}-heading`;
  return (
    <section className="aex-section" id={section.slug} aria-labelledby={headingId}>
      <div className="aex-container">
        <div className="section__head">
          <h2 id={headingId}>{section.title}</h2>
          {section.lede.map((paragraph, position) => (
            <p className="section__lede" key={`lede-${position}`}>
              <Inline paragraph={paragraph} />
            </p>
          ))}
        </div>
        <ul className={variant === "cards" ? "grid grid--cards" : "grid grid--list"}>
          {section.entries.map((entry) => (
            <li className={variant === "cards" ? "aex-card entry" : "entry entry--ruled"} key={entry.slug}>
              <h3>{entry.title}</h3>
              <Body paragraphs={entry.body} />
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}

function Body({ paragraphs }: Readonly<{ paragraphs: readonly Paragraph[] }>): ReactElement {
  return (
    <>
      {paragraphs.map((paragraph, position) => (
        <p className="entry__body" key={`body-${position}`}>
          <Inline paragraph={paragraph} />
        </p>
      ))}
    </>
  );
}

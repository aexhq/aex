import type { ReactElement } from "react";

import { loadDocsPage, type DocsBlock, type DocsHeading } from "../_lib/docs";
import { Inline } from "./inline";

export const docsPage = loadDocsPage();

export function DocsPage(): ReactElement {
  const sections = docsPage.blocks.filter(
    (block): block is DocsHeading => block.kind === "heading" && block.level === 2,
  );
  return (
    <section className="site-docs">
      <div className="aex-container site-docs__layout">
        <aside className="site-docs__nav" aria-label="On this page">
          <p className="site-kicker">On this page</p>
          <nav>{sections.map((section) => <a href={`#${section.slug}`} key={section.slug}>{section.text}</a>)}</nav>
        </aside>
        <article className="site-prose">
          {docsPage.blocks.map((block, position) => <DocsBlockView block={block} key={`${block.kind}-${position}`} />)}
        </article>
      </div>
    </section>
  );
}

function DocsBlockView({ block }: Readonly<{ block: DocsBlock }>): ReactElement {
  if (block.kind === "paragraph") return <p><Inline paragraph={block.body} /></p>;
  if (block.kind === "code") {
    return <pre aria-label={`${block.language || "plain text"} example`}><code>{block.value}</code></pre>;
  }
  if (block.level === 1) return <h1 id={block.slug}>{block.text}</h1>;
  if (block.level === 2) return <h2 id={block.slug}>{block.text}</h2>;
  return <h3 id={block.slug}>{block.text}</h3>;
}

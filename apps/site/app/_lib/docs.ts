import { readFileSync } from "node:fs";

import { parseInline, siteContentPath, slugify, type Paragraph } from "./marketing";

export interface DocsHeading {
  readonly kind: "heading";
  readonly level: 1 | 2 | 3;
  readonly text: string;
  readonly slug: string;
}

export interface DocsParagraph {
  readonly kind: "paragraph";
  readonly body: Paragraph;
}

export interface DocsCode {
  readonly kind: "code";
  readonly language: string;
  readonly value: string;
}

export type DocsBlock = DocsHeading | DocsParagraph | DocsCode;

export interface DocsPage {
  readonly title: string;
  readonly description: string;
  readonly blocks: readonly DocsBlock[];
}

export function loadDocsPage(path: string = siteContentPath("docs/index.mdx")): DocsPage {
  return parseDocsPage(readFileSync(path, "utf8"));
}

/**
 * Parse the deliberately small subset used by the public guide. General MDX
 * would permit executable components, while this page only needs headings,
 * prose, inline code, and fenced examples that can stay static and auditable.
 */
export function parseDocsPage(source: string): DocsPage {
  const normalized = source.replace(/\r\n/g, "\n");
  const { title, description, body } = splitFrontmatter(normalized);
  const lines = body.split("\n");
  const blocks: DocsBlock[] = [];

  for (let index = 0; index < lines.length;) {
    const line = lines[index] as string;
    if (line.trim().length === 0) {
      index += 1;
      continue;
    }

    if (line.startsWith("```")) {
      const language = line.slice(3).trim();
      const code: string[] = [];
      index += 1;
      while (index < lines.length && lines[index] !== "```") {
        code.push(lines[index] as string);
        index += 1;
      }
      if (index === lines.length) throw new Error("documentation has an unclosed code fence");
      blocks.push({ kind: "code", language, value: code.join("\n") });
      index += 1;
      continue;
    }

    const heading = /^(#{1,3})\s+(.+)$/.exec(line);
    if (heading !== null) {
      const text = (heading[2] as string).trim();
      blocks.push({
        kind: "heading",
        level: (heading[1] as string).length as 1 | 2 | 3,
        text,
        slug: slugify(text),
      });
      index += 1;
      continue;
    }

    const paragraph: string[] = [line.trim()];
    index += 1;
    while (index < lines.length) {
      const next = lines[index] as string;
      if (next.trim().length === 0 || next.startsWith("```") || /^#{1,3}\s+/.test(next)) break;
      paragraph.push(next.trim());
      index += 1;
    }
    blocks.push({ kind: "paragraph", body: parseInline(paragraph.join(" ")) });
  }

  const first = blocks[0];
  if (first?.kind !== "heading" || first.level !== 1) {
    throw new Error("documentation must open with a single `# ` heading");
  }
  if (blocks.filter((block) => block.kind === "heading" && block.level === 1).length !== 1) {
    throw new Error("documentation must contain exactly one `# ` heading");
  }
  return { title, description, blocks };
}

function splitFrontmatter(source: string): { title: string; description: string; body: string } {
  if (!source.startsWith("---\n")) throw new Error("documentation must open with `---` frontmatter");
  const end = source.indexOf("\n---\n", 3);
  if (end === -1) throw new Error("documentation frontmatter is not closed by `---`");
  const fields = new Map<string, string>();
  for (const line of source.slice(4, end).split("\n")) {
    const separator = line.indexOf(":");
    if (separator === -1) throw new Error(`frontmatter line is not "key: value": ${line}`);
    fields.set(line.slice(0, separator).trim(), line.slice(separator + 1).trim());
  }
  const title = fields.get("title");
  const description = fields.get("description");
  if (!title) throw new Error("frontmatter is missing \"title\"");
  if (!description) throw new Error("frontmatter is missing \"description\"");
  return { title, description, body: source.slice(end + 5) };
}

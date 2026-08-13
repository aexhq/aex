/**
 * The marketing content reader.
 *
 * `content/marketing/index.mdx` is the single file a contributor edits to change
 * a claim on the landing page. This module turns it into typed data; the
 * components under `app/_components` decide how it looks and never carry copy of
 * their own.
 *
 * The accepted shape is deliberately small, so the parser can be exact rather
 * than a general Markdown engine:
 *
 *   ---                       YAML-ish frontmatter, `key: value` only
 *   title: ...
 *   description: ...
 *   ---
 *   # Heading                 the hero heading, exactly one
 *   Paragraph                 hero body, one or more
 *   - [Label](/href)          hero links, exactly one list
 *   > Note                    hero note, exactly one blockquote
 *   ## Section                one or more sections
 *   Paragraph                 optional section lede
 *   ### Entry                 one or more entries per section
 *   Paragraph                 entry body, one or more
 *
 * Every deviation throws. A landing page that silently drops a claim because a
 * heading level was mistyped is worse than a build that fails.
 */
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

/** A run of plain text, or a run of `inline code`. */
export interface InlineSpan {
  readonly kind: "text" | "code";
  readonly value: string;
}

/** One rendered paragraph. */
export type Paragraph = readonly InlineSpan[];

export interface ContentLink {
  readonly label: string;
  readonly href: string;
}

export interface Entry {
  readonly slug: string;
  readonly title: string;
  readonly body: readonly Paragraph[];
}

export interface Section {
  readonly slug: string;
  readonly title: string;
  readonly lede: readonly Paragraph[];
  readonly entries: readonly Entry[];
}

export interface Hero {
  readonly heading: string;
  readonly body: readonly Paragraph[];
  readonly links: readonly ContentLink[];
  readonly note: Paragraph;
}

export interface MarketingPage {
  readonly title: string;
  readonly description: string;
  readonly hero: Hero;
  readonly sections: readonly Section[];
}

const LINK_ITEM = /^- \[([^\]]+)\]\(([^)]+)\)$/;

export function marketingSourcePath(root?: string): string {
  return siteContentPath("marketing/index.mdx", root);
}

/** Resolve canonical site prose while either Next workspace is being built. */
export function siteContentPath(relativePath: string, root?: string): string {
  const candidates = root === undefined
    ? [
        resolve(process.cwd(), "content", relativePath),
        resolve(process.cwd(), "apps", "site", "content", relativePath),
        resolve(process.cwd(), "..", "site", "content", relativePath),
      ]
    : [resolve(root, "content", relativePath)];
  const match = candidates.find((candidate) => existsSync(candidate));
  if (match === undefined) throw new Error(`site content is missing: ${relativePath}`);
  return match;
}

export function loadMarketingPage(path: string = marketingSourcePath()): MarketingPage {
  return parseMarketingPage(readFileSync(path, "utf8"));
}

export function parseMarketingPage(source: string): MarketingPage {
  const normalized = source.replace(/\r\n/g, "\n");
  const { frontmatter, body } = splitFrontmatter(normalized);
  const title = requireField(frontmatter, "title");
  const description = requireField(frontmatter, "description");

  const blocks = body
    .split(/\n{2,}/)
    .map((block) => block.trim())
    .filter((block) => block.length > 0);

  const [headingBlock, ...rest] = blocks;
  if (headingBlock === undefined || !headingBlock.startsWith("# ")) {
    throw new Error("marketing content must open with a single `# ` hero heading");
  }

  const heroBlocks: string[] = [];
  let index = 0;
  while (index < rest.length && !(rest[index] ?? "").startsWith("## ")) {
    heroBlocks.push(rest[index] as string);
    index += 1;
  }

  const hero = parseHero(headingBlock.slice(2).trim(), heroBlocks);
  const sections = parseSections(rest.slice(index));
  assertAnchorsResolve(hero.links, sections);

  return { title, description, hero, sections };
}

function parseHero(heading: string, blocks: readonly string[]): Hero {
  const body: Paragraph[] = [];
  let links: ContentLink[] | undefined;
  let note: Paragraph | undefined;

  for (const block of blocks) {
    if (block.startsWith("- ")) {
      if (links !== undefined) throw new Error("the hero accepts exactly one link list");
      links = block.split("\n").map(parseLinkItem);
      continue;
    }
    if (block.startsWith("> ")) {
      if (note !== undefined) throw new Error("the hero accepts exactly one note");
      note = parseInline(unwrapQuote(block));
      continue;
    }
    body.push(parseInline(joinLines(block)));
  }

  if (body.length === 0) throw new Error("the hero needs at least one paragraph");
  if (links === undefined || links.length === 0) throw new Error("the hero needs a link list");
  if (note === undefined) throw new Error("the hero needs a `> ` note");

  return { heading, body, links, note };
}

function parseSections(blocks: readonly string[]): Section[] {
  const sections: Section[] = [];
  let section: { title: string; lede: Paragraph[]; entries: Entry[] } | undefined;
  let entry: { title: string; body: Paragraph[] } | undefined;

  const closeEntry = (): void => {
    if (entry === undefined) return;
    if (section === undefined) throw new Error(`entry "${entry.title}" is outside any section`);
    if (entry.body.length === 0) throw new Error(`entry "${entry.title}" has no body`);
    section.entries.push({ slug: slugify(entry.title), title: entry.title, body: entry.body });
    entry = undefined;
  };

  const closeSection = (): void => {
    closeEntry();
    if (section === undefined) return;
    if (section.entries.length === 0) throw new Error(`section "${section.title}" has no entries`);
    sections.push({ slug: slugify(section.title), title: section.title, lede: section.lede, entries: section.entries });
    section = undefined;
  };

  for (const block of blocks) {
    if (block.startsWith("## ")) {
      closeSection();
      section = { title: block.slice(3).trim(), lede: [], entries: [] };
      continue;
    }
    if (block.startsWith("### ")) {
      closeEntry();
      entry = { title: block.slice(4).trim(), body: [] };
      continue;
    }
    const paragraph = parseInline(joinLines(block));
    if (entry !== undefined) {
      entry.body.push(paragraph);
      continue;
    }
    if (section === undefined) throw new Error("a paragraph appears before the first `## ` section");
    section.lede.push(paragraph);
  }
  closeSection();

  if (sections.length === 0) throw new Error("marketing content needs at least one `## ` section");
  return sections;
}

/** Splits a paragraph into plain and `code` runs. Unbalanced backticks throw. */
export function parseInline(text: string): Paragraph {
  const parts = text.split("`");
  if (parts.length % 2 === 0) throw new Error(`unbalanced backticks in: ${text}`);
  return parts
    .map((value, position) => ({ kind: position % 2 === 1 ? ("code" as const) : ("text" as const), value }))
    .filter((span) => span.value.length > 0);
}

export function slugify(title: string): string {
  const slug = title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  if (slug.length === 0) throw new Error(`title "${title}" produces an empty slug`);
  return slug;
}

function assertAnchorsResolve(links: readonly ContentLink[], sections: readonly Section[]): void {
  const slugs = new Set(sections.map((section) => section.slug));
  for (const link of links) {
    if (!link.href.startsWith("#")) continue;
    if (!slugs.has(link.href.slice(1))) {
      throw new Error(`hero link "${link.href}" does not match any section anchor`);
    }
  }
}

function parseLinkItem(line: string): ContentLink {
  const match = LINK_ITEM.exec(line.trim());
  if (match === null) throw new Error(`expected a "- [label](href)" list item, got: ${line}`);
  return { label: match[1] as string, href: match[2] as string };
}

function unwrapQuote(block: string): string {
  return joinLines(
    block
      .split("\n")
      .map((line) => {
        if (!line.startsWith(">")) throw new Error(`a note must prefix every line with ">", got: ${line}`);
        return line.replace(/^>\s?/, "");
      })
      .join("\n")
  );
}

function joinLines(block: string): string {
  return block
    .split("\n")
    .map((line) => line.trim())
    .join(" ");
}

function splitFrontmatter(source: string): { frontmatter: Map<string, string>; body: string } {
  if (!source.startsWith("---\n")) throw new Error("marketing content must open with `---` frontmatter");
  const end = source.indexOf("\n---\n", 3);
  if (end === -1) throw new Error("marketing frontmatter is not closed by `---`");
  const frontmatter = new Map<string, string>();
  for (const line of source.slice(4, end).split("\n")) {
    if (line.trim().length === 0) continue;
    const separator = line.indexOf(":");
    if (separator === -1) throw new Error(`frontmatter line is not "key: value": ${line}`);
    frontmatter.set(line.slice(0, separator).trim(), line.slice(separator + 1).trim());
  }
  return { frontmatter, body: source.slice(end + 5) };
}

function requireField(frontmatter: Map<string, string>, key: string): string {
  const value = frontmatter.get(key);
  if (value === undefined || value.length === 0) throw new Error(`frontmatter is missing "${key}"`);
  return value;
}

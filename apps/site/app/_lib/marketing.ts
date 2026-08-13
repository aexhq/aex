/**
 * The marketing content reader.
 *
 * `content/marketing/index.mdx` is the single file a contributor edits to change
 * the landing page. This module turns it into typed data; the components under
 * `app/_components` decide how it looks and never carry copy of their own.
 *
 * The accepted shape is deliberately small, so the parser can be exact rather
 * than a general Markdown engine:
 *
 *   ---                       YAML-ish frontmatter, `key: value` only
 *   title: ...
 *   description: ...
 *   ---
 *   # Heading                 the page heading, exactly one
 *   Paragraph                 page body, one or more
 *   - [Label](/href)          page links, exactly one list
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

export interface MarketingPage {
  readonly title: string;
  readonly description: string;
  readonly heading: string;
  readonly body: readonly Paragraph[];
  readonly links: readonly ContentLink[];
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

  const [headingBlock, ...contentBlocks] = blocks;
  if (headingBlock === undefined || !headingBlock.startsWith("# ")) {
    throw new Error("marketing content must open with a single `# ` heading");
  }

  const { body: pageBody, links } = parseBody(contentBlocks);
  return { title, description, heading: headingBlock.slice(2).trim(), body: pageBody, links };
}

function parseBody(blocks: readonly string[]): Pick<MarketingPage, "body" | "links"> {
  const body: Paragraph[] = [];
  let links: ContentLink[] | undefined;

  for (const block of blocks) {
    if (block.startsWith("- ")) {
      if (links !== undefined) throw new Error("the landing page accepts exactly one link list");
      links = block.split("\n").map(parseLinkItem);
      continue;
    }
    if (block.startsWith("#")) throw new Error("the landing page accepts only its opening heading");
    body.push(parseInline(joinLines(block)));
  }

  if (body.length === 0) throw new Error("the landing page needs at least one paragraph");
  if (links === undefined || links.length === 0) throw new Error("the landing page needs a link list");
  return { body, links };
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

function parseLinkItem(line: string): ContentLink {
  const match = LINK_ITEM.exec(line.trim());
  if (match === null) throw new Error(`expected a "- [label](href)" list item, got: ${line}`);
  return { label: match[1] as string, href: match[2] as string };
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

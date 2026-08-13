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
 *   Paragraph                 introduction, one or more
 *   - [Label](/href)          page links, exactly one list
 *   ## Feature               feature name, one or more
 *   Paragraph                feature explanation, one or more
 *   > Note                   status note, exactly one
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

export interface Feature {
  readonly title: string;
  readonly body: readonly Paragraph[];
}

export interface MarketingPage {
  readonly title: string;
  readonly description: string;
  readonly intro: readonly Paragraph[];
  readonly links: readonly ContentLink[];
  readonly features: readonly Feature[];
  readonly note: Paragraph;
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

  return { title, description, ...parseContent(blocks) };
}

function parseContent(blocks: readonly string[]): Pick<MarketingPage, "intro" | "links" | "features" | "note"> {
  const intro: Paragraph[] = [];
  let links: ContentLink[] | undefined;
  const features: Feature[] = [];
  let feature: { title: string; body: Paragraph[] } | undefined;
  let note: Paragraph | undefined;

  const closeFeature = (): void => {
    if (feature === undefined) return;
    if (feature.body.length === 0) throw new Error(`feature "${feature.title}" has no explanation`);
    features.push(feature);
    feature = undefined;
  };

  for (const block of blocks) {
    if (block.startsWith("# ")) throw new Error("the landing page does not use a top-level heading");
    if (block.startsWith("## ")) {
      closeFeature();
      feature = { title: block.slice(3).trim(), body: [] };
      continue;
    }
    if (block.startsWith("- ")) {
      if (feature !== undefined) throw new Error("the link list must appear before the features");
      if (links !== undefined) throw new Error("the landing page accepts exactly one link list");
      links = block.split("\n").map(parseLinkItem);
      continue;
    }
    if (block.startsWith("> ")) {
      if (note !== undefined) throw new Error("the landing page accepts exactly one status note");
      note = parseInline(unwrapQuote(block));
      continue;
    }
    if (block.startsWith("#")) throw new Error(`unsupported marketing heading: ${block}`);
    const paragraph = parseInline(joinLines(block));
    if (feature === undefined) intro.push(paragraph);
    else feature.body.push(paragraph);
  }
  closeFeature();

  if (intro.length === 0) throw new Error("the landing page needs an introduction");
  if (links === undefined || links.length === 0) throw new Error("the landing page needs a link list");
  if (features.length === 0) throw new Error("the landing page needs at least one feature");
  if (note === undefined) throw new Error("the landing page needs a status note");
  return { intro, links, features, note };
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

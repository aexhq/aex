import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { loadMarketingPage, parseInline, parseMarketingPage, slugify } from "../app/_lib/marketing.js";

const CONTENT_PATH = resolve(import.meta.dir, "../content/marketing/index.mdx");
const README_PATH = resolve(import.meta.dir, "../../../README.md");
const page = loadMarketingPage(CONTENT_PATH);

describe("the landing page content model", () => {
  test("parses the hero, its actions, and its status note", () => {
    expect(page.title.length).toBeGreaterThan(0);
    expect(page.description.length).toBeGreaterThan(0);
    expect(page.hero.heading).toBe("Infrastructure for agents that do real work");
    expect(page.hero.body.length).toBeGreaterThanOrEqual(1);
    expect(page.hero.links.map((link) => link.href)).toEqual(["/docs", "https://aex.dev/signin?next=/app"]);
    const status = page.hero.note.map((span) => span.value).join("");

    expect(status).toContain("active prelaunch development");
    expect(status).toContain("Expect breaking changes");
    expect(status).toContain("do not use it for production workloads yet");
  });

  test("keeps the repository entry point explicit about development status", () => {
    const readme = readFileSync(README_PATH, "utf8");

    expect(readme).toContain("active prelaunch development");
    expect(readme).toContain("not guaranteed to work");
    expect(readme).toContain("do not rely on AEX for production workloads yet");
  });

  test("keeps the landing page to two concise sections", () => {
    expect(page.sections.map((section) => section.slug)).toEqual(["what-you-get", "start-here"]);
    expect(page.sections.flatMap((section) => section.entries)).toHaveLength(6);
  });

  test("gives every entry a slug, a title, and a body", () => {
    const empty = page.sections.flatMap((section) =>
      section.entries.filter((entry) => entry.slug.length === 0 || entry.body.length === 0).map((entry) => entry.title)
    );

    expect(empty).toEqual([]);
  });

  test("never repeats an entry title across sections", () => {
    // The DRY rule the brief sets: a claim appears once, in one place.
    const titles = page.sections.flatMap((section) => section.entries.map((entry) => entry.title));

    expect(titles.length).toBe(new Set(titles).size);
  });

  test("resolves every in-page anchor to a real section", () => {
    const slugs = new Set(page.sections.map((section) => section.slug));
    const anchors = page.hero.links.filter((link) => link.href.startsWith("#")).map((link) => link.href.slice(1));

    expect(anchors.filter((anchor) => !slugs.has(anchor))).toEqual([]);
  });

  test("emits no empty code span", () => {
    const spans = [
      ...page.hero.body,
      page.hero.note,
      ...page.sections.flatMap((section) => [...section.lede, ...section.entries.flatMap((entry) => entry.body)])
    ].flat();

    expect(spans.filter((span) => span.value.length === 0)).toEqual([]);
  });

  test("publishes no rate-book value", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    expect(source).not.toMatch(/\$\s?\d/);
  });
});

describe("the content parser fails closed", () => {
  const frontmatter = "---\ntitle: T\ndescription: D\n---\n\n";
  const hero = "# Heading\n\nLede.\n\n- [Docs](/docs)\n\n> Note.\n\n";
  const section = "## Why AEX\n\n### Entry\n\nBody.\n";

  test("accepts the minimal well-formed document", () => {
    const parsed = parseMarketingPage(`${frontmatter}${hero}${section}`);

    expect(parsed.sections[0]?.entries[0]?.title).toBe("Entry");
  });

  test("rejects a missing frontmatter field", () => {
    expect(() => parseMarketingPage(`---\ntitle: T\n---\n\n${hero}${section}`)).toThrow(/description/);
  });

  test("rejects a document that does not open with a hero heading", () => {
    expect(() => parseMarketingPage(`${frontmatter}Body only.\n`)).toThrow(/hero heading/);
  });

  test("rejects a hero with no call to action", () => {
    expect(() => parseMarketingPage(`${frontmatter}# Heading\n\nLede.\n\n> Note.\n\n${section}`)).toThrow(/link list/);
  });

  test("rejects an entry with no body", () => {
    expect(() => parseMarketingPage(`${frontmatter}${hero}## Why AEX\n\n### Entry\n`)).toThrow(/has no body/);
  });

  test("rejects a section with no entries", () => {
    expect(() => parseMarketingPage(`${frontmatter}${hero}## Why AEX\n\nLede only.\n`)).toThrow(/has no entries/);
  });

  test("rejects an anchor that matches no section", () => {
    const broken = `${frontmatter}# Heading\n\nLede.\n\n- [Nowhere](#nowhere)\n\n> Note.\n\n${section}`;

    expect(() => parseMarketingPage(broken)).toThrow(/does not match any section anchor/);
  });

  test("rejects unbalanced inline code", () => {
    expect(() => parseInline("a `b")).toThrow(/unbalanced backticks/);
  });

  test("rejects a title that cannot produce an anchor", () => {
    expect(() => slugify("!!!")).toThrow(/empty slug/);
  });
});

/**
 * What this file gates, and what it deliberately does not.
 *
 * It gates the shape of the landing page: that the content parses, that every
 * feature has an explanation, that a malformed document fails the build rather
 * than silently dropping a claim, and the one publishing rule that is written
 * down elsewhere (no rates in public docs, see `references/rules.md`).
 *
 * It does not gate the wording. Asserting the exact feature titles, the phrases
 * in the README, or the copy in the shared shell turned every rewrite into a
 * red build and pushed the page towards internal vocabulary, because internal
 * vocabulary was what the assertions had frozen. Copy is reviewed by reading
 * it. Please do not pin sentences here again.
 */
import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { loadMarketingPage, parseInline, parseMarketingPage, slugify } from "../app/_lib/marketing.js";

const CONTENT_PATH = resolve(import.meta.dir, "../content/marketing/index.mdx");
const page = loadMarketingPage(CONTENT_PATH);

describe("the landing page content model", () => {
  test("parses an introduction, links, explained features, and a status note", () => {
    expect(page.title.length).toBeGreaterThan(0);
    expect(page.description.length).toBeGreaterThan(0);
    expect(page.intro.length).toBeGreaterThan(0);
    expect(page.features.length).toBeGreaterThan(0);
    expect(page.note.length).toBeGreaterThan(0);
    expect(page.features.filter((feature) => feature.body.length === 0)).toEqual([]);
    expect(page.links.filter((link) => link.label.length === 0 || link.href.length === 0)).toEqual([]);
  });

  test("emits no empty code span", () => {
    const spans = [...page.intro, page.note, ...page.features.flatMap((feature) => feature.body)].flat();

    expect(spans.filter((span) => span.value.length === 0)).toEqual([]);
  });

  test("uses no top-level headline or dash punctuation", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    expect(source).not.toMatch(/[—–]/);
    expect(source).not.toMatch(/^# /m);
  });

  test("publishes no rate-book value", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    expect(source).not.toMatch(/\$\s?\d/);
  });
});

describe("the content parser fails closed", () => {
  const frontmatter = "---\ntitle: T\ndescription: D\n---\n\n";
  const pageBody = "A short introduction.\n\n- [Docs](/docs)\n\n## Sessions\n\nRun them.\n\n> Prelaunch.\n";

  test("accepts the minimal well-formed document", () => {
    const parsed = parseMarketingPage(`${frontmatter}${pageBody}`);

    expect(parsed.intro).toHaveLength(1);
    expect(parsed.features[0]?.title).toBe("Sessions");
  });

  test("rejects a missing frontmatter field", () => {
    expect(() => parseMarketingPage(`---\ntitle: T\n---\n\n${pageBody}`)).toThrow(/description/);
  });

  test("rejects a top-level heading", () => {
    expect(() => parseMarketingPage(`${frontmatter}# Heading\n\n${pageBody}`)).toThrow(/top-level heading/);
  });

  test("rejects a page with no links", () => {
    const broken = `${frontmatter}A short introduction.\n\n## Sessions\n\nRun them.\n\n> Prelaunch.\n`;

    expect(() => parseMarketingPage(broken)).toThrow(/link list/);
  });

  test("rejects a feature with no explanation", () => {
    const broken = `${frontmatter}A short introduction.\n\n- [Docs](/docs)\n\n## Sessions\n\n> Prelaunch.\n`;

    expect(() => parseMarketingPage(broken)).toThrow(/has no explanation/);
  });

  test("rejects a page with no features", () => {
    const broken = `${frontmatter}A short introduction.\n\n- [Docs](/docs)\n\n> Prelaunch.\n`;

    expect(() => parseMarketingPage(broken)).toThrow(/at least one feature/);
  });

  test("rejects a page with no status note", () => {
    const broken = `${frontmatter}A short introduction.\n\n- [Docs](/docs)\n\n## Sessions\n\nRun them.\n`;

    expect(() => parseMarketingPage(broken)).toThrow(/status note/);
  });

  test("rejects unbalanced inline code", () => {
    expect(() => parseInline("a `b")).toThrow(/unbalanced backticks/);
  });

  test("rejects a title that cannot produce an anchor", () => {
    expect(() => slugify("!!!")).toThrow(/empty slug/);
  });
});

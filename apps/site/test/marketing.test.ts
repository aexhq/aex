import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { loadMarketingPage, parseInline, parseMarketingPage, slugify } from "../app/_lib/marketing.js";

const CONTENT_PATH = resolve(import.meta.dir, "../content/marketing/index.mdx");
const README_PATH = resolve(import.meta.dir, "../../../README.md");
const page = loadMarketingPage(CONTENT_PATH);

describe("the landing page content model", () => {
  test("parses a short essay and its links", () => {
    expect(page.title.length).toBeGreaterThan(0);
    expect(page.description.length).toBeGreaterThan(0);
    expect(page.heading).toBe("AEX");
    expect(page.body).toHaveLength(3);
    expect(page.links.map((link) => link.href)).toEqual(["/docs", "https://aex.dev/signin?next=/app"]);
    const status = page.body.at(-1)?.map((span) => span.value).join("");

    expect(status).toContain("prelaunch development");
    expect(status).toContain("breaking changes and interruptions");
    expect(status).toContain("Do not use it for production work yet");
  });

  test("keeps the repository entry point explicit about development status", () => {
    const readme = readFileSync(README_PATH, "utf8");

    expect(readme).toContain("active prelaunch development");
    expect(readme).toContain("not guaranteed to work");
    expect(readme).toContain("do not rely on AEX for production workloads yet");
  });

  test("emits no empty code span", () => {
    const spans = page.body.flat();

    expect(spans.filter((span) => span.value.length === 0)).toEqual([]);
  });

  test("uses no artificial headline or dash punctuation", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    expect(source).not.toContain("Infrastructure for agents that do real work");
    expect(source).not.toMatch(/[—–]/);
    expect(source).not.toMatch(/^## /m);
  });

  test("publishes no rate-book value", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    expect(source).not.toMatch(/\$\s?\d/);
  });
});

describe("the content parser fails closed", () => {
  const frontmatter = "---\ntitle: T\ndescription: D\n---\n\n";
  const pageBody = "# Heading\n\nA short paragraph.\n\n- [Docs](/docs)\n";

  test("accepts the minimal well-formed document", () => {
    const parsed = parseMarketingPage(`${frontmatter}${pageBody}`);

    expect(parsed.heading).toBe("Heading");
    expect(parsed.body).toHaveLength(1);
  });

  test("rejects a missing frontmatter field", () => {
    expect(() => parseMarketingPage(`---\ntitle: T\n---\n\n${pageBody}`)).toThrow(/description/);
  });

  test("rejects a document that does not open with a page heading", () => {
    expect(() => parseMarketingPage(`${frontmatter}Body only.\n`)).toThrow(/heading/);
  });

  test("rejects a page with no links", () => {
    expect(() => parseMarketingPage(`${frontmatter}# Heading\n\nA short paragraph.\n`)).toThrow(/link list/);
  });

  test("rejects a second heading", () => {
    const broken = `${frontmatter}# Heading\n\nA short paragraph.\n\n## Features\n\n- [Docs](/docs)\n`;

    expect(() => parseMarketingPage(broken)).toThrow(/only its opening heading/);
  });

  test("rejects unbalanced inline code", () => {
    expect(() => parseInline("a `b")).toThrow(/unbalanced backticks/);
  });

  test("rejects a title that cannot produce an anchor", () => {
    expect(() => slugify("!!!")).toThrow(/empty slug/);
  });
});

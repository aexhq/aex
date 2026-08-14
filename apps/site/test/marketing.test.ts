import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { loadMarketingPage, parseInline, parseMarketingPage, slugify } from "../app/_lib/marketing.js";

const CONTENT_PATH = resolve(import.meta.dir, "../content/marketing/index.mdx");
const README_PATH = resolve(import.meta.dir, "../../../README.md");
const PUBLIC_SHELL_PATH = resolve(import.meta.dir, "../app/_components/public-shell.tsx");
const LAYOUT_PATH = resolve(import.meta.dir, "../app/layout.tsx");
const page = loadMarketingPage(CONTENT_PATH);

describe("the landing page content model", () => {
  test("parses the positioning, features, and links", () => {
    expect(page.title.length).toBeGreaterThan(0);
    expect(page.description.length).toBeGreaterThan(0);
    expect(page.intro).toHaveLength(1);
    expect(page.features.map((feature) => feature.title)).toEqual([
      "Official models",
      "Durable sessions",
      "Latest-only files",
      "One sandbox",
      "Tools and MCP",
      "Parallel subagents",
      "Live and retained telemetry",
      "Prepaid billing",
    ]);
    expect(page.links.map((link) => link.href)).toEqual(["/docs", "https://aex.dev/signin?next=/app"]);
    const status = page.note.map((span) => span.value).join("");

    expect(status).toContain("active prelaunch development");
    expect(status).toContain("Expect breaking changes");
  });

  test("keeps the repository entry point explicit about development status", () => {
    const readme = readFileSync(README_PATH, "utf8");

    expect(readme).toContain("active prelaunch development");
    expect(readme).toContain("not guaranteed to work");
    expect(readme).toContain("do not rely on AEX for production workloads yet");
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

  test("states the implemented developer-facing capabilities", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    for (const provider of ["OpenAI", "Anthropic", "DeepSeek", "xAI", "Meta", "Moonshot AI", "Alibaba"]) {
      expect(source).toContain(provider);
    }
    expect(source).toContain("session-scoped provider key");
    expect(source).toContain("text-only");
    expect(source).toContain("latest-only");
    expect(source).toContain("`storage.persist`");
    expect(source).toContain("`mcp_call`");
    expect(source).toMatch(/12\s+child identities/);
    expect(source).toContain("depth 3");
    expect(source).toContain("committed reconciliation");
    expect(source).toContain("compressed download");
    expect(source).toContain("Prepaid billing");

    expect(source).not.toContain("800+ models");
    expect(source).not.toContain("Google");
    expect(source).not.toContain("OpenRouter");
    expect(source).not.toContain("Vercel AI Gateway");
    expect(source).not.toContain("AG-UI");
    expect(source).not.toContain("free during alpha");
  });

  test("keeps shared search and social metadata on the session-centered positioning", () => {
    const layout = readFileSync(LAYOUT_PATH, "utf8");

    expect(layout).toContain("AEX is a session-centered agent runtime");
    expect(layout).not.toContain("distributed agent runtime in the cloud");
  });

  test("links the open source repository and partnership contact", () => {
    const source = readFileSync(PUBLIC_SHELL_PATH, "utf8");

    expect(source).toContain('href="https://github.com/aexhq/aex"');
    expect(source).toContain("Open source under Apache-2.0");
    expect(source).toContain("For partnerships and enquiries, contact");
    expect(source).toContain('href="mailto:support@aex.dev"');
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

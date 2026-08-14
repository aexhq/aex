import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { loadDocsPage, parseDocsPage, type DocsHeading } from "../app/_lib/docs.js";

const CONTENT_PATH = resolve(import.meta.dir, "../content/docs/index.mdx");

describe("the documentation content model", () => {
  test("loads the concise public guide", () => {
    const page = loadDocsPage();
    const sections = page.blocks.filter(
      (block): block is DocsHeading => block.kind === "heading" && block.level === 2,
    );

    expect(page.title).toBe("Aex documentation");
    expect(page.blocks[0]).toMatchObject({ kind: "heading", level: 1, text: "Aex documentation" });
    expect(sections.map((block) => block.slug)).toEqual([
      "quickstart",
      "sessions",
      "files-and-mounts",
      "tools-mcp-and-structured-output",
      "streaming-and-telemetry",
      "credentials-and-billing",
    ]);
    expect(page.blocks.filter((block) => block.kind === "code")).toHaveLength(6);
  });

  test("documents the exact session-centered SDK instead of removed resources", () => {
    const source = readFileSync(CONTENT_PATH, "utf8");

    for (const current of [
      "providerApiKey",
      "workspaceFiles",
      "mcpServers",
      "responseFormat",
      "dashboardSession",
      "storage.persist",
      "aex account create",
      "aex account bootstrap",
      "aex api-key create",
      "Google browser-and-PKCE",
    ]) {
      expect(source).toContain(current);
    }
    for (const removed of ["providerCredentialId", "sessionSuspend", "session-files", "message attachment"]) {
      expect(source).not.toContain(removed);
    }
  });

  test("rejects prose before the first heading", () => {
    expect(() => parseDocsPage("---\ntitle: Docs\ndescription: Guide\n---\n\nStart here."))
      .toThrow(/open with a single `# `/);
  });

  test("rejects an unclosed code fence", () => {
    expect(() => parseDocsPage("---\ntitle: Docs\ndescription: Guide\n---\n\n# Docs\n\n```ts\nconst x = 1;"))
      .toThrow(/unclosed code fence/);
  });
});

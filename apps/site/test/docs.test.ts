import { describe, expect, test } from "bun:test";

import { loadDocsPage, parseDocsPage, type DocsHeading } from "../app/_lib/docs.js";

describe("the documentation content model", () => {
  test("loads the concise public guide", () => {
    const page = loadDocsPage();
    const sections = page.blocks.filter(
      (block): block is DocsHeading => block.kind === "heading" && block.level === 2,
    );

    expect(page.title).toBe("AEX documentation");
    expect(page.blocks[0]).toMatchObject({ kind: "heading", level: 1, text: "AEX documentation" });
    expect(sections.map((block) => block.slug)).toEqual(["quickstart", "sessions", "files-and-tools"]);
    expect(page.blocks.filter((block) => block.kind === "code")).toHaveLength(1);
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

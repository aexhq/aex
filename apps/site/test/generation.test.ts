import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

import { generateSiteViews } from "../generate/main.js";

test("generation is byte-identical and covers every generated operation", () => {
  const first = mkdtempSync(resolve(tmpdir(), "aex-site-first-"));
  const second = mkdtempSync(resolve(tmpdir(), "aex-site-second-"));
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (() => {
    throw new Error("static documentation generation attempted a network request");
  }) as typeof fetch;
  try {
    generateSiteViews(first);
    generateSiteViews(second);
  } finally {
    globalThis.fetch = originalFetch;
  }
  const names = readdirSync(first).sort();
  expect(names).toEqual(readdirSync(second).sort());
  for (const name of names) {
    expect(readFileSync(resolve(first, name))).toEqual(readFileSync(resolve(second, name)));
    expect(readFileSync(resolve(first, name), "utf8")).toContain("generatedBy");
  }
  const api = readFileSync(resolve(first, "api-reference.md"), "utf8");
  const routes = JSON.parse(
    readFileSync(resolve(import.meta.dir, "../../../api/generated/registries/routes.json"), "utf8"),
  ) as { routes: Array<{ operationId: string; deferredReason?: string }> };
  const operationIds = routes.routes.map(({ operationId }) => operationId);
  expect(operationIds.length).toBeGreaterThan(0);
  expect(new Set(operationIds).size).toBe(operationIds.length);
  for (const route of routes.routes) expect(api).toContain(`\`${route.operationId}\``);

  // The old floor mandated documenting operations that do not exist, with no way
  // to say so. Documenting them is still right — a reader who cannot find an
  // operation cannot tell "undocumented" from "nonexistent" — but the section
  // has to carry the fact, and the index has to list every one of them.
  const index = readFileSync(resolve(first, "not-yet-available.md"), "utf8");
  const deferred = routes.routes.filter((route) => route.deferredReason);
  for (const route of deferred) {
    const section = api.slice(api.indexOf(`## \`${route.operationId}\``));
    const body = section.slice(0, section.indexOf("\n## ") === -1 ? undefined : section.indexOf("\n## "));
    expect({ id: route.operationId, marked: body.includes("**Not yet available.**") })
      .toEqual({ id: route.operationId, marked: true });
    expect(index).toContain(`\`${route.operationId}\``);
    // No reason text is published anywhere.
    expect(api).not.toContain(route.deferredReason as string);
    expect(index).not.toContain(route.deferredReason as string);
  }
  for (const route of routes.routes.filter((candidate) => !candidate.deferredReason)) {
    expect(index).not.toContain(`\`${route.operationId}\``);
  }
}, 120_000);

test("the site is configured as a static export", async () => {
  const { default: config } = await import("../next.config.mjs");
  expect(config.output).toBe("export");
});

import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

import { generateSiteViews } from "../generate/main.js";

test("generation is byte-identical and covers every generated operation", () => {
  const first = mkdtempSync(resolve(tmpdir(), "aex-site-first-"));
  const second = mkdtempSync(resolve(tmpdir(), "aex-site-second-"));
  generateSiteViews(first);
  generateSiteViews(second);
  const names = readdirSync(first).sort();
  expect(names).toEqual(readdirSync(second).sort());
  for (const name of names) {
    expect(readFileSync(resolve(first, name))).toEqual(readFileSync(resolve(second, name)));
    expect(readFileSync(resolve(first, name), "utf8")).toContain("generatedBy");
  }
  const api = readFileSync(resolve(first, "api-reference.md"), "utf8");
  const routes = JSON.parse(
    readFileSync(resolve(import.meta.dir, "../../../api/generated/registries/routes.json"), "utf8"),
  ) as { routes: Array<{ operationId: string }> };
  const operationIds = routes.routes.map(({ operationId }) => operationId);
  expect(operationIds.length).toBeGreaterThan(0);
  expect(new Set(operationIds).size).toBe(operationIds.length);
  for (const route of routes.routes) expect(api).toContain(`\`${route.operationId}\``);
}, 120_000);

test("the site is a network-independent static export", () => {
  const config = readFileSync(resolve(import.meta.dir, "../next.config.mjs"), "utf8");
  expect(config).toContain('output: "export"');
  const generator = readFileSync(resolve(import.meta.dir, "../generate/main.ts"), "utf8");
  expect(generator).not.toContain("fetch(");
});

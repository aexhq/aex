import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(import.meta.dirname, "../..");
const canonicalPath = resolve(repoRoot, "packages/sdk/docs/telemetry.md");
const generatorPath = resolve(repoRoot, "scripts/docs/generate-all.mjs");

describe("external telemetry documentation ownership", () => {
  it("keeps telemetry prose in the canonical SDK docs", () => {
    expect(existsSync(canonicalPath), "packages/sdk/docs/telemetry.md must be the prose source").toBe(true);
    if (!existsSync(canonicalPath)) return;
    const docs = readFileSync(canonicalPath, "utf8");
    expect(docs).toMatch(/session\.otel\.traces\(\)/);
    expect(docs).toMatch(/session\.otel\.logs\(\)/);
    expect(docs).toMatch(/aex otel <session-id>/);
    expect(docs).toMatch(/OTLP\/HTTP JSON/i);
    expect(docs).toMatch(/gen_ai.*pre-1\.0/is);
    expect(docs).toMatch(/journal.*source of truth/is);
    expect(docs).toMatch(/Jaeger|collector/i);
  });

  it("routes telemetry through the docs generator instead of a hand-maintained site copy", () => {
    const generator = readFileSync(generatorPath, "utf8");
    expect(generator).toContain('["telemetry.md", "telemetry"]');
    expect(generator).toMatch(/guideSources/);
  });
});

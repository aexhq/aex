import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(import.meta.dirname, "../..");
const canonicalPath = resolve(repoRoot, "packages/sdk/docs/telemetry.md");
const generatorPath = resolve(repoRoot, "scripts/docs/generate-all.mjs");

describe("strict-v1 telemetry documentation ownership", () => {
  it("keeps telemetry prose and current SDK examples in the canonical SDK docs", () => {
    expect(existsSync(canonicalPath), "packages/sdk/docs/telemetry.md must be the prose source").toBe(true);
    if (!existsSync(canonicalPath)) return;
    const docs = readFileSync(canonicalPath, "utf8");
    expect(docs).toMatch(/session\.telemetry\.query\(/);
    expect(docs).toMatch(/session\.telemetry\.stream\(/);
    expect(docs).toMatch(/session\.telemetry\.export\(/);
    expect(docs).toMatch(/session\.telemetry\.exports\.download\(/);
    expect(docs).toMatch(/aex\.telemetry\.otlp\.(?:logs|traces|metrics)\(/);
    expect(docs).not.toMatch(/session\.otel\b|aex otel\b/);
  });

  it("routes telemetry through the docs generator instead of a hand-maintained site copy", () => {
    const generator = readFileSync(generatorPath, "utf8");
    expect(generator).toContain('["telemetry.md", "telemetry"]');
    expect(generator).toMatch(/guideSources/);
  });
});

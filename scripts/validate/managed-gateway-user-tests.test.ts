import { readdirSync, readFileSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(import.meta.dir, "..", "..");
const userTestsRoot = join(repoRoot, "apps", "user-tests");
const liveRoot = join(userTestsRoot, "test", "live");

function sourceFiles(root: string): readonly string[] {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = join(root, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return extname(entry.name) === ".ts" ? [path] : [];
  });
}

function namedSource(path: string): string {
  return `${relative(repoRoot, path)}\n${readFileSync(path, "utf8")}`;
}

describe("managed-gateway user-test policy", () => {
  it("uses creator/model slugs for live gateway models", () => {
    const legacyModel = /(?<![A-Za-z0-9_-]\/)(?:deepseek-v4-(?:flash|pro)|claude-haiku-4-5)/;
    for (const path of sourceFiles(userTestsRoot)) {
      expect(namedSource(path)).not.toMatch(legacyModel);
    }
  });

  it("does not require or forward retired customer provider credentials", () => {
    const retiredCredential =
      /DEEPSEEK_API_KEY|process\.env\.(?:PROVIDER_KEY|DEEPSEEK_KEY)|\bDEEPSEEK_KEY(?:_SUBMIT)?\s*:/;
    for (const path of sourceFiles(liveRoot)) {
      expect(namedSource(path)).not.toMatch(retiredCredential);
    }
  });

  it("keeps the declared dev smoke on the managed model surface", () => {
    const path = join(liveRoot, "v1-session.user.test.ts");
    const source = readFileSync(path, "utf8");
    expect(source).toContain('"anthropic/claude-haiku-4-5"');
    expect(source).not.toContain('provider: "anthropic"');
    expect(source).not.toMatch(/\bapiKeys\s*:/);
  });

  it("keeps the clean-installed CLI coverage free of removed provider flags", () => {
    const source = readFileSync(join(userTestsRoot, "test", "offline", "packed-v1.test.ts"), "utf8");
    expect(source).toContain('join(install.cliDir, "dist", "cli.mjs")');
    expect(source).not.toMatch(/--provider\b|--[a-z0-9-]+-api-key\b/);
  });
});

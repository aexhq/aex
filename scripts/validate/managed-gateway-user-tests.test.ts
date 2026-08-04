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
});

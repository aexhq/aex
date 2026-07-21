import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const sourceRoot = fileURLToPath(new URL("../src/", import.meta.url));
const owner = readFileSync(`${sourceRoot}sdk-secrets.ts`, "utf8");
const consumers = [
  "session-custody.ts",
  "session-retention.ts",
  "side-effect-audit.ts"
] as const;

describe("public-safe scanner ownership", () => {
  it("keeps the scanner, shared pattern corpus, entropy rules, and digest grammar in one module", () => {
    expect(owner).toContain("PUBLIC_SAFE_SECRET_PATTERNS");
    expect(owner).toContain("scanPublicSafePayload");
    expect(owner).toContain("isPlatformContentDigest");
    expect(owner).toContain("looksHighEntropySecret");

    for (const file of consumers) {
      const source = readFileSync(`${sourceRoot}${file}`, "utf8");
      expect(source, file).not.toContain("forbiddenStringPatterns");
      expect(source, file).not.toMatch(/function\s+visit(?:Custody|Retention|Audit)Value/);
      expect(source, file).not.toMatch(/CONTENT_(?:HASH|DIGEST)/);
      expect(source, file).not.toMatch(/function\s+(?:looksHighEntropySecret|highEntropy\w*|shannonEntropyBits|charClassCount)/);
      expect(source, file).not.toContain("sk-[A-Za-z0-9_-");
    }
  });
});

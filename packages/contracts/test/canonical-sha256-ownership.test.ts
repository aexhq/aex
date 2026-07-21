import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const sourceDir = fileURLToPath(new URL("../src/", import.meta.url));

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return entry.isFile() && entry.name.endsWith(".ts") ? [path] : [];
  });
}

function source(path: string): string {
  return readFileSync(join(sourceDir, path), "utf8");
}

describe("canonical SHA-256 digest ownership", () => {
  it("has exactly one public-contracts source owner for the prefixed grammar", () => {
    const exactLiteral = "/^sha256:[0-9a-f]{64}$/";
    const occurrences = sourceFiles(sourceDir).flatMap((file) => {
      const count = readFileSync(file, "utf8").split(exactLiteral).length - 1;
      const path = relative(sourceDir, file).replaceAll("\\", "/");
      return Array.from({ length: count }, () => path);
    });

    expect(occurrences).toEqual(["canonical-sha256.ts"]);
    expect(source("canonical-sha256.ts")).not.toMatch(/^import\s/m);
  });

  it("routes only the three verified consumers through the neutral owner", () => {
    expect(source("session-config.ts")).toContain(
      "CANONICAL_SHA256_DIGEST_PATTERN as INLINE_CONTENT_HASH_PATTERN"
    );
    expect(source("workspace-resources.ts")).toContain(
      "CANONICAL_SHA256_DIGEST_PATTERN.test(value.contentHash)"
    );
    expect(source("operations.ts")).toContain(
      "CANONICAL_SHA256_DIGEST_PATTERN.test(value.capabilityHash)"
    );
    expect(source("operations.ts")).not.toContain("CAPABILITY_SHA256_PATTERN");
  });

  it("leaves the distinct bare session-file checksum grammar local", () => {
    expect(source("operations.ts")).toContain(
      "const SESSION_FILE_SHA256_PATTERN = /^[0-9a-f]{64}$/;"
    );
  });
});

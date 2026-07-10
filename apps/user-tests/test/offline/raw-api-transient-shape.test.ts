import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const liveDir = join(dirname(fileURLToPath(import.meta.url)), "..", "live");

const rawApiPreludeFiles = [
  "edge-admission-gates.user.test.ts",
  "edge-runtime-size.user.test.ts",
  "edge-type-contract.user.test.ts"
] as const;

describe("raw API live-test transient classifiers", () => {
  it.each(rawApiPreludeFiles)("recognizes Bun socket-open transients in %s", (fileName) => {
    const contents = readFileSync(join(liveDir, fileName), "utf8");

    expect(contents).toContain('"FailedToOpenSocket"');
    expect(contents).toContain("ConnectionRefused|FailedToOpenSocket|E[A-Z0-9_]+|UND_ERR_[A-Z0-9_]+");
  });
});

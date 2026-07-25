import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";

const productionFiles = [
  "submission.ts",
  "session-config.ts",
  "post-hook.ts",
  "side-effect-audit.ts"
] as const;

function source(name: string): string {
  return readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
}

describe("allowed-key assertion ownership", () => {
  it("has one dependency-free private Object.keys assertion owner", () => {
    const owner = source("allowed-keys.ts");
    expect(owner.match(/for \(const key of Object\.keys\(/g)).toHaveLength(1);
    expect(owner).not.toMatch(/^import /m);
    expect(owner).not.toMatch(/requireRecord|parseObject|schema registry|new Set|Object\.freeze/);

    for (const file of productionFiles) {
      const text = source(file);
      expect(text, file).not.toMatch(/for \(const key of Object\.keys\(/);
      expect(text, file).not.toMatch(/assertSupportedNestedKeys/);
    }
  });

  it("keeps the helper off both supported barrels", () => {
    expect(source("index.ts")).not.toMatch(/allowed-keys/);
    expect(source("internal.ts")).not.toMatch(/allowed-keys/);
  });

  // The permitted runtime dependency set is asserted once, in
  // value-guards-ownership.test.ts, against `public-boundary-baseline.json`.
  // It used to be restated here as a literal, which meant two tests to update
  // for one decision — and both were stale for a while.

  // The remaining hand-rolled allow-list call sites are counted by the C1 parser
  // ratchet (`scripts/cicd/check-parser-ratchet.mjs`, run from root `lint`), which
  // fails when the count rises and reaches zero when the last family is ported.
  // A frozen count here would be a second statement of the same fact that also
  // fails on the way *down*.
});

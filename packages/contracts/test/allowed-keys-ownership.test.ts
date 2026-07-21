import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

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

  it("keeps the helper off both supported barrels and retains only fflate", () => {
    expect(source("index.ts")).not.toMatch(/allowed-keys/);
    expect(source("internal.ts")).not.toMatch(/allowed-keys/);
    const packageJson = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8")
    ) as { readonly dependencies?: Readonly<Record<string, string>> };
    expect(Object.keys(packageJson.dependencies ?? {})).toEqual(["fflate"]);
  });

  it("routes every static migrated boundary through typed ordered tuples", () => {
    const combined = productionFiles.map(source).join("\n");
    expect(combined.match(/defineAllowedKeys</g)).toHaveLength(29);
    expect(combined.match(/assertAllowedKeys\(/g)).toHaveLength(25);
  });
});

import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";

const consumers = ["submission.ts", "runner-event.ts", "operations.ts", "provider-fault.ts"] as const;

function source(name: string): string {
  return readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
}

describe("value guard ownership", () => {
  it("has one dependency-free private implementation owner", () => {
    const owner = source("value-guards.ts");
    expect(owner).not.toMatch(/^import /m);
    expect(owner.match(/function isRecord\(/g)).toHaveLength(1);
    expect(owner.match(/function isJsonValue\(/g)).toHaveLength(1);
    expect(owner.match(/function isJsonRecord\(/g)).toHaveLength(1);
    expect(owner.match(/function isStringLiteral</g)).toHaveLength(1);
    expect(owner).not.toMatch(/throw\s+new|\bclass\s+\w|Object\.freeze|new Set/);

    for (const file of consumers) {
      const text = source(file);
      expect(text, file).not.toMatch(/function isRecord\(/);
      expect(text, file).not.toMatch(/function isJsonValue\(/);
      expect(text, file).not.toMatch(/function isJsonRecord\(/);
      expect(text, file).toMatch(/from "\.\/value-guards\.js"/);
    }
  });

  it("routes domain enum checks through the neutral literal predicate", () => {
    const submission = source("submission.ts");
    const runner = source("runner-event.ts");
    const operations = source("operations.ts");

    expect(submission).toMatch(/function optionalEnum[\s\S]*?isStringLiteral\(input, allowed\)/);
    expect(runner).toMatch(/isStringLiteral\(evt\.kind, RUNNER_EVENT_KINDS\)/);
    expect(operations).toMatch(/function assertOneOf[\s\S]*?isStringLiteral\(value, allowed\)/);
  });

  it("keeps the runtime helpers off supported barrels and dependencies unchanged", () => {
    expect(source("index.ts")).not.toMatch(/value-guards/);
    expect(source("internal.ts")).not.toMatch(/value-guards/);
    const packageJson = JSON.parse(
      readFileSync(new URL("../package.json", import.meta.url), "utf8")
    ) as { readonly dependencies?: Readonly<Record<string, string>> };

    const boundary = JSON.parse(
      readFileSync(new URL("../../../scripts/cicd/public-boundary-baseline.json", import.meta.url), "utf8")
    ) as {
      readonly contractsPackage?: { readonly allowedRuntimeDependencies?: readonly string[] };
      readonly contractsInline?: {
        readonly allowedModuleFiles?: readonly string[];
        readonly privateModuleMaxBytes?: Readonly<Record<string, number>>;
      };
    };
    // Asserted AGAINST the boundary baseline rather than against a literal list.
    // The permitted dependency set is a fact this package, the SDK manifest and
    // the platform snapshot already state; a fourth copy here would be one more
    // thing to update in lockstep and one more place to forget.
    expect(Object.keys(packageJson.dependencies ?? {}).sort()).toEqual(
      [...(boundary.contractsPackage?.allowedRuntimeDependencies ?? [])].sort()
    );
    expect(boundary.contractsInline?.allowedModuleFiles).toContain("./value-guards.js");
    expect(boundary.contractsInline?.privateModuleMaxBytes?.["value-guards.js"]).toBe(2048);
  });
});

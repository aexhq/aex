import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import { SDK_VERSION } from "../../src/version.js";

describe("SDK_VERSION constant", () => {
  it("matches packages/sdk/package.json", () => {
    // The constant ships in the published bundle; the package.json
    // controls what the package registry advertises. If these drift, the SDK
    // reports a version that does not exist in the registry, which makes triage harder
    // than necessary.
    const packageJsonPath = resolve(fileURLToPath(import.meta.url), "../../../package.json");
    const pkg = JSON.parse(readFileSync(packageJsonPath, "utf8")) as { version: string };
    expect(SDK_VERSION).toBe(pkg.version);
  });
});

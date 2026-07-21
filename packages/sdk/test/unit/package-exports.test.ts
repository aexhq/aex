import { describe, expect, it } from "vitest";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..", "..");
const PRE_EXTRACTION_ROOT_JS_BYTES = 209_283;
const ROOT_JS_SPLIT_ALLOWANCE = Math.max(Math.ceil(PRE_EXTRACTION_ROOT_JS_BYTES * 0.02), 4_096);

interface PackageJson {
  readonly name?: string;
  readonly exports?: Record<string, unknown>;
}

function readJson(path: string): PackageJson {
  return JSON.parse(readFileSync(path, "utf8")) as PackageJson;
}

/**
 * Lock the agent-first invariant for the `aex` package surface:
 * exactly one user-visible import path. Subpath exports (`aex/proxy`,
 * `aex/core`, etc.) would create multiple places an agent has to
 * track when reading or editing a submission, which violates the
 * agent-first principle.
 */
describe("aex package: agent-first export surface", () => {
  it("declares exactly one entry in package.json#exports", () => {
    const pkg = readJson(resolve(sdkRoot, "package.json"));
    expect(pkg.name).toBe("@aexhq/sdk");
    const exportsField = pkg.exports ?? {};
    const keys = Object.keys(exportsField);
    expect(keys).toEqual(["."]);
  });

  it("emits private client leaves without publishing package subpaths", () => {
    for (const module of ["client-types", "event-projection", "session-validate", "submission-wire"]) {
      expect(existsSync(resolve(sdkRoot, "dist", `${module}.js`)), `${module}.js`).toBe(true);
      expect(existsSync(resolve(sdkRoot, "dist", `${module}.d.ts`)), `${module}.d.ts`).toBe(true);
    }
  });

  it("keeps aggregate emitted SDK runtime bytes within the split-module allowance", () => {
    const dist = resolve(sdkRoot, "dist");
    const rootJsBytes = readdirSync(dist)
      .filter((name) => name.endsWith(".js"))
      .reduce((total, name) => total + statSync(resolve(dist, name)).size, 0);
    expect(rootJsBytes).toBeLessThanOrEqual(PRE_EXTRACTION_ROOT_JS_BYTES + ROOT_JS_SPLIT_ALLOWANCE);
  });
});

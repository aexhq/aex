import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const sdkRoot = resolve(here, "..", "..");

interface PackageJson {
  readonly name?: string;
  readonly exports?: Record<string, unknown>;
}

function readJson(path: string): PackageJson {
  return JSON.parse(readFileSync(path, "utf8")) as PackageJson;
}

/**
 * Lock the agent-first invariant for the `antpath` package surface:
 * exactly one user-visible import path. Subpath exports (`antpath/proxy`,
 * `antpath/core`, etc.) would create multiple places an agent has to
 * track when reading or editing a submission, which violates the
 * agent-first principle.
 */
describe("antpath package: agent-first export surface", () => {
  it("declares exactly one entry in package.json#exports", () => {
    const pkg = readJson(resolve(sdkRoot, "package.json"));
    expect(pkg.name).toBe("antpath");
    const exportsField = pkg.exports ?? {};
    const keys = Object.keys(exportsField);
    expect(keys).toEqual(["."]);
  });
});

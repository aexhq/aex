/**
 * Scenario 1: install.test.ts
 *
 * Lock the published package's *shape*. A real user / AI agent who runs
 * `npm install @aexhq/sdk` should land in a tree that:
 *   - Has a sensible package.json (name, version, type, main, types,
 *     exports, bin).
 *   - Ships dist/cli.mjs with a Node shebang.
 *   - Ships dist/cli.mjs.sha256 that matches the actual cli.mjs content.
 *   - Has a platform executable entry point: POSIX execute bit or Windows shim.
 *
 * This scenario would have caught the 0.2.0 missing-bin regression.
 */
import { createHash } from "node:crypto";
import { existsSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, type InstallResult } from "../_fixtures/install.js";

describe("install shape", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  });

  afterAll(() => {
    install?.cleanup();
  });

  it("npm install produced node_modules/@aexhq/sdk/package.json with the canonical shape", () => {
    const pkg = install.aexPackageJson;
    expect(pkg.name).toBe("@aexhq/sdk");
    expect(pkg.version).toBe(install.resolvedVersion);
    expect(pkg.type).toBe("module");
    expect(pkg.main).toBe("./dist/index.js");
    expect(pkg.types).toBe("./dist/index.d.ts");
    // engines.node must require >=20 — the worker container assumes this.
    expect(pkg.engines).toEqual(expect.objectContaining({ node: expect.stringMatching(/>=\s*20/) }));
  });

  it("declares a single `aex` bin pointing at dist/cli.mjs", () => {
    const bin = install.aexPackageJson.bin;
    expect(bin).toBeDefined();
    expect(Object.keys(bin ?? {})).toEqual(["aex"]);
    expect(bin?.aex).toBe("./dist/cli.mjs");
  });

  it("exports only the root entry (single-surface invariant)", () => {
    const exports = install.aexPackageJson.exports as Record<string, unknown> | undefined;
    expect(exports).toBeDefined();
    expect(Object.keys(exports ?? {})).toEqual(["."]);
    const root = exports?.["."] as Record<string, unknown> | undefined;
    expect(root?.types).toBe("./dist/index.d.ts");
    expect(root?.import).toBe("./dist/index.js");
    // Critical: no `require` condition — the package is ESM-only.
    expect(root).not.toHaveProperty("require");
  });

  it("ships dist/cli.mjs with a Node shebang", () => {
    const cliPath = join(install.aexDir, "dist", "cli.mjs");
    const bytes = readFileSync(cliPath);
    expect(bytes.length).toBeGreaterThan(1024);
    // Shebang is the first line; CRLF tolerant.
    const head = bytes.toString("utf8", 0, 64);
    expect(head).toMatch(/^#!\/usr\/bin\/env node\r?\n/);
  });

  it("dist/cli.mjs.sha256 matches the actual cli.mjs content", () => {
    const cliPath = join(install.aexDir, "dist", "cli.mjs");
    const digestPath = join(install.aexDir, "dist", "cli.mjs.sha256");
    const cliBytes = readFileSync(cliPath);
    const digestText = readFileSync(digestPath, "utf8");
    // sidecar format: "<hex>  cli.mjs\n"
    const expectedHex = digestText.trim().split(/\s+/)[0];
    expect(expectedHex).toMatch(/^[0-9a-f]{64}$/);
    const actualHex = createHash("sha256").update(cliBytes).digest("hex");
    expect(actualHex).toBe(expectedHex);
  });

  it("dist/cli.mjs has a platform executable entry point", () => {
    const cliPath = join(install.aexDir, "dist", "cli.mjs");
    const executableEntryPoint =
      process.platform === "win32"
        ? (() => {
            const cmdShimPath = join(install.installDir, "node_modules", ".bin", "aex.cmd");
            const cmdShimExists = existsSync(cmdShimPath);
            const cmdShim = cmdShimExists ? readFileSync(cmdShimPath, "utf8").replace(/\\/g, "/") : "";
            return {
              kind: "windows" as const,
              cmdShimPath,
              cmdShimExists,
              cmdShimInvokesNode: /\bnode(?:\.exe)?\b/i.test(cmdShim),
              cmdShimTargetsCli: /@aexhq\/sdk\/dist\/cli\.mjs/.test(cmdShim),
            };
          })()
        : (() => {
            const mode = statSync(cliPath).mode & 0o777;
            return {
              kind: "posix" as const,
              ownerExecutable: (mode & 0o100) === 0o100,
            };
          })();

    expect(executableEntryPoint).toMatchObject(
      process.platform === "win32"
        ? {
            kind: "windows",
            cmdShimExists: true,
            cmdShimInvokesNode: true,
            cmdShimTargetsCli: true,
          }
        : {
            kind: "posix",
            ownerExecutable: true,
          }
    );
  });

  it("declares no @aexhq/* runtime dependencies (single-tarball invariant)", () => {
    // Workspace-internal build packages like @aexhq/contracts and @aexhq/cli are
    // NEVER published to npm. If any of them leak into the published
    // package.json under `dependencies` or `peerDependencies`, then
    // `npm install @aexhq/sdk` will 404 at install time. The SDK build inlines
    // @aexhq/contracts into dist/_contracts/ and keeps it in devDependencies so
    // pnpm pack strips it. This guard locks that contract in.
    const pkg = install.aexPackageJson as {
      dependencies?: Record<string, string>;
      peerDependencies?: Record<string, string>;
      optionalDependencies?: Record<string, string>;
    };
    const sections = ["dependencies", "peerDependencies", "optionalDependencies"] as const;
    for (const section of sections) {
      const deps = pkg[section];
      if (!deps) continue;
      const leaked = Object.keys(deps).filter((name) => name.startsWith("@aexhq/"));
      expect(
        leaked,
        `${section} contains workspace-internal @aexhq/* packages: ${leaked.join(", ")}. ` +
          `Move them to devDependencies and bundle/inline their dist into the SDK build.`
      ).toEqual([]);
    }
  });

});

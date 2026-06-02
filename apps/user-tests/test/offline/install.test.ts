/**
 * Scenario 1: install.test.ts
 *
 * Lock the published package's *shape*. A real user / AI agent who runs
 * `npm install antpath` should land in a tree that:
 *   - Has a sensible package.json (name, version, type, main, types,
 *     exports, bin).
 *   - Ships dist/cli.mjs with a Node shebang.
 *   - Ships dist/cli.mjs.sha256 that matches the actual cli.mjs content.
 *   - On POSIX has the executable bit on cli.mjs.
 *
 * This scenario would have caught the 0.2.0 missing-bin regression.
 */
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, type InstallResult } from "../_fixtures/install.js";

describe("install shape", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
  });

  afterAll(() => {
    install?.cleanup();
  });

  it("npm install produced node_modules/antpath/package.json with the canonical shape", () => {
    const pkg = install.antpathPackageJson;
    expect(pkg.name).toBe("antpath");
    expect(pkg.version).toBe(install.resolvedVersion);
    expect(pkg.type).toBe("module");
    expect(pkg.main).toBe("./dist/index.js");
    expect(pkg.types).toBe("./dist/index.d.ts");
    // engines.node must require >=20 — the worker container assumes this.
    expect(pkg.engines).toEqual(expect.objectContaining({ node: expect.stringMatching(/>=\s*20/) }));
  });

  it("declares a single `antpath` bin pointing at dist/cli.mjs", () => {
    const bin = install.antpathPackageJson.bin;
    expect(bin).toBeDefined();
    expect(Object.keys(bin ?? {})).toEqual(["antpath"]);
    expect(bin?.antpath).toBe("./dist/cli.mjs");
  });

  it("exports only the root entry (single-surface invariant)", () => {
    const exports = install.antpathPackageJson.exports as Record<string, unknown> | undefined;
    expect(exports).toBeDefined();
    expect(Object.keys(exports ?? {})).toEqual(["."]);
    const root = exports?.["."] as Record<string, unknown> | undefined;
    expect(root?.types).toBe("./dist/index.d.ts");
    expect(root?.import).toBe("./dist/index.js");
    // Critical: no `require` condition — the package is ESM-only.
    expect(root).not.toHaveProperty("require");
  });

  it("ships dist/cli.mjs with a Node shebang", () => {
    const cliPath = join(install.antpathDir, "dist", "cli.mjs");
    const bytes = readFileSync(cliPath);
    expect(bytes.length).toBeGreaterThan(1024);
    // Shebang is the first line; CRLF tolerant.
    const head = bytes.toString("utf8", 0, 64);
    expect(head).toMatch(/^#!\/usr\/bin\/env node\r?\n/);
  });

  it("dist/cli.mjs.sha256 matches the actual cli.mjs content", () => {
    const cliPath = join(install.antpathDir, "dist", "cli.mjs");
    const digestPath = join(install.antpathDir, "dist", "cli.mjs.sha256");
    const cliBytes = readFileSync(cliPath);
    const digestText = readFileSync(digestPath, "utf8");
    // sidecar format: "<hex>  cli.mjs\n"
    const expectedHex = digestText.trim().split(/\s+/)[0];
    expect(expectedHex).toMatch(/^[0-9a-f]{64}$/);
    const actualHex = createHash("sha256").update(cliBytes).digest("hex");
    expect(actualHex).toBe(expectedHex);
  });

  it.skipIf(process.platform === "win32")("dist/cli.mjs is executable on POSIX", () => {
    const cliPath = join(install.antpathDir, "dist", "cli.mjs");
    const mode = statSync(cliPath).mode & 0o777;
    // We only insist on owner-execute (0o100). npm install can strip group/world
    // bits depending on umask.
    expect(mode & 0o100).toBe(0o100);
  });

  it("declares no @antpath/* runtime dependencies (single-tarball invariant)", () => {
    // Workspace-internal build packages like @antpath/contracts and @antpath/cli are
    // NEVER published to npm. If any of them leak into the published
    // package.json under `dependencies` or `peerDependencies`, then
    // `npm install antpath` will 404 at install time. The SDK build inlines
    // @antpath/contracts into dist/_contracts/ and keeps it in devDependencies so
    // pnpm pack strips it. This guard locks that contract in.
    const pkg = install.antpathPackageJson as {
      dependencies?: Record<string, string>;
      peerDependencies?: Record<string, string>;
      optionalDependencies?: Record<string, string>;
    };
    const sections = ["dependencies", "peerDependencies", "optionalDependencies"] as const;
    for (const section of sections) {
      const deps = pkg[section];
      if (!deps) continue;
      const leaked = Object.keys(deps).filter((name) => name.startsWith("@antpath/"));
      expect(
        leaked,
        `${section} contains workspace-internal @antpath/* packages: ${leaked.join(", ")}. ` +
          `Move them to devDependencies and bundle/inline their dist into the SDK build.`
      ).toEqual([]);
    }
  });

  it("does NOT ship the internal docs/ folder (P10 invariant)", () => {
    // The SDK historically duplicated parts of docs/* into
    // packages/sdk/docs/* — those are partial mirrors of the
    // canonical docs/ tree at the repo root and would drift over
    // time. P1/P10 of the Skill+MCP redesign deletes them from disk AND
    // strips them from the tarball via packages/sdk/package.json `files`.
    // This guard prevents any future re-introduction.
    const referencesDir = join(install.antpathDir, "references");
    expect(
      existsSync(referencesDir),
      `${referencesDir} exists but should have been stripped by 'files' in packages/sdk/package.json`
    ).toBe(false);

    // Also walk the entire installed antpath directory and assert no
    // path segment named "references" exists. Catches both the top-level
    // folder above AND any deeply-nested mirror that might be added by a
    // future tooling change.
    const offenders: string[] = [];
    walkPaths(install.antpathDir, (relPath) => {
      const segments = relPath.split(/[\\/]/);
      if (segments.some((seg) => seg === "references")) {
        offenders.push(relPath);
      }
    });
    expect(
      offenders,
      `installed antpath package contains forbidden 'references' paths: ${offenders.join(", ")}`
    ).toEqual([]);
  });
});

/**
 * Recursively walk a directory and invoke `visit` for every entry's path
 * relative to `root`. Skips node_modules to keep the walk cheap.
 */
function walkPaths(root: string, visit: (relPath: string) => void): void {
  const stack: string[] = [""];
  while (stack.length > 0) {
    const rel = stack.pop()!;
    const abs = rel ? join(root, rel) : root;
    const entries = readdirSync(abs, { withFileTypes: true });
    for (const entry of entries) {
      if (entry.name === "node_modules") continue;
      const childRel = rel ? join(rel, entry.name) : entry.name;
      visit(childRel);
      if (entry.isDirectory()) {
        stack.push(childRel);
      }
    }
  }
}

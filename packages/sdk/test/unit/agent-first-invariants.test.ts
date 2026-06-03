import { describe, expect, it } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, posix, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(import.meta.url);
const repoRoot = join(here, "..", "..", "..", "..", "..");

const SKIP_DIR_NAMES = new Set([
  "node_modules",
  "dist",
  ".next",
  ".turbo",
  "coverage",
  ".git",
  ".vercel",
  "supabase-out",
  ".pnpm-store",
  "tmp"
]);

const SOURCE_EXTENSIONS = new Set([
  ".ts",
  ".tsx",
  ".cts",
  ".mts",
  ".js",
  ".jsx",
  ".cjs",
  ".mjs",
  ".md"
]);

function walk(dir: string, out: string[]): void {
  for (const entry of readdirSync(dir)) {
    if (SKIP_DIR_NAMES.has(entry)) continue;
    const abs = join(dir, entry);
    let stat;
    try {
      stat = statSync(abs);
    } catch {
      continue;
    }
    if (stat.isDirectory()) {
      walk(abs, out);
      continue;
    }
    if (!stat.isFile()) continue;
    const ext = entry.slice(entry.lastIndexOf("."));
    if (!SOURCE_EXTENSIONS.has(ext)) continue;
    out.push(abs);
  }
}

function listSourceFiles(): readonly string[] {
  const out: string[] = [];
  walk(repoRoot, out);
  return out;
}

function relPosix(abs: string): string {
  return relative(repoRoot, abs).split(sep).join(posix.sep);
}

/**
 * Mechanical enforcement of the agent-first surface invariants.
 *
 * Goodwill is not enforcement. If an invariant matters, a test asserts
 * it; an agent making a future change either keeps the test green or
 * deletes it deliberately (and is forced to read this header first).
 */
describe("agent-first invariants (workspace-wide)", () => {
  it("forbids subpath imports of the antpath package", () => {
    // The `antpath` npm package exposes exactly one entry point. Subpath
    // imports (`antpath/proxy`, `antpath/core`, ...) would create
    // multiple surfaces an agent has to track. The package's
    // package.json#exports field is also locked down in
    // `package-exports.test.ts`; this test catches drift on the
    // consumer side.
    const offenders: string[] = [];
    const importPattern = /from\s+["']antpath\/[^"']+["']/g;
    const requirePattern = /require\(["']antpath\/[^"']+["']\)/g;
    for (const file of listSourceFiles()) {
      const rel = relPosix(file);
      // The package itself defines the surface; allow its own README to
      // mention the (forbidden) pattern in cautionary text only if
      // explicitly flagged.
      if (rel.startsWith("packages/sdk/src/")) continue;
      const text = readFileSync(file, "utf8");
      for (const match of text.matchAll(importPattern)) {
        offenders.push(`${rel}: ${match[0]}`);
      }
      for (const match of text.matchAll(requirePattern)) {
        offenders.push(`${rel}: ${match[0]}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("does not read process.env.ANTPATH_* from any user-facing parser surface", () => {
    // Platform-operator env vars (`ANTPATH_PROXY_TOKEN_PEPPER`,
    // `ANTPATH_CLI_BUNDLE_PATH`, etc.) live in BFF / worker code
    // ONLY. The user-facing parser surfaces (the SDK client and the
    // public contracts parser) must NEVER read process.env directly:
    // doing so creates an implicit default the agent reading the
    // submission call site cannot see. All inputs flow through
    // function parameters.
    const userFacingRoots = [
      "packages/sdk/src",
      "packages/contracts/src"
    ];
    const offenders: string[] = [];
    const pattern = /process\.env\.ANTPATH_/g;
    const allowlist = new Set<string>();
    for (const file of listSourceFiles()) {
      const rel = relPosix(file);
      if (!userFacingRoots.some((root) => rel.startsWith(`${root}/`))) continue;
      if (allowlist.has(rel)) continue;
      const text = readFileSync(file, "utf8");
      for (const match of text.matchAll(pattern)) {
        offenders.push(`${rel}: ${match[0]}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});

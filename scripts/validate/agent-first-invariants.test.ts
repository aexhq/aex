import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, extname, join, posix, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SOURCE_EXTENSIONS = new Set([".ts", ".tsx", ".cts", ".mts", ".js", ".jsx", ".cjs", ".mjs", ".md"]);

let sourceFilesCache: readonly string[] | undefined;

function listSourceFiles(root = repoRoot): readonly string[] {
  if (root === repoRoot && sourceFilesCache !== undefined) return sourceFilesCache;

  const listed = execFileSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024
  });
  const files = listed
    .split("\0")
    .filter((path) => path !== "" && SOURCE_EXTENSIONS.has(extname(path)))
    .map((path) => resolve(root, path))
    .filter(existsSync)
    .sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));

  if (root === repoRoot) sourceFilesCache = files;
  return files;
}

function relPosix(root: string, abs: string): string {
  return relative(root, abs).split(sep).join(posix.sep);
}

describe("agent-first invariants (workspace-wide)", () => {
  it("enumerates tracked and untracked source without scanning ignored build trees", () => {
    const fixture = mkdtempSync(join(tmpdir(), "aex-agent-invariants-"));
    try {
      execFileSync("git", ["init", "--quiet"], { cwd: fixture });
      mkdirSync(join(fixture, "src"), { recursive: true });
      mkdirSync(join(fixture, "ignored"), { recursive: true });
      writeFileSync(join(fixture, ".gitignore"), "ignored/\n", "utf8");
      writeFileSync(join(fixture, "src", "tracked.ts"), "export {};\n", "utf8");
      writeFileSync(join(fixture, "src", "deleted.ts"), "export {};\n", "utf8");
      writeFileSync(join(fixture, "src", "untracked.md"), "# Draft\n", "utf8");
      writeFileSync(join(fixture, "src", "not-source.bin"), "ignored by extension\n", "utf8");
      writeFileSync(join(fixture, "ignored", "generated.ts"), "throw new Error();\n", "utf8");
      execFileSync("git", ["add", ".gitignore", "src/deleted.ts", "src/tracked.ts"], { cwd: fixture });
      rmSync(join(fixture, "src", "deleted.ts"));

      const listed = listSourceFiles(fixture).map((path) => relPosix(fixture, path));
      expect(listed).toEqual(["src/tracked.ts", "src/untracked.md"]);
    } finally {
      rmSync(fixture, { recursive: true, force: true });
    }
  });

  // Windows antivirus and Git's working-tree enumeration make this
  // workspace-wide source scan legitimately slower than Vitest's 5s default.
  // Keep the test strict while giving the bounded scan an explicit budget.
  it("forbids subpath imports of the aex package", () => {
    const offenders: string[] = [];
    const importPattern = /from\s+["']aex\/[^"']+["']/g;
    const requirePattern = /require\(["']aex\/[^"']+["']\)/g;
    for (const file of listSourceFiles()) {
      const rel = relPosix(repoRoot, file);
      if (rel.startsWith("packages/sdk/src/")) continue;
      const source = readFileSync(file, "utf8");
      for (const match of source.matchAll(importPattern)) offenders.push(`${rel}: ${match[0]}`);
      for (const match of source.matchAll(requirePattern)) offenders.push(`${rel}: ${match[0]}`);
    }
    expect(offenders).toEqual([]);
  }, 30_000);

  it("does not read process.env.AEX_* from any user-facing parser surface", () => {
    const userFacingRoots = ["packages/sdk/src", "packages/contracts/src"];
    const offenders: string[] = [];
    const pattern = /process\.env\.AEX_/g;
    for (const file of listSourceFiles()) {
      const rel = relPosix(repoRoot, file);
      if (!userFacingRoots.some((root) => rel.startsWith(`${root}/`))) continue;
      if (rel.startsWith("packages/contracts/src/testing/")) continue;
      const source = readFileSync(file, "utf8");
      for (const match of source.matchAll(pattern)) offenders.push(`${rel}: ${match[0]}`);
    }
    expect(offenders).toEqual([]);
  });
});

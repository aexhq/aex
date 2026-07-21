import { mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, relative, resolve } from "node:path";
import { spawnSync, type SpawnSyncReturns } from "node:child_process";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const cliRoot = resolve(here, "..");
const distRoot = resolve(cliRoot, "dist");
const obsoleteResolver = ["parse", "Common", "Host", "Flags"].join("");

function filesUnder(root: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = resolve(root, entry.name);
    if (entry.isDirectory()) files.push(...filesUnder(path));
    else files.push(path);
  }
  return files;
}

function run(command: string, args: readonly string[], cwd: string): SpawnSyncReturns<string> {
  return spawnSync(command, [...args], { cwd, encoding: "utf8", windowsHide: true });
}

describe("@aexhq/cli supported and packed package surface", () => {
  it("retains exactly the promised root/runtime entrypoints and runtime keys", async () => {
    const pkg = JSON.parse(readFileSync(resolve(cliRoot, "package.json"), "utf8")) as {
      exports: Record<string, { types: string; import: string }>;
    };
    expect(pkg.exports).toEqual({
      ".": { types: "./dist/index.d.ts", import: "./dist/index.js" },
      "./runtime": { types: "./dist/runtime.d.ts", import: "./dist/runtime.js" }
    });

    const root = await import(new URL("../dist/index.js", import.meta.url).href);
    const runtime = await import(new URL("../dist/runtime.js", import.meta.url).href);
    expect(Object.keys(root).sort()).toEqual([
      "AEX_INDEX_PATH",
      "CLI_VERBS",
      "CLI_VERB_NAMES",
      "FILES_SUBVERBS",
      "START_FLAGS",
      "executeCli",
      "findVerbSpec"
    ]);
    expect(Object.keys(runtime).sort()).toEqual(["AEX_INDEX_PATH", "executeFilesSyncCmd"]);

    const rootDeclarations = readFileSync(resolve(distRoot, "index.d.ts"), "utf8");
    const runtimeDeclarations = readFileSync(resolve(distRoot, "runtime.d.ts"), "utf8");
    for (const symbol of ["executeCli", "AEX_INDEX_PATH", "CliIO", "CLI_VERBS", "CliVerbSpec"]) {
      expect(rootDeclarations).toContain(symbol);
    }
    for (const symbol of ["executeFilesSyncCmd", "CliExitCode", "AEX_INDEX_PATH", "CliIO", "SessionFilesSyncFileEntry"]) {
      expect(runtimeDeclarations).toContain(symbol);
    }
  }, 15_000);

  it("omits the obsolete resolver from every generated JavaScript and declaration", () => {
    const offenders = filesUnder(distRoot)
      .filter((path) => /\.(?:js|mjs|d\.ts)$/.test(path))
      .filter((path) => readFileSync(path, "utf8").includes(obsoleteResolver))
      .map((path) => relative(cliRoot, path).replaceAll("\\", "/"));
    expect(offenders).toEqual([]);
  });

  it("omits the obsolete resolver from the tarball and keeps host/deep subpaths blocked", () => {
    const scratch = mkdtempSync(resolve(tmpdir(), "aex-cli-surface-"));
    try {
      const pack = run("bun", ["pm", "pack", "--destination", scratch, "--ignore-scripts", "--quiet"], cliRoot);
      expect(pack.status, pack.stderr || pack.stdout).toBe(0);
      const tarballs = readdirSync(scratch).filter((name) => name.endsWith(".tgz"));
      expect(tarballs).toHaveLength(1);
      const extract = run("tar", ["-xzf", resolve(scratch, tarballs[0]!), "-C", scratch], scratch);
      expect(extract.status, extract.stderr || extract.stdout).toBe(0);

      const packedRoot = resolve(scratch, "package");
      const packedOffenders = filesUnder(packedRoot)
        .filter((path) => /(?:^|[\\/])(?:src|dist)[\\/].*\.(?:ts|js|mjs)$/.test(path))
        .filter((path) => !path.endsWith(".map"))
        .filter((path) => readFileSync(path, "utf8").includes(obsoleteResolver))
        .map((path) => relative(packedRoot, path).replaceAll("\\", "/"));
      expect(packedOffenders).toEqual([]);

      const installedRoot = resolve(scratch, "node_modules", "@aexhq", "cli");
      mkdirSync(dirname(installedRoot), { recursive: true });
      renameSync(packedRoot, installedRoot);
      for (const specifier of ["@aexhq/cli/host", "@aexhq/cli/dist/host/common.js"]) {
        const probe = run(
          "node",
          ["--input-type=module", "--eval", `import.meta.resolve(${JSON.stringify(specifier)})`],
          scratch
        );
        expect(probe.status).not.toBe(0);
        expect(`${probe.stdout}${probe.stderr}`).toContain("ERR_PACKAGE_PATH_NOT_EXPORTED");
      }
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }, 15_000);
});

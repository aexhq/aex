import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import { listPublishableModules } from "../cicd/public-modules.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
}

function listFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...listFiles(full));
    } else if (entry.isFile()) {
      out.push(full);
    }
  }
  return out;
}

function listTerraformRoots(): string[] {
  return ["infra/examples", "infra/modules"]
    .flatMap((parent) =>
      readdirSync(resolve(repoRoot, parent), { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => `${parent}/${entry.name}`)
        .filter((root) =>
          readdirSync(resolve(repoRoot, root), { withFileTypes: true }).some(
            (entry) => entry.isFile() && entry.name.endsWith(".tf")
          )
        )
    )
    .sort();
}

describe("repository hygiene", () => {
  it("keeps generated release and suite scratch out of tracked public files", () => {
    const ignore = read(".gitignore");

    expect(ignore).toMatch(/^\.tmp\/$/m);
    expect(ignore).toMatch(/^\.release-diagnostics\/$/m);
    expect(ignore).toMatch(/^\.release-worktrees\/$/m);
    expect(ignore).toMatch(/^\.suite-diagnostics\/$/m);
    expect(ignore).toMatch(/^\.suite-diagnostics-\*\/$/m);
    expect(ignore).toMatch(/^release-diagnostics\/$/m);

    const tracked = execFileSync(
      "git",
      [
        "ls-files",
        "-z",
        "--",
        ".tmp",
        ".release-diagnostics",
        ".release-worktrees",
        ".suite-diagnostics",
        ".suite-diagnostics-*",
        "release-diagnostics"
      ],
      { cwd: repoRoot, encoding: "utf8" }
    );

    expect(tracked).toBe("");
  });

  it("uses AGENTS.md only as an index to durable internal references", () => {
    const agents = read("AGENTS.md");

    expect(agents).toMatch(/table of contents/i);
    expect(agents).toContain("references/README.md");
    expect(agents).toContain("references/rules.md");
    expect(agents).toContain("references/repository-hygiene.md");
  });

  it("keeps checkout-local generated-dist mutex state out of Git", () => {
    const ignore = read(".gitignore");

    expect(ignore).toMatch(/^\.aex-generated-dist\.lock\/$/m);
    expect(ignore).toMatch(/^\.aex-generated-dist\.lock\.breaker\/$/m);
  });

  it("tracks a provider lockfile for every Terraform root", () => {
    const roots = listTerraformRoots();
    const trackedLockfiles = new Set(
      execFileSync("git", ["ls-files", "-z", "--", "infra/examples", "infra/modules"], {
        cwd: repoRoot,
        encoding: "utf8"
      })
        .split("\0")
        .filter((path) => path.endsWith("/.terraform.lock.hcl"))
    );

    expect(
      roots
        .map((root) => `${root}/.terraform.lock.hcl`)
        .filter((lockfile) => !trackedLockfiles.has(lockfile))
    ).toEqual([]);
  });

  it("locks Terraform providers for both supported runner platforms", () => {
    const incomplete: string[] = [];

    for (const root of listTerraformRoots()) {
      const lockfile = read(`${root}/.terraform.lock.hcl`);
      for (const match of lockfile.matchAll(/^provider "([^"]+)" \{([\s\S]*?)^\}/gm)) {
        const provider = match[1];
        const body = match[2] ?? "";
        // HCL records package hashes without their platform labels. Locking the
        // two supported runners contributes one distinct h1 hash per provider.
        const packageHashes = body.match(/^\s+"h1:[^"]+",$/gm) ?? [];
        if (packageHashes.length < 2) incomplete.push(`${root}: ${provider}`);
      }
    }

    expect(incomplete).toEqual([]);
  });

  it("keeps every published JavaScript sourcemap reference resolvable", () => {
    const dangling: string[] = [];

    for (const module of listPublishableModules(repoRoot)) {
      const dir = resolve(module.dir, "dist");
      expect(statSync(dir).isDirectory()).toBe(true);

      for (const file of listFiles(dir).filter((candidate) => candidate.endsWith(".js"))) {
        const source = readFileSync(file, "utf8");
        for (const match of source.matchAll(/sourceMappingURL=([^\s]+)/g)) {
          const reference = match[1];
          if (reference === undefined || reference.startsWith("data:")) continue;
          if (!existsSync(resolve(dirname(file), reference))) {
            dangling.push(file.slice(repoRoot.length + 1).replace(/\\/g, "/"));
          }
        }
      }
    }

    expect(dangling).toEqual([]);
  });

  it("serializes repository validation with generated SDK dist rebuilds", () => {
    const pkg = JSON.parse(read("package.json")) as {
      readonly scripts?: Record<string, string>;
    };

    expect(pkg.scripts?.["test:validate"]).toBe(
      "bun run test:graph && bun scripts/with-generated-dist-lock.mjs bun scripts/cicd/run-validation-tests.mjs"
    );
  });
});

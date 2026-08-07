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
      "bun scripts/with-generated-dist-lock.mjs bun scripts/cicd/run-validation-tests.mjs"
    );
  });
});

import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

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
  it("keeps local release diagnostics out of tracked public files", () => {
    expect(read(".gitignore")).toMatch(/^\.release-diagnostics\/$/m);

    const tracked = execFileSync("git", ["ls-files", "-z", "--", ".release-diagnostics"], {
      cwd: repoRoot,
      encoding: "utf8"
    });

    expect(tracked).toBe("");
  });

  it("does not ship dangling sourcemap comments for inlined SDK contracts", () => {
    const dir = resolve(repoRoot, "packages/sdk/dist/_contracts");
    expect(statSync(dir).isDirectory()).toBe(true);

    const dangling = listFiles(dir)
      .filter((file) => file.endsWith(".js"))
      .filter((file) => readFileSync(file, "utf8").includes("sourceMappingURL="))
      .map((file) => file.slice(repoRoot.length + 1).replace(/\\/g, "/"));

    expect(dangling).toEqual([]);
  });

  it("serializes docs generation with SDK dist rebuilds", () => {
    const pkg = JSON.parse(read("apps/docs/package.json")) as {
      readonly scripts?: Record<string, string>;
    };

    expect(pkg.scripts?.generate).toBe(
      "bun ../../scripts/with-generated-dist-lock.mjs bun run generate:unlocked"
    );
    expect(pkg.scripts?.["generate:unlocked"]).toBe("bun ../../scripts/docs/generate-all.mjs");
  });
});

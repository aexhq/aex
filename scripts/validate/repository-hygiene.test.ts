import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
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
});

import { describe, expect, it } from "vitest";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const checker = resolve(repoRoot, "scripts/validate/check-session-terminology.mjs");

function runInFixture(files) {
  const dir = mkdtempSync(join(tmpdir(), "aex-terminology-"));
  try {
    for (const [name, contents] of Object.entries(files)) {
      const path = join(dir, name);
      mkdirSync(resolve(path, ".."), { recursive: true });
      writeFileSync(path, contents, "utf8");
    }
    expect(spawnSync("git", ["init", "--quiet"], { cwd: dir }).status).toBe(0);
    expect(spawnSync("git", ["add", "."], { cwd: dir }).status).toBe(0);
    return spawnSync(process.execPath, [checker], { cwd: dir, encoding: "utf8" });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("session terminology checker", () => {
  it("allows AG-UI run identity, run outcomes, workflow vocabulary, and ordinary English", () => {
    const result = runInFixture({
      "src/session-run.ts": `
export interface SessionRun { runId: string; outcome: "succeeded" | "interrupted" }
export const RUN_FINISHED = "RUN_FINISHED";
// Run this command after the current run finishes.
`,
      ".github/workflows/ci.yml": "run: bun run test\n",
    });

    expect(result.status).toBe(0);
    expect(result.stdout).toContain("session terminology check passed");
  });

  it("still rejects the retired runs route, CLI command, quota field, and list API", () => {
    const result = runInFixture({
      "src/product.ts": `
const route = "/runs";
const command = "aex run";
const maxConcurrentRuns = 4;
function listRuns() {}
`,
    });

    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("legacy runs route");
    expect(result.stderr).toContain("legacy CLI command");
    expect(result.stderr).toContain("legacy public quota field");
    expect(result.stderr).toContain("legacy list-runs API");
  });

  it("rejects grammatical corruption from mechanical run-to-session rewrites", () => {
    const result = runInFixture({
      "src/runtime.ts": `
// The brain sessions a turn and feeds a sessionning hash.
// Ordinary sessions the user owns remain valid product vocabulary.
`
    });

    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain("mechanical session rewrite");
  });
});

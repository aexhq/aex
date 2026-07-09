import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const checker = resolve(repoRoot, "scripts/cicd/check-generated-dist-closure.mjs");

function withDist(files: Record<string, string>, fn: (dist: string) => void): void {
  const root = mkdtempSync(join(tmpdir(), "aex-dist-closure-"));
  const dist = join(root, "dist");
  mkdirSync(dist, { recursive: true });
  try {
    for (const [name, text] of Object.entries(files)) {
      const path = join(dist, name);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, text);
    }
    fn(dist);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function runChecker(dist: string): { readonly stdout: string; readonly stderr: string } {
  try {
    const stdout = execFileSync(process.execPath, [checker, dist], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"]
    });
    return { stdout, stderr: "" };
  } catch (error) {
    const failure = error as { readonly stdout?: Buffer | string; readonly stderr?: Buffer | string };
    return {
      stdout: String(failure.stdout ?? ""),
      stderr: String(failure.stderr ?? "")
    };
  }
}

describe("generated dist closure", () => {
  it("accepts a coherent generated ESM dist tree", () => {
    withDist(
      {
        "index.js": 'export * from "./session-retention.js";\n',
        "index.d.ts": 'export * from "./session-retention.js";\n',
        "session-retention.js": "export const retention = true;\n",
        "session-retention.d.ts": "export declare const retention: boolean;\n"
      },
      (dist) => {
        expect(runChecker(dist).stdout).toContain("generated-dist closure OK");
      }
    );
  });

  it("fails clearly when a generated module target is missing", () => {
    withDist(
      {
        "index.js": 'export * from "./session-retention.js";\n',
        "index.d.ts": 'export * from "./session-retention.js";\n'
      },
      (dist) => {
        expect(runChecker(dist).stderr).toContain("references missing module ./session-retention.js");
      }
    );
  });

  it("fails clearly when a generated declaration target is missing", () => {
    withDist(
      {
        "index.js": 'export * from "./session-retention.js";\n',
        "index.d.ts": 'export * from "./session-retention.js";\n',
        "session-retention.js": "export const retention = true;\n"
      },
      (dist) => {
        const stderr = runChecker(dist).stderr;
        expect(stderr).toContain("session-retention.js is missing declaration session-retention.d.ts");
        expect(stderr).toContain("references ./session-retention.js but session-retention.d.ts is missing");
      }
    );
  });
});

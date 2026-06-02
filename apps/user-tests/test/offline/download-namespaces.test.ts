/**
 * Offline scenario: download-namespaces.test.ts
 *
 * Verifies the run-artifact download surface ships in the *installed*
 * package — no live API required:
 *
 *   - `AntpathClient` exposes the whole-run verb `download` plus the four
 *     per-namespace verbs `downloadOutputs` / `downloadLogs` /
 *     `downloadEvents` / `downloadMetadata`.
 *   - The CLI `download` command validates `--only <namespace>` BEFORE any
 *     network call: an unknown namespace exits non-zero with the
 *     documented "must be one of" usage error, and the bare-usage banner
 *     advertises the flag.
 *
 * Every assertion runs against `node_modules/antpath` in a fresh install
 * tempdir, so it exercises the published shape, not the monorepo symlink.
 */
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

const IS_WINDOWS = process.platform === "win32";

describe("download namespaces surface (offline)", () => {
  let install: InstallResult;
  let binPath: string;

  beforeAll(async () => {
    install = await installAntpath();
    binPath = join(install.installDir, "node_modules", ".bin", IS_WINDOWS ? "antpath.cmd" : "antpath");
  });

  afterAll(() => {
    install?.cleanup();
  });

  it("AntpathClient exposes the whole-run + per-namespace download verbs", async () => {
    const script = `
      const { AntpathClient } = await import("antpath");
      const c = new AntpathClient({ apiToken: "t", baseUrl: "https://example.test" });
      const verbs = ["download", "downloadOutputs", "downloadLogs", "downloadEvents", "downloadMetadata"];
      const result = {};
      for (const v of verbs) result[v] = typeof c[v];
      process.stdout.write(JSON.stringify(result));
    `;
    const path = join(install.installDir, "download-verbs.mjs");
    writeFileSync(path, script);
    const child = await runCommand(process.execPath, [path], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(child.exitCode).toBe(0);
    const result = JSON.parse(child.stdout) as Record<string, string>;
    for (const v of ["download", "downloadOutputs", "downloadLogs", "downloadEvents", "downloadMetadata"]) {
      expect(result[v], `AntpathClient.${v} should be a function`).toBe("function");
    }
  });

  it("`antpath download` usage advertises --only and its namespaces", async () => {
    expect(existsSync(binPath)).toBe(true);
    // No run id → usage error (exit 2) that lists the --only namespaces.
    const result = await runCommand(
      binPath,
      ["download", "--api-token", "t", "--antpath-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only outputs\|logs\|events\|metadata/);
  });

  it("`antpath download --only <bogus>` rejects before any network call", async () => {
    const result = await runCommand(
      binPath,
      ["download", "run-x", "--only", "bogus", "--api-token", "t", "--antpath-url", "https://example.test"],
      { cwd: install.installDir, timeoutMs: 30_000 }
    );
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/--only must be one of: outputs, logs, events, metadata/);
  });
});

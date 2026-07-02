/**
 * Scenario 2: cli-bin.test.ts
 *
 * The agent-first promise: `bun add @aexhq/sdk` puts a working
 * `aex` executable in node_modules/.bin. This scenario verifies:
 *   - The bin symlink/shim resolves.
 *   - `aex --help` exits 0 and prints the canonical usage banner.
 *   - Removed launch-era verbs stay unavailable.
 *   - Setting AEX_* env vars in the child does NOT change behavior
 *     (agent-first invariant: zero env-var reads in the shipped bundle).
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getAexBinPath, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("cli bin", () => {
  let install: InstallResult;
  let binPath: string;
  let cliMjsPath: string;

  beforeAll(async () => {
    install = await installAex();
    binPath = getAexBinPath(install.installDir);
    cliMjsPath = join(install.aexDir, "dist", "cli.mjs");
  });

  afterAll(() => {
    install?.cleanup();
  });

  it("creates a runnable bin link in node_modules/.bin", () => {
    expect(existsSync(binPath)).toBe(true);
  });

  it("`aex --help` exits 0 and prints the unified host usage banner", async () => {
    const result = await runCommand(binPath, ["--help"], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(result.exitCode).toBe(0);
    expect(result.stdout).toMatch(/Usage:/);
    expect(result.stdout).toMatch(/aex run/);
    expect(result.stdout).toMatch(/aex whoami/);
    expect(result.stdout).toMatch(/aex --help/);
    expect(result.stdout).not.toMatch(/proxy/);
  });

  it("removed launch-era verbs are not accepted", async () => {
    const result = await runCommand(binPath, ["proxy", "--help"], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toMatch(/unknown subcommand: proxy/);
  });

  it("invocation is byte-identical regardless of AEX_* env vars (no-env-var invariant)", async () => {
    const baseline = await runCommand(binPath, ["--help"], { cwd: install.installDir, timeoutMs: 30_000 });
    const withEnv = await runCommand(binPath, ["--help"], {
      cwd: install.installDir,
      timeoutMs: 30_000,
      env: {
        ...process.env,
        AEX_TEST_UNUSED_ONE: "/dev/null",
        AEX_TEST_UNUSED_TWO: "/dev/null",
        AEX_CLI_BUNDLE_PATH: "/dev/null"
      }
    });
    expect(withEnv.exitCode).toBe(baseline.exitCode);
    expect(withEnv.stdout).toBe(baseline.stdout);
    expect(withEnv.stderr).toBe(baseline.stderr);
  });

  it("the shipped bundle source contains no `process.env.AEX_*` reads", () => {
    const bytes = readFileSync(cliMjsPath, "utf8");
    expect(bytes).not.toMatch(/process\.env\.AEX_/);
    expect(bytes).not.toMatch(/AEX_CLI_BUNDLE_PATH/);
  });
});

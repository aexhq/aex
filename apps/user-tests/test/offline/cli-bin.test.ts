/**
 * Scenario 2: cli-bin.test.ts
 *
 * The agent-first promise: `npm install antpath` puts a working
 * `antpath` executable in node_modules/.bin. This scenario verifies:
 *   - The bin symlink (or .cmd shim on Windows) resolves.
 *   - `antpath --help` exits 0 and prints the canonical usage banner.
 *   - `antpath proxy --help` exits 0 even with no manifest mounted.
 *   - `antpath proxy <name>` without a manifest exits non-zero AND emits
 *     the documented "manifest not mounted" error envelope.
 *   - Setting ANTPATH_* env vars in the child does NOT change behavior
 *     (agent-first invariant: zero env-var reads in the shipped bundle).
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

const IS_WINDOWS = process.platform === "win32";

describe("cli bin", () => {
  let install: InstallResult;
  let binPath: string;
  let cliMjsPath: string;

  beforeAll(async () => {
    install = await installAntpath();
    // node_modules/.bin/antpath (POSIX symlink) or .cmd shim on Windows.
    const binDir = join(install.installDir, "node_modules", ".bin");
    binPath = join(binDir, IS_WINDOWS ? "antpath.cmd" : "antpath");
    cliMjsPath = join(install.antpathDir, "dist", "cli.mjs");
  });

  afterAll(() => {
    install?.cleanup();
  });

  it("creates a runnable bin link in node_modules/.bin", () => {
    expect(existsSync(binPath)).toBe(true);
  });

  it("`antpath --help` exits 0 and prints the unified host usage banner", async () => {
    const result = await runCommand(binPath, ["--help"], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(result.exitCode).toBe(0);
    expect(result.stdout).toMatch(/Usage:/);
    // Host mode (no manifest mounted): help lists the host subcommands.
    expect(result.stdout).toMatch(/antpath run/);
    expect(result.stdout).toMatch(/antpath whoami/);
    expect(result.stdout).toMatch(/antpath --help/);
  });

  it("`antpath proxy --help` exits 0 even with no manifest mounted", async () => {
    const result = await runCommand(binPath, ["proxy", "--help"], { cwd: install.installDir, timeoutMs: 30_000 });
    expect(result.exitCode).toBe(0);
    expect(result.stdout).toMatch(/--method/);
    expect(result.stdout).toMatch(/--path/);
    expect(result.stdout).toMatch(/--response-mode/);
  });

  it("`antpath proxy <name>` without manifest exits non-zero with the documented error envelope", async () => {
    const result = await runCommand(binPath, ["proxy", "someendpoint"], {
      cwd: install.installDir,
      timeoutMs: 30_000
    });
    expect(result.exitCode).not.toBe(0);
    // Error envelope is JSON-on-one-line to stderr.
    const lastNonEmpty = result.stderr.split(/\r?\n/).filter((line) => line.trim().length > 0).pop();
    expect(lastNonEmpty).toBeDefined();
    const body = JSON.parse(lastNonEmpty!) as { error: string; message: string };
    expect(body.error).toBe("internal_error");
    expect(body.message).toMatch(/manifest not mounted/i);
  });

  it("invocation is byte-identical regardless of ANTPATH_* env vars (no-env-var invariant)", async () => {
    const baseline = await runCommand(binPath, ["--help"], { cwd: install.installDir, timeoutMs: 30_000 });
    const withEnv = await runCommand(binPath, ["--help"], {
      cwd: install.installDir,
      timeoutMs: 30_000,
      env: {
        ...process.env,
        ANTPATH_RUN_TOKEN_FILE: "/dev/null",
        ANTPATH_PROXY_BASE_FILE: "/dev/null",
        ANTPATH_PROXY_INDEX_FILE: "/dev/null",
        ANTPATH_CLI_BUNDLE_PATH: "/dev/null"
      }
    });
    expect(withEnv.exitCode).toBe(baseline.exitCode);
    expect(withEnv.stdout).toBe(baseline.stdout);
  });

  it("the shipped bundle source contains no `process.env.ANTPATH_*` reads", () => {
    // Mechanical double-check on the installed artifact (the workspace
    // test packages/cli/test/no-env-vars.test.ts does the same against
    // the build output; we re-run it here against what the registry
    // actually serves).
    const bytes = readFileSync(cliMjsPath, "utf8");
    expect(bytes).not.toMatch(/process\.env\.ANTPATH_/);
    expect(bytes).not.toMatch(/ANTPATH_RUN_TOKEN_FILE/);
    expect(bytes).not.toMatch(/ANTPATH_PROXY_BASE_FILE/);
    expect(bytes).not.toMatch(/ANTPATH_PROXY_INDEX_FILE/);
    expect(bytes).not.toMatch(/ANTPATH_CLI_BUNDLE_PATH/);
  });
});

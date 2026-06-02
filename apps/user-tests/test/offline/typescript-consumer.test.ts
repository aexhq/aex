/**
 * Scenario 4: typescript-consumer.test.ts
 *
 * Catches broken .d.ts / export metadata. A real consumer project (and
 * any AI agent doing `tsc --noEmit`) must be able to import the SDK's
 * single canonical class under `moduleResolution: "NodeNext"` and have
 * it type-check.
 *
 * Strategy:
 *   1. Install antpath into the shared fixture tempdir.
 *   2. Add TypeScript as a devDependency in the same tempdir.
 *   3. Write a minimal tsconfig + consumer.ts that imports + uses
 *      `AntpathClient` (the single user-facing class).
 *   4. Spawn `tsc --noEmit` from the install's local typescript.
 *   5. Assert exit 0 with no diagnostics.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

const IS_WINDOWS = process.platform === "win32";

describe("typescript consumer", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
    // Add typescript to the same install tempdir.
    const npm = IS_WINDOWS ? "npm.cmd" : "npm";
    const result = await runCommand(
      npm,
      ["install", "typescript@5.8.3", "--no-audit", "--no-fund", "--ignore-scripts"],
      { cwd: install.installDir, timeoutMs: 120_000 }
    );
    if (result.exitCode !== 0) {
      throw new Error(`typescript install failed: ${result.stderr}`);
    }
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it("AntpathClient type-checks from a clean NodeNext consumer", async () => {
    const tsconfig = {
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        strict: true,
        noEmit: true,
        skipLibCheck: true,
        types: []
      },
      include: ["consumer.ts"]
    };
    const consumer = `
      import { AntpathClient } from "antpath";
      // Type-only check; we don't actually run the constructor here.
      type C = InstanceType<typeof AntpathClient>;
      declare const _c: C;
      void _c;
    `;
    writeFileSync(join(install.installDir, "tsconfig.json"), JSON.stringify(tsconfig, null, 2));
    writeFileSync(join(install.installDir, "consumer.ts"), consumer);

    const tscBin = join(install.installDir, "node_modules", ".bin", IS_WINDOWS ? "tsc.cmd" : "tsc");
    const result = await runCommand(tscBin, ["--noEmit", "-p", "tsconfig.json"], {
      cwd: install.installDir,
      timeoutMs: 120_000
    });
    if (result.exitCode !== 0) {
      throw new Error(
        `tsc --noEmit failed (exit ${result.exitCode}):\n--- stdout ---\n${result.stdout}\n--- stderr ---\n${result.stderr}`
      );
    }
    expect(result.exitCode).toBe(0);
  });
});

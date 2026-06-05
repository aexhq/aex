/**
 * Scenario 3: sdk-imports.test.ts
 *
 * Verifies the single-surface invariant on the *installed* package:
 *   - `await import("@aexhq/sdk")` resolves at runtime and exports the
 *     canonical named bindings.
 *   - `require("@aexhq/sdk")` fails with ERR_REQUIRE_ESM (the package is
 *     ESM-only — that's the contract).
 *   - Subpath imports such as `@aexhq/sdk/platform` or `@aexhq/sdk/proxy`
 *     fail with ERR_PACKAGE_PATH_NOT_EXPORTED.
 *
 * Every assertion runs in a child Node process whose cwd is the install
 * tempdir, so resolution goes through the installed `node_modules/@aexhq/sdk`
 * and NOT the monorepo's pnpm symlink.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("sdk imports", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  });

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, file: string): Promise<{ exitCode: number; stdout: string; stderr: string }> {
    const path = join(install.installDir, file);
    writeFileSync(path, script);
    return await runCommand(process.execPath, [path], { cwd: install.installDir, timeoutMs: 30_000 });
  }

  it("await import(\"@aexhq/sdk\") resolves and exports the canonical names", async () => {
    const script = `
      const mod = await import("@aexhq/sdk");
      const names = [
        "AgentExecutor",
        "Skill",
        "McpServer",
        "RUN_RECORD_SCHEMA_VERSION",
        "RUN_RECORD_MANIFEST_SCHEMA_VERSION",
        "validateProxyAuth",
        "buildPlatformAllowedHosts"
      ];
      const result = {};
      for (const name of names) {
        result[name] = typeof mod[name];
      }
      // Confirm the renamed SDK client and legacy platform export are GONE — single-surface invariant.
      result.AexClient_present = (typeof mod.AexClient !== "undefined");
      result.AexPlatformClient_present = (typeof mod.AexPlatformClient !== "undefined");
      // Confirm the legacy Template/compileTemplate exports are GONE — flat surface invariant (P5).
      result.Template_present = (typeof mod.Template !== "undefined");
      result.TemplateDefinition_present = (typeof mod.TemplateDefinition !== "undefined");
      result.Blueprint_present = (typeof mod.Blueprint !== "undefined");
      result.defineRun_present = (typeof mod.defineRun !== "undefined");
      result.compileTemplate_present = (typeof mod.compileTemplate !== "undefined");
      result.submitResolvedRun_present = (typeof mod.submitResolvedRun !== "undefined");
      result.RunRef_present = (typeof mod.RunRef !== "undefined");
      process.stdout.write(JSON.stringify(result));
    `;
    const child = await runChild(script, "esm-import.mjs");
    expect(child.exitCode).toBe(0);
    const result = JSON.parse(child.stdout) as Record<string, string | boolean>;
    expect(result["AgentExecutor"]).toBe("function");
    expect(result["Skill"]).toBe("function");
    expect(result["McpServer"]).toBe("function");
    expect(result["RUN_RECORD_SCHEMA_VERSION"]).toBe("string");
    expect(result["RUN_RECORD_MANIFEST_SCHEMA_VERSION"]).toBe("string");
    expect(result["validateProxyAuth"]).toBe("function");
    expect(result["buildPlatformAllowedHosts"]).toBe("function");
    expect(result["AexClient_present"]).toBe(false);
    expect(result["AexPlatformClient_present"]).toBe(false);
    expect(result["Template_present"]).toBe(false);
    expect(result["TemplateDefinition_present"]).toBe(false);
    expect(result["Blueprint_present"]).toBe(false);
    expect(result["defineRun_present"]).toBe(false);
    expect(result["compileTemplate_present"]).toBe(false);
    expect(result["submitResolvedRun_present"]).toBe(false);
    expect(result["RunRef_present"]).toBe(false);
  });

  it("require(\"@aexhq/sdk\") fails with a clear no-CJS error", async () => {
    // .cjs forces CommonJS context regardless of host package.json type.
    const script = `
      try {
        require("@aexhq/sdk");
        process.stdout.write(JSON.stringify({ ok: true }));
      } catch (err) {
        process.stdout.write(JSON.stringify({ ok: false, code: err.code, message: String(err.message).slice(0, 200) }));
      }
    `;
    const child = await runChild(script, "cjs-require.cjs");
    expect(child.exitCode).toBe(0);
    const result = JSON.parse(child.stdout) as { ok: boolean; code?: string; message?: string };
    expect(result.ok).toBe(false);
    // The package's "exports" map has no `require` condition, so Node
    // rejects at the export-resolution layer with ERR_PACKAGE_PATH_NOT_EXPORTED
    // (not ERR_REQUIRE_ESM, which fires later in the pipeline). Either
    // code communicates the same contract: @aexhq/sdk is ESM-only via the
    // root entry. Accept both for forward-compat with future Node
    // resolver changes.
    expect(["ERR_PACKAGE_PATH_NOT_EXPORTED", "ERR_REQUIRE_ESM"]).toContain(result.code);
  });

  it("subpath imports fail with ERR_PACKAGE_PATH_NOT_EXPORTED", async () => {
    const script = `
      const probes = ["@aexhq/sdk/platform", "@aexhq/sdk/proxy", "@aexhq/sdk/cli"];
      const out = {};
      for (const spec of probes) {
        try {
          await import(spec);
          out[spec] = { ok: true };
        } catch (err) {
          out[spec] = { ok: false, code: err.code };
        }
      }
      process.stdout.write(JSON.stringify(out));
    `;
    const child = await runChild(script, "subpath.mjs");
    expect(child.exitCode).toBe(0);
    const result = JSON.parse(child.stdout) as Record<string, { ok: boolean; code?: string }>;
    for (const spec of ["@aexhq/sdk/platform", "@aexhq/sdk/proxy", "@aexhq/sdk/cli"]) {
      expect(result[spec]).toBeDefined();
      expect(result[spec]!.ok).toBe(false);
      expect(result[spec]!.code).toBe("ERR_PACKAGE_PATH_NOT_EXPORTED");
    }
  });
});

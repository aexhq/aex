/**
 * Scenario 3: sdk-imports.test.ts
 *
 * Verifies the single-surface invariant on the *installed* package:
 *   - `await import("@aexhq/sdk")` resolves at runtime and exports the
 *     slim launch root bindings.
 *   - `require("@aexhq/sdk")` fails with ERR_REQUIRE_ESM (the package is
 *     ESM-only — that's the contract).
 *   - Subpath imports such as `@aexhq/sdk/platform` or `@aexhq/sdk/proxy`
 *     fail with ERR_PACKAGE_PATH_NOT_EXPORTED.
 *
 * Every assertion runs in a child Bun process whose cwd is the install
 * tempdir, so resolution goes through the installed `node_modules/@aexhq/sdk`
 * and NOT the monorepo workspace symlink.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

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
    return await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 30_000 });
  }

  it("await import(\"@aexhq/sdk\") resolves to the slim launch surface", async () => {
    const script = `
      const mod = await import("@aexhq/sdk");
      const forbidden = [
        "AgentExecutor",
        "createDataTools",
        "createCorpusTools",
        "DataTools",
        "DataToolError",
        "DATA_TOOLS_INSTRUCTIONS",
        "ProxyEndpoint",
        "decodeAssistantText",
        "decodeToolCalls",
        "summarizeTurnTrace",
        "summarizeSessionUsage",
        "textOf",
        "AexClient",
        "AexPlatformClient",
        "Template",
        "TemplateDefinition",
        "Blueprint",
        "defineRun",
        "compileTemplate",
        "submitResolvedRun",
        "LegacySessionRef",
        "ChatClient",
        "ChatSession",
        "ChatTurnStream",
        "billing"
      ];
      const result = {
        Aex: typeof mod.Aex,
        forbidden: {},
        constructString: false,
        constructStringAndOptions: false
      };
      for (const name of forbidden) {
        result.forbidden[name] = typeof mod[name];
      }
      const fetch = async () => new Response("{}", { headers: { "content-type": "application/json" } });
      try {
        new mod.Aex("aex_runtime_surface");
        result.constructString = true;
      } catch (err) {
        result.constructStringError = String(err?.message ?? err);
      }
      try {
        new mod.Aex("aex_runtime_surface", { baseUrl: "https://example.invalid", fetch });
        result.constructStringAndOptions = true;
      } catch (err) {
        result.constructStringAndOptionsError = String(err?.message ?? err);
      }
      process.stdout.write(JSON.stringify(result));
    `;
    const child = await runChild(script, "esm-import.mjs");
    expect(child.exitCode).toBe(0);
    const result = JSON.parse(child.stdout) as {
      Aex: string;
      forbidden: Record<string, string>;
      constructString: boolean;
      constructStringError?: string;
      constructStringAndOptions: boolean;
      constructStringAndOptionsError?: string;
    };
    expect(result["Aex"]).toBe("function");
    expect(result.constructString, result.constructStringError).toBe(true);
    expect(result.constructStringAndOptions, result.constructStringAndOptionsError).toBe(true);
    for (const [name, type] of Object.entries(result.forbidden)) {
      expect(type, `${name} should not be exported from the slim root surface`).toBe("undefined");
    }
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
    // Bun reports this as MODULE_NOT_FOUND, while Node ESM loaders may report
    // export-map or ESM-only codes. The contract is that the CJS surface does
    // not resolve.
    expect(["MODULE_NOT_FOUND", "ERR_PACKAGE_PATH_NOT_EXPORTED", "ERR_REQUIRE_ESM"]).toContain(result.code);
  });

  it("subpath imports do not resolve", async () => {
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
      expect(["ERR_MODULE_NOT_FOUND", "ERR_PACKAGE_PATH_NOT_EXPORTED"]).toContain(result[spec]!.code);
    }
  });
});

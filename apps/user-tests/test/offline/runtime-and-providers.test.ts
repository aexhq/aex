/**
 * Scenario 5: runtime-and-providers.test.ts
 *
 * Locks the dual-runtime + widened-provider surface as it appears
 * inside a clean `npm install antpath` tempdir — the same way a real
 * user / AI agent sees the package. Catches regressions where:
 *   - SDK drops the `runtime?` option from SubmitRunOptions
 *   - SDK silently rejects providers in RUN_PROVIDERS
 *   - Server-side validation (selectRuntime) stops being importable
 *   - RuntimeValidationError code values shift
 *
 * Every assertion runs in a child Node process whose cwd is the
 * install tempdir, so resolution goes through the installed
 * `node_modules/antpath` and NOT the monorepo's pnpm symlink.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("dual-runtime + widened providers (published surface)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
  });

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, file: string) {
    const path = join(install.installDir, file);
    writeFileSync(path, script);
    return runCommand(process.execPath, [path], { cwd: install.installDir, timeoutMs: 30_000 });
  }

  it("exports the v1 provider set + RUNTIME_KINDS + selectRuntime", async () => {
    const script = `
      const mod = await import("antpath");
      const result = {
        providers: mod.RUN_PROVIDERS,
        runtimes: mod.RUNTIME_KINDS,
        defaultProvider: mod.DEFAULT_RUN_PROVIDER,
        hasSelectRuntime: typeof mod.selectRuntime === "function",
        hasCollect: typeof mod.collectNativeOnlyFeatures === "function",
        hasError: typeof mod.RuntimeValidationError === "function",
        validationCodes: mod.RUNTIME_VALIDATION_CODES
      };
      console.log(JSON.stringify(result));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "runtime-exports.mjs");
    expect(exitCode, stderr).toBe(0);
    const out = JSON.parse(stdout.trim()) as {
      providers: string[];
      runtimes: string[];
      defaultProvider: string;
      hasSelectRuntime: boolean;
      hasCollect: boolean;
      hasError: boolean;
      validationCodes: string[];
    };
    expect(out.providers).toEqual(["anthropic", "deepseek", "openai", "gemini", "mistral"]);
    expect(out.runtimes).toEqual(["native", "managed"]);
    expect(out.defaultProvider).toBe("anthropic");
    expect(out.hasSelectRuntime).toBe(true);
    expect(out.hasCollect).toBe(true);
    expect(out.hasError).toBe(true);
    expect(out.validationCodes).toEqual([
      "runtime_native_unsupported",
      "feature_runtime_mismatch"
    ]);
  });

  it("selectRuntime auto-routes anthropic→native and deepseek→managed", async () => {
    const script = `
      const { selectRuntime } = await import("antpath");
      const base = {
        workspaceId: "ws", idempotencyKey: "id", provider: "anthropic",
        submission: { model: "claude-haiku-4-5", prompt: ["hi"], skills: [], agentsMd: [], files: [], mcpServers: [] },
        secrets: { anthropic: { apiKey: "sk-ant-test-1" } }
      };
      const a = selectRuntime(base);
      const b = selectRuntime({
        ...base, provider: "deepseek",
        secrets: { deepseek: { apiKey: "sk-d-test" } }
      });
      console.log(JSON.stringify({ anthropic: a, deepseek: b }));
    `;
    const { exitCode, stdout } = await runChild(script, "select-runtime.mjs");
    expect(exitCode).toBe(0);
    expect(JSON.parse(stdout.trim())).toEqual({ anthropic: "native", deepseek: "managed" });
  });

  it("selectRuntime throws runtime_native_unsupported for non-anthropic + native", async () => {
    const script = `
      const { selectRuntime, RuntimeValidationError } = await import("antpath");
      const req = {
        workspaceId: "ws", idempotencyKey: "id", provider: "openai", runtime: "native",
        submission: { model: "gpt-4o-mini", prompt: ["hi"], skills: [], agentsMd: [], files: [], mcpServers: [] },
        secrets: { openai: { apiKey: "sk-o-test" } }
      };
      try {
        selectRuntime(req);
        console.log(JSON.stringify({ caught: false }));
      } catch (err) {
        console.log(JSON.stringify({
          caught: true,
          isClass: err instanceof RuntimeValidationError,
          code: err.code,
          messageHasProvider: typeof err.message === "string" && err.message.includes("provider")
        }));
      }
    `;
    const { exitCode, stdout } = await runChild(script, "select-native-reject.mjs");
    expect(exitCode).toBe(0);
    const out = JSON.parse(stdout.trim());
    expect(out).toEqual({
      caught: true,
      isClass: true,
      code: "runtime_native_unsupported",
      messageHasProvider: true
    });
  });

  it("selectRuntime throws feature_runtime_mismatch for Skill.provider on managed", async () => {
    const script = `
      const { selectRuntime } = await import("antpath");
      const req = {
        workspaceId: "ws", idempotencyKey: "id", provider: "anthropic", runtime: "managed",
        submission: {
          model: "claude-haiku-4-5", prompt: ["hi"],
          skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf" }],
          agentsMd: [], files: [], mcpServers: []
        },
        secrets: { anthropic: { apiKey: "sk-ant-test-1" } }
      };
      try {
        selectRuntime(req);
        console.log(JSON.stringify({ caught: false }));
      } catch (err) {
        console.log(JSON.stringify({
          caught: true,
          code: err.code,
          mentionsPdf: err.message.includes("pdf"),
          mentionsNative: err.message.includes("native")
        }));
      }
    `;
    const { exitCode, stdout } = await runChild(script, "select-feature-reject.mjs");
    expect(exitCode).toBe(0);
    expect(JSON.parse(stdout.trim())).toEqual({
      caught: true,
      code: "feature_runtime_mismatch",
      mentionsPdf: true,
      mentionsNative: true
    });
  });

  it("AntpathClient.submitRun rejects non-matching provider secrets at the SDK boundary", async () => {
    // The dispatcher rejects cross-provider secrets; the SDK does the
    // same check synchronously before hitting the network. We confirm
    // that by calling submitRun with the wrong shape and asserting it
    // throws before any fetch happens. Use a fetch-fake that records
    // calls to prove no network call leaked.
    const script = `
      const { AntpathClient } = await import("antpath");
      const calls = [];
      const fetchFake = async (...args) => { calls.push(args); return new Response("never", { status: 500 }); };
      const client = new AntpathClient({ apiToken: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      let caught = null;
      try {
        await client.submitRun({
          provider: "openai",
          model: "gpt-4o-mini",
          prompt: "hi",
          secrets: { anthropic: { apiKey: "sk-ant-wrong" } }
        });
      } catch (err) {
        caught = err.message;
      }
      console.log(JSON.stringify({ caught, fetchCalls: calls.length }));
    `;
    const { exitCode, stdout } = await runChild(script, "client-secret-mismatch.mjs");
    expect(exitCode).toBe(0);
    const out = JSON.parse(stdout.trim()) as { caught: string | null; fetchCalls: number };
    expect(out.caught).toMatch(/secrets\.openai\.apiKey is required/);
    expect(out.fetchCalls).toBe(0);
  });

  it("AntpathClient.submitRun forwards the optional runtime field on the wire", async () => {
    const script = `
      const { AntpathClient } = await import("antpath");
      const requests = [];
      const fetchFake = async (url, init) => {
        let body = init?.body;
        if (typeof body !== "string" && body) {
          body = await new Response(body).text();
        }
        requests.push({ url: typeof url === "string" ? url : url.toString(), body });
        return new Response(JSON.stringify({
          id: "run_test_user_e2e",
          workspaceId: "ws_t",
          status: "queued",
          createdAt: new Date().toISOString()
        }), { status: 202, headers: { "content-type": "application/json" } });
      };
      const client = new AntpathClient({ apiToken: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      await client.submitRun({
        provider: "anthropic",
        runtime: "managed",
        model: "claude-haiku-4-5",
        prompt: "test",
        secrets: { anthropic: { apiKey: "sk-ant-test-12345" } }
      });
      const submitBody = JSON.parse(requests[0].body);
      console.log(JSON.stringify({
        url: requests[0].url,
        provider: submitBody.provider,
        runtime: submitBody.runtime,
        hasSecrets: typeof submitBody.secrets === "object"
      }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "client-runtime-forward.mjs");
    expect(exitCode, stderr).toBe(0);
    const out = JSON.parse(stdout.trim()) as {
      url: string;
      provider: string;
      runtime: string;
      hasSecrets: boolean;
    };
    expect(out.url).toMatch(/example\.invalid/);
    expect(out.provider).toBe("anthropic");
    expect(out.runtime).toBe("managed");
    expect(out.hasSecrets).toBe(true);
  });

  it("AntpathClient.submitRun omits runtime from the wire when the caller doesn't supply it", async () => {
    const script = `
      const { AntpathClient } = await import("antpath");
      const requests = [];
      const fetchFake = async (url, init) => {
        let body = init?.body;
        if (typeof body !== "string" && body) {
          body = await new Response(body).text();
        }
        requests.push({ body });
        return new Response(JSON.stringify({
          id: "run_test_omit",
          workspaceId: "ws_t",
          status: "queued",
          createdAt: new Date().toISOString()
        }), { status: 202, headers: { "content-type": "application/json" } });
      };
      const client = new AntpathClient({ apiToken: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      await client.submitRun({
        provider: "anthropic",
        model: "claude-haiku-4-5",
        prompt: "hi",
        secrets: { anthropic: { apiKey: "sk-ant-test-12345" } }
      });
      const body = JSON.parse(requests[0].body);
      console.log(JSON.stringify({ hasRuntime: "runtime" in body }));
    `;
    const { exitCode, stdout } = await runChild(script, "client-runtime-omit.mjs");
    expect(exitCode).toBe(0);
    expect(JSON.parse(stdout.trim())).toEqual({ hasRuntime: false });
  });
});

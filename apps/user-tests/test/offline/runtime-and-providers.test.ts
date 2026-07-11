/**
 * Locks the clean public provider surface as it appears inside a clean
 * `npm i @aexhq/sdk` tempdir.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("managed-only provider surface (published package)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  });

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, file: string) {
    const path = join(install.installDir, file);
    writeFileSync(path, script);
    return runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 30_000 });
  }

  it("exports providers without runtime/region selector helpers", async () => {
    const script = `
      const mod = await import("@aexhq/sdk");
      console.log(JSON.stringify({
        providers: mod.PROVIDERS,
        defaultProvider: mod.DEFAULT_PROVIDER,
        hasRuntimeKinds: "RUNTIME_KINDS" in mod,
        hasRegions: "REGIONS" in mod || "Regions" in mod,
        hasSelectRuntime: "selectRuntime" in mod,
        hasRuntimeValidationError: "RuntimeValidationError" in mod
      }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "provider-exports.mjs");
    expect(exitCode, stderr).toBe(0);
    expect(JSON.parse(stdout.trim())).toEqual({
      providers: ["anthropic", "deepseek", "openai", "gemini", "mistral", "openrouter", "doubao", "doubao-cn"],
      defaultProvider: "anthropic",
      hasRuntimeKinds: false,
      hasRegions: false,
      hasSelectRuntime: false,
      hasRuntimeValidationError: false
    });
  });

  it("openSession rejects removed legacy options before any HTTP call", async () => {
    const script = `
      const { Aex } = await import("@aexhq/sdk");
      const calls = [];
      const fetchFake = async (...args) => { calls.push(args); return new Response("never", { status: 500 }); };
      const client = new Aex({ apiKey: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      // The one-shot/submit surface folded into sessions: these fields are the
      // legacy submit inputs that no longer exist on the session API. Each must
      // be rejected at the SDK boundary before any HTTP call.
      const fields = ["prompt", "secrets", "secretEnv", "runtimeSize", "timeout", "limits", "parentSessionId"];
      const results = [];
      for (const field of fields) {
        try {
          await client.sessions.create({
            provider: "anthropic",
            model: "claude-haiku-4-5",
            apiKeys: { anthropic: "sk-ant-test" },
            [field]: field === "secrets"
              ? { apiKeys: { anthropic: "sk-ant-test" } }
              : field === "limits"
                ? { maxConcurrentChildSessions: 2 }
                : field === "runtimeSize"
                  ? "shared-2x-8gb"
                  : "unsupported"
          });
          results.push({ field, caught: false });
        } catch (err) {
          results.push({ field, caught: true, message: err.message });
        }
      }
      console.log(JSON.stringify({ results, fetchCalls: calls.length }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "removed-options.mjs");
    expect(exitCode, stderr).toBe(0);
    const out = JSON.parse(stdout.trim()) as { results: Array<{ field: string; caught: boolean; message: string }>; fetchCalls: number };
    expect(out.fetchCalls).toBe(0);
    expect(out.results.map((result) => result.field)).toEqual(["prompt", "secrets", "secretEnv", "runtimeSize", "timeout", "limits", "parentSessionId"]);
    expect(out.results.every((result) => result.caught && result.message.includes("not a supported option"))).toBe(true);
  });

  it("sessions.create posts canonical top-level apiKeys secrets only", async () => {
    const script = `
      const { Aex } = await import("@aexhq/sdk");
      const requests = [];
      const fetchFake = async (url, init) => {
        let body = init?.body;
        if (typeof body !== "string" && body) body = await new Response(body).text();
        requests.push({ url: typeof url === "string" ? url : url.toString(), method: init?.method, body });
        return new Response(JSON.stringify({
          session: { id: "sess_test_user_e2e", status: "idle", acceptsMessages: true }
        }), { status: 201, headers: { "content-type": "application/json" } });
      };
      const client = new Aex({ apiKey: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      await client.sessions.create({
        provider: "anthropic",
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-ant-test-12345" }
      });
      const createBody = JSON.parse(requests[0].body);
      console.log(JSON.stringify({
        url: requests[0].url,
        method: requests[0].method,
        provider: createBody.provider,
        hasRuntimeSize: "runtimeSize" in createBody,
        hasRegion: "region" in createBody,
        secrets: createBody.secrets
      }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "client-canonical-create.mjs");
    expect(exitCode, stderr).toBe(0);
    expect(JSON.parse(stdout.trim())).toMatchObject({
      url: expect.stringContaining("/api/sessions"),
      method: "POST",
      provider: "anthropic",
      hasRuntimeSize: false,
      hasRegion: false,
      secrets: { apiKeys: { anthropic: "sk-ant-test-12345" } }
    });
  });
});

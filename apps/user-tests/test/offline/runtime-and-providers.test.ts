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

  it("exports providers + runtime-kind helpers but no region selectors", async () => {
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
      providers: ["anthropic", "deepseek", "openai", "gemini", "mistral", "openrouter", "doubao"],
      defaultProvider: "anthropic",
      hasRuntimeKinds: true,
      hasRegions: false,
      hasSelectRuntime: false,
      hasRuntimeValidationError: false
    });
  });

  it("keeps exact SDK factory provenance in the packed artifact", async () => {
    const script = `
      const { Skill, Tool } = await import("@aexhq/sdk");
      const messages = [];
      try {
        await Skill.fromContent("no frontmatter");
      } catch (error) {
        messages.push(error.message);
      }
      try {
        await Tool.fromFiles({
          name: "packed_tool",
          description: "Packed tool",
          input_schema: { type: "object", properties: {} },
          entry: "tool.ts",
          files: { "tool.ts": "export default async function () {}" }
        });
      } catch (error) {
        messages.push(error.message);
      }
      console.log(JSON.stringify(messages));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "sdk-factory-provenance.mjs");
    expect(exitCode, stderr).toBe(0);
    expect(JSON.parse(stdout.trim())).toEqual([
      "Skill.fromContent: a skill name is required — pass { name }, add a `name:` field to the SKILL.md YAML frontmatter, or (for fromDir) use a directory whose basename slugifies to a valid name",
      'Tool.fromFiles: entry must be a JS module (.js/.mjs/.cjs) that default-exports a function or { execute }; got "tool.ts"'
    ]);
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
                  ? "2cpu-8gb"
                  : "unsupported"
          });
          results.push({ field, caught: false });
        } catch (err) {
          const details = err && err.details && typeof err.details === "object" && !Array.isArray(err.details)
            ? err.details
            : null;
          results.push({
            field,
            caught: true,
            name: err && err.name ? String(err.name) : null,
            code: err && err.code ? String(err.code) : null,
            hasMessage: !!(err && err.message),
            detailsField: details && typeof details.field === "string" ? details.field : null
          });
        }
      }
      console.log(JSON.stringify({ results, fetchCalls: calls.length }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "removed-options.mjs");
    expect(exitCode, stderr).toBe(0);
    const out = JSON.parse(stdout.trim()) as {
      results: Array<{
        field: string;
        caught: boolean;
        name: string | null;
        code: string | null;
        hasMessage: boolean;
        detailsField: string | null;
      }>;
      fetchCalls: number;
    };
    expect(out.fetchCalls).toBe(0);
    expect(out.results.map((result) => result.field)).toEqual(["prompt", "secrets", "secretEnv", "runtimeSize", "timeout", "limits", "parentSessionId"]);
    for (const result of out.results) {
      expect(result).toMatchObject({
        caught: true,
        name: "SessionConfigValidationError",
        code: "SESSION_CONFIG_INVALID",
        hasMessage: true,
        detailsField: result.field
      });
    }
  });

  it("rejects invalid and unsupported session overrides with stable field details", async () => {
    const script = `
      const { Aex } = await import("@aexhq/sdk");
      const calls = [];
      const fetchFake = async (...args) => { calls.push(args); return new Response("never", { status: 500 }); };
      const client = new Aex({ apiKey: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      const base = {
        provider: "anthropic",
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-ant-test" }
      };
      const cases = [
        { label: "runtime", options: { runtime: "unsupported-size" }, field: "runtime" },
        { label: "timeout", options: { overrides: { timeout: "unsupported-duration" } }, field: "overrides.timeout" },
        { label: "spend", options: { overrides: { maxSpendUsd: -1 } }, field: "overrides.maxSpendUsd" },
        { label: "concurrency", options: { overrides: { maxConcurrentChildSessions: -1 } }, field: "overrides.maxConcurrentChildSessions" },
        { label: "depth", options: { overrides: { maxSubagentDepth: -1 } }, field: "overrides.maxSubagentDepth" }
      ];
      const results = [];
      for (const item of cases) {
        try {
          await client.sessions.create({ ...base, ...item.options });
          results.push({ label: item.label, field: item.field, caught: false });
        } catch (err) {
          const details = err && err.details && typeof err.details === "object" && !Array.isArray(err.details)
            ? err.details
            : null;
          results.push({
            label: item.label,
            field: item.field,
            caught: true,
            name: err && err.name ? String(err.name) : null,
            code: err && err.code ? String(err.code) : null,
            hasMessage: !!(err && err.message),
            detailsField: details && typeof details.field === "string" ? details.field : null
          });
        }
      }
      console.log(JSON.stringify({ results, fetchCalls: calls.length }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "session-config-errors.mjs");
    expect(exitCode, stderr).toBe(0);
    const out = JSON.parse(stdout.trim()) as {
      results: Array<{
        label: string;
        field: string;
        caught: boolean;
        name: string | null;
        code: string | null;
        hasMessage: boolean;
        detailsField: string | null;
      }>;
      fetchCalls: number;
    };
    expect(out.fetchCalls).toBe(0);
    expect(out.results.map(({ label, field }) => ({ label, field }))).toEqual([
      { label: "runtime", field: "runtime" },
      { label: "timeout", field: "overrides.timeout" },
      { label: "spend", field: "overrides.maxSpendUsd" },
      { label: "concurrency", field: "overrides.maxConcurrentChildSessions" },
      { label: "depth", field: "overrides.maxSubagentDepth" }
    ]);
    for (const result of out.results) {
      expect(result).toMatchObject({
        caught: true,
        name: "SessionConfigValidationError",
        code: "SESSION_CONFIG_INVALID",
        hasMessage: true,
        detailsField: result.field
      });
    }
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

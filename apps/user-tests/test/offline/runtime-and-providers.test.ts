/**
 * Locks the managed-gateway public model surface as it appears inside a clean
 * `npm i @aexhq/sdk` tempdir. Under managed keys there is no provider concept and
 * no closed model catalog: model ids are open `creator/model` gateway slugs.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

describe("managed-only model surface (published package)", () => {
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

  it("exports the gateway-slug model helpers but no provider or region selectors", async () => {
    const script = `
      const mod = await import("@aexhq/sdk");
      console.log(JSON.stringify({
        hasProviders: "PROVIDERS" in mod,
        hasDefaultProvider: "DEFAULT_PROVIDER" in mod,
        hasProvidersConst: "Providers" in mod,
        hasModels: "Models" in mod,
        hasSupportedModels: "SUPPORTED_MODELS" in mod,
        hasParseModelSlug: typeof mod.parseModelSlug === "function",
        hasIsModelSlug: typeof mod.isModelSlug === "function",
        hasModelSlugPattern: mod.MODEL_SLUG_PATTERN instanceof RegExp,
        hasRuntimeKinds: "RUNTIME_KINDS" in mod,
        hasRegions: "REGIONS" in mod || "Regions" in mod,
        hasStreamableShapes: "STREAMABLE_SHAPES" in mod || "isStreamableProvider" in mod
      }));
    `;
    const { exitCode, stdout, stderr } = await runChild(script, "provider-exports.mjs");
    expect(exitCode, stderr).toBe(0);
    expect(JSON.parse(stdout.trim())).toEqual({
      hasProviders: false,
      hasDefaultProvider: false,
      hasProvidersConst: false,
      hasModels: false,
      hasSupportedModels: false,
      hasParseModelSlug: true,
      hasIsModelSlug: true,
      hasModelSlugPattern: true,
      hasRuntimeKinds: true,
      hasRegions: false,
      hasStreamableShapes: false
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

  it("sessions.create rejects removed legacy + provider/BYOK options before any HTTP call", async () => {
    const script = `
      const { Aex } = await import("@aexhq/sdk");
      const calls = [];
      const fetchFake = async (...args) => { calls.push(args); return new Response("never", { status: 500 }); };
      const client = new Aex({ apiKey: "ant_test_t0k3n", baseUrl: "https://example.invalid", fetch: fetchFake });
      // Legacy submit inputs AND the now-removed managed-key fields (provider,
      // apiKeys) must all be rejected at the SDK boundary before any HTTP call.
      const fields = ["prompt", "secrets", "secretEnv", "runtimeSize", "timeout", "limits", "parentSessionId", "provider", "apiKeys"];
      const results = [];
      for (const field of fields) {
        try {
          await client.sessions.create({
            model: "anthropic/claude-haiku-4-5",
            [field]: field === "secrets"
              ? { mcpServers: [] }
              : field === "limits"
                ? { maxConcurrentChildSessions: 2 }
                : field === "runtimeSize"
                  ? "2cpu-8gb"
                  : field === "apiKeys"
                    ? { anthropic: "sk-ant-test" }
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
    expect(out.results.map((result) => result.field)).toEqual(["prompt", "secrets", "secretEnv", "runtimeSize", "timeout", "limits", "parentSessionId", "provider", "apiKeys"]);
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
      const base = { model: "anthropic/claude-haiku-4-5" };
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

  it("sessions.create posts a gateway slug with no provider selector and an empty secrets bundle", async () => {
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
      await client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
      const createBody = JSON.parse(requests[0].body);
      console.log(JSON.stringify({
        url: requests[0].url,
        method: requests[0].method,
        hasProvider: "provider" in createBody,
        model: createBody.submission && createBody.submission.model,
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
      hasProvider: false,
      model: "anthropic/claude-haiku-4-5",
      hasRuntimeSize: false,
      hasRegion: false,
      secrets: {}
    });
  });
});

import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, CredentialValidationError, SessionConfigValidationError } from "../../src/index.js";
import { unvalidatedCreateOptions } from "../helpers/unvalidated.js";

function recordingFetch(): { fetch: FetchLike; calls: string[] } {
  const calls: string[] = [];
  const f: FetchLike = async (input) => {
    calls.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
    return new Response(JSON.stringify({ session: { id: "session-1", status: "idle", acceptsMessages: true } }), {
      status: 201,
      headers: { "content-type": "application/json" }
    });
  };
  return { fetch: f, calls };
}

async function expectConfigError(
  operation: () => Promise<unknown>,
  field: string,
  calls: readonly string[]
): Promise<void> {
  const error = await operation().then(
    () => undefined,
    (caught: unknown) => caught
  );
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toMatchObject({
    name: "SessionConfigValidationError",
    code: "SESSION_CONFIG_INVALID"
  });
  expect((error as SessionConfigValidationError).details).toEqual({ field });
  expect((error as Error).message.trim().length).toBeGreaterThan(0);
  expect(calls).toHaveLength(0);
}

async function expectExactConfigError(
  operation: () => Promise<unknown>,
  field: string,
  message: string,
  calls: readonly string[]
): Promise<void> {
  const error = await operation().then(
    () => undefined,
    (caught: unknown) => caught
  );
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toMatchObject({
    name: "SessionConfigValidationError",
    code: "SESSION_CONFIG_INVALID",
    message,
    details: { field }
  });
  expect(calls).toHaveLength(0);
}

describe("aex.sessions.create — removed field validation", () => {
  it("rejects the legacy runtimeSize field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        runtimeSize: "shared-1x-4gb",
        model: "anthropic/claude-haiku-4-5",
      }))
    ).rejects.toThrow(/runtimeSize is not a supported option; use runtime/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy secretEnv field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        secretEnv: { SERPER_API_KEY: { ref: "serper" } }
      }))
    ).rejects.toThrow(/secretEnv is not a supported option; use environment\.secrets/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy nested secrets object without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        secrets: { apiKeys: { anthropic: "sk-x" } }
      }))
    ).rejects.toThrow(/secrets is not a supported option/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy parentSessionId field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        parentSessionId: "ses_parent"
      }))
    ).rejects.toThrow(/parentSessionId is not a supported option; subagent lineage is assigned by the platform/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects a message field without an HTTP call (was silently dropped: empty session, no turn)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        message: "hello there"
      }))
    ).rejects.toThrow(/message is not a supported option; sessions are created without a first message/);

    expect(rec.calls).toHaveLength(0);
  });
});

describe("aex.sessions.create — submit-boundary validation (Theme A, pre-network)", () => {
  it("rejects the first unknown top-level key with the exact fail-fast error", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expectExactConfigError(
      () => client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        futureFirst: true,
        futureSecond: true
      })),
      "futureFirst",
      "aex.sessions.create: futureFirst is not a supported option",
      rec.calls
    );
  });

  it("rejects an invalid runtime.size token without an HTTP call (F11)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        runtime: { size: "lite" }
      }))
    ).rejects.toThrow(SessionConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("rejects an invalid runtime.kind without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        runtime: { kind: "fargate" }
      }))
    ).rejects.toThrow(SessionConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("rejects an unknown runtime sub-key without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        runtime: { tier: "big" }
      }))
    ).rejects.toThrow(SessionConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("rejects a malformed overrides.timeout without an HTTP call (F12)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.sessions.create({
        model: "anthropic/claude-haiku-4-5",
        overrides: { timeout: "banana" }
      })
    ).rejects.toThrow(SessionConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("rejects an out-of-range timeout (below the 1m floor) without an HTTP call (F12)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expect(
      client.sessions.create({
        model: "anthropic/claude-haiku-4-5",
        overrides: { timeout: "10s" }
      })
    ).rejects.toThrow(SessionConfigValidationError);
    expect(rec.calls).toHaveLength(0);
  });

  it("accepts a valid runtime + timeout (regression: does not over-reject)", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
      runtime: { kind: "spot_container", size: "0.5cpu-4gb" },
      overrides: { timeout: "30m" }
    });
    // A valid config DOES reach the network (create call).
    expect(rec.calls.length).toBeGreaterThan(0);
  });

  it.each([
    ["runtime.size", { runtime: { size: "sensitive-invalid-runtime" } }, "runtime.size"],
    ["runtime.kind", { runtime: { kind: "sensitive-invalid-kind" } }, "runtime.kind"],
    ["timeout", { overrides: { timeout: "sensitive-invalid-timeout" } }, "overrides.timeout"],
    ["webhook", { webhook: { url: "sensitive-invalid-webhook" } }, "webhook.url"],
    ["maxSpendUsd", { overrides: { maxSpendUsd: -1 } }, "overrides.maxSpendUsd"],
    ["maxTurns", { overrides: { maxTurns: 0 } }, "overrides.maxTurns"]
  ] as const)("uses a stable field-only error for invalid %s", async (_label, extra, field) => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expectConfigError(
      () => client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        ...extra
      })),
      field,
      rec.calls
    );
  });

  it.each([
    ["overrides", { overrides: { maxTurn: 2 } }, "overrides.maxTurn"],
    ["assets", { assets: { skill: [] } }, "assets.skill"],
    [
      "asset item",
      {
        assets: {
          files: [{
            kind: "file",
            resourceId: `wres_${"1".repeat(32)}`,
            version: 1,
            assetId: "asset-1",
            contentHash: `sha256:${"a".repeat(64)}`,
            name: "input",
            mountpath: "/workspace/input"
          }]
        }
      },
      "assets.files[0].mountpath"
    ],
    ["fileCapture", { fileCapture: { maxFileByte: 10 } }, "fileCapture.maxFileByte"],
    ["environment", { environment: { variable: { MODE: "test" } } }, "environment.variable"],
    [
      "environment networking",
      { environment: { networking: { mode: "limited", allowedHost: ["example.test"] } } },
      "environment.networking.allowedHost"
    ],
    [
      "environment package",
      { environment: { packages: [{ name: "apt:curl", versions: "1" }] } },
      "environment.packages[0].versions"
    ],
    ["webhook", { webhook: { uri: "https://hooks.example.test/aex" } }, "webhook.uri"],
    [
      "responseFormat",
      { responseFormat: { kind: "json_schema", schema: {}, stric: true } },
      "responseFormat.stric"
    ],
    ["approvalGate", { approvalGate: { tool: ["bash"] } }, "approvalGate.tool"]
  ] as const)("rejects an unknown nested %s key before HTTP", async (_label, extra, field) => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expectConfigError(
      () => client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        ...extra
      })),
      field,
      rec.calls
    );
  });

  it.each([
    ["asset file", { assets: { files: [{ future: true }] } }, "assets.files[0].future"],
    ["asset skill", { assets: { skills: [{ future: true }] } }, "assets.skills[0].future"],
    ["asset tool", { assets: { tools: [{ future: true }] } }, "assets.tools[0].future"],
    ["asset instruction", { assets: { instructions: [{ future: true }] } }, "assets.instructions[0].future"],
    ["text response", { responseFormat: { kind: "text", schema: {} } }, "responseFormat.schema"]
  ] as const)("pins the exact unknown-key error for %s", async (_label, extra, field) => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    await expectExactConfigError(
      () => client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        ...extra
      })),
      field,
      `aex.sessions.create: ${field} is not a supported option`,
      rec.calls
    );
  });

  it("accepts published resource metadata and arbitrary keys in intentionally open maps", async () => {
    const rec = recordingFetch();
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });
    const common = {
      resourceId: `wres_${"1".repeat(32)}`,
      version: 1,
      assetId: `asset_${"a".repeat(64)}`,
      contentHash: `sha256:${"a".repeat(64)}`,
      createdAt: "2026-07-21T00:00:00.000Z",
      updatedAt: "2026-07-21T01:00:00.000Z",
      sizeBytes: 42,
      contentType: "application/zip"
    } as const;
    await client.sessions.create(unvalidatedCreateOptions({
      model: "anthropic/claude-haiku-4-5",
      metadata: { callerDefined: { nested: true } },
      assets: {
        files: [{ ...common, kind: "file", name: "input.txt", mountPath: "/workspace/input.txt" }],
        skills: [{ ...common, kind: "skill", name: "skill", description: "A skill" }],
        tools: [{
          ...common,
          kind: "tool",
          name: "tool",
          description: "A tool",
          input_schema: { type: "object", callerKeyword: true },
          entry: "index.js"
        }],
        instructions: [{ ...common, kind: "instruction", name: "guide" }]
      },
      environment: { variables: { CALLER_DEFINED: "yes" } },
      responseFormat: {
        kind: "json_schema",
        schema: { type: "object", properties: { callerDefined: { type: "string" } } }
      }
    }));
    expect(rec.calls).toHaveLength(1);
  });
});

describe("new Aex(...) — credential validation (F2)", () => {
  it("throws a typed CredentialValidationError (an AexError), not a bare Error, on a missing credential", () => {
    expect(() => new Aex({})).toThrow(CredentialValidationError);
    try {
      new Aex({});
    } catch (err) {
      // A caller catching the SDK error base must catch this too.
      expect(err).toBeInstanceOf(CredentialValidationError);
      expect((err as CredentialValidationError).code).toBe("CREDENTIAL_INVALID");
    }
  });
});

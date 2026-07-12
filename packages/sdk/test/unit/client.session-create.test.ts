import { describe, expect, it, vi } from "vitest";
import { Aex, McpServer, SessionConfigValidationError, type WorkspaceToolRef } from "../../src/index.js";

interface Call {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
  readonly body: Record<string, unknown>;
}

function harness() {
  const calls: Call[] = [];
  const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const body = typeof init?.body === "string" ? JSON.parse(init.body) as Record<string, unknown> : {};
    calls.push({ url, method: init?.method ?? "GET", headers: new Headers(init?.headers), body });
    return new Response(JSON.stringify({
      session: {
        id: "session_1",
        status: "idle",
        acceptsMessages: true,
        runtimeSize: body.runtimeSize ?? "shared-0.25x-1gb"
      }
    }), {
      status: 201,
      headers: { "content-type": "application/json" }
    });
  });
  return { client: new Aex({ apiKey: "token", baseUrl: "https://api.example.test", fetch }), calls };
}

const tool: WorkspaceToolRef = {
  kind: "tool",
  resourceId: `wres_${"1".repeat(32)}`,
  version: 2,
  assetId: `asset_${"a".repeat(64)}`,
  contentHash: `sha256:${"a".repeat(64)}`,
  name: "calendar",
  description: "Read a calendar",
  input_schema: { type: "object", properties: {} },
  entry: "index.js"
};

describe("aex.sessions.create", () => {
  it("groups pinned resources under assets and keeps builtin tools separate", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.create({
      model: "claude-haiku-4-5",
      system: "Be concise.",
      assets: { tools: [tool] },
      builtinTools: ["bash"],
      apiKeys: { anthropic: "sk-test" },
      idempotencyKey: "idem_1"
    });

    expect(session.id).toBe("session_1");
    const call = calls[0]!;
    expect(call.url).toBe("https://api.example.test/api/sessions");
    expect(call.method).toBe("POST");
    expect(call.headers.get("idempotency-key")).toBe("idem_1");
    const submission = call.body.submission as Record<string, unknown>;
    expect(submission.assets).toEqual({ files: [], skills: [], tools: [tool], instructions: [] });
    expect(submission.builtinTools).toEqual(["bash"]);
    expect(submission).not.toHaveProperty("tools");
    expect(call.body.retention).toEqual({ idleTtl: "3m" });
  });

  it("moves MCP credentials to the secret channel", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.create({
      model: "claude-haiku-4-5",
      mcpServers: [McpServer.remote({
        name: "github",
        url: "https://mcp.example.test/github",
        headers: { Authorization: "Bearer secret" }
      })],
      apiKeys: { anthropic: "sk-test" }
    });
    const body = calls[0]!.body;
    expect((body.submission as Record<string, unknown>).mcpServers).toEqual([
      { name: "github", url: "https://mcp.example.test/github" }
    ]);
    expect((body.secrets as Record<string, unknown>).mcpServers).toEqual([
      { name: "github", url: "https://mcp.example.test/github", headers: { Authorization: "Bearer secret" } }
    ]);
  });

  it("serializes file capture, runtime, limits, timeout, and webhook", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.create({
      model: "claude-haiku-4-5",
      runtime: "shared-0.5x-4gb",
      overrides: { timeout: "10m", maxSpendUsd: 3, maxTurns: 20, idleTtl: "5m" },
      fileCapture: { allowedDirs: ["/workspace/out"], maxFiles: 20 },
      webhook: { url: "https://hooks.example.test/aex" },
      apiKeys: { anthropic: "sk-test" }
    });
    expect(calls[0]!.body).toMatchObject({
      runtimeSize: "shared-0.5x-4gb",
      timeout: "10m",
      limits: { maxSpendUsd: 3, maxTurns: 20 },
      retention: { idleTtl: "5m" },
      webhook: { url: "https://hooks.example.test/aex" },
      submission: { fileCapture: { allowedDirs: ["/workspace/out"], maxFiles: 20 } }
    });
    expect(session.record.runtime).toBe("shared-0.5x-4gb");
    expect(session.record).not.toHaveProperty("runtimeSize");
  });

  it("requires the selected provider key before network access", async () => {
    const { client, calls } = harness();
    const error = await client.sessions.create({ model: "claude-haiku-4-5" }).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(SessionConfigValidationError);
    expect(error).toMatchObject({ name: "SessionConfigValidationError", code: "SESSION_CONFIG_INVALID" });
    expect((error as SessionConfigValidationError).details).toEqual({ field: "apiKeys.anthropic" });
    expect((error as Error).message.trim().length).toBeGreaterThan(0);
    expect(calls).toHaveLength(0);
  });

  it("derives deepseek from its model", async () => {
    const { client, calls } = harness();
    await client.sessions.create({ model: "deepseek-v4-flash", apiKeys: { deepseek: "sk-test" } });
    expect(calls[0]!.body.provider).toBe("deepseek");
  });

  it("rejects a provider that cannot serve the selected model", async () => {
    const { client, calls } = harness();
    const error = await client.sessions.create({
      model: "gpt-4.1",
      provider: "anthropic",
      apiKeys: { anthropic: "sk-test" }
    }).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(SessionConfigValidationError);
    expect((error as SessionConfigValidationError).details).toEqual({ field: "provider" });
    expect((error as Error).message.trim().length).toBeGreaterThan(0);
    expect(calls).toHaveLength(0);
  });

  for (const field of ["tools", "skills", "files", "agentsMd"] as const) {
    it(`rejects the legacy ${field} field rather than ignoring it`, async () => {
      const { client, calls } = harness();
      await expect(client.sessions.create({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-test" },
        [field]: []
      } as never)).rejects.toThrow(new RegExp(`${field} is not a supported option`));
      expect(calls).toHaveLength(0);
    });
  }
});

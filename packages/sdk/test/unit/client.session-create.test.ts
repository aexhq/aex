import { describe, expect, it, mock } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, McpServer, SessionConfigValidationError, type WorkspaceToolRef } from "../../src/index.js";
import { unvalidatedCreateOptions } from "../helpers/unvalidated.js";

interface Call {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
  readonly body: Record<string, unknown>;
}

function harness() {
  const calls: Call[] = [];
  const fetch: FetchLike = mock(async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const body = typeof init?.body === "string" ? JSON.parse(init.body) as Record<string, unknown> : {};
    calls.push({ url, method: init?.method ?? "GET", headers: new Headers(init?.headers), body });
    return new Response(JSON.stringify({
      session: {
        id: "session_1",
        status: "idle",
        acceptsMessages: true,
        runtimeSize: body.runtimeSize ?? "0.25cpu-1gb",
        ...(body.runtimeKind ? { runtimeKind: body.runtimeKind } : {})
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
      model: "anthropic/claude-haiku-4-5",
      system: "Be concise.",
      assets: { tools: [tool] },
      builtinTools: ["bash"]
    });

    expect(session.id).toBe("session_1");
    const call = calls[0]!;
    expect(call.url).toBe("https://api.example.test/api/sessions");
    expect(call.method).toBe("POST");
    expect(call.headers.get("idempotency-key")).toMatch(/^idem_[0-9a-f]{32}$/);
    const submission = call.body.submission as Record<string, unknown>;
    expect(submission.assets).toEqual({ files: [], skills: [], tools: [tool], instructions: [] });
    expect(submission.builtinTools).toEqual(["bash"]);
    expect(submission).not.toHaveProperty("tools");
    expect(call.body.retention).toEqual({ idleTtl: "3m" });
  });

  it("moves MCP credentials to the secret channel", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
      mcpServers: [McpServer.remote({
        name: "github",
        url: "https://mcp.example.test/github",
        headers: { Authorization: "Bearer secret" }
      })],
    });
    const body = calls[0]!.body;
    expect((body.submission as Record<string, unknown>).mcpServers).toEqual([
      { name: "github", url: "https://mcp.example.test/github" }
    ]);
    expect((body.secrets as Record<string, unknown>).mcpServers).toEqual([
      { name: "github", url: "https://mcp.example.test/github", headers: { Authorization: "Bearer secret" } }
    ]);
  });

  it("serializes file capture, runtime.size, limits, timeout, and webhook", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
      runtime: { size: "0.5cpu-4gb" },
      overrides: { timeout: "10m", maxSpendUsd: 3, maxTurns: 20, idleTtl: "5m" },
      fileCapture: { allowedDirs: ["/workspace/out"], maxFiles: 20 },
      webhook: { url: "https://hooks.example.test/aex" },
    });
    expect(calls[0]!.body).toMatchObject({
      runtimeSize: "0.5cpu-4gb",
      timeout: "10m",
      limits: { maxSpendUsd: 3, maxTurns: 20 },
      retention: { idleTtl: "5m" },
      submission: { fileCapture: { allowedDirs: ["/workspace/out"], maxFiles: 20 } }
    });
    // The flat wire fields fold into the grouped `runtime: { kind, size }`.
    expect(session.record.runtime).toEqual({ size: "0.5cpu-4gb" });
    expect(session.record).not.toHaveProperty("runtimeSize");
  });

  it("forwards runtime.kind + runtime.size to the wire and exposes both on the record", async () => {
    const { client, calls } = harness();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
      runtime: { kind: "spot_container", size: "2cpu-8gb" },
    });
    expect(calls[0]!.body).toMatchObject({
      runtimeSize: "2cpu-8gb",
      runtimeKind: "spot_container"
    });
    expect(session.record.runtime).toEqual({ kind: "spot_container", size: "2cpu-8gb" });
  });

  it("omits the runtime wire fields when not selected (spot_container default applied downstream)", async () => {
    const { client, calls } = harness();
    await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    expect(calls[0]!.body).not.toHaveProperty("runtimeKind");
    expect(calls[0]!.body).not.toHaveProperty("runtime");
  });

  it("creates a session from a model slug alone — no provider key needed (managed keys)", async () => {
    const { client, calls } = harness();
    await client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    expect(calls).toHaveLength(1);
    expect(calls[0]!.body).not.toHaveProperty("provider");
    expect(calls[0]!.body.secrets).toEqual({});
  });

  it("puts the model slug on the wire submission with no provider selector", async () => {
    const { client, calls } = harness();
    await client.sessions.create({ model: "deepseek/deepseek-v4-flash" });
    expect(calls[0]!.body).not.toHaveProperty("provider");
    expect((calls[0]!.body.submission as { model: string }).model).toBe("deepseek/deepseek-v4-flash");
  });

  for (const field of ["tools", "skills", "files", "agentsMd"] as const) {
    it(`rejects the legacy ${field} field rather than ignoring it`, async () => {
      const { client, calls } = harness();
      await expect(client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        [field]: []
      }))).rejects.toThrow(new RegExp(`${field} is not a supported option`));
      expect(calls).toHaveLength(0);
    });
  }
});

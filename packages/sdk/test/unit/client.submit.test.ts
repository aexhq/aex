import { describe, expect, it, vi } from "vitest";
import { AgentsMd, AntpathClient, McpServer, Skill } from "../../src/index.js";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

function makeStubFetch(): { fetch: typeof fetch; calls: CapturedRequest[] } {
  const calls: CapturedRequest[] = [];
  const stub: typeof fetch = vi.fn(async (input, init) => {
    let url: string;
    if (typeof input === "string") {
      url = input;
    } else if (input instanceof URL) {
      url = input.toString();
    } else {
      url = (input as Request).url;
    }
    const method = (init?.method ?? "GET").toString();
    const headers: Record<string, string> = {};
    const initHeaders = init?.headers;
    if (initHeaders) {
      if (initHeaders instanceof Headers) {
        for (const [k, v] of initHeaders.entries()) headers[k] = v;
      } else if (Array.isArray(initHeaders)) {
        for (const [k, v] of initHeaders) headers[k] = v;
      } else {
        Object.assign(headers, initHeaders);
      }
    }
    let body: unknown = init?.body;
    if (typeof body === "string") {
      try {
        body = JSON.parse(body);
      } catch {
        // leave as string
      }
    }
    calls.push({ url, method, headers, body });
    if (url.endsWith("/assets/upload")) {
      const lc: Record<string, string> = {};
      for (const [k, v] of Object.entries(headers)) lc[k.toLowerCase()] = v;
      const hash = lc["x-asset-hash"] ?? `sha256:${"a".repeat(64)}`;
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return new Response(
        JSON.stringify({
          ok: true,
          exists: false,
          path: `assets/11111111-1111-4111-8111-111111111111/${hex}`,
          hash,
          sizeBytes: Number(lc["content-length"] ?? "0")
        }),
        { status: 201, headers: { "content-type": "application/json" } }
      );
    }
    return new Response(
      JSON.stringify({ id: "run_test", status: "queued" }),
      { status: 200, headers: { "content-type": "application/json" } }
    );
  });
  return { fetch: stub, calls };
}

describe("AntpathClient.submitRun (flat surface, wire shape)", () => {
  it("builds the flat submission and routes MCP headers into secrets", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AntpathClient({
      apiToken: "tkn_test",
      baseUrl: "https://example.test",
      fetch
    });

    const runId = await client.submitRun({
      model: "claude-sonnet-4-5-20250929",
      system: "You are tidy.",
      prompt: "do work",
      skills: [
        Skill.provider({ vendor: "anthropic", skillId: "pdf", version: "v1" })
      ],
      mcpServers: [
        McpServer.remote({
          name: "github",
          url: "https://mcp.example/github",
          headers: { Authorization: "Bearer t" }
        }),
        McpServer.remote({ name: "noauth", url: "https://mcp.example/noauth" })
      ],
      cleanup: { session: "delete" },
      secrets: { anthropic: { apiKey: "sk-test" } },
      idempotencyKey: "idem_unit"
    });

    expect(runId).toBe("run_test");
    expect(calls).toHaveLength(1);
    const call = calls[0]!;
    expect(call.method).toBe("POST");
    expect(call.url).toBe("https://example.test/api/runs");
    expect(call.headers["authorization"] ?? call.headers["Authorization"]).toBe("Bearer tkn_test");

    const body = call.body as Record<string, unknown>;
    expect(body.idempotencyKey).toBe("idem_unit");
    expect(body.cleanup).toEqual({ session: "delete" });

    const submission = body.submission as Record<string, unknown>;
    expect(submission.model).toBe("claude-sonnet-4-5-20250929");
    expect(submission.system).toBe("You are tidy.");
    expect(submission.prompt).toEqual(["do work"]);
    expect(submission.skills).toEqual([
      { kind: "provider", vendor: "anthropic", skillId: "pdf", version: "v1" }
    ]);
    expect(submission.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github" },
      { name: "noauth", url: "https://mcp.example/noauth" }
    ]);

    const secrets = body.secrets as Record<string, unknown>;
    expect(secrets.anthropic).toEqual({ apiKey: "sk-test" });
    expect(secrets.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github", headers: { Authorization: "Bearer t" } }
    ]);
  });

  it("requires anthropic.apiKey", async () => {
    const { fetch } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        secrets: { anthropic: { apiKey: "" } }
      })
    ).rejects.toThrow(/secrets\.anthropic\.apiKey/);
  });

  it("submits DeepSeek provider runs with DeepSeek secrets only", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submitRun({
      provider: "deepseek",
      model: "deepseek-chat",
      prompt: "p",
      secrets: { deepseek: { apiKey: "sk-ds-test" } },
      idempotencyKey: "idem-deepseek"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.provider).toBe("deepseek");
    expect(body.secrets).toEqual({ deepseek: { apiKey: "sk-ds-test" } });
  });

  it("rejects cross-provider secrets before submitting", async () => {
    const { fetch } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        provider: "deepseek",
        model: "m",
        prompt: "p",
        secrets: { anthropic: { apiKey: "sk-ant-test" } }
      })
    ).rejects.toThrow(/secrets\.deepseek\.apiKey/);
  });

  it("rejects empty prompts", async () => {
    const { fetch } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "",
        secrets: { anthropic: { apiKey: "k" } }
      })
    ).rejects.toThrow(/prompt/);
  });

  it("accepts prompt arrays", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submitRun({
      model: "m",
      prompt: ["one", "two"],
      secrets: { anthropic: { apiKey: "k" } },
      idempotencyKey: "i"
    });
    const submission = (calls[0]!.body as Record<string, unknown>).submission as Record<string, unknown>;
    expect(submission.prompt).toEqual(["one", "two"]);
  });

  it("rejects mismatched MCP urls between run request server and explicit secret", async () => {
    const { fetch } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        mcpServers: [
          McpServer.remote({
            name: "github",
            url: "https://a.example/github",
            headers: { Authorization: "Bearer t" }
          })
        ],
        secrets: {
          anthropic: { apiKey: "k" },
          mcpServers: [
            { name: "github", url: "https://b.example/github", headers: { Authorization: "Bearer u" } }
          ]
        }
      })
    ).rejects.toThrow(/conflicts/);
  });

  it("rejects non-Skill entries with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        skills: [{ kind: "workspace", id: "skl_x" } as unknown as Skill],
        secrets: { anthropic: { apiKey: "k" } }
      })
    ).rejects.toThrow(/skills\[0\] must be a Skill instance/);
  });

  it("uploads an inline AgentsMd to /assets/upload then submits a JSON body with a kind:'r2' ref", async () => {
    // Phase B: inline AgentsMd is materialized to R2 before submit.
    // The run body is JSON; the agentsMd entry becomes a r2 ref.
    const { fetch, calls } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await AgentsMd.fromContent("# Rules\nBe helpful.\n", { name: "rules" });
    await client.submitRun({
      model: "m",
      prompt: "p",
      agentsMd: [draft],
      secrets: { anthropic: { apiKey: "k" } },
      idempotencyKey: "idem-r2-agentsmd"
    });
    const uploadCalls = calls.filter((c) => c.url.endsWith("/assets/upload"));
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(uploadCalls).toHaveLength(1);
    expect(runCalls).toHaveLength(1);
    const submission = (runCalls[0]!.body as { submission: { agentsMd: ReadonlyArray<{ kind: string; name?: string }> } })
      .submission;
    expect(submission.agentsMd[0]).toMatchObject({ kind: "r2", name: "rules" });
  });

  it("rejects non-AgentsMd entries in the agentsMd array with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        agentsMd: [{ kind: "workspace_agentsmd", id: "amd_x" } as unknown as AgentsMd],
        secrets: { anthropic: { apiKey: "k" } }
      })
    ).rejects.toThrow(/agentsMd\[0\] must be an AgentsMd instance/);
  });
});

import { describe, expect, it, vi } from "vitest";
import { AgentsMd, AntpathClient, File as AntpathFile, McpServer, Skill } from "../../src/index.js";

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

  it("materializes draft Skill, AgentsMd, and File refs to /assets/upload before submitting", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const skill = await Skill.fromFiles({
      name: "rules",
      files: {
        "SKILL.md": "# rules\n\nKeep responses short.\n"
      }
    });
    const agentsMd = await AgentsMd.fromContent("# Session rules\nBe precise.\n", { name: "session-rules" });
    const file = await AntpathFile.fromBytes({
      name: "dataset",
      bytes: new TextEncoder().encode("id,value\n1,alpha\n"),
      mountPath: "/workspace/input/dataset.csv"
    });
    const skillHash = skill.ref.kind === "draft" ? skill.ref.contentHash : "";
    const agentsMdHash = agentsMd.ref.kind === "draft" ? agentsMd.ref.contentHash : "";
    const fileHash = file.ref.kind === "draft" ? file.ref.contentHash : "";

    await client.submitRun({
      model: "m",
      prompt: "p",
      skills: [skill],
      agentsMd: [agentsMd],
      files: [file],
      secrets: { anthropic: { apiKey: "k" } },
      idempotencyKey: "idem-assets"
    });

    expect(calls.map((c) => c.url)).toEqual([
      "https://x/assets/upload",
      "https://x/assets/upload",
      "https://x/assets/upload",
      "https://x/api/runs"
    ]);
    const uploadCalls = calls.slice(0, 3);
    const expectedHashes = [skillHash, agentsMdHash, fileHash];
    for (let i = 0; i < uploadCalls.length; i++) {
      const call = uploadCalls[i]!;
      const bytes = call.body as Uint8Array;
      expect(call.method).toBe("POST");
      expect(bytes).toBeInstanceOf(Uint8Array);
      expect(call.headers["content-type"]).toBe("application/zip");
      expect(call.headers["content-length"]).toBe(String(bytes.byteLength));
      expect(call.headers["x-asset-hash"]).toBe(expectedHashes[i]);
      expect(call.headers.authorization).toBe("Bearer tkn");
    }

    const submission = (calls[3]!.body as { submission: Record<string, unknown> }).submission;
    const r2Ref = (name: string, hash: string, call: CapturedRequest, extra: Record<string, string> = {}) => {
      const bytes = call.body as Uint8Array;
      return {
        kind: "r2",
        path: `assets/11111111-1111-4111-8111-111111111111/${hash.slice("sha256:".length)}`,
        hash,
        sizeBytes: bytes.byteLength,
        name,
        ...extra
      };
    };
    expect(submission.skills).toEqual([r2Ref("rules", skillHash, uploadCalls[0]!)]);
    expect(submission.agentsMd).toEqual([r2Ref("session-rules", agentsMdHash, uploadCalls[1]!)]);
    expect(submission.files).toEqual([
      r2Ref("dataset", fileHash, uploadCalls[2]!, { mountPath: "/workspace/input/dataset.csv" })
    ]);
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

describe("AntpathClient.deleteWorkspaceAsset", () => {
  it("DELETEs /assets/:hash and normalizes sha256-prefixed hashes", async () => {
    const calls: CapturedRequest[] = [];
    const stub: typeof fetch = vi.fn(async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const headers: Record<string, string> = {};
      if (init?.headers instanceof Headers) {
        for (const [k, v] of init.headers.entries()) headers[k] = v;
      } else if (Array.isArray(init?.headers)) {
        for (const [k, v] of init.headers) headers[k] = v;
      } else if (init?.headers) {
        Object.assign(headers, init.headers);
      }
      calls.push({ url, method: (init?.method ?? "GET").toString(), headers, body: init?.body });
      return new Response(null, { status: 204 });
    });
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch: stub });
    const hex = "b".repeat(64);

    await client.deleteWorkspaceAsset(`sha256:${hex}`);
    await client.deleteWorkspaceAsset(hex);

    expect(calls.map((c) => c.url)).toEqual([
      `https://x/assets/${hex}`,
      `https://x/assets/${hex}`
    ]);
    expect(calls.map((c) => c.method)).toEqual(["DELETE", "DELETE"]);
    expect(calls.map((c) => c.headers.authorization)).toEqual(["Bearer tkn", "Bearer tkn"]);
  });
});

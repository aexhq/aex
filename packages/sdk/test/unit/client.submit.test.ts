import { describe, expect, it, vi } from "vitest";
import { AgentsMd, AexClient, File as AexFile, McpServer, Skill } from "../../src/index.js";

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
    if (url.endsWith("/api/runs/run_test/bootstrap/status")) {
      return new Response(
        JSON.stringify({
          status: "ready",
          uploadBaseUrl: "https://bootstrap.example/run_test/bootstrap",
          routingHeaders: { "x-aex-route": "machine-1" }
        }),
        { status: 200, headers: { "content-type": "application/json" } }
      );
    }
    if (url.startsWith("https://bootstrap.example/run_test/bootstrap/inputs/")) {
      return new Response("", { status: 200 });
    }
    if (url === "https://bootstrap.example/run_test/bootstrap/commit") {
      return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
    }
    if (url === "https://bootstrap.example/run_test/bootstrap/abort") {
      return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
    }
    // Legacy direct-to-storage workspace asset flow.
    if (url.endsWith("/assets/presign")) {
      const reqBody = (body ?? {}) as { hash?: string; sizeBytes?: number };
      const hash = reqBody.hash ?? `sha256:${"a".repeat(64)}`;
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return new Response(
        JSON.stringify({
          ok: true,
          exists: false,
          assetId: `asset_${hex}`,
          contentHash: hash,
          uploadUrl: `https://acct.r2.cloudflarestorage.com/bucket/assets/ws/${hex}?X-Amz-Signature=sig`,
          requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" },
          expiresInSeconds: 300
        }),
        { status: 201, headers: { "content-type": "application/json" } }
      );
    }
    if (url.includes("r2.cloudflarestorage.com")) {
      return new Response("", { status: 200 }); // object storage accepts the direct PUT
    }
    if (url.endsWith("/assets/finalize")) {
      const reqBody = (body ?? {}) as { hash?: string; sizeBytes?: number };
      const hash = reqBody.hash ?? `sha256:${"a".repeat(64)}`;
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return new Response(
        JSON.stringify({ ok: true, exists: false, assetId: `asset_${hex}`, contentHash: hash, sizeBytes: reqBody.sizeBytes ?? 0 }),
        { status: 200, headers: { "content-type": "application/json" } }
      );
    }
    if (url.endsWith("/assets")) {
      const lc: Record<string, string> = {};
      for (const [k, v] of Object.entries(headers)) lc[k.toLowerCase()] = v;
      const hash = lc["x-asset-hash"] ?? `sha256:${"a".repeat(64)}`;
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return new Response(
        JSON.stringify({
          ok: true,
          exists: false,
          assetId: `asset_${hex}`,
          contentHash: hash,
          sizeBytes: Number(lc["content-length"] ?? "0")
        }),
        { status: 201, headers: { "content-type": "application/json" } }
      );
    }
    const runBody = body as { directInputs?: unknown[] } | undefined;
    return new Response(
      JSON.stringify({
        id: "run_test",
        status: "queued",
        ...(Array.isArray(runBody?.directInputs) && runBody.directInputs.length > 0
          ? {
              bootstrapStatusUrl: "https://example.test/api/runs/run_test/bootstrap/status",
              bootstrapToken: "boot_test",
              bootstrapExpiresAt: new Date(Date.now() + 60_000).toISOString()
            }
          : {})
      }),
      { status: 200, headers: { "content-type": "application/json" } }
    );
  });
  return { fetch: stub, calls };
}

describe("AexClient.submitRun (flat surface, wire shape)", () => {
  it("builds the flat submission and routes MCP headers into secrets", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AexClient({
      apiToken: "tkn_test",
      baseUrl: "https://example.test",
      fetch
    });

    const runId = await client.submitRun({
      model: "claude-sonnet-4-5-20250929",
      system: "You are tidy.",
      prompt: "do work",
      outputMode: "stream",
      mcpServers: [
        McpServer.remote({
          name: "github",
          url: "https://mcp.example/github",
          headers: { Authorization: "Bearer t" }
        }),
        McpServer.remote({ name: "noauth", url: "https://mcp.example/noauth" })
      ],
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
    expect("cleanup" in body).toBe(false);

    const submission = body.submission as Record<string, unknown>;
    expect(submission.model).toBe("claude-sonnet-4-5-20250929");
    expect(submission.outputMode).toBe("stream");
    expect(submission.system).toBe("You are tidy.");
    expect(submission.prompt).toEqual(["do work"]);
    expect(submission.skills).toEqual([]);
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        skills: [{ kind: "workspace", id: "skl_x" } as unknown as Skill],
        secrets: { anthropic: { apiKey: "k" } }
      })
    ).rejects.toThrow(/skills\[0\] must be a Skill instance/);
  });

  it("submits an inline AgentsMd as a direct bootstrap input without /assets calls", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await AgentsMd.fromContent("# Rules\nBe helpful.\n", { name: "rules" });
    await client.submitRun({
      model: "m",
      prompt: "p",
      agentsMd: [draft],
      secrets: { anthropic: { apiKey: "k" } },
      idempotencyKey: "idem-asset-agentsmd"
    });
    expect(calls.filter((c) => c.url.includes("/assets"))).toHaveLength(0);
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(runCalls).toHaveLength(1);
    const body = runCalls[0]!.body as {
      bootstrapMode: string;
      directInputs: ReadonlyArray<{ role: string; inputId: string; name: string; sha256: string; sizeBytes: number }>;
      submission: { agentsMd: ReadonlyArray<{ kind: string; name?: string; assetId?: string }> };
    };
    expect(body.bootstrapMode).toBe("direct");
    expect(body.directInputs).toHaveLength(1);
    expect(body.directInputs[0]).toMatchObject({ role: "agentsMd", name: "rules" });
    expect("bytes" in body.directInputs[0]!).toBe(false);
    expect(body.submission.agentsMd[0]).toMatchObject({ kind: "asset", name: "rules" });
    expect(calls.filter((c) => c.url.endsWith("/bootstrap/status"))).toHaveLength(1);
    expect(calls.filter((c) => c.url.includes("bootstrap.example") && c.method === "PUT")).toHaveLength(1);
    expect(calls.filter((c) => c.url.endsWith("/bootstrap/commit"))).toHaveLength(1);
  });

  it("uploads draft Skill, AgentsMd, and File refs to the bootstrap target before resolving", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const skill = await Skill.fromFiles({
      name: "rules",
      files: {
        "SKILL.md": "# rules\n\nKeep responses short.\n"
      }
    });
    const agentsMd = await AgentsMd.fromContent("# Session rules\nBe precise.\n", { name: "session-rules" });
    const file = await AexFile.fromBytes({
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

    const assetCalls = calls.filter((c) => c.url.includes("/assets"));
    const bootstrapPutCalls = calls.filter((c) => c.url.includes("bootstrap.example") && c.method === "PUT");
    const commitCalls = calls.filter((c) => c.url.endsWith("/bootstrap/commit"));
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(assetCalls).toHaveLength(0);
    expect(bootstrapPutCalls).toHaveLength(3);
    expect(commitCalls).toHaveLength(1);
    expect(runCalls).toHaveLength(1);

    const expectedHashes = [skillHash, agentsMdHash, fileHash];
    const runBody = runCalls[0]!.body as {
      bootstrapMode: string;
      directInputs: ReadonlyArray<{ role: string; name: string; sha256: string; sizeBytes: number; mountPath?: string }>;
      submission: Record<string, unknown>;
    };
    expect(runBody.bootstrapMode).toBe("direct");
    expect(runBody.directInputs.map((input) => input.sha256)).toEqual(expectedHashes);
    expect(runBody.directInputs.map((input) => input.role)).toEqual(["skill", "agentsMd", "file"]);
    expect(runBody.directInputs[2]).toMatchObject({ mountPath: "/workspace/input/dataset.csv" });
    for (const put of bootstrapPutCalls) {
      expect(put.method).toBe("PUT");
      expect(put.body).toBeInstanceOf(Uint8Array);
      expect(put.headers.authorization).toBe("Bearer boot_test");
      expect(put.headers["x-aex-route"]).toBe("machine-1");
      expect(put.headers["x-aex-input-sha256"]).toMatch(/^sha256:[0-9a-f]{64}$/);
    }

    const submission = runBody.submission;
    const assetRef = (name: string, hash: string, extra: Record<string, string> = {}) => {
      return {
        kind: "asset",
        assetId: `asset_${hash.slice("sha256:".length)}`,
        name,
        ...extra
      };
    };
    expect(submission.skills).toEqual([assetRef("rules", skillHash)]);
    expect(submission.agentsMd).toEqual([assetRef("session-rules", agentsMdHash)]);
    expect(submission.files).toEqual([
      assetRef("dataset", fileHash, { mountPath: "/workspace/input/dataset.csv" })
    ]);
  });

  it("best-effort aborts bootstrap when a direct input upload fails", async () => {
    const calls: CapturedRequest[] = [];
    const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push({ url, method: (init?.method ?? "GET").toString(), headers: {}, body: init?.body });
      if (url.endsWith("/api/runs")) {
        return new Response(
          JSON.stringify({
            id: "run_test",
            status: "queued",
            bootstrapStatusUrl: "https://x/api/runs/run_test/bootstrap/status",
            bootstrapToken: "boot_fail",
            bootstrapExpiresAt: new Date(Date.now() + 60_000).toISOString()
          }),
          { status: 202, headers: { "content-type": "application/json" } }
        );
      }
      if (url.endsWith("/bootstrap/status")) {
        return new Response(
          JSON.stringify({ status: "ready", uploadBaseUrl: "https://bootstrap.example/run_test/bootstrap" }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
      if (url.includes("/bootstrap/inputs/")) {
        return new Response(JSON.stringify({ ok: false, code: "bad_upload" }), {
          status: 500,
          headers: { "content-type": "application/json" }
        });
      }
      if (url.endsWith("/bootstrap/abort")) {
        return new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "content-type": "application/json" }
        });
      }
      return new Response(JSON.stringify({ ok: false, code: "asset_finalize_failed" }), {
        status: 500,
        headers: { "content-type": "application/json" }
      });
    });
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const skill = await Skill.fromFiles({
      name: "rules",
      files: { "SKILL.md": "# rules\n" }
    });

    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        skills: [skill],
        secrets: { anthropic: { apiKey: "k" } }
      })
    ).rejects.toThrow();

    expect(calls.map((c) => c.url)).toEqual([
      "https://x/api/runs",
      "https://x/api/runs/run_test/bootstrap/status",
      expect.stringContaining("https://bootstrap.example/run_test/bootstrap/inputs/"),
      "https://x/api/runs/run_test/bootstrap/abort"
    ]);
  });

  it("rejects non-AgentsMd entries in the agentsMd array with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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

describe("AexClient.deleteWorkspaceAsset", () => {
  it("DELETEs /assets/:assetId and normalizes sha256-prefixed hashes", async () => {
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
    const client = new AexClient({ apiToken: "tkn", baseUrl: "https://x", fetch: stub });
    const hex = "b".repeat(64);

    await client.deleteWorkspaceAsset(`sha256:${hex}`);
    await client.deleteWorkspaceAsset(`asset_${hex}`);

    expect(calls.map((c) => c.url)).toEqual([
      `https://x/assets/asset_${hex}`,
      `https://x/assets/asset_${hex}`
    ]);
    expect(calls.map((c) => c.method)).toEqual(["DELETE", "DELETE"]);
    expect(calls.map((c) => c.headers.authorization)).toEqual(["Bearer tkn", "Bearer tkn"]);
  });
});

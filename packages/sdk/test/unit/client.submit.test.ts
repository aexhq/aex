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
    // Direct-to-storage upload flow: presign → PUT (object storage) → finalize.
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

  it("uploads an inline AgentsMd via presign→object storage PUT→finalize then submits a kind:'asset' ref", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await AgentsMd.fromContent("# Rules\nBe helpful.\n", { name: "rules" });
    await client.submitRun({
      model: "m",
      prompt: "p",
      agentsMd: [draft],
      secrets: { anthropic: { apiKey: "k" } },
      idempotencyKey: "idem-asset-agentsmd"
    });
    // Direct-to-storage flow: presign (control plane) → PUT (object storage, no worker bytes) → finalize.
    expect(calls.filter((c) => c.url.endsWith("/assets/presign"))).toHaveLength(1);
    expect(calls.filter((c) => c.url.includes("r2.cloudflarestorage.com"))).toHaveLength(1);
    expect(calls.filter((c) => c.url.endsWith("/assets/finalize"))).toHaveLength(1);
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(runCalls).toHaveLength(1);
    const submission = (runCalls[0]!.body as { submission: { agentsMd: ReadonlyArray<{ kind: string; name?: string }> } })
      .submission;
    expect(submission.agentsMd[0]).toMatchObject({ kind: "asset", name: "rules" });
  });

  it("materializes draft Skill, AgentsMd, and File refs via presign→object storage PUT→finalize before submitting", async () => {
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

    // Three uploads, each presign → object storage PUT → finalize, then the run submit.
    const presignCalls = calls.filter((c) => c.url.endsWith("/assets/presign"));
    const r2PutCalls = calls.filter((c) => c.url.includes("r2.cloudflarestorage.com"));
    const finalizeCalls = calls.filter((c) => c.url.endsWith("/assets/finalize"));
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(presignCalls).toHaveLength(3);
    expect(r2PutCalls).toHaveLength(3);
    expect(finalizeCalls).toHaveLength(3);
    expect(runCalls).toHaveLength(1);

    // presign declares the content hash + size; the worker never sees bytes.
    const expectedHashes = [skillHash, agentsMdHash, fileHash];
    for (let i = 0; i < presignCalls.length; i++) {
      const call = presignCalls[i]!;
      expect(call.method).toBe("POST");
      expect(call.headers.authorization).toBe("Bearer tkn");
      expect((call.body as { hash: string }).hash).toBe(expectedHashes[i]);
    }
    // The object storage PUT carries the bytes + the signed checksum header (no Bearer).
    for (const put of r2PutCalls) {
      expect(put.method).toBe("PUT");
      expect(put.body).toBeInstanceOf(Uint8Array);
      expect(put.headers["x-amz-checksum-sha256"]).toBe("Y2hlY2tzdW0=");
      expect(put.headers.authorization).toBeUndefined();
    }

    const submission = (runCalls[0]!.body as { submission: Record<string, unknown> }).submission;
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

  it("does not submit the run when draft skill finalization fails", async () => {
    const calls: CapturedRequest[] = [];
    const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push({ url, method: (init?.method ?? "GET").toString(), headers: {}, body: init?.body });
      if (url.endsWith("/assets/presign")) {
        const body = (init?.body ? JSON.parse(String(init.body)) : {}) as { hash?: string };
        const hash = body.hash ?? `sha256:${"a".repeat(64)}`;
        const hex = hash.slice("sha256:".length);
        return new Response(
          JSON.stringify({
            ok: true,
            exists: false,
            assetId: `asset_${hex}`,
            contentHash: hash,
            uploadUrl: `https://acct.r2.cloudflarestorage.com/bucket/assets/ws/${hex}?X-Amz-Signature=sig`,
            requiredHeaders: {}
          }),
          { status: 201, headers: { "content-type": "application/json" } }
        );
      }
      if (url.includes("r2.cloudflarestorage.com")) {
        return new Response("", { status: 200 });
      }
      return new Response(JSON.stringify({ ok: false, code: "asset_finalize_failed" }), {
        status: 500,
        headers: { "content-type": "application/json" }
      });
    });
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch });
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
      "https://x/assets/presign",
      expect.stringContaining("r2.cloudflarestorage.com"),
      "https://x/assets/finalize"
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
    const client = new AntpathClient({ apiToken: "tkn", baseUrl: "https://x", fetch: stub });
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

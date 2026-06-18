import { describe, expect, it, vi } from "vitest";
import { AgentsMd, AgentExecutor, File as AexFile, McpServer, Models, Skill } from "../../src/index.js";

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
    // Direct-to-storage workspace asset flow.
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
          uploadUrl: `https://object-storage.example.test/bucket/assets/ws/${hex}?X-Amz-Signature=sig`,
          requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" },
          expiresInSeconds: 300
        }),
        { status: 201, headers: { "content-type": "application/json" } }
      );
    }
    if (url.includes("object-storage.example.test")) {
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

describe("AgentExecutor.submit (flat surface, wire shape)", () => {
  it("builds the flat submission and routes MCP headers into secrets", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({
      apiToken: "tkn_test",
      baseUrl: "https://example.test",
      fetch
    });

    const runId = await client.submit({
      model: "claude-haiku-4-5",
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
      secrets: { apiKey: "sk-test" },
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
    expect(submission.model).toBe("claude-haiku-4-5");
    expect(submission.outputMode).toBe("stream");
    expect(submission.system).toBe("You are tidy.");
    expect(submission.prompt).toEqual(["do work"]);
    expect(submission.skills).toEqual([]);
    expect(submission.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github" },
      { name: "noauth", url: "https://mcp.example/noauth" }
    ]);

    const secrets = body.secrets as Record<string, unknown>;
    expect(secrets.apiKey).toBe("sk-test");
    expect(secrets.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github", headers: { Authorization: "Bearer t" } }
    ]);
  });

  it("requires secrets.apiKey", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "p",
        secrets: { apiKey: "" }
      })
    ).rejects.toThrow(/AgentExecutor\.submit: secrets\.apiKey is required/);
  });

  it("serializes outputs when only capture overrides are supplied", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      secrets: { apiKey: "sk-x" },
      outputs: {
        captureTimeoutMs: 120000,
        maxFileBytes: 1_000_000_000_000,
        maxTotalBytes: 1_000_000_000_000,
        maxFiles: 50_000
      }
    });

    const body = calls[0]!.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    expect(submission.outputs).toEqual({
      captureTimeoutMs: 120000,
      maxFileBytes: 1_000_000_000_000,
      maxTotalBytes: 1_000_000_000_000,
      maxFiles: 50_000
    });
  });

  it("submits DeepSeek provider runs with a flat apiKey", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submit({
      provider: "deepseek",
      model: "deepseek-chat",
      prompt: "p",
      secrets: { apiKey: "sk-ds-test" },
      idempotencyKey: "idem-deepseek"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.provider).toBe("deepseek");
    expect(body.secrets).toEqual({ apiKey: "sk-ds-test" });
  });

  it("derives provider from model when provider is omitted", async () => {
    const cases = [
      [Models.CLAUDE_HAIKU_4_5, "anthropic"],
      [Models.GPT_4_1, "openai"],
      [Models.GEMINI_2_5_FLASH, "gemini"],
      [Models.MISTRAL_LARGE_LATEST, "mistral"],
      [Models.DEEPSEEK_V4_FLASH, "deepseek"],
      [Models.DEEPSEEK_V4_PRO, "deepseek"]
    ] as const;
    for (const [model, expectedProvider] of cases) {
      const { fetch, calls } = makeStubFetch();
      const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
      await client.submit({ model, prompt: "p", secrets: { apiKey: "sk-x" } });
      const body = calls[0]!.body as Record<string, unknown>;
      expect(body.provider, `model ${model}`).toBe(expectedProvider);
    }
  });

  it("throws when an explicit provider does not serve the model", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        provider: "anthropic",
        model: "gpt-4.1",
        prompt: "p",
        secrets: { apiKey: "sk-x" }
      })
    ).rejects.toThrow(/is not available for model "gpt-4\.1" \(supported: openai\)/);
  });

  it("accepts a non-default provider for a multi-provider model", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    // gpt-4o-mini is served by openai (default) and openrouter; the canonical
    // model id is sent on the wire untranslated — the platform maps it to the
    // provider-native id (openrouter → "openai/gpt-4o-mini").
    await client.submit({
      provider: "openrouter",
      model: Models.GPT_4O_MINI,
      prompt: "p",
      secrets: { apiKey: "sk-or-test" }
    });
    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.provider).toBe("openrouter");
    expect((body.submission as Record<string, unknown>).model).toBe("gpt-4o-mini");
  });

  it("forwards postHook on the top-level submission wire shape", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      postHook: {
        command: "pnpm test",
        timeout: "2m",
        maxTurns: 3,
        maxChars: null
      },
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-post-hook"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.postHook).toEqual({
      command: "pnpm test",
      timeout: "2m",
      maxTurns: 3,
      maxChars: null
    });
  });

  it("omits postHook when the command is empty", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      postHook: { command: " " },
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-empty-post-hook"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.postHook).toBeUndefined();
  });

  it("rejects empty prompts", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "",
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow(/prompt/);
  });

  it("accepts prompt arrays", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submit({
      model: "claude-haiku-4-5",
      prompt: ["one", "two"],
      secrets: { apiKey: "k" },
      idempotencyKey: "i"
    });
    const submission = (calls[0]!.body as Record<string, unknown>).submission as Record<string, unknown>;
    expect(submission.prompt).toEqual(["one", "two"]);
  });

  it("rejects mismatched MCP urls between run request server and explicit secret", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "p",
        mcpServers: [
          McpServer.remote({
            name: "github",
            url: "https://a.example/github",
            headers: { Authorization: "Bearer t" }
          })
        ],
        secrets: {
          apiKey: "k",
          mcpServers: [
            { name: "github", url: "https://b.example/github", headers: { Authorization: "Bearer u" } }
          ]
        }
      })
    ).rejects.toThrow(/conflicts/);
  });

  it("rejects non-Skill entries with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "p",
        skills: [{ kind: "workspace", id: "skl_x" } as unknown as Skill],
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow(/skills\[0\] must be a Skill instance/);
  });

  it("auto-uploads an inline AgentsMd to the asset store and submits a plain asset ref", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await AgentsMd.fromContent("# Rules\nBe helpful.\n", { name: "rules" });
    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      agentsMd: [draft],
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-asset-agentsmd"
    });
    // The draft staged through the content-addressable asset path before submit.
    expect(calls.some((c) => c.url.endsWith("/assets/presign"))).toBe(true);
    expect(calls.some((c) => c.url.includes("object-storage.example.test") && c.method === "PUT")).toBe(true);
    expect(calls.some((c) => c.url.endsWith("/assets/finalize"))).toBe(true);
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(runCalls).toHaveLength(1);
    const body = runCalls[0]!.body as {
      bootstrapMode?: unknown;
      directInputs?: unknown;
      submission: { agentsMd: ReadonlyArray<{ kind: string; name?: string; assetId?: string }> };
    };
    expect("bootstrapMode" in body).toBe(false);
    expect("directInputs" in body).toBe(false);
    expect(body.submission.agentsMd[0]).toMatchObject({ kind: "asset", name: "rules" });
    expect(body.submission.agentsMd[0]!.assetId).toMatch(/^asset_[0-9a-f]{64}$/);
  });

  it("auto-uploads draft Skill, AgentsMd, and File refs as assets before submitting", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
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

    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      skills: [skill],
      agentsMd: [agentsMd],
      files: [file],
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-assets"
    });

    // Each draft staged through the content-addressable asset path: one
    // presign + one object-storage PUT + one finalize per draft.
    const presignCalls = calls.filter((c) => c.url.endsWith("/assets/presign"));
    const storagePutCalls = calls.filter((c) => c.url.includes("object-storage.example.test") && c.method === "PUT");
    const finalizeCalls = calls.filter((c) => c.url.endsWith("/assets/finalize"));
    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(presignCalls).toHaveLength(3);
    expect(storagePutCalls).toHaveLength(3);
    expect(finalizeCalls).toHaveLength(3);
    expect(runCalls).toHaveLength(1);

    const runBody = runCalls[0]!.body as {
      bootstrapMode?: unknown;
      directInputs?: unknown;
      submission: Record<string, unknown>;
    };
    expect("bootstrapMode" in runBody).toBe(false);
    expect("directInputs" in runBody).toBe(false);

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

  it("pre-uploads a Skill via .upload(client) and submits it as a plain asset ref", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await Skill.fromFiles({ name: "rules", files: { "SKILL.md": "# rules\n" } });
    const draftHash = draft.ref.kind === "draft" ? draft.ref.contentHash : "";
    const draftHex = draftHash.slice("sha256:".length);

    // Pre-upload: blocking, returns a materialized asset-ref Skill.
    const uploaded = await draft.upload(client);
    expect(uploaded.isDraft).toBe(false);
    // The pre-upload hit the content-addressed asset store, not the bootstrap path.
    expect(calls.some((c) => c.url.endsWith("/assets/presign"))).toBe(true);

    const assetCallsBefore = calls.filter((c) => c.url.includes("/assets")).length;
    expect(assetCallsBefore).toBeGreaterThan(0);

    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      skills: [uploaded],
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-preuploaded"
    });

    const runCalls = calls.filter((c) => c.url.endsWith("/api/runs"));
    expect(runCalls).toHaveLength(1);
    const runBody = runCalls[0]!.body as {
      bootstrapMode?: unknown;
      directInputs?: unknown;
      submission: { skills: ReadonlyArray<Record<string, unknown>> };
    };
    // A pre-uploaded skill submits as a plain asset ref — no direct bootstrap.
    expect("bootstrapMode" in runBody).toBe(false);
    expect("directInputs" in runBody).toBe(false);
    expect(runBody.submission.skills).toEqual([
      { kind: "asset", assetId: `asset_${draftHex}`, name: "rules" }
    ]);
    // Submit performed no bootstrap round-trips.
    expect(calls.filter((c) => c.url.includes("bootstrap")).length).toBe(0);
  });

  it("auto-uploads a draft Skill (not pre-uploaded) as an asset and submits a plain asset ref", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await Skill.fromFiles({ name: "rules", files: { "SKILL.md": "# rules\n" } });
    const draftHex = draft.ref.kind === "draft" ? draft.ref.contentHash.slice("sha256:".length) : "";

    await client.submit({
      model: "claude-haiku-4-5",
      prompt: "p",
      skills: [draft],
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-draft-inline"
    });

    // Inline draft staged through the content-addressable asset path.
    expect(calls.some((c) => c.url.endsWith("/assets/presign"))).toBe(true);
    expect(calls.some((c) => c.url.includes("object-storage.example.test") && c.method === "PUT")).toBe(true);
    expect(calls.some((c) => c.url.endsWith("/assets/finalize"))).toBe(true);
    const runBody = calls.find((c) => c.url.endsWith("/api/runs"))!.body as {
      bootstrapMode?: unknown;
      directInputs?: unknown;
      submission: { skills: ReadonlyArray<Record<string, unknown>> };
    };
    expect("bootstrapMode" in runBody).toBe(false);
    expect("directInputs" in runBody).toBe(false);
    expect(runBody.submission.skills).toEqual([
      { kind: "asset", assetId: `asset_${draftHex}`, name: "rules" }
    ]);
  });

  it("rejects non-AgentsMd entries in the agentsMd array with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "p",
        agentsMd: [{ kind: "workspace_agentsmd", id: "amd_x" } as unknown as AgentsMd],
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow(/agentsMd\[0\] must be an AgentsMd instance/);
  });
});

describe("AgentExecutor.deleteWorkspaceAsset", () => {
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
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch: stub });
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

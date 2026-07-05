import { describe, expect, it, vi } from "vitest";
import { zipSync } from "fflate";
import { AgentsMd, Aex, BuiltinTools, File as AexFile, McpServer, Models, Skill, Tool } from "../../src/index.js";

/** Build a zip-archived skill (SKILL.md + YAML frontmatter) fetchable by Skill.fromUrl. */
function skillZip(name: string, description: string): Uint8Array {
  const md = `---\nname: ${name}\ndescription: ${description}\n---\n# ${name}\n`;
  return zipSync({ "SKILL.md": new TextEncoder().encode(md) }, { level: 0 });
}

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
    if (url.includes("/api/skills/") && method === "PUT") {
      const reqBody = (body ?? {}) as { contentHash?: string; description?: string; sizeBytes?: number };
      const name = decodeURIComponent(url.split("/api/skills/")[1] ?? "");
      return new Response(
        JSON.stringify({
          skill: {
            kind: "skill",
            name,
            contentHash: reqBody.contentHash,
            description: reqBody.description,
            sizeBytes: reqBody.sizeBytes ?? 0,
            version: 1
          },
          updated: true
        }),
        { status: 200, headers: { "content-type": "application/json" } }
      );
    }
    // Session create (POST /api/sessions) and any other read.
    return new Response(
      JSON.stringify({ id: "run_test", status: "queued" }),
      { status: 200, headers: { "content-type": "application/json" } }
    );
  });
  return { fetch: stub, calls };
}

function idempotencyHeader(call: CapturedRequest): string | undefined {
  return call.headers["Idempotency-Key"] ?? call.headers["idempotency-key"];
}

describe("Aex.openSession — session-create wire shape", () => {
  it("builds the session-create submission and routes MCP headers into secrets", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({
      apiKey: "tkn_test",
      baseUrl: "https://example.test",
      fetch
    });

    const session = await client.openSession({
      model: "claude-haiku-4-5",
      system: "You are tidy.",
      outputMode: "stream",
      mcpServers: [
        McpServer.remote({
          name: "github",
          url: "https://mcp.example/github",
          headers: { Authorization: "Bearer t" }
        }),
        McpServer.remote({ name: "noauth", url: "https://mcp.example/noauth" })
      ],
      apiKeys: { anthropic: "sk-test" },
      idempotencyKey: "idem_unit"
    });

    expect(session.id).toBe("run_test");
    expect(calls).toHaveLength(1);
    const call = calls[0]!;
    expect(call.method).toBe("POST");
    expect(call.url).toBe("https://example.test/api/sessions");
    expect(call.headers["authorization"] ?? call.headers["Authorization"]).toBe("Bearer tkn_test");
    // The idempotency key rides the header, not the body.
    expect(idempotencyHeader(call)).toBe("idem_unit");

    const body = call.body as Record<string, unknown>;
    // Session retention is always present on the create body.
    expect(body.retention).toEqual({ idleTtl: "3m" });

    const submission = body.submission as Record<string, unknown>;
    expect(submission.model).toBe("claude-haiku-4-5");
    expect(submission.outputMode).toBe("stream");
    expect(submission.system).toBe("You are tidy.");
    expect(submission.tools).toEqual([]);
    // The one-shot message travels via /messages, never the create submission.
    expect("prompt" in submission).toBe(false);
    expect(submission.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github" },
      { name: "noauth", url: "https://mcp.example/noauth" }
    ]);

    const secrets = body.secrets as Record<string, unknown>;
    expect((secrets.apiKeys as Record<string, unknown>).anthropic).toBe("sk-test");
    expect(secrets.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github", headers: { Authorization: "Bearer t" } }
    ]);
  });

  it("requires a provider key for the selected provider", async () => {
    const { fetch } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "" }
      })
    ).rejects.toThrow(/Aex\.openSession: a provider API key is required/);
  });

  it("serializes outputs when only capture overrides are supplied", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "k" },
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

  it("uploads draft tools and includes value-free tool refs in the submission", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const tool = await Tool.fromFiles({
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      inputSchema: {
        type: "object",
        properties: { start: { type: "string" } },
        required: ["start"]
      },
      entry: "index.js",
      files: {
        "index.js": "export default async function ({ input }) { return { content: [{ type: 'text', text: input.start }] }; }\n"
      }
    });

    await client.openSession({
      model: "claude-haiku-4-5",
      tools: [tool],
      apiKeys: { anthropic: "k" }
    });

    const createCall = calls.find((call) => call.url === "https://x/api/sessions")!;
    const body = createCall.body as Record<string, unknown>;
    const submission = body.submission as Record<string, unknown>;
    expect(submission.tools).toEqual([
      {
        kind: "asset",
        assetId: expect.stringMatching(/^asset_[0-9a-f]{64}$/),
        name: "calendar_lookup",
        description: "Looks up calendar availability.",
        input_schema: {
          type: "object",
          properties: { start: { type: "string" } },
          required: ["start"]
        },
        entry: "index.js"
      }
    ]);

    // The draft is reusable across sessions: a second create reuses the cached
    // asset id (no re-upload) and produces the identical wire ref.
    calls.length = 0;
    await client.openSession({
      model: "claude-haiku-4-5",
      tools: [tool],
      apiKeys: { anthropic: "k" }
    });
    expect(calls.some((c) => c.url.endsWith("/assets/presign"))).toBe(false);
    const second = calls.find((c) => c.url === "https://x/api/sessions")!.body as Record<string, unknown>;
    expect((second.submission as Record<string, unknown>).tools).toEqual([
      {
        kind: "asset",
        assetId: expect.stringMatching(/^asset_[0-9a-f]{64}$/),
        name: "calendar_lookup",
        description: "Looks up calendar availability.",
        input_schema: {
          type: "object",
          properties: { start: { type: "string" } },
          required: ["start"]
        },
        entry: "index.js"
      }
    ]);
  });

  it("threads includeBuiltinTools and places builtin tool refs (strings) before custom tools on the wire", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const tool = await Tool.fromFiles({
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      inputSchema: { type: "object", properties: {}, required: [] },
      entry: "index.js",
      files: { "index.js": "export default async () => ({ content: [] });\n" }
    });

    await client.openSession({
      model: "claude-haiku-4-5",
      includeBuiltinTools: false,
      // BuiltinTools.git is just the name string "git".
      tools: [BuiltinTools.git, tool],
      apiKeys: { anthropic: "k" }
    });

    const createCall = calls.find((call) => call.url === "https://x/api/sessions")!;
    const submission = (createCall.body as Record<string, unknown>).submission as Record<string, unknown>;
    expect(submission.includeBuiltinTools).toBe(false);
    // Builtin string refs first, then the custom tool ref object.
    const wireTools = submission.tools as unknown[];
    expect(wireTools[0]).toBe("git");
    expect((wireTools[1] as { name: string }).name).toBe("calendar_lookup");
  });

  it("creates DeepSeek provider sessions with per-provider apiKeys", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await client.openSession({
      provider: "deepseek",
      model: "deepseek-v4-flash",
      apiKeys: { deepseek: "sk-ds-test" },
      idempotencyKey: "idem-deepseek"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.provider).toBe("deepseek");
    expect(body.secrets).toEqual({ apiKeys: { deepseek: "sk-ds-test" } });
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
      const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
      await client.openSession({ model, apiKeys: { [expectedProvider]: "sk-x" } });
      const body = calls[0]!.body as Record<string, unknown>;
      expect(body.provider, `model ${model}`).toBe(expectedProvider);
    }
  });

  it("throws when an explicit provider does not serve the model", async () => {
    const { fetch } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.openSession({
        provider: "anthropic",
        model: "gpt-4.1",
        apiKeys: { anthropic: "sk-x" }
      })
    ).rejects.toThrow(/is not available for model "gpt-4\.1" \(supported: openai\)/);
  });

  it("accepts a non-default provider for a multi-provider model", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    // gpt-4o-mini is served by openai (default) and openrouter; the canonical
    // model id is sent on the wire untranslated — the platform maps it to the
    // provider-native id (openrouter → "openai/gpt-4o-mini").
    await client.openSession({
      provider: "openrouter",
      model: Models.GPT_4O_MINI,
      apiKeys: { openrouter: "sk-or-test" }
    });
    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.provider).toBe("openrouter");
    expect((body.submission as Record<string, unknown>).model).toBe("gpt-4o-mini");
  });

  it("includes webhook on the top-level request body when supplied", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await client.openSession({
      model: "claude-haiku-4-5",
      webhook: { url: "https://hooks.example.com/aex" },
      apiKeys: { anthropic: "k" },
      idempotencyKey: "idem-webhook"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect(body.webhook).toEqual({ url: "https://hooks.example.com/aex" });
  });

  it("omits webhook from the request body when not supplied", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "k" },
      idempotencyKey: "idem-no-webhook"
    });

    const body = calls[0]!.body as Record<string, unknown>;
    expect("webhook" in body).toBe(false);
  });

  it("rejects an empty one-shot message before any HTTP request", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.run({
        model: "claude-haiku-4-5",
        message: "",
        apiKeys: { anthropic: "k" }
      })
    ).rejects.toThrow(/message must be a non-empty string/);
    expect(calls).toHaveLength(0);
  });

  it("rejects two same-named MCP servers whose urls conflict", async () => {
    const { fetch } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "k" },
        mcpServers: [
          McpServer.remote({
            name: "github",
            url: "https://a.example/github",
            headers: { Authorization: "Bearer t" }
          }),
          McpServer.remote({
            name: "github",
            url: "https://b.example/github",
            headers: { Authorization: "Bearer u" }
          })
        ]
      })
    ).rejects.toThrow(/conflicts/);
  });

  it("rejects non-Tool entries in tools with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        tools: [{ kind: "workspace", id: "x" } as unknown as Tool],
        apiKeys: { anthropic: "k" }
      })
    ).rejects.toThrow(/tools\[0\] must be a Tool or a builtin tool name/);
  });

  it("accepts Skill entries through the top-level skills option", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const zip = skillZip("rules", "Keep responses short.");
    const skill = await Skill.fromUrl("https://skills.example/rules.zip", {
      fetch: async () => new Response(zip)
    });

    await client.openSession({
      model: "claude-haiku-4-5",
      skills: [skill],
      apiKeys: { anthropic: "k" }
    });

    const upsert = calls.find((call) => call.url === "https://x/api/skills/rules");
    expect(upsert?.method).toBe("PUT");
    const create = calls.find((call) => call.url === "https://x/api/sessions")!.body as {
      submission: Record<string, unknown>;
    };
    expect(create.submission.skills).toEqual([{ kind: "skill", name: "rules" }]);
    expect(create.submission.tools).toEqual([]);
  });

  it("auto-uploads an inline AgentsMd to the asset store and submits a plain asset ref", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const draft = await AgentsMd.fromContent("# Rules\nBe helpful.\n", { name: "rules" });
    await client.openSession({
      model: "claude-haiku-4-5",
      agentsMd: [draft],
      apiKeys: { anthropic: "k" },
      idempotencyKey: "idem-asset-agentsmd"
    });
    // The draft staged through the content-addressable asset path before create.
    expect(calls.some((c) => c.url.endsWith("/assets/presign"))).toBe(true);
    expect(calls.some((c) => c.url.includes("object-storage.example.test") && c.method === "PUT")).toBe(true);
    expect(calls.some((c) => c.url.endsWith("/assets/finalize"))).toBe(true);
    const createCalls = calls.filter((c) => c.url.endsWith("/api/sessions"));
    expect(createCalls).toHaveLength(1);
    const body = createCalls[0]!.body as {
      bootstrapMode?: unknown;
      directInputs?: unknown;
      submission: { agentsMd: ReadonlyArray<{ kind: string; name?: string; assetId?: string }> };
    };
    expect("bootstrapMode" in body).toBe(false);
    expect("directInputs" in body).toBe(false);
    expect(body.submission.agentsMd[0]).toMatchObject({ kind: "asset", name: "rules" });
    expect(body.submission.agentsMd[0]!.assetId).toMatch(/^asset_[0-9a-f]{64}$/);
  });

  it("auto-uploads and upserts a draft Skill, AgentsMd, and File refs before creating", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const zip = skillZip("rules", "Keep responses short.");
    const skillTool = await Skill.fromUrl("https://skills.example/rules.zip", {
      fetch: async () => new Response(zip)
    });
    const agentsMd = await AgentsMd.fromContent("# Session rules\nBe precise.\n", { name: "session-rules" });
    const file = await AexFile.fromBytes({
      name: "dataset",
      bytes: new TextEncoder().encode("id,value\n1,alpha\n"),
      mountPath: "/workspace/input/dataset.csv"
    });
    const skillHash = skillTool.ref.kind === "draft" ? skillTool.ref.contentHash : "";
    const agentsMdHash = agentsMd.ref.kind === "draft" ? agentsMd.ref.contentHash : "";
    const fileHash = (file.ref.kind === "draft" ? file.ref.contentHash : "") ?? "";

    await client.openSession({
      model: "claude-haiku-4-5",
      skills: [skillTool],
      agentsMd: [agentsMd],
      files: [file],
      apiKeys: { anthropic: "k" },
      idempotencyKey: "idem-assets"
    });

    // Each draft staged through the content-addressable asset path: one
    // presign + one object-storage PUT + one finalize per draft.
    const presignCalls = calls.filter((c) => c.url.endsWith("/assets/presign"));
    const storagePutCalls = calls.filter((c) => c.url.includes("object-storage.example.test") && c.method === "PUT");
    const finalizeCalls = calls.filter((c) => c.url.endsWith("/assets/finalize"));
    const skillUpsertCalls = calls.filter((c) => c.url === "https://x/api/skills/rules");
    const createCalls = calls.filter((c) => c.url.endsWith("/api/sessions"));
    expect(presignCalls).toHaveLength(3);
    expect(storagePutCalls).toHaveLength(3);
    expect(finalizeCalls).toHaveLength(3);
    expect(skillUpsertCalls).toHaveLength(1);
    expect(createCalls).toHaveLength(1);

    const createBody = createCalls[0]!.body as {
      bootstrapMode?: unknown;
      directInputs?: unknown;
      submission: Record<string, unknown>;
    };
    expect("bootstrapMode" in createBody).toBe(false);
    expect("directInputs" in createBody).toBe(false);

    const submission = createBody.submission;
    const assetRef = (name: string, hash: string, extra: Record<string, string> = {}) => {
      return {
        kind: "asset",
        assetId: `asset_${hash.slice("sha256:".length)}`,
        name,
        ...extra
      };
    };
    expect(skillUpsertCalls[0]!.body).toEqual({
      contentHash: skillHash,
      description: "Keep responses short.",
      sizeBytes: expect.any(Number)
    });
    expect(submission.tools).toEqual([]);
    expect(submission.skills).toEqual([{ kind: "skill", name: "rules" }]);
    expect(submission.agentsMd).toEqual([assetRef("session-rules", agentsMdHash)]);
    expect(submission.files).toEqual([
      assetRef("dataset", fileHash, { mountPath: "/workspace/input/dataset.csv" })
    ]);
  });

  it("uploads draft skill bytes once and reuses the cached asset id across sessions", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const zip = skillZip("rules", "Keep responses short.");
    const skillTool = await Skill.fromUrl("https://skills.example/rules.zip", {
      fetch: async () => new Response(zip)
    });
    const expectedRef = { kind: "skill", name: "rules" };

    await client.openSession({
      model: "claude-haiku-4-5",
      skills: [skillTool],
      apiKeys: { anthropic: "k" },
      idempotencyKey: "idem-skilltool"
    });
    const firstBody = calls.find((c) => c.url.endsWith("/api/sessions"))!.body as {
      submission: { skills: ReadonlyArray<Record<string, unknown>> };
    };
    expect(firstBody.submission.skills).toEqual([expectedRef]);

    // The draft is reusable: a second create reuses the cached workspace upload
    // state (no asset upload, no registry upsert) and produces the identical
    // name-only wire ref.
    calls.length = 0;
    await client.openSession({
      model: "claude-haiku-4-5",
      skills: [skillTool],
      apiKeys: { anthropic: "k" },
      idempotencyKey: "idem-skilltool-2"
    });
    expect(calls.some((c) => c.url.endsWith("/assets/presign"))).toBe(false);
    expect(calls.filter((c) => c.url === "https://x/api/skills/rules")).toHaveLength(0);
    const secondBody = calls.find((c) => c.url.endsWith("/api/sessions"))!.body as {
      submission: { skills: ReadonlyArray<Record<string, unknown>> };
    };
    expect(secondBody.submission.skills).toEqual([expectedRef]);
  });

  it("rejects non-AgentsMd entries in the agentsMd array with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        agentsMd: [{ kind: "not_asset", id: "amd_x" } as unknown as AgentsMd],
        apiKeys: { anthropic: "k" }
      })
    ).rejects.toThrow(/agentsMd\[0\] must be an AgentsMd instance/);
  });
});

describe("Aex.deleteWorkspaceAsset", () => {
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
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch: stub });
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

import { describe, expect, it, vi } from "vitest";
import { AgentsMd, AgentExecutor, File as AexFile, McpServer, Skill } from "../../src/index.js";

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

describe("AgentExecutor.submitRun (flat surface, wire shape)", () => {
  it("builds the flat submission and routes MCP headers into secrets", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({
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
    expect(secrets.apiKey).toBe("sk-test");
    expect(secrets.mcpServers).toEqual([
      { name: "github", url: "https://mcp.example/github", headers: { Authorization: "Bearer t" } }
    ]);
  });

  it("requires secrets.apiKey", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        secrets: { apiKey: "" }
      })
    ).rejects.toThrow(/AgentExecutor\.submitRun: secrets\.apiKey is required/);
  });

  it("submits DeepSeek provider runs with a flat apiKey", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submitRun({
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

  it("rejects empty prompts", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
        prompt: "",
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow(/prompt/);
  });

  it("accepts prompt arrays", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await client.submitRun({
      model: "m",
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
      client.submitRun({
        model: "m",
        prompt: "p",
        skills: [{ kind: "workspace", id: "skl_x" } as unknown as Skill],
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow(/skills\[0\] must be a Skill instance/);
  });

  it("submits an inline AgentsMd as a direct bootstrap input without /assets calls", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await AgentsMd.fromContent("# Rules\nBe helpful.\n", { name: "rules" });
    await client.submitRun({
      model: "m",
      prompt: "p",
      agentsMd: [draft],
      secrets: { apiKey: "k" },
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

  it("retries a direct bootstrap input upload after a 502 and then commits", async () => {
    const draft = await AgentsMd.fromContent("# Rules\nRetry the upload.\n", { name: "rules" });
    const calls: CapturedRequest[] = [];
    let inputPutCount = 0;
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-06T00:00:00Z"));
    try {
      const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
        calls.push({ url, method: (init?.method ?? "GET").toString(), headers: {}, body: init?.body });
        if (url.endsWith("/api/runs")) {
          return new Response(
            JSON.stringify({
              id: "run_test",
              status: "queued",
              bootstrapStatusUrl: "https://x/api/runs/run_test/bootstrap/status",
              bootstrapToken: "boot_retry",
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
          inputPutCount += 1;
          return inputPutCount === 1
            ? new Response("try again", { status: 502 })
            : new Response("", { status: 200 });
        }
        if (url.endsWith("/bootstrap/commit")) {
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (url.endsWith("/bootstrap/abort")) {
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ ok: false }), {
          status: 500,
          headers: { "content-type": "application/json" }
        });
      });
      const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });

      const submitted = client.submitRun({
        model: "m",
        prompt: "p",
        agentsMd: [draft],
        secrets: { apiKey: "k" }
      });

      // Drive the loop until the first (502) input upload lands, then advance
      // the 100ms backoff on fake timers so the retry fires without a real sleep.
      for (let i = 0; i < 10 && inputPutCount === 0; i += 1) {
        await vi.advanceTimersByTimeAsync(0);
      }
      expect(inputPutCount).toBe(1);
      await vi.advanceTimersByTimeAsync(100);

      await expect(submitted).resolves.toBe("run_test");

      const inputPutCalls = calls.filter((c) => c.url.includes("/bootstrap/inputs/"));
      expect(inputPutCalls).toHaveLength(2);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/commit"))).toHaveLength(1);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/abort"))).toHaveLength(0);
      expect(calls.map((c) => c.url)).toEqual([
        "https://x/api/runs",
        "https://x/api/runs/run_test/bootstrap/status",
        expect.stringContaining("https://bootstrap.example/run_test/bootstrap/inputs/"),
        expect.stringContaining("https://bootstrap.example/run_test/bootstrap/inputs/"),
        "https://bootstrap.example/run_test/bootstrap/commit"
      ]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a thrown direct bootstrap input upload error and then commits", async () => {
    const draft = await AgentsMd.fromContent("# Rules\nRetry the upload.\n", { name: "rules" });
    const calls: CapturedRequest[] = [];
    let inputPutCount = 0;
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-06T00:00:00Z"));
    try {
      const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
        calls.push({ url, method: (init?.method ?? "GET").toString(), headers: {}, body: init?.body });
        if (url.endsWith("/api/runs")) {
          return new Response(
            JSON.stringify({
              id: "run_test",
              status: "queued",
              bootstrapStatusUrl: "https://x/api/runs/run_test/bootstrap/status",
              bootstrapToken: "boot_retry",
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
          inputPutCount += 1;
          if (inputPutCount === 1) throw new TypeError("fetch failed");
          return new Response("", { status: 200 });
        }
        if (url.endsWith("/bootstrap/commit")) {
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (url.endsWith("/bootstrap/abort")) {
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ ok: false }), {
          status: 500,
          headers: { "content-type": "application/json" }
        });
      });
      const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });

      const submitted = client.submitRun({
        model: "m",
        prompt: "p",
        agentsMd: [draft],
        secrets: { apiKey: "k" }
      });

      // Drive the loop until the first (thrown) input upload lands, then advance
      // the 100ms backoff on fake timers so the retry fires without a real sleep.
      for (let i = 0; i < 10 && inputPutCount === 0; i += 1) {
        await vi.advanceTimersByTimeAsync(0);
      }
      expect(inputPutCount).toBe(1);
      await vi.advanceTimersByTimeAsync(100);

      await expect(submitted).resolves.toBe("run_test");

      expect(calls.filter((c) => c.url.includes("/bootstrap/inputs/"))).toHaveLength(2);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/commit"))).toHaveLength(1);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/abort"))).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a stalled direct bootstrap commit and then resolves", async () => {
    const draft = await AgentsMd.fromContent("# Rules\nRetry the commit.\n", { name: "rules" });
    const calls: CapturedRequest[] = [];
    let commitCount = 0;
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-06T00:00:00Z"));
    try {
      const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
        calls.push({ url, method: (init?.method ?? "GET").toString(), headers: {}, body: init?.body });
        if (url.endsWith("/api/runs")) {
          return new Response(
            JSON.stringify({
              id: "run_test",
              status: "queued",
              bootstrapStatusUrl: "https://x/api/runs/run_test/bootstrap/status",
              bootstrapToken: "boot_retry",
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
          return new Response("", { status: 200 });
        }
        if (url.endsWith("/bootstrap/commit")) {
          commitCount += 1;
          if (commitCount === 1) {
            return await new Promise<Response>((_resolve, reject) => {
              init?.signal?.addEventListener("abort", () => reject(new Error("commit response stalled")), { once: true });
            });
          }
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        if (url.endsWith("/bootstrap/abort")) {
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ ok: false }), {
          status: 500,
          headers: { "content-type": "application/json" }
        });
      });
      const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });

      const submitted = client.submitRun({
        model: "m",
        prompt: "p",
        agentsMd: [draft],
        secrets: { apiKey: "k" }
      });

      for (let i = 0; i < 10 && commitCount === 0; i += 1) {
        await vi.advanceTimersByTimeAsync(0);
      }
      expect(commitCount).toBe(1);
      await vi.advanceTimersByTimeAsync(10_000);
      await vi.advanceTimersByTimeAsync(100);

      await expect(submitted).resolves.toBe("run_test");
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/commit"))).toHaveLength(2);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/abort"))).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("uses the run read path when a direct bootstrap commit response is lost", async () => {
    const draft = await AgentsMd.fromContent("# Rules\nCommit can race cleanup.\n", { name: "rules" });
    const calls: CapturedRequest[] = [];
    let commitCount = 0;
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-06T00:00:00Z"));
    try {
      const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
        const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
        calls.push({ url, method: (init?.method ?? "GET").toString(), headers: {}, body: init?.body });
        if (url.endsWith("/api/runs") && (init?.method ?? "GET") === "POST") {
          return new Response(
            JSON.stringify({
              id: "run_test",
              status: "queued",
              bootstrapStatusUrl: "https://x/api/runs/run_test/bootstrap/status",
              bootstrapToken: "boot_retry",
              bootstrapExpiresAt: new Date(Date.now() + 60_000).toISOString()
            }),
            { status: 202, headers: { "content-type": "application/json" } }
          );
        }
        if (url.endsWith("/api/runs/run_test")) {
          return new Response(
            JSON.stringify({
              id: "run_test",
              status: "succeeded",
              terminalAt: new Date(Date.now()).toISOString()
            }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        if (url.endsWith("/bootstrap/status")) {
          return new Response(
            JSON.stringify({ status: "ready", uploadBaseUrl: "https://bootstrap.example/run_test/bootstrap" }),
            { status: 200, headers: { "content-type": "application/json" } }
          );
        }
        if (url.includes("/bootstrap/inputs/")) {
          return new Response("", { status: 200 });
        }
        if (url.endsWith("/bootstrap/commit")) {
          commitCount += 1;
          return await new Promise<Response>(() => undefined);
        }
        if (url.endsWith("/bootstrap/abort")) {
          return new Response(JSON.stringify({ ok: true }), {
            status: 200,
            headers: { "content-type": "application/json" }
          });
        }
        return new Response(JSON.stringify({ ok: false }), {
          status: 500,
          headers: { "content-type": "application/json" }
        });
      });
      const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });

      const submitted = client.submitRun({
        model: "m",
        prompt: "p",
        agentsMd: [draft],
        secrets: { apiKey: "k" }
      });

      for (let i = 0; i < 10 && commitCount === 0; i += 1) {
        await vi.advanceTimersByTimeAsync(0);
      }
      expect(commitCount).toBe(1);
      await vi.advanceTimersByTimeAsync(10_000);

      await expect(submitted).resolves.toBe("run_test");
      expect(calls.filter((c) => c.url.endsWith("/api/runs/run_test"))).toHaveLength(1);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/commit"))).toHaveLength(1);
      expect(calls.filter((c) => c.url.endsWith("/bootstrap/abort"))).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("uploads draft Skill, AgentsMd, and File refs to the bootstrap target before resolving", async () => {
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

    await client.submitRun({
      model: "m",
      prompt: "p",
      skills: [skill],
      agentsMd: [agentsMd],
      files: [file],
      secrets: { apiKey: "k" },
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
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const skill = await Skill.fromFiles({
      name: "rules",
      files: { "SKILL.md": "# rules\n" }
    });

    await expect(
      client.submitRun({
        model: "m",
        prompt: "p",
        skills: [skill],
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow();

    expect(calls.map((c) => c.url)).toEqual([
      "https://x/api/runs",
      "https://x/api/runs/run_test/bootstrap/status",
      expect.stringContaining("https://bootstrap.example/run_test/bootstrap/inputs/"),
      "https://x/api/runs/run_test/bootstrap/abort"
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

    await client.submitRun({
      model: "m",
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

  it("submits a draft Skill (not pre-uploaded) via the inline direct-bootstrap path", async () => {
    const { fetch, calls } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const draft = await Skill.fromFiles({ name: "rules", files: { "SKILL.md": "# rules\n" } });

    await client.submitRun({
      model: "m",
      prompt: "p",
      skills: [draft],
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-draft-inline"
    });

    // Inline draft: no /assets calls, direct bootstrap instead.
    expect(calls.filter((c) => c.url.includes("/assets"))).toHaveLength(0);
    const runBody = calls.find((c) => c.url.endsWith("/api/runs"))!.body as {
      bootstrapMode: string;
      directInputs: ReadonlyArray<{ role: string }>;
    };
    expect(runBody.bootstrapMode).toBe("direct");
    expect(runBody.directInputs.map((i) => i.role)).toEqual(["skill"]);
    expect(calls.filter((c) => c.url.endsWith("/bootstrap/commit"))).toHaveLength(1);
  });

  it("rejects non-AgentsMd entries in the agentsMd array with index in the message", async () => {
    const { fetch } = makeStubFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch });
    await expect(
      client.submitRun({
        model: "m",
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

describe("AgentExecutor.submitRun — inline draft Skill against the locked P0 bootstrap contract", () => {
  it("PUTs each input with sha256/size/bearer headers then commits the descriptor set", async () => {
    const BOOT_TOKEN = "boot_p0";
    const UPLOAD_BASE = "https://bootstrap.p0.example/run_p0/bootstrap";
    const ROUTING = { "x-aex-route": "machine-7" };
    const calls: CapturedRequest[] = [];

    const fetch: typeof globalThis.fetch = vi.fn(async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const method = (init?.method ?? "GET").toString();
      // Capture headers as a flat lowercase-friendly record.
      const headers: Record<string, string> = {};
      const ih = init?.headers;
      if (ih instanceof Headers) for (const [k, v] of ih.entries()) headers[k] = v;
      else if (Array.isArray(ih)) for (const [k, v] of ih) headers[k] = v;
      else if (ih) Object.assign(headers, ih);
      let body: unknown = init?.body;
      if (typeof body === "string") {
        try { body = JSON.parse(body); } catch { /* leave raw */ }
      }
      calls.push({ url, method, headers, body });

      // POST /api/runs → 202 with the full direct-bootstrap envelope.
      if (url.endsWith("/api/runs") && method === "POST") {
        return new Response(
          JSON.stringify({
            runId: "run_p0",
            bootstrapToken: BOOT_TOKEN,
            bootstrapStatusUrl: "https://api.p0.example/api/runs/run_p0/bootstrap/status",
            bootstrapExpiresAt: new Date(Date.now() + 60_000).toISOString(),
            uploadBaseUrl: UPLOAD_BASE,
            routingHeaders: ROUTING
          }),
          { status: 202, headers: { "content-type": "application/json" } }
        );
      }
      // GET status → ready target.
      if (url.endsWith("/bootstrap/status")) {
        return new Response(
          JSON.stringify({ status: "ready", uploadBaseUrl: UPLOAD_BASE, routingHeaders: ROUTING }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
      // PUT {uploadBaseUrl}/inputs/:inputId → 200.
      if (url.startsWith(`${UPLOAD_BASE}/inputs/`)) {
        return new Response("", { status: 200 });
      }
      // POST {uploadBaseUrl}/commit → 200.
      if (url === `${UPLOAD_BASE}/commit`) {
        return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
      }
      return new Response(JSON.stringify({ ok: false }), { status: 500, headers: { "content-type": "application/json" } });
    });

    const client = new AgentExecutor({ apiToken: "tkn_p0", baseUrl: "https://api.p0.example", fetch });
    const skill = await Skill.fromFiles({ name: "rules", files: { "SKILL.md": "# rules\n" } });
    const skillHash = skill.ref.kind === "draft" ? skill.ref.contentHash : "";

    const runId = await client.submitRun({
      model: "m",
      prompt: "p",
      skills: [skill],
      secrets: { apiKey: "k" },
      idempotencyKey: "idem-p0"
    });
    expect(runId).toBe("run_p0");

    // Submit carried the descriptor on the wire.
    const submit = calls.find((c) => c.url.endsWith("/api/runs"))!;
    const submitBody = submit.body as {
      bootstrapMode: string;
      directInputs: ReadonlyArray<{ inputId: string; sha256: string; sizeBytes: number; role: string }>;
    };
    expect(submitBody.bootstrapMode).toBe("direct");
    expect(submitBody.directInputs).toHaveLength(1);
    const descriptor = submitBody.directInputs[0]!;
    expect(descriptor.sha256).toBe(skillHash);

    // PUT input: bearer + sha256 + size headers (+ routing), body is the bytes.
    const put = calls.find((c) => c.method === "PUT" && c.url.startsWith(`${UPLOAD_BASE}/inputs/`))!;
    expect(put.url).toBe(`${UPLOAD_BASE}/inputs/${encodeURIComponent(descriptor.inputId)}`);
    expect(put.headers.authorization).toBe(`Bearer ${BOOT_TOKEN}`);
    expect(put.headers["x-aex-input-sha256"]).toBe(skillHash);
    expect(put.headers["x-aex-input-size"]).toBe(String(descriptor.sizeBytes));
    expect(put.headers["x-aex-route"]).toBe("machine-7");
    expect(put.body).toBeInstanceOf(Uint8Array);

    // Commit: bearer + JSON body listing each input.
    const commit = calls.find((c) => c.url === `${UPLOAD_BASE}/commit`)!;
    expect(commit.method).toBe("POST");
    expect(commit.headers.authorization).toBe(`Bearer ${BOOT_TOKEN}`);
    expect(commit.body).toEqual({
      inputs: [
        { inputId: descriptor.inputId, sha256: descriptor.sha256, sizeBytes: descriptor.sizeBytes }
      ]
    });

    // Exactly one of each round-trip in order.
    expect(calls.map((c) => `${c.method} ${c.url}`)).toEqual([
      "POST https://api.p0.example/api/runs",
      `PUT ${UPLOAD_BASE}/inputs/${encodeURIComponent(descriptor.inputId)}`,
      `POST ${UPLOAD_BASE}/commit`
    ]);
  });
});

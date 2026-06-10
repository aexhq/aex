import { describe, expect, it, vi } from "vitest";
import { AgentExecutor, ProxyEndpoint } from "../../src/index.js";

function makeFetch(): { fetch: typeof fetch; calls: Array<{ body: Record<string, unknown> }> } {
  const calls: Array<{ body: Record<string, unknown> }> = [];
  const stub: typeof fetch = vi.fn(async (_input, init) => {
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : {};
    calls.push({ body });
    return new Response(JSON.stringify({ id: "run_x", status: "queued" }), {
      status: 200,
      headers: { "content-type": "application/json" }
    });
  });
  return { fetch: stub, calls };
}

describe("ProxyEndpoint", () => {
  it("bearer/header/basic/query build the correct authShape + per-name secret", () => {
    const bearer = ProxyEndpoint.bearer({
      name: "stripe",
      baseUrl: "https://api.stripe.com",
      token: "sk_live_abc",
      allowMethods: ["GET", "POST"],
      allowPathPrefixes: ["/v1/charges"]
    });
    expect(bearer.declaration.authShape).toEqual({ type: "bearer" });
    expect(bearer.auth).toEqual({ name: "stripe", value: { type: "bearer", token: "sk_live_abc" } });

    const header = ProxyEndpoint.header({
      name: "datadog",
      baseUrl: "https://api.datadoghq.com",
      header: "DD-API-KEY",
      value: "ddkey",
      allowMethods: ["POST"],
      allowPathPrefixes: ["/api/v1/series"]
    });
    expect(header.declaration.authShape).toEqual({ type: "header", name: "DD-API-KEY" });
    expect(header.auth).toEqual({ name: "datadog", value: { type: "header", value: "ddkey" } });

    const basic = ProxyEndpoint.basic({
      name: "legacy",
      baseUrl: "https://legacy.example/api",
      username: "u",
      password: "p",
      allowMethods: ["GET"],
      allowPathPrefixes: ["/v1"]
    });
    expect(basic.declaration.authShape).toEqual({ type: "basic" });
    expect(basic.auth).toEqual({ name: "legacy", value: { type: "basic", username: "u", password: "p" } });

    const query = ProxyEndpoint.query({
      name: "weather",
      baseUrl: "https://api.weather.test",
      query: "apikey",
      value: "wk",
      allowMethods: ["GET"],
      allowPathPrefixes: ["/forecast"]
    });
    expect(query.declaration.authShape).toEqual({ type: "query", name: "apikey" });
    expect(query.auth).toEqual({ name: "weather", value: { type: "query", value: "wk" } });
  });

  it("submitRun splits ProxyEndpoint instances into declaration + secrets bag", async () => {
    const { fetch, calls } = makeFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x.test", fetch });
    await client.submitRun({
      model: "claude-haiku-4-5",
      prompt: "p",
      proxyEndpoints: [
        ProxyEndpoint.bearer({
          name: "stripe",
          baseUrl: "https://api.stripe.com",
          token: "sk_live_abc",
          allowMethods: ["GET"],
          allowPathPrefixes: ["/v1/charges"],
          retry: {
            maxAttempts: 4,
            initialDelayMs: 100,
            maxDelayMs: 1000,
            jitter: "none",
            retryOnStatuses: [429, 503],
            retryOnMethods: ["GET"],
            respectRetryAfter: false
          }
        })
      ],
      secrets: { apiKey: "k" },
      idempotencyKey: "i-px"
    });
    const body = calls[0]!.body;
    expect(body.proxyEndpoints).toEqual([
      {
        name: "stripe",
        baseUrl: "https://api.stripe.com",
        authShape: { type: "bearer" },
        allowMethods: ["GET"],
        allowPathPrefixes: ["/v1/charges"],
        retry: {
          maxAttempts: 4,
          initialDelayMs: 100,
          maxDelayMs: 1000,
          jitter: "none",
          retryOnStatuses: [429, 503],
          retryOnMethods: ["GET"],
          respectRetryAfter: false
        }
      }
    ]);
    const secrets = body.secrets as Record<string, unknown>;
    expect(secrets.proxyEndpointAuth).toEqual([
      { name: "stripe", value: { type: "bearer", token: "sk_live_abc" } }
    ]);
    // Token must not leak into the public submission declarations.
    const declJson = JSON.stringify(body.proxyEndpoints);
    expect(declJson).not.toContain("sk_live_abc");
  });

  it("rejects responseMode strings that don't match the wire enum", () => {
    expect(() =>
      ProxyEndpoint.bearer({
        name: "x",
        baseUrl: "https://x",
        token: "t",
        allowMethods: ["GET"],
        allowPathPrefixes: ["/"],
        // @ts-expect-error compile-time TS error too; we additionally
        // assert a runtime guard for callers writing JS or `any`-typed
        // configs.
        responseMode: "json"
      })
    ).toThrow(/responseMode/);
  });

  it("rejects an auth-header collision in allowHeaders", () => {
    expect(() =>
      ProxyEndpoint.bearer({
        name: "x",
        baseUrl: "https://x",
        token: "t",
        allowMethods: ["GET"],
        allowPathPrefixes: ["/"],
        allowHeaders: ["Authorization"]
      })
    ).toThrow(/auth header/);
  });

  it("rejects duplicate endpoint names within one submitRun call", async () => {
    const { fetch } = makeFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x.test", fetch });
    await expect(
      client.submitRun({
        model: "claude-haiku-4-5",
        prompt: "p",
        proxyEndpoints: [
          ProxyEndpoint.bearer({
            name: "dup",
            baseUrl: "https://a",
            token: "t1",
            allowMethods: ["GET"],
            allowPathPrefixes: ["/"]
          }),
          ProxyEndpoint.bearer({
            name: "dup",
            baseUrl: "https://b",
            token: "t2",
            allowMethods: ["GET"],
            allowPathPrefixes: ["/"]
          })
        ],
        secrets: { apiKey: "k" }
      })
    ).rejects.toThrow(/duplicate name/);
  });

  it("none() builds a declaration with authShape:none and no auth value", () => {
    const ep = ProxyEndpoint.none({
      name: "wikimedia",
      baseUrl: "https://commons.wikimedia.org",
      allowMethods: ["GET"],
      allowPathPrefixes: ["/wiki/", "/w/api.php"]
    });
    expect(ep.declaration.authShape).toEqual({ type: "none" });
    expect(ep.auth).toBeNull();
  });

  it("submitRun omits keyless endpoints from secrets.proxyEndpointAuth", async () => {
    const { fetch, calls } = makeFetch();
    const client = new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x.test", fetch });
    await client.submitRun({
      model: "claude-haiku-4-5",
      prompt: "p",
      proxyEndpoints: [
        ProxyEndpoint.none({
          name: "wikimedia",
          baseUrl: "https://commons.wikimedia.org",
          allowMethods: ["GET"],
          allowPathPrefixes: ["/wiki/"]
        }),
        ProxyEndpoint.bearer({
          name: "stripe",
          baseUrl: "https://api.stripe.com",
          token: "sk_live_abc",
          allowMethods: ["GET"],
          allowPathPrefixes: ["/v1/charges"]
        })
      ],
      secrets: { apiKey: "k" },
      idempotencyKey: "i-px-mixed"
    });
    const body = calls[0]!.body;
    const decls = body.proxyEndpoints as Array<{ name: string; authShape: { type: string } }>;
    expect(decls).toHaveLength(2);
    expect(decls.find((d) => d.name === "wikimedia")?.authShape).toEqual({ type: "none" });
    expect(decls.find((d) => d.name === "stripe")?.authShape).toEqual({ type: "bearer" });
    const secrets = body.secrets as Record<string, unknown>;
    // Keyless endpoint must NOT appear in the auth bag — only the bearer one.
    expect(secrets.proxyEndpointAuth).toEqual([
      { name: "stripe", value: { type: "bearer", token: "sk_live_abc" } }
    ]);
  });

  it("none() still rejects responseMode strings that don't match the wire enum", () => {
    expect(() =>
      ProxyEndpoint.none({
        name: "x",
        baseUrl: "https://x",
        allowMethods: ["GET"],
        allowPathPrefixes: ["/"],
        // @ts-expect-error runtime-enforced too
        responseMode: "json"
      })
    ).toThrow(/responseMode/);
  });
});

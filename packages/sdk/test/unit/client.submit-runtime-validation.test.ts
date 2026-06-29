import { describe, expect, it } from "vitest";
import { AgentExecutor } from "../../src/index.js";

function recordingFetch(): { fetch: typeof fetch; calls: string[] } {
  const calls: string[] = [];
  const f: typeof fetch = async (input) => {
    calls.push(typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url);
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
  };
  return { fetch: f, calls };
}

describe("AgentExecutor.submit — removed field validation", () => {
  it("rejects credentialMode without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.submit({
        credentialMode: "managed",
        model: "claude-haiku-4-5",
        prompt: "hi",
        secrets: { apiKeys: { anthropic: "sk-x" } }
      } as unknown as Parameters<AgentExecutor["submit"]>[0])
    ).rejects.toThrow(/credentialMode is not a supported option/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects runtime without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.submit({
        provider: "deepseek",
        runtime: "native",
        model: "deepseek-chat",
        prompt: "hi",
        secrets: { apiKeys: { deepseek: "sk-x" } }
      } as unknown as Parameters<AgentExecutor["submit"]>[0])
    ).rejects.toThrow(/runtime is not a supported option/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects secrets.apiKey without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "hi",
        secrets: { apiKey: "sk-x" }
      } as unknown as Parameters<AgentExecutor["submit"]>[0])
    ).rejects.toThrow(/secrets\.apiKey is not supported/);

    expect(rec.calls).toHaveLength(0);
  });
});

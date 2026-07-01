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

describe("Aex.openSession — removed field validation", () => {
  it("rejects the legacy runtimeSize field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        runtimeSize: "shared-1x-4gb",
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" }
      } as never)
    ).rejects.toThrow(/runtimeSize is not a supported option; use runtime/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy secretEnv field without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        secretEnv: { SERPER_API_KEY: { ref: "serper" } }
      } as never)
    ).rejects.toThrow(/secretEnv is not a supported option; use environment\.secrets/);

    expect(rec.calls).toHaveLength(0);
  });

  it("rejects the legacy nested secrets object without an HTTP call", async () => {
    const rec = recordingFetch();
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: rec.fetch });

    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        secrets: { apiKeys: { anthropic: "sk-x" } }
      } as never)
    ).rejects.toThrow(/secrets is not a supported option/);

    expect(rec.calls).toHaveLength(0);
  });
});

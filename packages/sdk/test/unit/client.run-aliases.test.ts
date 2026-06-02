import { describe, expect, it } from "vitest";
import { AntpathClient } from "../../src/index.js";

interface RecordedCall {
  readonly url: string;
  readonly method: string;
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function aliasClient(): { readonly client: AntpathClient; readonly calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    calls.push({ url, method });
    if (url.endsWith("/api/runs/run-1/events")) return json({ events: [{ id: "evt-1", type: "agent.message" }] });
    if (url.endsWith("/api/runs/run-1/outputs")) return json({ outputs: [] });
    if (url.endsWith("/api/runs/run-1/logs")) return json({ logs: [] });
    return json({ id: "run-1", status: "succeeded" });
  };
  return {
    client: new AntpathClient({ apiToken: "tkn", baseUrl: "https://example.test", fetch }),
    calls
  };
}

describe("AntpathClient run-id aliases", () => {
  it("delegate to the explicit run operations", async () => {
    const { client, calls } = aliasClient();

    await client.get("run-1");
    await client.getUnit("run-1");
    await client.events("run-1");

    const streamed: string[] = [];
    for await (const event of client.stream("run-1", { intervalMs: 1 })) {
      streamed.push(event.id);
    }
    expect(streamed).toEqual(["evt-1"]);

    await client.wait("run-1", { intervalMs: 1, timeoutMs: 100 });
    await client.outputs("run-1");
    await client.debugLogs("run-1");
    await client.cancel("run-1");
    await client.delete("run-1");

    expect(calls).toEqual([
      { url: "https://example.test/api/runs/run-1", method: "GET" },
      { url: "https://example.test/api/runs/run-1", method: "GET" },
      { url: "https://example.test/api/runs/run-1/events", method: "GET" },
      { url: "https://example.test/api/runs/run-1/events", method: "GET" },
      { url: "https://example.test/api/runs/run-1", method: "GET" },
      { url: "https://example.test/api/runs/run-1", method: "GET" },
      { url: "https://example.test/api/runs/run-1/outputs", method: "GET" },
      { url: "https://example.test/api/runs/run-1/logs", method: "GET" },
      { url: "https://example.test/api/runs/run-1/cancel", method: "POST" },
      { url: "https://example.test/api/runs/run-1", method: "DELETE" }
    ]);
  });
});

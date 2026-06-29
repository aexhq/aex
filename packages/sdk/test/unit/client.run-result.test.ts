import { describe, expect, it } from "vitest";
import { AgentExecutor, type RunResult } from "../../src/index.js";

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

const SETTLED_EVENTS = [
  { id: "e0", type: "RUN_STARTED", data: {} },
  { id: "e1", type: "TEXT_MESSAGE_CONTENT", data: { text: "hello ", messageId: "m1" } },
  { id: "e2", type: "TEXT_MESSAGE_CONTENT", data: { text: "world", messageId: "m1" } },
  { id: "e3", type: "RUN_FINISHED", data: { reason: "complete" } }
];

/** A fetch stub that drives one run to a terminal record + settled events. */
function runClient(run: Record<string, unknown>): { client: AgentExecutor; urls: string[] } {
  const urls: string[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    urls.push(`${method} ${url}`);
    if (url.endsWith("/api/runs/run-1/events")) return json({ events: SETTLED_EVENTS });
    if (url.endsWith("/api/runs/run-1/outputs")) return json({ outputs: [{ id: "o1", filename: "report.txt" }] });
    if (url.endsWith("/api/runs/run-1")) return json(run);
    if (url.endsWith("/api/runs")) return json({ id: "run-1", status: "queued" }); // submit
    return json({});
  };
  return { client: new AgentExecutor({ apiToken: "tkn", baseUrl: "https://x", fetch }), urls };
}

describe("AgentExecutor.run → RunResult", () => {
  it("returns a settle-consistent RunResult for a succeeded run", async () => {
    const { client } = runClient({
      id: "run-1",
      status: "succeeded",
      usage: { inputTokens: 10, outputTokens: 5, totalTokens: 15 },
      costTelemetry: { schemaVersion: "1", billedCostUsd: 0.0123 }
    });

    const result: RunResult = await client.run({
      model: "claude-haiku-4-5",
      prompt: "say hello world",
      secrets: { apiKeys: { anthropic: "sk-ant" } }
    });

    expect(result.runId).toBe("run-1");
    expect(result.ok).toBe(true);
    expect(result.status).toBe("succeeded");
    expect(result.text).toBe("hello world");
    expect(result.events.map((e) => e.type)).toEqual([
      "RUN_STARTED",
      "TEXT_MESSAGE_CONTENT",
      "TEXT_MESSAGE_CONTENT",
      "RUN_FINISHED"
    ]);
    expect(result.trace.text.map((t) => t.text)).toEqual(["hello ", "world"]);
    expect(result.outputs).toEqual([{ id: "o1", filename: "report.txt" }]);
    expect(result.usage).toEqual({ inputTokens: 10, outputTokens: 5, totalTokens: 15 });
    expect(result.costUsd).toBe(0.0123);
    expect(result.error).toBeUndefined();
  });

  it("runAndCollect is an alias for run", async () => {
    const { client } = runClient({ id: "run-1", status: "succeeded" });
    const result = await client.runAndCollect({
      model: "claude-haiku-4-5",
      prompt: "p",
      secrets: { apiKeys: { anthropic: "sk-ant" } }
    });
    expect(result.ok).toBe(true);
    expect(result.text).toBe("hello world");
  });

  it("returns ok:false with error for a failed run by default (no throw)", async () => {
    const { client } = runClient({ id: "run-1", status: "failed", errorMessage: "boom" });
    const result = await client.run({
      model: "claude-haiku-4-5",
      prompt: "p",
      secrets: { apiKeys: { anthropic: "sk-ant" } }
    });
    expect(result.ok).toBe(false);
    expect(result.status).toBe("failed");
    expect(result.error).toBe("boom");
  });

  it("throws when throwOnFailure is set and the run did not succeed", async () => {
    const { client } = runClient({ id: "run-1", status: "failed", errorMessage: "boom" });
    await expect(
      client.run(
        {
          model: "claude-haiku-4-5",
          prompt: "p",
          secrets: { apiKeys: { anthropic: "sk-ant" } }
        },
        { throwOnFailure: true }
      )
    ).rejects.toThrow(/run run-1 ended failed: boom/);
  });
});

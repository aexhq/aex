/**
 * `aex.sessions.searchOutputs` (chat-mvp) — client-side cross-session output
 * search. Drives a real AgentExecutor with a fake fetch so the contracts output
 * filter (filename / extension / contentType) runs end-to-end.
 */
import { describe, expect, it, vi } from "vitest";
import { AgentExecutor } from "../../src/index.js";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

const OUTPUTS: Record<string, Array<Record<string, unknown>>> = {
  "run-a": [
    { id: "a1", filename: "report.md", sizeBytes: 100, contentType: "text/markdown" },
    { id: "a2", filename: "data.json", sizeBytes: 50, contentType: "application/json" }
  ],
  "run-b": [
    { id: "b1", filename: "summary-report.md", sizeBytes: 200, contentType: "text/markdown" },
    { id: "b2", filename: "chart.png", sizeBytes: 999, contentType: "image/png" }
  ]
};

function makeClient(): { client: AgentExecutor; calls: string[] } {
  const calls: string[] = [];
  const fetchImpl: typeof fetch = vi.fn(async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    calls.push(url);
    const m = /\/api\/sessions\/([^/]+)\/outputs$/.exec(url);
    if (m) return jsonResponse({ outputs: OUTPUTS[m[1]!] ?? [] });
    if (/\/api\/sessions(\?|$)/.test(url)) {
      return jsonResponse({ sessions: [{ id: "run-a" }, { id: "run-b" }] });
    }
    throw new Error(`no responder for ${url}`);
  });
  return { client: new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: fetchImpl }), calls };
}

describe("aex.sessions.searchOutputs", () => {
  it("filters by filename substring across the corpus and returns references only", async () => {
    const { client } = makeClient();
    const page = await client.sessions.searchOutputs({ runIds: ["run-a", "run-b"], filename: "report" });
    const ids = page.hits.map((h) => `${h.runId}:${h.outputId}`);
    expect(ids).toEqual(["run-a:a1", "run-b:b1"]);
    // references only — no bytes/text
    expect(page.hits[0]).toEqual({ runId: "run-a", outputId: "a1", filename: "report.md", sizeBytes: 100, contentType: "text/markdown" });
  });

  it("filters by extension", async () => {
    const { client } = makeClient();
    const page = await client.sessions.searchOutputs({ runIds: ["run-a", "run-b"], extension: "json" });
    expect(page.hits.map((h) => h.outputId)).toEqual(["a2"]);
  });

  it("filters by content type wildcard", async () => {
    const { client } = makeClient();
    const page = await client.sessions.searchOutputs({ runIds: ["run-a", "run-b"], contentType: "image/*" });
    expect(page.hits.map((h) => h.outputId)).toEqual(["b2"]);
  });

  it("honors the limit", async () => {
    const { client } = makeClient();
    const page = await client.sessions.searchOutputs({ runIds: ["run-a", "run-b"], limit: 1 });
    expect(page.hits).toHaveLength(1);
    expect(page.hits[0]!.runId).toBe("run-a");
  });

  it("falls back to the whole workspace via listSessions when no runIds given", async () => {
    const { client, calls } = makeClient();
    const page = await client.sessions.searchOutputs({ extension: "md" });
    expect(page.hits.map((h) => h.outputId)).toEqual(["a1", "b1"]);
    expect(calls.some((u) => /\/api\/sessions(\?|$)/.test(u))).toBe(true);
  });
});

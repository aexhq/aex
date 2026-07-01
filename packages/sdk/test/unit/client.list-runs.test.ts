import { describe, expect, it } from "vitest";
import { AgentExecutor } from "../../src/index.js";

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

function listClient(page: unknown): { readonly client: AgentExecutor; readonly calls: RecordedCall[] } {
  const calls: RecordedCall[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    calls.push({ url, method: (init?.method ?? "GET").toString() });
    return json(page);
  };
  return {
    client: new AgentExecutor({ apiToken: "tkn", baseUrl: "https://example.test", fetch }),
    calls
  };
}

describe("aex.sessions.list", () => {
  it("GETs the bare /api/sessions collection and threads query params", async () => {
    const page = { sessions: [{ id: "sess-1", status: "idle", createdAt: "t", updatedAt: "t" }], nextCursor: "c2" };
    const { client, calls } = listClient(page);

    const result = await client.sessions.list({ status: "idle", limit: 2, cursor: "c1" });

    expect(result).toEqual(page);
    expect(calls).toHaveLength(1);
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/sessions");
    expect(calls[0]!.method).toBe("GET");
    expect(url.searchParams.get("status")).toBe("idle");
    expect(url.searchParams.get("limit")).toBe("2");
    expect(url.searchParams.get("cursor")).toBe("c1");
  });

  it("omits params when no query is given", async () => {
    const { client, calls } = listClient({ sessions: [] });
    await client.sessions.list();
    const url = new URL(calls[0]!.url);
    expect(url.pathname).toBe("/api/sessions");
    expect(url.search).toBe("");
  });
});

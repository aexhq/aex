/**
 * SessionFile search — RELOCATED (WS6): the cross-session search lives at
 * `aex.files.search(...)` and the per-session search at
 * `session.files(id).search(...)`; the old `aex.sessions.searchFiles` is
 * GONE. Filename accepts `string | RegExp` (no `escapeRegExp` crash), and a
 * content-shaped query throws a typed "content search unsupported".
 */
import { describe, expect, it, vi } from "vitest";
import { Aex } from "../../src/index.js";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function errorResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

const FILES: Record<string, Array<Record<string, unknown>>> = {
  "session-a": [
    { id: "a1", filename: "report.md", sizeBytes: 100, contentType: "text/markdown" },
    { id: "a2", filename: "data.json", sizeBytes: 50, contentType: "application/json" }
  ],
  "session-b": [
    { id: "b1", filename: "summary-report.md", sizeBytes: 200, contentType: "text/markdown" },
    { id: "b2", filename: "chart.png", sizeBytes: 999, contentType: "image/png" }
  ]
};

function makeClient(): { client: Aex; calls: string[] } {
  const calls: string[] = [];
  const fetchImpl: typeof fetch = vi.fn(async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    calls.push(url);
    const m = /\/api\/sessions\/([^/]+)\/files$/.exec(url);
    if (m) return jsonResponse({ files: FILES[m[1]!] ?? [] });
    if (/\/api\/sessions(\?|$)/.test(url)) {
      return jsonResponse({ sessions: [{ id: "session-a" }, { id: "session-b" }] });
    }
    throw new Error(`no responder for ${url}`);
  });
  return { client: new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: fetchImpl }), calls };
}

describe("aex.files.search (cross-session)", () => {
  it("RELOCATED: aex.sessions.searchFiles is removed", () => {
    const { client } = makeClient();
    expect((client.sessions as unknown as Record<string, unknown>).searchFiles).toBeUndefined();
    expect(typeof client.files.search).toBe("function");
  });

  it("filters by filename substring across the corpus and returns references only", async () => {
    const { client } = makeClient();
    const page = await client.files.search({ sessionIds: ["session-a", "session-b"], filename: "report" });
    const ids = page.hits.map((h) => `${h.sessionId}:${h.fileId}`);
    expect(ids).toEqual(["session-a:a1", "session-b:b1"]);
    // references only — no bytes/text
    expect(page.hits[0]).toEqual({ sessionId: "session-a", fileId: "a1", filename: "report.md", sizeBytes: 100, contentType: "text/markdown" });
  });

  it("accepts a RegExp filename (no escapeRegExp crash) with the SAME hits as the string form", async () => {
    const { client } = makeClient();
    const viaString = await client.files.search({ sessionIds: ["session-a", "session-b"], filename: "report" });
    const viaRegExp = await client.files.search({ sessionIds: ["session-a", "session-b"], filename: /report/i });
    expect(viaRegExp.hits.map((h) => h.fileId)).toEqual(viaString.hits.map((h) => h.fileId));
    expect(viaRegExp.hits.map((h) => h.fileId)).toEqual(["a1", "b1"]);
  });

  it("throws a typed 'content search unsupported' for a content-shaped query", async () => {
    const { client } = makeClient();
    await expect(
      client.files.search({ sessionIds: ["session-a"], content: "hunter2" } as never)
    ).rejects.toThrow(/content search is not supported/);
  });

  it("filters by extension", async () => {
    const { client } = makeClient();
    const page = await client.files.search({ sessionIds: ["session-a", "session-b"], extension: "json" });
    expect(page.hits.map((h) => h.fileId)).toEqual(["a2"]);
  });

  it("filters by content type wildcard", async () => {
    const { client } = makeClient();
    const page = await client.files.search({ sessionIds: ["session-a", "session-b"], contentType: "image/*" });
    expect(page.hits.map((h) => h.fileId)).toEqual(["b2"]);
  });

  it("honors the limit", async () => {
    const { client } = makeClient();
    const page = await client.files.search({ sessionIds: ["session-a", "session-b"], limit: 1 });
    expect(page.hits).toHaveLength(1);
    expect(page.hits[0]!.sessionId).toBe("session-a");
  });

  it("unscoped search stops paging sessions once the hit limit is satisfied", async () => {
    const calls: string[] = [];
    const fetchImpl: typeof fetch = vi.fn(async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push(url);
      const parsed = new URL(url);
      if (parsed.pathname === "/api/sessions" && parsed.searchParams.get("cursor") === null) {
        return jsonResponse({ sessions: [{ id: "session-a" }], nextCursor: "page-2" });
      }
      if (parsed.pathname === "/api/sessions" && parsed.searchParams.get("cursor") === "page-2") {
        return jsonResponse({ sessions: [{ id: "session-b" }] });
      }
      if (/\/api\/sessions\/session-a\/files$/.test(parsed.pathname)) {
        return jsonResponse({ files: FILES["session-a"] });
      }
      if (/\/api\/sessions\/session-b\/files$/.test(parsed.pathname)) {
        return jsonResponse({ files: FILES["session-b"] });
      }
      throw new Error(`no responder for ${url}`);
    });
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: fetchImpl });

    const page = await client.files.search({ limit: 1 });

    expect(page.hits).toHaveLength(1);
    expect(page.hits[0]!.sessionId).toBe("session-a");
    expect(calls.some((u) => new URL(u).searchParams.get("cursor") === "page-2")).toBe(false);
  });

  it("falls back to the whole workspace via listSessions when no sessionIds given", async () => {
    const { client, calls } = makeClient();
    const page = await client.files.search({ extension: "md" });
    expect(page.hits.map((h) => h.fileId)).toEqual(["a1", "b1"]);
    expect(calls.some((u) => /\/api\/sessions(\?|$)/.test(u))).toBe(true);
  });

  it("continues an unscoped workspace scan when a listed session is deleted before its files are read", async () => {
    const calls: string[] = [];
    const fetchImpl: typeof fetch = vi.fn(async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      calls.push(url);
      const parsed = new URL(url);
      const m = /\/api\/sessions\/([^/]+)\/files$/.exec(parsed.pathname);
      if (m?.[1] === "session-deleted") return errorResponse(404, { error: "not_found" });
      if (m) return jsonResponse({ files: FILES[m[1]!] ?? [] });
      if (parsed.pathname === "/api/sessions") {
        return jsonResponse({ sessions: [{ id: "session-deleted" }, { id: "session-a" }] });
      }
      throw new Error(`no responder for ${url}`);
    });
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: fetchImpl });

    const page = await client.files.search({ extension: "md" });

    expect(page.hits.map((h) => h.fileId)).toEqual(["a1"]);
    expect(calls).toContain("https://dash.test/api/sessions/session-deleted/files");
  });

  it("preserves 404s for explicitly scoped file searches", async () => {
    const fetchImpl: typeof fetch = vi.fn(async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (/\/api\/sessions\/session-deleted\/files$/.test(url)) return errorResponse(404, { error: "not_found" });
      throw new Error(`no responder for ${url}`);
    });
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: fetchImpl });

    await expect(client.files.search({ sessionIds: ["session-deleted"], extension: "md" })).rejects.toMatchObject({
      status: 404
    });
  });

  it("fails fast instead of looping forever when listSessions repeats a cursor", async () => {
    const fetchImpl: typeof fetch = vi.fn(async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      const parsed = new URL(url);
      if (parsed.pathname === "/api/sessions") {
        return jsonResponse({ sessions: [{ id: "session-a" }], nextCursor: "same_cursor" });
      }
      if (/\/api\/sessions\/session-a\/files$/.test(parsed.pathname)) {
        return jsonResponse({ files: [] });
      }
      throw new Error(`no responder for ${url}`);
    });
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: fetchImpl });

    await expect(client.files.search({ extension: "md" })).rejects.toThrow(/repeated cursor/);
  });
});

describe("session.files().search (per-session)", () => {
  it("searches ONE session's files and rejects a content-shaped query", async () => {
    const { client } = makeClient();
    const accessor = client.sessions.files("session-b");
    const page = await accessor.search({ filename: /report/i });
    expect(page.hits.map((h) => h.fileId)).toEqual(["b1"]);
    await expect(accessor.search({ text: "x" } as never)).rejects.toThrow(/content search is not supported/);
  });
});

import { describe, expect, it } from "vitest";
import { HttpClient, SessionStateError, type SessionFile } from "../src/index.js";
import { operations } from "../src/internal.js";

const BASE = "https://api.test";

interface RecordedCall {
  readonly path: string;
  readonly method: string;
  readonly body?: string;
  readonly authorization: string | null;
}

const files: readonly SessionFile[] = [
  { id: "json", checkpointId: "cp_1", filename: "files/reports/summary.json", contentType: "application/json", sizeBytes: 10 },
  { id: "txt", checkpointId: "cp_1", filename: "reports/notes.txt", contentType: "text/plain; charset=utf-8", sizeBytes: 20 },
  { id: "deep", checkpointId: "cp_1", filename: "reports/nested/frame.png", contentType: "image/png", sizeBytes: 30 },
  { id: "pdf", checkpointId: "cp_1", filename: "docs/spec.pdf", contentType: "application/octet-stream", sizeBytes: 40 },
  { id: "zip", checkpointId: "cp_1", filename: "bundle.zip", contentType: "application/zip", sizeBytes: 50 },
  { id: "video", checkpointId: "cp_1", filename: "media/clip.mp4", sizeBytes: 60 }
];

const snapshot = (items: readonly SessionFile[]) => ({
  revision: { checkpointId: "cp_1", runId: "run_1", turnSeq: 1, committedAt: "2026-07-10T00:00:00.000Z", throughSeq: 9 },
  files: items
});

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function clientFor(routes: Record<string, (init: RequestInit | undefined) => Response>) {
  const calls: RecordedCall[] = [];
  const fetchImpl = async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const parsed = new URL(url);
    const path = `${parsed.pathname}${parsed.search}`;
    const method = (init?.method ?? "GET").toString();
    calls.push({
      path,
      method,
      ...(typeof init?.body === "string" ? { body: init.body } : {}),
      authorization: new Headers(init?.headers).get("authorization")
    });
    return routes[path]?.(init) ?? json({ ok: false, code: "not_found" });
  };
  return { http: new HttpClient({ apiKey: "tok", baseUrl: BASE, fetch: fetchImpl }), calls };
}

describe("operations file discovery", () => {
  it("filters by normalized path, basename, directory, extension, content type, and high-level type", async () => {
    const { http } = clientFor({
      "/api/sessions/session-1/files": () => json(snapshot(files))
    });

    await expect(operations.listSessionFiles(http, "session-1", { path: "/files/reports/summary.json" }))
      .resolves.toMatchObject({ files: [files[0]] });
    await expect(operations.listSessionFiles(http, "session-1", { filename: /^notes\.txt$/ }))
      .resolves.toMatchObject({ files: [files[1]] });
    await expect(operations.listSessionFiles(http, "session-1", { dir: "reports", recursive: false }))
      .resolves.toMatchObject({ files: [files[0], files[1]] });
    await expect(operations.listSessionFiles(http, "session-1", { dir: "reports" }))
      .resolves.toMatchObject({ files: [files[0], files[1], files[2]] });
    await expect(operations.listSessionFiles(http, "session-1", { extension: ".json" }))
      .resolves.toMatchObject({ files: [files[0]] });
    await expect(operations.listSessionFiles(http, "session-1", { contentType: "image/*" }))
      .resolves.toMatchObject({ files: [files[2]] });
    await expect(operations.listSessionFiles(http, "session-1", { type: "video" }))
      .resolves.toMatchObject({ files: [files[5]] });
    await expect(operations.listSessionFiles(http, "session-1", { type: "pdf" }))
      .resolves.toMatchObject({ files: [] });
  });

  it("classifies from content type first and extension second", () => {
    expect(operations.classifySessionFile({ filename: "data.json", contentType: "application/octet-stream" })).toBe("binary");
    expect(operations.classifySessionFile({ filename: "spec.pdf" })).toBe("pdf");
    expect(operations.classifySessionFile({ filename: "clip.mp4" })).toBe("video");
    expect(operations.classifySessionFile({ filename: "bundle.tar.gz" })).toBe("archive");
  });

  it("returns null for no match and throws SessionStateError for ambiguous single-file lookup", async () => {
    const { http } = clientFor({
      "/api/sessions/session-1/files": () =>
        json(snapshot([
          { id: "a", checkpointId: "cp_1", filename: "a/report.txt" },
          { id: "b", checkpointId: "cp_1", filename: "b/report.txt" }
        ]))
    });

    await expect(operations.findSessionFile(http, "session-1", { filename: "missing.txt" })).resolves.toBeNull();
    await expect(operations.findSessionFile(http, "session-1", { extension: "txt" })).rejects.toBeInstanceOf(SessionStateError);
  });
});

describe("operations file links", () => {
  it("resolves a query, posts normalized TTL seconds, and returns resolved file metadata", async () => {
    const { http, calls } = clientFor({
      "/api/sessions/session-1/files": () => json(snapshot(files)),
      "/api/sessions/session-1/files/txt/link?checkpointId=cp_1": () =>
        json({ url: "https://storage.example/direct.txt", expiresAt: "2026-06-18T12:00:00.000Z" })
    });

    const link = await operations.sessionFileLink(http, "session-1", { filename: "notes.txt" }, { expiresIn: "15m" });

    expect(calls.map((call) => [call.method, call.path])).toEqual([
      ["GET", "/api/sessions/session-1/files"],
      ["POST", "/api/sessions/session-1/files/txt/link?checkpointId=cp_1"]
    ]);
    expect(JSON.parse(calls[1]!.body!)).toEqual({ expiresInSeconds: 900 });
    expect(link).toMatchObject({
      url: "https://storage.example/direct.txt",
      expiresInSeconds: 900,
      file: { id: "txt", filename: "reports/notes.txt" }
    });
  });

  it("uses a checkpoint-pinned id without listing files first", async () => {
    const { http, calls } = clientFor({
      "/api/sessions/session-1/files/txt/link?checkpointId=cp_1": () => json({ url: "https://storage.example/direct.txt" })
    });

    const link = await operations.sessionFileLink(http, "session-1", { id: "txt", checkpointId: "cp_1" });

    expect(calls.map((call) => [call.method, call.path])).toEqual([["POST", "/api/sessions/session-1/files/txt/link?checkpointId=cp_1"]]);
    expect(JSON.parse(calls[0]!.body!)).toEqual({ expiresInSeconds: 3600 });
    expect(link.expiresInSeconds).toBe(3600);
    expect(link.file).toEqual({ id: "txt", checkpointId: "cp_1" });
  });

  it("posts event archive link requests with the same TTL body", async () => {
    const { http, calls } = clientFor({
      "/api/sessions/session-1/events/link": () => json({ url: "https://storage.example/events.jsonl" })
    });

    const link = await operations.eventArchiveLink(http, "session-1", { expiresIn: "1d" });

    expect(calls.map((call) => [call.method, call.path])).toEqual([["POST", "/api/sessions/session-1/events/link"]]);
    expect(JSON.parse(calls[0]!.body!)).toEqual({ expiresInSeconds: 86400 });
    expect(link.expiresInSeconds).toBe(86400);
  });

  it("synthesizes the documented expiresAt when the server omits it", async () => {
    const { http } = clientFor({
      "/api/sessions/session-1/files/txt/link?checkpointId=cp_1": () =>
        json({ url: "https://storage.example/direct.txt", expiresInSeconds: 900 })
    });

    const before = Date.now();
    const link = await operations.sessionFileLink(http, "session-1", { id: "txt", checkpointId: "cp_1" }, { expiresIn: "15m" });
    const after = Date.now();

    expect(typeof link.expiresAt).toBe("string");
    const at = new Date(link.expiresAt!).getTime();
    expect(at).toBeGreaterThanOrEqual(before + 900_000);
    expect(at).toBeLessThanOrEqual(after + 900_000);
  });

  it("keeps a server-provided expiresAt untouched", async () => {
    const { http } = clientFor({
      "/api/sessions/session-1/files/txt/link?checkpointId=cp_1": () =>
        json({ url: "https://storage.example/direct.txt", expiresAt: "2026-06-18T12:00:00.000Z" })
    });

    const link = await operations.sessionFileLink(http, "session-1", { id: "txt", checkpointId: "cp_1" });
    expect(link.expiresAt).toBe("2026-06-18T12:00:00.000Z");
  });
});

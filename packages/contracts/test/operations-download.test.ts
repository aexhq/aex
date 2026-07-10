/**
 * The `download*` verbs assemble per-session zips client-side from the public read
 * endpoints. The public archive contains metadata, typed events, and files.
 * Internal diagnostics are not downloaded through this surface.
 */
import { describe, expect, it } from "vitest";
import { unzipSync } from "fflate";
import { HttpClient } from "../src/http.js";
import { SessionStateError, operations } from "../src/index.js";

const BASE = "https://api.test";
const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

function clientFor(routes: Record<string, () => Response>) {
  const fetchImpl = async (input: string | URL | Request) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = url.slice(BASE.length);
    const handler = routes[path];
    if (!handler) return new Response("not found", { status: 404 });
    return handler();
  };
  return new HttpClient({ apiKey: "tok", baseUrl: BASE, fetch: fetchImpl });
}

const json = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });

function runWithSessionFile() {
  return clientFor({
    "/api/sessions/session-1": () => json({ id: "session-1", status: "succeeded" }),
    "/api/sessions/session-1/events": () => json({ events: [{ seq: 0, kind: "a" }, { seq: 1, kind: "b" }] }),
    "/api/sessions/session-1/files": () =>
      json({ files: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }] }),
    "/api/sessions/session-1/files/o1/download": () => new Response("hello", { status: 200 })
  });
}

describe("operations.download", () => {
  it("bundles public metadata, typed events, files, and manifest", async () => {
    const entries = unzipSync(await operations.download(runWithSessionFile(), "session-1"));

    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "files/report.txt",
      "manifest.json",
      "metadata/session.json"
    ]);
    expect(JSON.parse(decode(entries["metadata/session.json"]!)).id).toBe("session-1");
    expect(decode(entries["events/events.jsonl"]!).split("\n").map((l) => JSON.parse(l))).toEqual([
      { seq: 0, kind: "a" },
      { seq: 1, kind: "b" }
    ]);
    expect(decode(entries["files/report.txt"]!)).toBe("hello");

    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.schemaVersion).toBe("aex.session-record.manifest.v1");
    expect(manifest.sessionRecordSchemaVersion).toBe("aex.session-record.v1");
    expect(manifest.namespaces.map((entry: { name: string }) => entry.name)).toEqual(["metadata", "events", "files"]);
    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/session.json", status: "present" }),
        expect.objectContaining({ path: "metadata/cost.json", status: "pending" }),
        expect.objectContaining({ path: "metadata/custody.json", status: "pending" }),
        expect.objectContaining({ path: "events/events.jsonl", role: "typed_events", recordCount: 2 }),
        expect.objectContaining({ path: "files/report.txt", role: "file", status: "present", id: "o1" })
      ])
    );
    expect(manifest.sessionFiles.map((file: { id: string }) => file.id)).toEqual(["o1"]);
    expect(manifest.logs).toBeUndefined();
    expect(manifest.errors).toEqual([]);
  });

  it("records a failed per-file fetch in manifest.errors without aborting the rest", async () => {
    const http = clientFor({
      "/api/sessions/session-1": () => json({ id: "session-1", status: "failed" }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () =>
        json({ files: [{ id: "ok", filename: "good.txt" }, { id: "bad", filename: "missing.txt" }] }),
      "/api/sessions/session-1/files/ok/download": () => new Response("present", { status: 200 }),
      "/api/sessions/session-1/files/bad/download": () => new Response("gone", { status: 404 })
    });

    const entries = unzipSync(await operations.download(http, "session-1"));

    expect(decode(entries["files/good.txt"]!)).toBe("present");
    expect(entries["files/missing.txt"]).toBeUndefined();
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.sessionFiles.map((file: { id: string }) => file.id)).toEqual(["ok"]);
    expect(manifest.errors).toHaveLength(1);
    expect(manifest.errors[0]).toMatchObject({ namespace: "files", id: "bad", filename: "missing.txt" });
  });

  it("rejects secret-shaped JSON/text archive entries before writing the zip", async () => {
    const http = clientFor({
      "/api/sessions/session-1": () => json({ id: "session-1", status: "failed", errorMessage: "Authorization: Bearer abcdefgh" }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () => json({ files: [] })
    });

    await expect(operations.download(http, "session-1")).rejects.toThrow(/session record archive contains non-public data/);
  });

  it("does not scan customer file bytes for secret-shaped content", async () => {
    const http = clientFor({
      "/api/sessions/session-1": () => json({ id: "session-1", status: "succeeded" }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () =>
        json({ files: [{ id: "o1", filename: "report.txt", sizeBytes: 27, contentType: "text/plain" }] }),
      "/api/sessions/session-1/files/o1/download": () => new Response("Authorization: Bearer abcdefgh", { status: 200 })
    });

    const entries = unzipSync(await operations.download(http, "session-1"));

    expect(decode(entries["files/report.txt"]!)).toBe("Authorization: Bearer abcdefgh");
  });

  it("allows public opaque artifact ids in manifest rows", async () => {
    const opaqueId = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    const http = clientFor({
      "/api/sessions/session-1": () => json({ id: "session-1", status: "succeeded" }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () =>
        json({ files: [{ id: opaqueId, filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }] }),
      [`/api/sessions/session-1/files/${opaqueId}/download`]: () => new Response("hello", { status: 200 })
    });

    const entries = unzipSync(await operations.download(http, "session-1"));
    const manifest = JSON.parse(decode(entries["manifest.json"]!));

    expect(manifest.sessionFiles[0].id).toBe(opaqueId);
  });
});

describe("operations.downloadSessionFiles", () => {
  it("zips deliverables at the root", async () => {
    const entries = unzipSync(await operations.downloadSessionFiles(runWithSessionFile(), "session-1"));

    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "report.txt"]);
    expect(decode(entries["report.txt"]!)).toBe("hello");
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.namespace).toBe("files");
    expect(manifest.files.map((o: { id: string }) => o.id)).toEqual(["o1"]);
  });
});

describe("operations.downloadSessionFile", () => {
  it("uses a 30s default per-attempt file transfer timeout for live redirects", () => {
    expect(operations.SESSION_FILE_TRANSFER_DEFAULT_TIMEOUT_MS).toBe(30_000);
  });

  it("downloads a file by exact listed filename", async () => {
    const result = await operations.downloadSessionFile(runWithSessionFile(), "session-1", { path: "/report.txt" });

    expect(result.file).toMatchObject({ id: "o1", filename: "report.txt" });
    expect(decode(result.bytes)).toBe("hello");
  });

  it("downloads a file by suffix when requested", async () => {
    const http = clientFor({
      "/api/sessions/session-2/files": () =>
        json({ files: [{ id: "report", filename: "files/report-folder/report.txt", contentType: "text/plain" }] }),
      "/api/sessions/session-2/files/report/download": () => new Response("marker", { status: 200 })
    });

    const result = await operations.downloadSessionFile(http, "session-2", { path: "report.txt", match: "suffix" });

    expect(result.file.filename).toBe("files/report-folder/report.txt");
    expect(decode(result.bytes)).toBe("marker");
  });

  it("rejects a missing path selector", async () => {
    await expect(
      operations.downloadSessionFile(runWithSessionFile(), "session-1", { path: "missing.txt" })
    ).rejects.toBeInstanceOf(SessionStateError);
  });

  it("rejects an ambiguous suffix selector", async () => {
    const http = clientFor({
      "/api/sessions/session-3/files": () =>
        json({ files: [{ id: "a", filename: "a/report.txt" }, { id: "b", filename: "b/report.txt" }] })
    });

    await expect(
      operations.downloadSessionFile(http, "session-3", { path: "report.txt", match: "suffix" })
    ).rejects.toThrow(/matched multiple files/);
  });

  it("downloads by file id without listing files first", async () => {
    const http = clientFor({
      "/api/sessions/session-4/files/o1/download": () => new Response("direct", { status: 200 })
    });

    const result = await operations.downloadSessionFile(http, "session-4", { id: "o1" });

    expect(result.file).toEqual({ id: "o1" });
    expect(decode(result.bytes)).toBe("direct");
  });

  it("preserves file metadata when the selector is a SessionFile object", async () => {
    const http = clientFor({
      "/api/sessions/session-5/files/o1/download": () => new Response("bytes", { status: 200 })
    });

    const result = await operations.downloadSessionFile(http, "session-5", {
      id: "o1",
      filename: "report.txt",
      contentType: "text/plain"
    });

    expect(result.file).toMatchObject({ id: "o1", filename: "report.txt", contentType: "text/plain" });
    expect(decode(result.bytes)).toBe("bytes");
  });
});

describe("operations.downloadEvents", () => {
  it("zips the indexed event archive with an events namespace manifest", async () => {
    const entries = unzipSync(await operations.downloadEvents(runWithSessionFile(), "session-1"));
    expect(Object.keys(entries).sort()).toEqual(["events.jsonl", "manifest.json"]);
    expect(decode(entries["events.jsonl"]!).split("\n").map((l) => JSON.parse(l))).toEqual([
      { seq: 0, kind: "a" },
      { seq: 1, kind: "b" }
    ]);
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest).toMatchObject({
      sessionId: "session-1",
      namespace: "events",
      files: [{ path: "events.jsonl", role: "typed_events", status: "present", recordCount: 2 }],
      errors: []
    });
  });
});

describe("operations.downloadMetadata", () => {
  it("zips the session record with a metadata namespace manifest", async () => {
    const entries = unzipSync(await operations.downloadMetadata(runWithSessionFile(), "session-1"));
    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "session.json"]);
    expect(JSON.parse(decode(entries["session.json"]!)).id).toBe("session-1");
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest).toMatchObject({
      sessionId: "session-1",
      namespace: "metadata",
      files: [{ path: "session.json", role: "session_metadata", status: "present" }],
      errors: []
    });
  });
});

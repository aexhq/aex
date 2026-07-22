/**
 * The `download*` verbs assemble per-session zips client-side from the public read
 * endpoints. The public archive contains metadata, typed events, and files.
 * Internal diagnostics are not downloaded through this surface.
 */
import { describe, expect, it } from "vitest";
import { unzipSync } from "fflate";
import { createHash } from "node:crypto";
import { HttpClient } from "../src/http.js";
import { SessionStateError } from "../src/index.js";
import { operations } from "../src/internal.js";

const BASE = "https://api.test";
const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
const file = (
  id: string,
  filename: string,
  contents: string,
  extra: Record<string, unknown> = {}
) => ({
  id,
  filename,
  sizeBytes: new TextEncoder().encode(contents).byteLength,
  sha256: createHash("sha256").update(contents).digest("hex"),
  ...extra
});

function clientFor(routes: Record<string, () => Response>) {
  const fetchImpl = async (input: string | URL | Request) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const path = url.slice(BASE.length);
    const handler = routes[path] ?? routes[path.replace(/\?checkpointId=[^&]+$/, "")];
    if (!handler) return new Response("not found", { status: 404 });
    return handler();
  };
  return new HttpClient({ apiKey: "tok", baseUrl: BASE, fetch: fetchImpl });
}

const json = (body: unknown) => {
  const record = body && typeof body === "object" && !Array.isArray(body) ? body as Record<string, unknown> : undefined;
  const normalized = record && Array.isArray(record.files) && record.revision === undefined
    ? {
        ...record,
        revision: {
          checkpointId: "cp_1",
          runId: "run_1",
          turnSeq: 1,
          committedAt: "2026-07-10T00:00:00.000Z",
          throughSeq: 9
        },
        files: record.files.map((file) => ({ ...(file as object), checkpointId: "cp_1" }))
      }
    : body;
  return new Response(JSON.stringify(normalized), { status: 200, headers: { "content-type": "application/json" } });
};

function runWithSessionFile() {
  return clientFor({
    "/api/sessions/session-1": () => json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }),
    "/api/sessions/session-1/events": () => json({ events: [{ seq: 0, kind: "a" }, { seq: 1, kind: "b" }] }),
    "/api/sessions/session-1/files": () =>
      json({ files: [file("o1", "report.txt", "hello", { contentType: "text/plain" })] }),
    "/api/sessions/session-1/files/o1/download": () => new Response("hello", { status: 200 })
  });
}

describe("operations.download", () => {
  it("keeps every deterministic archive byte-for-byte stable", async () => {
    const digest = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");
    const http = runWithSessionFile();

    await expect(Promise.all([
      operations.download(http, "session-1"),
      operations.downloadSessionFiles(http, "session-1"),
      operations.downloadEvents(http, "session-1"),
      operations.downloadMetadata(http, "session-1")
    ]).then((archives) => archives.map(digest))).resolves.toEqual([
      "13b09c985f2073f35f0363780464e23076220f6c55ef5bf334d1b907f8ab6896",
      "160fb3cb0008a7a95e9a323a66445294cbcae7a08cb2258253791768d1575aae",
      "a08eb8f3d19a5bed6647a00c8ddc356d348dcb4e70c8c0795801762383689a78",
      "0e88fd02076b0ac7f19d7662cba355d0aa870e0e004561b5418bebcc49accab0"
    ]);
  });

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
      "/api/sessions/session-1": () => json({ session: { id: "session-1", status: "error", acceptsMessages: true } }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () =>
        json({ files: [file("ok", "good.txt", "present"), file("bad", "missing.txt", "gone")] }),
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

  it("treats checkpoint integrity mismatch as terminal instead of returning a partial archive", async () => {
    let downloads = 0;
    const http = clientFor({
      "/api/sessions/session-1": () => json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () => json({ files: [file("bad", "report.txt", "expected")] }),
      "/api/sessions/session-1/files/bad/download": () => {
        downloads += 1;
        return new Response("tampered", { status: 200 });
      }
    });

    await expect(operations.download(http, "session-1")).rejects.toThrow(/integrity/i);
    expect(downloads).toBe(1);
  });

  it("rejects secret-shaped JSON/text archive entries before writing the zip", async () => {
    const http = clientFor({
      "/api/sessions/session-1": () => json({ session: { id: "session-1", status: "error", acceptsMessages: true, errorMessage: "Authorization: Bearer abcdefgh" } }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () => json({ files: [] })
    });

    await expect(operations.download(http, "session-1")).rejects.toThrow(/session record archive contains non-public data/);
  });

  it("does not scan customer file bytes for secret-shaped content", async () => {
    const http = clientFor({
      "/api/sessions/session-1": () => json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () =>
        json({ files: [file("o1", "report.txt", "Authorization: Bearer abcdefgh", { contentType: "text/plain" })] }),
      "/api/sessions/session-1/files/o1/download": () => new Response("Authorization: Bearer abcdefgh", { status: 200 })
    });

    const entries = unzipSync(await operations.download(http, "session-1"));

    expect(decode(entries["files/report.txt"]!)).toBe("Authorization: Bearer abcdefgh");
  });

  it("allows public opaque artifact ids in manifest rows", async () => {
    const opaqueId = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    const http = clientFor({
      "/api/sessions/session-1": () => json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }),
      "/api/sessions/session-1/events": () => json({ events: [] }),
      "/api/sessions/session-1/files": () =>
        json({ files: [file(opaqueId, "report.txt", "hello", { contentType: "text/plain" })] }),
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
        json({ files: [file("report", "files/report-folder/report.txt", "marker", { contentType: "text/plain" })] }),
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
        json({ files: [file("a", "a/report.txt", "a"), file("b", "b/report.txt", "b")] })
    });

    await expect(
      operations.downloadSessionFile(http, "session-3", { path: "report.txt", match: "suffix" })
    ).rejects.toThrow(/matched multiple files/);
  });

  it("resolves a file id from its pinned checkpoint before downloading", async () => {
    const http = clientFor({
      "/api/sessions/session-4/files": () => json({ files: [file("o1", "report.txt", "direct")] }),
      "/api/sessions/session-4/files/o1/download": () => new Response("direct", { status: 200 })
    });

    const result = await operations.downloadSessionFile(http, "session-4", { id: "o1", checkpointId: "cp_1" });

    expect(result.file).toMatchObject({ id: "o1", checkpointId: "cp_1", filename: "report.txt" });
    expect(decode(result.bytes)).toBe("direct");
  });

  it("uses authoritative checkpoint metadata instead of caller-supplied file metadata", async () => {
    const http = clientFor({
      "/api/sessions/session-5/files": () =>
        json({ files: [file("o1", "report.txt", "bytes", { contentType: "text/plain" })] }),
      "/api/sessions/session-5/files/o1/download": () => new Response("bytes", { status: 200 })
    });

    const result = await operations.downloadSessionFile(http, "session-5", {
      id: "o1",
      checkpointId: "cp_1",
      filename: "stale.txt",
      sizeBytes: 999,
      sha256: "0".repeat(64),
      contentType: "application/octet-stream"
    });

    expect(result.file).toMatchObject({ id: "o1", filename: "report.txt", contentType: "text/plain" });
    expect(decode(result.bytes)).toBe("bytes");
  });

  it.each([
    ["size", file("o1", "report.txt", "longer")],
    ["sha256", { ...file("o1", "report.txt", "bytes"), sha256: "0".repeat(64) }]
  ])("rejects a %s mismatch after one download attempt", async (_kind, listedFile) => {
    let downloads = 0;
    const http = clientFor({
      "/api/sessions/session-6/files": () => json({ files: [listedFile] }),
      "/api/sessions/session-6/files/o1/download": () => {
        downloads += 1;
        return new Response("bytes", { status: 200 });
      }
    });

    await expect(
      operations.downloadSessionFile(http, "session-6", { id: "o1", checkpointId: "cp_1" })
    ).rejects.toThrow(/integrity/i);
    expect(downloads).toBe(1);
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

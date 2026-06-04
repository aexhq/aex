/**
 * The `download*` verbs assemble per-run zips CLIENT-SIDE from the public
 * read endpoints — there is no server-side archive route. A run's content
 * is organised into four namespaces:
 *
 *   - `download`         → metadata/ + events/ + outputs/ + logs/ + manifest
 *   - `downloadOutputs`  → deliverables only (debug artifacts excluded)
 *   - `downloadLogs`     → platform diagnostics only
 *   - `downloadEvents`   → events.jsonl
 *   - `downloadMetadata` → run.json
 *
 * These tests pin the zip layouts, the outputs-vs-logs split, canonical log
 * namespace normalization, and the
 * best-effort contract: a per-output byte fetch that fails is recorded in
 * `manifest.errors[]` rather than aborting the whole archive (surfaced,
 * never silent).
 */
import { describe, expect, it } from "vitest";
import { unzipSync } from "fflate";
import { HttpClient } from "../src/http.js";
import { RunStateError, operations } from "../src/index.js";

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
  return new HttpClient({ apiToken: "tok", baseUrl: BASE, fetch: fetchImpl });
}

const json = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });

// A run with one deliverable (report.txt, served from the `outputs`
// namespace) and one legacy-named diagnostic served from the `logs` namespace.
function runWithOutputAndLog() {
  return clientFor({
    "/api/runs/run-1": () => json({ id: "run-1", status: "succeeded" }),
    "/api/runs/run-1/events": () => json({ events: [{ seq: 0, kind: "a" }, { seq: 1, kind: "b" }] }),
    "/api/runs/run-1/outputs": () =>
      json({ outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }] }),
    "/api/runs/run-1/logs": () =>
      json({ logs: [{ id: "o2", filename: "anthropic-debug/files-list.json", sizeBytes: 2, contentType: "application/json" }] }),
    "/api/runs/run-1/outputs/o1/download": () => new Response("hello", { status: 200 }),
    "/api/runs/run-1/logs/o2/download": () => new Response("{}", { status: 200 })
  });
}

describe("operations.download (everything)", () => {
  it("bundles the four namespace folders with deliverables and logs split apart", async () => {
    const entries = unzipSync(await operations.download(runWithOutputAndLog(), "run-1"));

    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "logs/provider-proxy/files-list.json",
      "manifest.json",
      "metadata/run.json",
      "outputs/report.txt"
    ]);
    expect(JSON.parse(decode(entries["metadata/run.json"]!)).id).toBe("run-1");
    expect(decode(entries["events/events.jsonl"]!).split("\n")).toHaveLength(2);
    expect(decode(entries["outputs/report.txt"]!)).toBe("hello");

    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.schemaVersion).toBe("aex.run-record.manifest.v1");
    expect(manifest.runRecordSchemaVersion).toBe("aex.run-record.v1");
    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/run.json", status: "present" }),
        expect.objectContaining({ path: "metadata/cost.json", status: "pending" }),
        expect.objectContaining({ path: "metadata/custody.json", status: "pending" }),
        expect.objectContaining({ path: "events/events.jsonl", role: "typed_events", recordCount: 2 }),
        expect.objectContaining({ path: "events/logs.jsonl", role: "log_events", status: "unavailable" })
      ])
    );
    expect(manifest.outputs.map((o: { id: string }) => o.id)).toEqual(["o1"]);
    expect(manifest.logs.map((o: { id: string }) => o.id)).toEqual(["o2"]);
    expect(manifest.errors).toEqual([]);
  });

  it("adds log-channel and full-stream JSONL files when the event API proves they are available", async () => {
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "succeeded" }),
      "/api/runs/run-1/events": () => json({ events: [{ id: "e1", type: "TEXT_MESSAGE_CONTENT" }] }),
      "/api/runs/run-1/events?channel=log": () =>
        json({ events: [{ id: "l1", type: "LOG", channel: "log", level: "info", message: "settled" }] }),
      "/api/runs/run-1/events?channel=all": () =>
        json({
          events: [
            { id: "e1", type: "TEXT_MESSAGE_CONTENT" },
            { id: "l1", type: "LOG", channel: "log", level: "info", message: "settled" }
          ]
        }),
      "/api/runs/run-1/outputs": () => json({ outputs: [] }),
      "/api/runs/run-1/logs": () => json({ logs: [] })
    });

    const entries = unzipSync(await operations.download(http, "run-1"));

    expect(Object.keys(entries).sort()).toEqual([
      "events/all.jsonl",
      "events/events.jsonl",
      "events/logs.jsonl",
      "manifest.json",
      "metadata/run.json"
    ]);
    expect(decode(entries["events/logs.jsonl"]!)).toContain("\"channel\":\"log\"");
    expect(decode(entries["events/all.jsonl"]!).split("\n")).toHaveLength(2);
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "events/logs.jsonl", role: "log_events", status: "present", recordCount: 1 }),
        expect.objectContaining({ path: "events/all.jsonl", role: "all_events", status: "present", recordCount: 2 })
      ])
    );
  });

  it("records a failed per-output fetch in manifest.errors without aborting the rest", async () => {
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "failed" }),
      "/api/runs/run-1/events": () => json({ events: [] }),
      "/api/runs/run-1/outputs": () =>
        json({ outputs: [{ id: "ok", filename: "good.txt" }, { id: "bad", filename: "missing.txt" }] }),
      "/api/runs/run-1/logs": () => json({ logs: [] }),
      "/api/runs/run-1/outputs/ok/download": () => new Response("present", { status: 200 }),
      // R2 object gone — the per-output download 404s.
      "/api/runs/run-1/outputs/bad/download": () => new Response("gone", { status: 404 })
    });

    const entries = unzipSync(await operations.download(http, "run-1"));

    expect(decode(entries["outputs/good.txt"]!)).toBe("present");
    expect(entries["outputs/missing.txt"]).toBeUndefined();
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.outputs.map((o: { id: string }) => o.id)).toEqual(["ok"]);
    expect(manifest.errors).toHaveLength(1);
    expect(manifest.errors[0]).toMatchObject({ namespace: "outputs", id: "bad", filename: "missing.txt" });
  });

  it("rejects secret-shaped JSON/text archive entries before writing the zip", async () => {
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "failed", errorMessage: "Authorization: Bearer abcdefgh" }),
      "/api/runs/run-1/events": () => json({ events: [] }),
      "/api/runs/run-1/outputs": () => json({ outputs: [] }),
      "/api/runs/run-1/logs": () => json({ logs: [] })
    });

    await expect(operations.download(http, "run-1")).rejects.toThrow(/run record archive contains non-public data/);
  });

  it("does not scan customer output bytes for secret-shaped content", async () => {
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "succeeded" }),
      "/api/runs/run-1/events": () => json({ events: [] }),
      "/api/runs/run-1/outputs": () =>
        json({ outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 27, contentType: "text/plain" }] }),
      "/api/runs/run-1/logs": () => json({ logs: [] }),
      "/api/runs/run-1/outputs/o1/download": () => new Response("Authorization: Bearer abcdefgh", { status: 200 })
    });

    const entries = unzipSync(await operations.download(http, "run-1"));

    expect(decode(entries["outputs/report.txt"]!)).toBe("Authorization: Bearer abcdefgh");
  });

  it("allows public opaque artifact ids in manifest rows", async () => {
    const opaqueId = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "succeeded" }),
      "/api/runs/run-1/events": () => json({ events: [] }),
      "/api/runs/run-1/outputs": () =>
        json({ outputs: [{ id: opaqueId, filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }] }),
      "/api/runs/run-1/logs": () => json({ logs: [] }),
      [`/api/runs/run-1/outputs/${opaqueId}/download`]: () => new Response("hello", { status: 200 })
    });

    const entries = unzipSync(await operations.download(http, "run-1"));
    const manifest = JSON.parse(decode(entries["manifest.json"]!));

    expect(manifest.outputs[0].id).toBe(opaqueId);
  });
});

describe("operations.downloadOutputs (deliverables only)", () => {
  it("zips deliverables at the root and excludes the logs namespace", async () => {
    const entries = unzipSync(await operations.downloadOutputs(runWithOutputAndLog(), "run-1"));

    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "report.txt"]);
    expect(decode(entries["report.txt"]!)).toBe("hello");
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.namespace).toBe("outputs");
    expect(manifest.outputs.map((o: { id: string }) => o.id)).toEqual(["o1"]);
  });
});

describe("operations.downloadOutput (single deliverable)", () => {
  it("downloads an output by exact listed filename", async () => {
    const result = await operations.downloadOutput(runWithOutputAndLog(), "run-1", { path: "/report.txt" });

    expect(result.output).toMatchObject({ id: "o1", filename: "report.txt" });
    expect(decode(result.bytes)).toBe("hello");
  });

  it("downloads an output by suffix when requested", async () => {
    const http = clientFor({
      "/api/runs/run-2/outputs": () =>
        json({ outputs: [{ id: "report", filename: "outputs/report-folder/report.txt", contentType: "text/plain" }] }),
      "/api/runs/run-2/outputs/report/download": () => new Response("marker", { status: 200 })
    });

    const result = await operations.downloadOutput(http, "run-2", { path: "report.txt", match: "suffix" });

    expect(result.output.filename).toBe("outputs/report-folder/report.txt");
    expect(decode(result.bytes)).toBe("marker");
  });

  it("rejects a missing path selector", async () => {
    await expect(
      operations.downloadOutput(runWithOutputAndLog(), "run-1", { path: "missing.txt" })
    ).rejects.toBeInstanceOf(RunStateError);
  });

  it("rejects an ambiguous suffix selector", async () => {
    const http = clientFor({
      "/api/runs/run-3/outputs": () =>
        json({ outputs: [{ id: "a", filename: "a/report.txt" }, { id: "b", filename: "b/report.txt" }] })
    });

    await expect(
      operations.downloadOutput(http, "run-3", { path: "report.txt", match: "suffix" })
    ).rejects.toThrow(/matched multiple files/);
  });

  it("downloads by output id without listing outputs first", async () => {
    const http = clientFor({
      "/api/runs/run-4/outputs/o1/download": () => new Response("direct", { status: 200 })
    });

    const result = await operations.downloadOutput(http, "run-4", { id: "o1" });

    expect(result.output).toEqual({ id: "o1" });
    expect(decode(result.bytes)).toBe("direct");
  });

  it("preserves output metadata when the selector is an Output object", async () => {
    const http = clientFor({
      "/api/runs/run-5/outputs/o1/download": () => new Response("bytes", { status: 200 })
    });

    const result = await operations.downloadOutput(http, "run-5", {
      id: "o1",
      filename: "report.txt",
      contentType: "text/plain"
    });

    expect(result.output).toMatchObject({ id: "o1", filename: "report.txt", contentType: "text/plain" });
    expect(decode(result.bytes)).toBe("bytes");
  });
});

describe("operations.downloadLogs (diagnostics only)", () => {
  it("zips only canonical log artifacts", async () => {
    const entries = unzipSync(await operations.downloadLogs(runWithOutputAndLog(), "run-1"));

    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "provider-proxy/files-list.json"]);
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.namespace).toBe("logs");
    expect(manifest.logs.map((o: { id: string }) => o.id)).toEqual(["o2"]);
  });
});

describe("operations.downloadEvents", () => {
  it("zips the indexed event archive as events.jsonl", async () => {
    const entries = unzipSync(await operations.downloadEvents(runWithOutputAndLog(), "run-1"));
    expect(Object.keys(entries)).toEqual(["events.jsonl"]);
    expect(decode(entries["events.jsonl"]!).split("\n")).toHaveLength(2);
  });

  it("includes event-channel opt-in files when available", async () => {
    const http = clientFor({
      "/api/runs/run-1/events": () => json({ events: [{ id: "e1", type: "TEXT_MESSAGE_CONTENT" }] }),
      "/api/runs/run-1/events?channel=log": () =>
        json({ events: [{ id: "l1", type: "LOG", channel: "log", level: "warn", message: "retry" }] }),
      "/api/runs/run-1/events?channel=all": () =>
        json({
          events: [
            { id: "e1", type: "TEXT_MESSAGE_CONTENT" },
            { id: "l1", type: "LOG", channel: "log", level: "warn", message: "retry" }
          ]
        })
    });

    const entries = unzipSync(await operations.downloadEvents(http, "run-1"));

    expect(Object.keys(entries).sort()).toEqual(["all.jsonl", "events.jsonl", "logs.jsonl"]);
  });
});

describe("operations.downloadMetadata", () => {
  it("zips the run record as run.json", async () => {
    const entries = unzipSync(await operations.downloadMetadata(runWithOutputAndLog(), "run-1"));
    expect(Object.keys(entries)).toEqual(["run.json"]);
    expect(JSON.parse(decode(entries["run.json"]!)).id).toBe("run-1");
  });
});

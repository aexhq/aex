/**
 * The `download*` verbs assemble per-run zips client-side from the public read
 * endpoints. The public archive contains metadata, typed events, and outputs.
 * Internal diagnostics are not downloaded through this surface.
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
  return new HttpClient({ apiKey: "tok", baseUrl: BASE, fetch: fetchImpl });
}

const json = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });

function runWithOutput() {
  return clientFor({
    "/api/runs/run-1": () => json({ id: "run-1", status: "succeeded" }),
    "/api/runs/run-1/events": () => json({ events: [{ seq: 0, kind: "a" }, { seq: 1, kind: "b" }] }),
    "/api/runs/run-1/outputs": () =>
      json({ outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }] }),
    "/api/runs/run-1/outputs/o1/download": () => new Response("hello", { status: 200 })
  });
}

describe("operations.download", () => {
  it("bundles public metadata, typed events, outputs, and manifest", async () => {
    const entries = unzipSync(await operations.download(runWithOutput(), "run-1"));

    expect(Object.keys(entries).sort()).toEqual([
      "events/events.jsonl",
      "manifest.json",
      "metadata/run.json",
      "outputs/report.txt"
    ]);
    expect(JSON.parse(decode(entries["metadata/run.json"]!)).id).toBe("run-1");
    expect(decode(entries["events/events.jsonl"]!).split("\n").map((l) => JSON.parse(l))).toEqual([
      { seq: 0, kind: "a" },
      { seq: 1, kind: "b" }
    ]);
    expect(decode(entries["outputs/report.txt"]!)).toBe("hello");

    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.schemaVersion).toBe("aex.run-record.manifest.v1");
    expect(manifest.runRecordSchemaVersion).toBe("aex.run-record.v1");
    expect(manifest.namespaces.map((entry: { name: string }) => entry.name)).toEqual(["metadata", "events", "outputs"]);
    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/run.json", status: "present" }),
        expect.objectContaining({ path: "metadata/cost.json", status: "pending" }),
        expect.objectContaining({ path: "metadata/custody.json", status: "pending" }),
        expect.objectContaining({ path: "events/events.jsonl", role: "typed_events", recordCount: 2 }),
        expect.objectContaining({ path: "outputs/report.txt", role: "output", status: "present", id: "o1" })
      ])
    );
    expect(manifest.outputs.map((o: { id: string }) => o.id)).toEqual(["o1"]);
    expect(manifest.logs).toBeUndefined();
    expect(manifest.errors).toEqual([]);
  });

  it("records a failed per-output fetch in manifest.errors without aborting the rest", async () => {
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "failed" }),
      "/api/runs/run-1/events": () => json({ events: [] }),
      "/api/runs/run-1/outputs": () =>
        json({ outputs: [{ id: "ok", filename: "good.txt" }, { id: "bad", filename: "missing.txt" }] }),
      "/api/runs/run-1/outputs/ok/download": () => new Response("present", { status: 200 }),
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
      "/api/runs/run-1/outputs": () => json({ outputs: [] })
    });

    await expect(operations.download(http, "run-1")).rejects.toThrow(/run record archive contains non-public data/);
  });

  it("does not scan customer output bytes for secret-shaped content", async () => {
    const http = clientFor({
      "/api/runs/run-1": () => json({ id: "run-1", status: "succeeded" }),
      "/api/runs/run-1/events": () => json({ events: [] }),
      "/api/runs/run-1/outputs": () =>
        json({ outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 27, contentType: "text/plain" }] }),
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
      [`/api/runs/run-1/outputs/${opaqueId}/download`]: () => new Response("hello", { status: 200 })
    });

    const entries = unzipSync(await operations.download(http, "run-1"));
    const manifest = JSON.parse(decode(entries["manifest.json"]!));

    expect(manifest.outputs[0].id).toBe(opaqueId);
  });
});

describe("operations.downloadOutputs", () => {
  it("zips deliverables at the root", async () => {
    const entries = unzipSync(await operations.downloadOutputs(runWithOutput(), "run-1"));

    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "report.txt"]);
    expect(decode(entries["report.txt"]!)).toBe("hello");
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest.namespace).toBe("outputs");
    expect(manifest.outputs.map((o: { id: string }) => o.id)).toEqual(["o1"]);
  });
});

describe("operations.downloadOutput", () => {
  it("downloads an output by exact listed filename", async () => {
    const result = await operations.downloadOutput(runWithOutput(), "run-1", { path: "/report.txt" });

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
      operations.downloadOutput(runWithOutput(), "run-1", { path: "missing.txt" })
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

describe("operations.downloadEvents", () => {
  it("zips the indexed event archive with an events namespace manifest", async () => {
    const entries = unzipSync(await operations.downloadEvents(runWithOutput(), "run-1"));
    expect(Object.keys(entries).sort()).toEqual(["events.jsonl", "manifest.json"]);
    expect(decode(entries["events.jsonl"]!).split("\n").map((l) => JSON.parse(l))).toEqual([
      { seq: 0, kind: "a" },
      { seq: 1, kind: "b" }
    ]);
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest).toMatchObject({
      runId: "run-1",
      namespace: "events",
      files: [{ path: "events.jsonl", role: "typed_events", status: "present", recordCount: 2 }],
      errors: []
    });
  });
});

describe("operations.downloadMetadata", () => {
  it("zips the run record with a metadata namespace manifest", async () => {
    const entries = unzipSync(await operations.downloadMetadata(runWithOutput(), "run-1"));
    expect(Object.keys(entries).sort()).toEqual(["manifest.json", "run.json"]);
    expect(JSON.parse(decode(entries["run.json"]!)).id).toBe("run-1");
    const manifest = JSON.parse(decode(entries["manifest.json"]!));
    expect(manifest).toMatchObject({
      runId: "run-1",
      namespace: "metadata",
      files: [{ path: "run.json", role: "run_metadata", status: "present" }],
      errors: []
    });
  });
});

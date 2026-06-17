import { describe, expect, it } from "vitest";
import {
  RUN_RECORD_MANIFEST_SCHEMA_VERSION,
  RUN_RECORD_SCHEMA_VERSION,
  assertRunRecordArchivePublicSafeV1,
  buildRunRecordDownloadManifestV1,
  scanRunRecordArchiveEntriesV1,
  type RunRecordArchiveEntryForRedactionV1
} from "../src/run-record.js";

describe("RunRecordV1 manifest helpers", () => {
  it("builds the versioned public whole-run download manifest", () => {
    const manifest = buildRunRecordDownloadManifestV1({
      runId: "run-1",
      typedEventCount: 2,
      outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }],
      errors: [{ namespace: "outputs", id: "missing", filename: "missing.txt", message: "not found" }]
    });

    expect(manifest.schemaVersion).toBe(RUN_RECORD_MANIFEST_SCHEMA_VERSION);
    expect(manifest.runRecordSchemaVersion).toBe(RUN_RECORD_SCHEMA_VERSION);
    expect(manifest.namespaces.map((entry) => entry.name)).toEqual(["metadata", "events", "outputs"]);
    expect(manifest.outputs.map((entry) => entry.id)).toEqual(["o1"]);
    expect(manifest.errors[0]).toMatchObject({ namespace: "outputs", id: "missing" });

    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/run.json", role: "run_metadata", status: "present" }),
        expect.objectContaining({ path: "metadata/submission.json", role: "submission_snapshot", status: "unavailable" }),
        expect.objectContaining({ path: "metadata/cost.json", role: "cost", status: "pending" }),
        expect.objectContaining({ path: "metadata/custody.json", role: "custody", status: "pending" }),
        expect.objectContaining({ path: "events/events.jsonl", role: "typed_events", status: "present", recordCount: 2 }),
        expect.objectContaining({ path: "outputs/report.txt", role: "output", status: "present", id: "o1" })
      ])
    );
  });

  it("reflects optional public metadata files as present when assembled", () => {
    const manifest = buildRunRecordDownloadManifestV1({
      runId: "run-2",
      typedEventCount: 1,
      outputs: [],
      submission: { status: "present" },
      cost: { status: "present" }
    });

    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/submission.json", role: "submission_snapshot", status: "present" }),
        expect.objectContaining({ path: "metadata/cost.json", role: "cost", status: "present" }),
        expect.objectContaining({ path: "metadata/custody.json", role: "custody", status: "pending" })
      ])
    );
  });
});

describe("scanRunRecordArchiveEntriesV1 — public-safety guard (download over-masking fix)", () => {
  const SHA256 = "abf1471e1a247d1839f66d723e15b46456ea30926d36a7a37a2e12bdb4787deb";
  const enc = (s: string) => new TextEncoder().encode(s);

  it("PASSES a successful run's archive with a sha256-named output and web-search result events", () => {
    // Mirrors the broll Stage-1 download() that used to throw 14
    // RunRecordArchiveRedactionError findings: an output captured under its
    // sha256 name, plus events.jsonl carrying web_search/web_fetch RESULT text
    // and a normal fetched URL (all legitimately high-entropy, none secret).
    const manifest = buildRunRecordDownloadManifestV1({
      runId: "9728bf4e-9711-4e6f-9152-15d6a9c70578",
      typedEventCount: 2,
      outputs: [
        { id: "out_report0001", filename: "report.txt", sizeBytes: 10, contentType: "text/plain" },
        { id: "out_hashed9999", filename: SHA256, sizeBytes: 22508, contentType: "application/octet-stream" }
      ]
    });
    const events = [
      {
        type: "TOOL_CALL_RESULT",
        data: { content: [{ type: "text", text: `https://www.reddit.com/r/television/comments/1rma2ro/ted_season_2_peacock_official_discussion_thread/ — ${"result text. ".repeat(150)}` }] }
      },
      { type: "TOOL_CALL_START", data: { name: "web_fetch", arguments: { url: "https://en.wikipedia.org/wiki/Norah_Jones?oldid=123456789" } } }
    ];

    const entries: RunRecordArchiveEntryForRedactionV1[] = [
      { path: "events/events.jsonl", bytes: enc(events.map((e) => JSON.stringify(e)).join("\n")), contentType: "application/x-ndjson" },
      { path: "manifest.json", bytes: enc(JSON.stringify(manifest)), contentType: "application/json; charset=utf-8" }
    ];

    expect(scanRunRecordArchiveEntriesV1(entries)).toEqual([]);
    expect(() => assertRunRecordArchivePublicSafeV1(entries)).not.toThrow();
  });

  it("STILL throws when a real provider key leaks into a scanned event entry (no weakening)", () => {
    const leaked = JSON.stringify({ type: "TEXT", data: { text: "key=sk-ant-api03-aBcD1234efGh5678ijKlmnop" } });
    const entries: RunRecordArchiveEntryForRedactionV1[] = [
      { path: "events/events.jsonl", bytes: enc(leaked), contentType: "application/x-ndjson" }
    ];
    expect(() => assertRunRecordArchivePublicSafeV1(entries)).toThrow();
  });
});

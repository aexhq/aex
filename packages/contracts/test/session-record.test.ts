import { describe, expect, it } from "bun:test";
import {
  SESSION_RECORD_MANIFEST_SCHEMA_VERSION,
  SESSION_RECORD_SCHEMA_VERSION,
  assertSessionRecordArchivePublicSafeV1,
  buildSessionRecordDownloadManifestV1,
  scanSessionRecordArchiveEntriesV1,
  type SessionRecordArchiveEntryForRedactionV1
} from "../src/session-record.js";

describe("SessionRecordV1 manifest helpers", () => {
  it("builds the versioned public whole-session download manifest", () => {
    const manifest = buildSessionRecordDownloadManifestV1({
      sessionId: "session-1",
      typedEventCount: 2,
      sessionFiles: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }],
      errors: [{ namespace: "files", id: "missing", filename: "missing.txt", message: "not found" }]
    });

    expect(manifest.schemaVersion).toBe(SESSION_RECORD_MANIFEST_SCHEMA_VERSION);
    expect(manifest.sessionRecordSchemaVersion).toBe(SESSION_RECORD_SCHEMA_VERSION);
    expect(manifest.namespaces.map((entry) => entry.name)).toEqual(["metadata", "events", "files"]);
    expect(manifest.sessionFiles.map((entry) => entry.id)).toEqual(["o1"]);
    expect(manifest.errors[0]).toMatchObject({ namespace: "files", id: "missing" });

    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/session.json", role: "session_metadata", status: "present" }),
        expect.objectContaining({ path: "metadata/submission.json", role: "submission_snapshot", status: "unavailable" }),
        expect.objectContaining({ path: "metadata/cost.json", role: "cost", status: "pending" }),
        expect.objectContaining({ path: "metadata/custody.json", role: "custody", status: "pending" }),
        expect.objectContaining({ path: "events/events.jsonl", role: "typed_events", status: "present", recordCount: 2 }),
        expect.objectContaining({ path: "files/report.txt", role: "file", status: "present", id: "o1" })
      ])
    );
  });

  it("reflects optional public metadata files as present when assembled", () => {
    const manifest = buildSessionRecordDownloadManifestV1({
      sessionId: "session-2",
      typedEventCount: 1,
      sessionFiles: [],
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

describe("scanSessionRecordArchiveEntriesV1 — public-safety guard (download over-masking fix)", () => {
  const SHA256 = "abf1471e1a247d1839f66d723e15b46456ea30926d36a7a37a2e12bdb4787deb";
  const enc = (s: string) => new TextEncoder().encode(s);

  it("PASSES a successful session's archive with a sha256-named file and web-search result events", () => {
    // Mirrors the broll Stage-1 download() that used to throw 14
    // SessionRecordArchiveRedactionError findings: a session file captured under its
    // sha256 name, plus events.jsonl carrying web_search/web_fetch RESULT text
    // and a normal fetched URL (all legitimately high-entropy, none secret).
    const manifest = buildSessionRecordDownloadManifestV1({
      sessionId: "9728bf4e-9711-4e6f-9152-15d6a9c70578",
      typedEventCount: 2,
      sessionFiles: [
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

    const entries: SessionRecordArchiveEntryForRedactionV1[] = [
      { path: "events/events.jsonl", bytes: enc(events.map((e) => JSON.stringify(e)).join("\n")), contentType: "application/x-ndjson" },
      { path: "manifest.json", bytes: enc(JSON.stringify(manifest)), contentType: "application/json; charset=utf-8" }
    ];

    expect(scanSessionRecordArchiveEntriesV1(entries)).toEqual([]);
    expect(() => assertSessionRecordArchivePublicSafeV1(entries)).not.toThrow();
  });

  it("STILL throws when a real provider key leaks into a scanned event entry (no weakening)", () => {
    const leaked = JSON.stringify({ type: "TEXT", data: { text: "key=sk-ant-api03-aBcD1234efGh5678ijKlmnop" } });
    const entries: SessionRecordArchiveEntryForRedactionV1[] = [
      { path: "events/events.jsonl", bytes: enc(leaked), contentType: "application/x-ndjson" }
    ];
    expect(() => assertSessionRecordArchivePublicSafeV1(entries)).toThrow();
  });
});

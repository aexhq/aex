import { describe, expect, it } from "vitest";
import {
  RUN_RECORD_MANIFEST_SCHEMA_VERSION,
  RUN_RECORD_SCHEMA_VERSION,
  buildRunRecordDownloadManifestV1
} from "../src/run-record.js";

describe("RunRecordV1 manifest helpers", () => {
  it("builds the versioned whole-run download manifest", () => {
    const manifest = buildRunRecordDownloadManifestV1({
      runId: "run-1",
      typedEventCount: 2,
      outputs: [{ id: "o1", filename: "report.txt", sizeBytes: 5, contentType: "text/plain" }],
      logs: [{ id: "l1", filename: "goose-logs/stderr.log", sizeBytes: 3, contentType: "text/plain" }],
      errors: [{ namespace: "outputs", id: "missing", filename: "missing.txt", message: "not found" }]
    });

    expect(manifest.schemaVersion).toBe(RUN_RECORD_MANIFEST_SCHEMA_VERSION);
    expect(manifest.runRecordSchemaVersion).toBe(RUN_RECORD_SCHEMA_VERSION);
    expect(manifest.namespaces.map((entry) => entry.name)).toEqual(["metadata", "events", "outputs", "logs"]);
    expect(manifest.outputs.map((entry) => entry.id)).toEqual(["o1"]);
    expect(manifest.logs.map((entry) => entry.id)).toEqual(["l1"]);
    expect(manifest.errors[0]).toMatchObject({ namespace: "outputs", id: "missing" });

    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/run.json", role: "run_metadata", status: "present" }),
        expect.objectContaining({ path: "metadata/submission.json", role: "submission_snapshot", status: "unavailable" }),
        expect.objectContaining({ path: "metadata/cost.json", role: "cost", status: "pending" }),
        expect.objectContaining({ path: "metadata/custody.json", role: "custody", status: "pending" }),
        expect.objectContaining({ path: "events/events.jsonl", role: "typed_events", status: "present", recordCount: 2 }),
        expect.objectContaining({ path: "events/logs.jsonl", role: "log_events", status: "unavailable" }),
        expect.objectContaining({ path: "outputs/report.txt", role: "output", status: "present", id: "o1" }),
        expect.objectContaining({ path: "logs/goose-logs/stderr.log", role: "log", status: "present", id: "l1" })
      ])
    );
  });

  it("reflects optional files as present when the client assembled actual entries", () => {
    const manifest = buildRunRecordDownloadManifestV1({
      runId: "run-2",
      typedEventCount: 1,
      outputs: [],
      logs: [],
      submission: { status: "present" },
      cost: { status: "present" },
      logEvents: { status: "present", recordCount: 2 },
      allEvents: { status: "present", recordCount: 3 }
    });

    expect(manifest.files).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ path: "metadata/submission.json", role: "submission_snapshot", status: "present" }),
        expect.objectContaining({ path: "metadata/cost.json", role: "cost", status: "present" }),
        expect.objectContaining({ path: "metadata/custody.json", role: "custody", status: "pending" }),
        expect.objectContaining({ path: "events/logs.jsonl", role: "log_events", status: "present", recordCount: 2 }),
        expect.objectContaining({ path: "events/all.jsonl", role: "all_events", status: "present", recordCount: 3 })
      ])
    );
  });
});

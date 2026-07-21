import { strToU8, zipSync } from "fflate";
import type { AexEvent } from "./event-envelope.js";
import { isRecord } from "./value-guards.js";
import {
  assertSessionRecordArchivePublicSafeV1,
  buildSessionRecordDownloadManifestV1,
  type SessionRecordArchiveEntryForRedactionV1,
  type SessionRecordArtifactSummaryV1,
  type SessionRecordDownloadErrorV1
} from "./session-record.js";
import type { SessionCostTelemetry } from "./session-cost.js";
import type { Session } from "./runtime-types.js";
import type { PlatformSubmission } from "./submission.js";

export interface SessionArchiveEntry extends SessionRecordArchiveEntryForRedactionV1 {
  readonly path: string;
  readonly bytes: Uint8Array;
}

export interface CollectedSessionArchiveArtifacts {
  readonly entries: readonly SessionArchiveEntry[];
  readonly captured: readonly SessionRecordArtifactSummaryV1[];
  readonly errors: readonly SessionRecordDownloadErrorV1[];
}

export function buildSessionArchive(
  sessionId: string,
  session: Session,
  events: readonly AexEvent[],
  files: CollectedSessionArchiveArtifacts
): Uint8Array {
  const submissionSnapshot = extractSubmissionSnapshot(session);
  const costTelemetry = extractCostTelemetry(session);
  const manifest = buildSessionRecordDownloadManifestV1({
    sessionId,
    sessionFiles: files.captured,
    errors: files.errors,
    typedEventCount: events.length,
    ...(submissionSnapshot ? { submission: { status: "present" } } : {}),
    ...(costTelemetry ? { cost: { status: "present" } } : {})
  });

  return zipEntries([
    jsonEntry("metadata/session.json", session),
    ...(submissionSnapshot ? [jsonEntry("metadata/submission.json", submissionSnapshot)] : []),
    ...(costTelemetry ? [jsonEntry("metadata/cost.json", costTelemetry)] : []),
    jsonlEntry("events/events.jsonl", events),
    ...files.entries,
    jsonEntry("manifest.json", manifest)
  ]);
}

export function buildSessionFilesArchive(
  sessionId: string,
  files: CollectedSessionArchiveArtifacts
): Uint8Array {
  return zipEntries([
    ...files.entries,
    jsonEntry("manifest.json", {
      sessionId,
      namespace: "files",
      files: files.captured,
      errors: files.errors
    })
  ]);
}

export function buildSessionEventsArchive(sessionId: string, events: readonly AexEvent[]): Uint8Array {
  return zipEntries([
    jsonlEntry("events.jsonl", events),
    jsonEntry("manifest.json", {
      sessionId,
      namespace: "events",
      files: [{ path: "events.jsonl", role: "typed_events", status: "present", recordCount: events.length }],
      errors: []
    })
  ]);
}

export function buildSessionMetadataArchive(sessionId: string, session: Session): Uint8Array {
  return zipEntries([
    jsonEntry("session.json", session),
    jsonEntry("manifest.json", {
      sessionId,
      namespace: "metadata",
      files: [{ path: "session.json", role: "session_metadata", status: "present" }],
      errors: []
    })
  ]);
}

function zipEntries(entries: readonly SessionArchiveEntry[]): Uint8Array {
  assertSessionRecordArchivePublicSafeV1(entries);
  const files: Record<string, Uint8Array> = {};
  for (const entry of entries) {
    files[entry.path] = entry.bytes;
  }
  return zipSync(files);
}

function jsonEntry(path: string, value: unknown): SessionArchiveEntry {
  return {
    path,
    bytes: strToU8(JSON.stringify(value, null, 2)),
    contentType: "application/json; charset=utf-8"
  };
}

function jsonlEntry(path: string, events: readonly AexEvent[]): SessionArchiveEntry {
  return {
    path,
    bytes: strToU8(events.map((event) => JSON.stringify(event)).join("\n")),
    contentType: "application/jsonl; charset=utf-8"
  };
}

function extractSubmissionSnapshot(session: Session): { readonly submission: PlatformSubmission } | undefined {
  const raw = (session as Session & { readonly submission?: unknown }).submission;
  if (!isRecord(raw) || raw.kind !== "submission" || !isRecord(raw.submission)) {
    return undefined;
  }
  return {
    submission: raw.submission as unknown as PlatformSubmission
  };
}

function extractCostTelemetry(session: Session): SessionCostTelemetry | undefined {
  return session.costTelemetry;
}

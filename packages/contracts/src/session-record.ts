import type { SessionCostTelemetry } from "./session-cost.js";
import {
  scanCustodyPayloadForSensitiveValues,
  type CustodyManifestV1,
  type CustodyRedactionFinding
} from "./session-custody.js";
import type { AexEvent } from "./event-envelope.js";
import type { SessionRecord, SessionFile } from "./runtime-types.js";
import type { PlatformSubmission } from "./submission.js";

export const SESSION_RECORD_SCHEMA_VERSION = "aex.session-record.v1" as const;
export const SESSION_RECORD_MANIFEST_SCHEMA_VERSION = "aex.session-record.manifest.v1" as const;

export type SessionRecordArchiveNamespaceV1 = "metadata" | "events" | "files";

export type SessionRecordFileStatusV1 =
  | "present"
  | "absent"
  | "pending"
  | "unavailable"
  | "not_applicable"
  | "error";

export type SessionRecordArchiveFileRoleV1 =
  | "session_metadata"
  | "submission_snapshot"
  | "cost"
  | "custody"
  | "typed_events"
  | "coordinator_events_manifest"
  | "file";

export interface SessionRecordSubmissionSnapshotV1 {
  readonly submission: PlatformSubmission;
}

export interface SessionRecordCostV1 {
  readonly status: SessionRecordFileStatusV1;
  readonly telemetry?: SessionCostTelemetry;
}

export interface SessionRecordMetadataV1 {
  readonly session: SessionRecord;
  readonly submission?: SessionRecordSubmissionSnapshotV1;
  readonly cost?: SessionRecordCostV1;
  readonly custody?: CustodyManifestV1;
}

export interface SessionRecordEventsV1 {
  /** Typed `channel: "event"` records in the SDK `events/events.jsonl` export. */
  readonly typed: readonly AexEvent[];
}

export interface SessionRecordV1 {
  readonly schemaVersion: typeof SESSION_RECORD_SCHEMA_VERSION;
  readonly sessionId: string;
  readonly metadata: SessionRecordMetadataV1;
  readonly events: SessionRecordEventsV1;
  readonly sessionFiles: readonly SessionFile[];
  readonly manifest: SessionRecordManifestV1;
}

export interface SessionRecordNamespaceV1 {
  readonly name: SessionRecordArchiveNamespaceV1;
  readonly prefix: `${SessionRecordArchiveNamespaceV1}/`;
  readonly status: SessionRecordFileStatusV1;
  readonly description: string;
}

export interface SessionRecordArchiveFileV1 {
  readonly namespace: SessionRecordArchiveNamespaceV1;
  readonly path: string;
  readonly role: SessionRecordArchiveFileRoleV1;
  readonly status: SessionRecordFileStatusV1;
  readonly id?: string;
  readonly filename?: string | null;
  readonly sizeBytes?: number;
  readonly contentType?: string;
  readonly recordCount?: number;
}

export interface SessionRecordArtifactSummaryV1 {
  readonly id: string;
  readonly filename: string | null;
  readonly sizeBytes?: number;
  readonly contentType?: string;
}

export interface SessionRecordDownloadErrorV1 {
  readonly namespace: "files";
  readonly id: string;
  readonly filename: string | null;
  readonly message: string;
}

export interface SessionRecordManifestV1 {
  readonly schemaVersion: typeof SESSION_RECORD_MANIFEST_SCHEMA_VERSION;
  readonly sessionRecordSchemaVersion: typeof SESSION_RECORD_SCHEMA_VERSION;
  readonly sessionId: string;
  readonly namespaces: readonly SessionRecordNamespaceV1[];
  readonly files: readonly SessionRecordArchiveFileV1[];
  /** Captured session files included in the archive. */
  readonly sessionFiles: readonly SessionRecordArtifactSummaryV1[];
  readonly errors: readonly SessionRecordDownloadErrorV1[];
}

export interface BuildSessionRecordDownloadManifestV1Input {
  readonly sessionId: string;
  readonly sessionFiles: readonly SessionRecordArtifactSummaryV1[];
  readonly errors?: readonly SessionRecordDownloadErrorV1[];
  readonly typedEventCount?: number;
  readonly submission?: SessionRecordFileManifestInputV1;
  readonly cost?: SessionRecordFileManifestInputV1;
  readonly custody?: SessionRecordFileManifestInputV1;
  readonly coordinatorEventsManifest?: SessionRecordFileManifestInputV1;
}

export interface SessionRecordFileManifestInputV1 {
  readonly status: SessionRecordFileStatusV1;
  readonly recordCount?: number;
}

export interface SessionRecordArchiveEntryForRedactionV1 {
  readonly path: string;
  readonly bytes: Uint8Array;
  readonly contentType?: string;
  /**
   * Customer-authored file bytes are intentionally outside the public-record
   * redaction guarantee. Metadata, event exports, and manifests remain scanned.
   */
  readonly customerContent?: boolean;
}

export interface SessionRecordArchiveRedactionFindingV1 {
  readonly entryPath: string;
  readonly path: string;
  readonly reason: CustodyRedactionFinding["reason"];
  readonly valueLength?: number;
}

export class SessionRecordArchiveRedactionError extends Error {
  readonly code = "session_record_archive_not_public_safe";
  readonly findings: readonly SessionRecordArchiveRedactionFindingV1[];

  constructor(findings: readonly SessionRecordArchiveRedactionFindingV1[]) {
    super(`session record archive contains non-public data at ${formatArchiveFindingPaths(findings)}`);
    this.name = "SessionRecordArchiveRedactionError";
    this.findings = Object.freeze([...findings]);
  }
}

export function buildSessionRecordDownloadManifestV1(
  input: BuildSessionRecordDownloadManifestV1Input
): SessionRecordManifestV1 {
  const sessionFiles = input.sessionFiles.map((file) => normalizeArtifactSummary(file));
  const errors = (input.errors ?? []).map((error) => Object.freeze({ ...error }));

  return Object.freeze({
    schemaVersion: SESSION_RECORD_MANIFEST_SCHEMA_VERSION,
    sessionRecordSchemaVersion: SESSION_RECORD_SCHEMA_VERSION,
    sessionId: input.sessionId,
    namespaces: Object.freeze([
      namespace("metadata", "SessionRecord metadata, submission snapshot, custody, and cost files."),
      namespace("events", "Typed event-channel exports."),
      namespace("files", "Captured files produced by the session.")
    ]),
    files: Object.freeze([
      file("metadata", "metadata/session.json", "session_metadata", "present"),
      file("metadata", "metadata/submission.json", "submission_snapshot", input.submission?.status ?? "unavailable"),
      file("metadata", "metadata/cost.json", "cost", input.cost?.status ?? "pending"),
      file("metadata", "metadata/custody.json", "custody", input.custody?.status ?? "pending"),
      file("events", "events/events.jsonl", "typed_events", "present", {
        recordCount: input.typedEventCount ?? 0
      }),
      file(
        "events",
        "events/manifest.json",
        "coordinator_events_manifest",
        input.coordinatorEventsManifest?.status ?? "unavailable"
      ),
      ...sessionFiles.map((fileSummary) =>
        artifactFile("files", "file", "files/", fileSummary)
      )
    ]),
    sessionFiles: Object.freeze(sessionFiles),
    errors: Object.freeze(errors)
  });
}

function namespace(
  name: SessionRecordArchiveNamespaceV1,
  description: string
): SessionRecordNamespaceV1 {
  return Object.freeze({
    name,
    prefix: `${name}/`,
    status: "present",
    description
  });
}

function file(
  namespaceName: SessionRecordArchiveNamespaceV1,
  path: string,
  role: SessionRecordArchiveFileRoleV1,
  status: SessionRecordFileStatusV1,
  extra?: Pick<SessionRecordArchiveFileV1, "recordCount">
): SessionRecordArchiveFileV1 {
  return Object.freeze({
    namespace: namespaceName,
    path,
    role,
    status,
    ...(extra?.recordCount !== undefined ? { recordCount: extra.recordCount } : {})
  });
}

function artifactFile(
  namespaceName: "files",
  role: "file",
  prefix: "files/",
  artifact: SessionRecordArtifactSummaryV1
): SessionRecordArchiveFileV1 {
  return Object.freeze({
    namespace: namespaceName,
    path: `${prefix}${artifact.filename ?? artifact.id}`,
    role,
    status: "present",
    id: artifact.id,
    filename: artifact.filename,
    ...(artifact.sizeBytes !== undefined ? { sizeBytes: artifact.sizeBytes } : {}),
    ...(artifact.contentType !== undefined ? { contentType: artifact.contentType } : {})
  });
}

function normalizeArtifactSummary(input: SessionRecordArtifactSummaryV1): SessionRecordArtifactSummaryV1 {
  return Object.freeze({
    id: input.id,
    filename: input.filename,
    ...(input.sizeBytes !== undefined ? { sizeBytes: input.sizeBytes } : {}),
    ...(input.contentType !== undefined ? { contentType: input.contentType } : {})
  });
}

export function scanSessionRecordArchiveEntriesV1(
  entries: readonly SessionRecordArchiveEntryForRedactionV1[]
): readonly SessionRecordArchiveRedactionFindingV1[] {
  const findings: SessionRecordArchiveRedactionFindingV1[] = [];
  for (const entry of entries) {
    if (entry.customerContent || !shouldScanArchiveEntry(entry)) {
      continue;
    }
    for (const finding of scanArchiveEntry(entry)) {
      findings.push(finding);
    }
  }
  return Object.freeze(findings);
}

export function assertSessionRecordArchivePublicSafeV1(
  entries: readonly SessionRecordArchiveEntryForRedactionV1[]
): void {
  const findings = scanSessionRecordArchiveEntriesV1(entries);
  if (findings.length > 0) {
    throw new SessionRecordArchiveRedactionError(findings);
  }
}

function shouldScanArchiveEntry(entry: SessionRecordArchiveEntryForRedactionV1): boolean {
  if (entry.path.startsWith("files/")) {
    return false;
  }
  const contentType = entry.contentType?.toLowerCase() ?? "";
  if (
    contentType.startsWith("text/") ||
    contentType.includes("json") ||
    contentType.includes("xml") ||
    contentType.includes("yaml")
  ) {
    return true;
  }
  return /\.(?:json|jsonl|ndjson|txt|log|md|csv|tsv|ya?ml)$/i.test(entry.path);
}

function scanArchiveEntry(
  entry: SessionRecordArchiveEntryForRedactionV1
): readonly SessionRecordArchiveRedactionFindingV1[] {
  const text = new TextDecoder().decode(entry.bytes);
  const parsedValues = parseArchiveTextValues(entry.path, text);
  const rawFindings =
    parsedValues.length > 0
      ? parsedValues.flatMap((value) => [...scanCustodyPayloadForSensitiveValues(value.value)].map((finding) => ({
          ...finding,
          path: `${value.path}${finding.path === "$" ? "" : finding.path.slice(1)}`
        })))
      : scanCustodyPayloadForSensitiveValues(text);
  return Object.freeze(
    rawFindings
      .filter((finding) => !isAllowedArchiveHighEntropyField(entry.path, finding))
      .map((finding) =>
        Object.freeze({
          entryPath: entry.path,
          path: finding.path,
          reason: finding.reason,
          ...(finding.valueLength !== undefined ? { valueLength: finding.valueLength } : {})
        })
      )
  );
}

function isAllowedArchiveHighEntropyField(
  entryPath: string,
  finding: CustodyRedactionFinding
): boolean {
  if (finding.reason !== "high_entropy_token" || !entryPath.endsWith("manifest.json")) {
    return false;
  }
  return /^\$(?:\.files\[\d+\]|\.sessionFiles\[\d+\])\.id$/.test(finding.path);
}

function parseArchiveTextValues(
  path: string,
  text: string
): ReadonlyArray<{ readonly path: string; readonly value: unknown }> {
  if (/\.json$/i.test(path)) {
    const parsed = tryParseJson(text);
    return parsed.ok ? Object.freeze([{ path: "$", value: parsed.value }]) : Object.freeze([]);
  }
  if (/\.(?:jsonl|ndjson)$/i.test(path)) {
    const values: Array<{ path: string; value: unknown }> = [];
    const lines = text.split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      if (!line?.trim()) {
        continue;
      }
      const parsed = tryParseJson(line);
      if (!parsed.ok) {
        return Object.freeze([]);
      }
      values.push({ path: `$[${i}]`, value: parsed.value });
    }
    return Object.freeze(values);
  }
  return Object.freeze([]);
}

function tryParseJson(text: string): { readonly ok: true; readonly value: unknown } | { readonly ok: false } {
  try {
    return { ok: true, value: JSON.parse(text) as unknown };
  } catch {
    return { ok: false };
  }
}

function formatArchiveFindingPaths(findings: readonly SessionRecordArchiveRedactionFindingV1[]): string {
  return findings.map((finding) => `${finding.entryPath}${finding.path} (${finding.reason})`).join(", ");
}

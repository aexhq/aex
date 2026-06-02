import type { RunCostTelemetry } from "./run-cost.js";
import {
  scanCustodyPayloadForSensitiveValues,
  type CustodyManifestV1,
  type CustodyRedactionFinding
} from "./run-custody.js";
import type { Run, RunEvent, Output } from "./runtime-types.js";
import type { PlatformCleanupPolicy, PlatformSubmission } from "./submission.js";

export const RUN_RECORD_SCHEMA_VERSION = "antpath.run-record.v1" as const;
export const RUN_RECORD_MANIFEST_SCHEMA_VERSION = "antpath.run-record.manifest.v1" as const;

export type RunRecordArchiveNamespaceV1 = "metadata" | "events" | "outputs" | "logs";

export type RunRecordFileStatusV1 =
  | "present"
  | "absent"
  | "pending"
  | "unavailable"
  | "not_applicable"
  | "error";

export type RunRecordArchiveFileRoleV1 =
  | "run_metadata"
  | "submission_snapshot"
  | "cost"
  | "custody"
  | "typed_events"
  | "log_events"
  | "all_events"
  | "coordinator_events_manifest"
  | "output"
  | "log";

export interface RunRecordSubmissionSnapshotV1 {
  readonly submission: PlatformSubmission;
  readonly cleanup?: PlatformCleanupPolicy;
}

export interface RunRecordCostV1 {
  readonly status: RunRecordFileStatusV1;
  readonly telemetry?: RunCostTelemetry;
}

export interface RunRecordMetadataV1 {
  readonly run: Run;
  readonly submission?: RunRecordSubmissionSnapshotV1;
  readonly cost?: RunRecordCostV1;
  readonly custody?: CustodyManifestV1;
}

export interface RunRecordEventsV1 {
  /**
   * Typed `channel: "event"` records. This is the current SDK
   * `events/events.jsonl` export. Log-channel records are not mixed into this
   * file.
   */
  readonly typed: readonly RunEvent[];
  readonly logs?: readonly RunEvent[];
  readonly all?: readonly RunEvent[];
}

export interface RunRecordV1 {
  readonly schemaVersion: typeof RUN_RECORD_SCHEMA_VERSION;
  readonly runId: string;
  readonly metadata: RunRecordMetadataV1;
  readonly events: RunRecordEventsV1;
  readonly outputs: readonly Output[];
  readonly logs: readonly Output[];
  readonly manifest: RunRecordManifestV1;
}

export interface RunRecordNamespaceV1 {
  readonly name: RunRecordArchiveNamespaceV1;
  readonly prefix: `${RunRecordArchiveNamespaceV1}/`;
  readonly status: RunRecordFileStatusV1;
  readonly description: string;
}

export interface RunRecordArchiveFileV1 {
  readonly namespace: RunRecordArchiveNamespaceV1;
  readonly path: string;
  readonly role: RunRecordArchiveFileRoleV1;
  readonly status: RunRecordFileStatusV1;
  readonly id?: string;
  readonly filename?: string | null;
  readonly sizeBytes?: number;
  readonly contentType?: string;
  readonly recordCount?: number;
}

export interface RunRecordArtifactSummaryV1 {
  readonly id: string;
  readonly filename: string | null;
  readonly sizeBytes?: number;
  readonly contentType?: string;
}

export interface RunRecordDownloadErrorV1 {
  readonly namespace: "outputs" | "logs";
  readonly id: string;
  readonly filename: string | null;
  readonly message: string;
}

export interface RunRecordManifestV1 {
  readonly schemaVersion: typeof RUN_RECORD_MANIFEST_SCHEMA_VERSION;
  readonly runRecordSchemaVersion: typeof RUN_RECORD_SCHEMA_VERSION;
  readonly runId: string;
  readonly namespaces: readonly RunRecordNamespaceV1[];
  readonly files: readonly RunRecordArchiveFileV1[];
  /**
   * Compatibility aliases for existing consumers of `manifest.json`.
   * Prefer `files[]` for new code because it carries namespace, role, and
   * presence state for optional run-record members.
   */
  readonly outputs: readonly RunRecordArtifactSummaryV1[];
  readonly logs: readonly RunRecordArtifactSummaryV1[];
  readonly errors: readonly RunRecordDownloadErrorV1[];
}

export interface BuildRunRecordDownloadManifestV1Input {
  readonly runId: string;
  readonly outputs: readonly RunRecordArtifactSummaryV1[];
  readonly logs: readonly RunRecordArtifactSummaryV1[];
  readonly errors?: readonly RunRecordDownloadErrorV1[];
  readonly typedEventCount?: number;
  readonly submission?: RunRecordFileManifestInputV1;
  readonly cost?: RunRecordFileManifestInputV1;
  readonly custody?: RunRecordFileManifestInputV1;
  readonly logEvents?: RunRecordFileManifestInputV1;
  readonly allEvents?: RunRecordFileManifestInputV1;
  readonly coordinatorEventsManifest?: RunRecordFileManifestInputV1;
}

export interface RunRecordFileManifestInputV1 {
  readonly status: RunRecordFileStatusV1;
  readonly recordCount?: number;
}

export interface RunRecordArchiveEntryForRedactionV1 {
  readonly path: string;
  readonly bytes: Uint8Array;
  readonly contentType?: string;
  /**
   * Customer-authored output bytes are intentionally outside the public-record
   * redaction guarantee. Metadata, event exports, manifests, and platform logs
   * remain scanned.
   */
  readonly customerContent?: boolean;
}

export interface RunRecordArchiveRedactionFindingV1 {
  readonly entryPath: string;
  readonly path: string;
  readonly reason: CustodyRedactionFinding["reason"];
  readonly valueLength?: number;
}

export class RunRecordArchiveRedactionError extends Error {
  readonly code = "run_record_archive_not_public_safe";
  readonly findings: readonly RunRecordArchiveRedactionFindingV1[];

  constructor(findings: readonly RunRecordArchiveRedactionFindingV1[]) {
    super(`run record archive contains non-public data at ${formatArchiveFindingPaths(findings)}`);
    this.name = "RunRecordArchiveRedactionError";
    this.findings = Object.freeze([...findings]);
  }
}

export function buildRunRecordDownloadManifestV1(
  input: BuildRunRecordDownloadManifestV1Input
): RunRecordManifestV1 {
  const outputs = input.outputs.map((file) => normalizeArtifactSummary(file));
  const logs = input.logs.map((file) => normalizeArtifactSummary(file));
  const errors = (input.errors ?? []).map((error) => Object.freeze({ ...error }));

  return Object.freeze({
    schemaVersion: RUN_RECORD_MANIFEST_SCHEMA_VERSION,
    runRecordSchemaVersion: RUN_RECORD_SCHEMA_VERSION,
    runId: input.runId,
    namespaces: Object.freeze([
      namespace("metadata", "Run metadata, submission snapshot, custody, and cost files."),
      namespace("events", "Typed event-channel exports and optional full-stream/log-channel exports."),
      namespace("outputs", "Captured deliverables produced by the run."),
      namespace("logs", "Platform diagnostics and runtime log artifacts.")
    ]),
    files: Object.freeze([
      file("metadata", "metadata/run.json", "run_metadata", "present"),
      file("metadata", "metadata/submission.json", "submission_snapshot", input.submission?.status ?? "unavailable"),
      file("metadata", "metadata/cost.json", "cost", input.cost?.status ?? "pending"),
      file("metadata", "metadata/custody.json", "custody", input.custody?.status ?? "pending"),
      file("events", "events/events.jsonl", "typed_events", "present", {
        recordCount: input.typedEventCount ?? 0
      }),
      file(
        "events",
        "events/logs.jsonl",
        "log_events",
        input.logEvents?.status ?? "unavailable",
        recordCountExtra(input.logEvents)
      ),
      file(
        "events",
        "events/all.jsonl",
        "all_events",
        input.allEvents?.status ?? "unavailable",
        recordCountExtra(input.allEvents)
      ),
      file(
        "events",
        "events/manifest.json",
        "coordinator_events_manifest",
        input.coordinatorEventsManifest?.status ?? "unavailable"
      ),
      ...outputs.map((output) =>
        artifactFile("outputs", "output", "outputs/", output)
      ),
      ...logs.map((log) =>
        artifactFile("logs", "log", "logs/", log)
      )
    ]),
    outputs: Object.freeze(outputs),
    logs: Object.freeze(logs),
    errors: Object.freeze(errors)
  });
}

function namespace(
  name: RunRecordArchiveNamespaceV1,
  description: string
): RunRecordNamespaceV1 {
  return Object.freeze({
    name,
    prefix: `${name}/`,
    status: "present",
    description
  });
}

function file(
  namespaceName: RunRecordArchiveNamespaceV1,
  path: string,
  role: RunRecordArchiveFileRoleV1,
  status: RunRecordFileStatusV1,
  extra?: Pick<RunRecordArchiveFileV1, "recordCount">
): RunRecordArchiveFileV1 {
  return Object.freeze({
    namespace: namespaceName,
    path,
    role,
    status,
    ...(extra?.recordCount !== undefined ? { recordCount: extra.recordCount } : {})
  });
}

function recordCountExtra(
  input: RunRecordFileManifestInputV1 | undefined
): Pick<RunRecordArchiveFileV1, "recordCount"> | undefined {
  return input?.status === "present" && input.recordCount !== undefined
    ? { recordCount: input.recordCount }
    : undefined;
}

function artifactFile(
  namespaceName: "outputs" | "logs",
  role: "output" | "log",
  prefix: "outputs/" | "logs/",
  artifact: RunRecordArtifactSummaryV1
): RunRecordArchiveFileV1 {
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

function normalizeArtifactSummary(input: RunRecordArtifactSummaryV1): RunRecordArtifactSummaryV1 {
  return Object.freeze({
    id: input.id,
    filename: input.filename,
    ...(input.sizeBytes !== undefined ? { sizeBytes: input.sizeBytes } : {}),
    ...(input.contentType !== undefined ? { contentType: input.contentType } : {})
  });
}

export function scanRunRecordArchiveEntriesV1(
  entries: readonly RunRecordArchiveEntryForRedactionV1[]
): readonly RunRecordArchiveRedactionFindingV1[] {
  const findings: RunRecordArchiveRedactionFindingV1[] = [];
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

export function assertRunRecordArchivePublicSafeV1(
  entries: readonly RunRecordArchiveEntryForRedactionV1[]
): void {
  const findings = scanRunRecordArchiveEntriesV1(entries);
  if (findings.length > 0) {
    throw new RunRecordArchiveRedactionError(findings);
  }
}

function shouldScanArchiveEntry(entry: RunRecordArchiveEntryForRedactionV1): boolean {
  if (entry.path.startsWith("outputs/")) {
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
  entry: RunRecordArchiveEntryForRedactionV1
): readonly RunRecordArchiveRedactionFindingV1[] {
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
  return /^\$(?:\.files\[\d+\]|\.outputs\[\d+\]|\.logs\[\d+\])\.id$/.test(finding.path);
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

function formatArchiveFindingPaths(findings: readonly RunRecordArchiveRedactionFindingV1[]): string {
  return findings.map((finding) => `${finding.entryPath}${finding.path} (${finding.reason})`).join(", ");
}

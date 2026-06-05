import type { RunStatus } from "./status.js";
import { isTerminalRunStatus } from "./status.js";

export const RUN_RETENTION_SCHEMA_VERSION = 1;
export const RUN_DELETION_MANIFEST_KIND = "aex.run_deletion_manifest.v1";
export const RUN_DELETION_JOB_KIND = "aex.run_deletion_job.v1";
export const RUN_DELETION_MANIFEST_CONTENT_TYPE = "application/json; charset=utf-8";
export const RUN_RETENTION_REDACTION_SCANNER_VERSION = 1;

export const RUN_DELETION_REASONS = ["manual_delete", "retention_gc"] as const;
export type RunDeletionReason = (typeof RUN_DELETION_REASONS)[number];

export const RUN_DELETION_MANIFEST_MODES = ["dry_run", "final"] as const;
export type RunDeletionManifestMode = (typeof RUN_DELETION_MANIFEST_MODES)[number];

export const RUN_DELETION_CANDIDATE_STATUSES = ["selected", "blocked"] as const;
export type RunDeletionCandidateStatus = (typeof RUN_DELETION_CANDIDATE_STATUSES)[number];

export const RUN_DELETION_BLOCKERS = [
  "non_terminal",
  "already_deleted",
  "concurrent_delete",
  "retention_policy_disabled",
  "unexpired",
  "held",
  "retention_exempt",
  "unresolved_cleanup",
  "unresolved_custody"
] as const;
export type RunDeletionBlocker = (typeof RUN_DELETION_BLOCKERS)[number];

export const RUN_DELETION_COUNT_CLASSES = [
  "object_store_objects",
  "outputs",
  "logs",
  "events",
  "assets",
  "db_event_rows",
  "db_output_rows",
  "capture_failures",
  "storage_samples",
  "custody_manifests"
] as const;
export type RunDeletionCountClass = (typeof RUN_DELETION_COUNT_CLASSES)[number];

export const RUN_DELETION_COUNT_STATUSES = ["counted", "not_counted", "partial", "failed"] as const;
export type RunDeletionCountStatus = (typeof RUN_DELETION_COUNT_STATUSES)[number];

export const RUN_DELETION_JOB_STATUSES = [
  "queued",
  "planning",
  "blocked",
  "manifest_written",
  "deleting",
  "delete_failed",
  "completed",
  "failed"
] as const;
export type RunDeletionJobStatus = (typeof RUN_DELETION_JOB_STATUSES)[number];

export const RUN_DELETION_PROOF_STATUSES = [
  "not_started",
  "running",
  "completed",
  "failed"
] as const;
export type RunDeletionProofStatus = (typeof RUN_DELETION_PROOF_STATUSES)[number];

export const RUN_DELETION_WRITE_STATUSES = ["not_written", "written", "write_failed"] as const;
export type RunDeletionWriteStatus = (typeof RUN_DELETION_WRITE_STATUSES)[number];

export const RUN_RETENTION_EXCLUDED_VALUE_CLASSES = [
  "raw_paths",
  "object_keys",
  "filenames",
  "object_sizes",
  "hashes",
  "provider_ids",
  "vault_ids",
  "resource_ids",
  "resource_handles",
  "signed_urls"
] as const;
export type RunRetentionExcludedValueClass = (typeof RUN_RETENTION_EXCLUDED_VALUE_CLASSES)[number];

export interface RunRetentionPolicyV1 {
  readonly mode: "retain_indefinitely" | "delete_after_days";
  readonly manualDelete: "enabled";
  readonly automaticDeletion: "disabled" | "enabled";
  readonly retentionDays?: number;
}

export interface RunRetentionPolicyInput {
  readonly automaticDeletion?: boolean;
  readonly retentionDays?: number;
}

export interface RunDeletionCandidateRunV1 {
  readonly runId: string;
  readonly workspaceId: string;
  readonly status: RunStatus | string;
  readonly createdAt?: string;
  readonly terminalAt?: string;
  readonly pendingDeleteAt?: string;
  readonly deletedAt?: string;
  readonly held?: boolean;
  readonly retentionExempt?: boolean;
  readonly unresolvedCleanup?: boolean;
  readonly unresolvedCustody?: boolean;
}

export interface RunDeletionBlockerV1 {
  readonly code: RunDeletionBlocker;
  readonly observedAt: string;
}

export interface RunDeletionCandidateV1 {
  readonly status: RunDeletionCandidateStatus;
  readonly reason: RunDeletionReason;
  readonly evaluatedAt: string;
  readonly eligibleAt?: string;
  readonly blockers: readonly RunDeletionBlockerV1[];
}

export interface RunDeletionCandidateInput {
  readonly run: RunDeletionCandidateRunV1;
  readonly reason: RunDeletionReason;
  readonly policy?: RunRetentionPolicyV1 | RunRetentionPolicyInput;
  readonly now: string;
}

export interface RunDeletionCountV1 {
  readonly class: RunDeletionCountClass | string;
  readonly count: number;
  readonly status: RunDeletionCountStatus;
  readonly countedAt?: string;
  readonly errorClass?: string;
}

export interface RunDeletionManifestRunV1 {
  readonly runId: string;
  readonly workspaceId: string;
  readonly status: RunStatus | string;
  readonly createdAt?: string;
  readonly terminalAt?: string;
  readonly eligibleAt?: string;
  readonly pendingDeleteAt?: string;
  readonly deletedAt?: string;
}

export interface RunDeletionManifestRequestV1 {
  readonly reason: RunDeletionReason;
  readonly actorClass: "user" | "api_token" | "system" | "operator";
}

export interface RunDeletionManifestSummaryV1 {
  readonly totalCount: number;
  readonly failedCountClasses: number;
  readonly partialCountClasses: number;
  readonly blockerCount: number;
  readonly counts: readonly RunDeletionCountV1[];
}

export interface RunDeletionManifestRedactionV1 {
  readonly policy: "counts_status_timestamps_only";
  readonly scannerVersion: typeof RUN_RETENTION_REDACTION_SCANNER_VERSION;
  readonly excludes: readonly RunRetentionExcludedValueClass[];
}

export interface RunDeletionManifestV1 {
  readonly schemaVersion: typeof RUN_RETENTION_SCHEMA_VERSION;
  readonly kind: typeof RUN_DELETION_MANIFEST_KIND;
  readonly generatedAt: string;
  readonly mode: RunDeletionManifestMode;
  readonly run: RunDeletionManifestRunV1;
  readonly request: RunDeletionManifestRequestV1;
  readonly candidate: RunDeletionCandidateV1;
  readonly summary: RunDeletionManifestSummaryV1;
  readonly redaction: RunDeletionManifestRedactionV1;
}

export interface RunDeletionManifestInput {
  readonly generatedAt: string;
  readonly mode: RunDeletionManifestMode;
  readonly run: RunDeletionCandidateRunV1;
  readonly request: RunDeletionManifestRequestV1;
  readonly candidate?: RunDeletionCandidateV1;
  readonly policy?: RunRetentionPolicyV1 | RunRetentionPolicyInput;
  readonly counts?: readonly RunDeletionCountV1[];
}

export interface RunDeletionManifestProofV1 {
  readonly status: RunDeletionWriteStatus;
  readonly mode?: RunDeletionManifestMode;
  readonly writtenAt?: string;
}

export interface RunDeletionPurgeProofV1 {
  readonly status: RunDeletionProofStatus;
  readonly startedAt?: string;
  readonly completedAt?: string;
  readonly deletedObjectCount?: number;
}

export interface RunDeletionOrderProofV1 {
  readonly manifest: RunDeletionManifestProofV1;
  readonly purge: RunDeletionPurgeProofV1;
}

export interface RunDeletionJobV1 {
  readonly schemaVersion: typeof RUN_RETENTION_SCHEMA_VERSION;
  readonly kind: typeof RUN_DELETION_JOB_KIND;
  readonly jobId: string;
  readonly runId: string;
  readonly workspaceId: string;
  readonly reason: RunDeletionReason;
  readonly mode: RunDeletionManifestMode;
  readonly status: RunDeletionJobStatus;
  readonly createdAt: string;
  readonly updatedAt?: string;
  readonly order: RunDeletionOrderProofV1;
  readonly candidate?: RunDeletionCandidateV1;
  readonly summary?: RunDeletionManifestSummaryV1;
}

export interface RunDeletionJobInput {
  readonly jobId: string;
  readonly runId: string;
  readonly workspaceId: string;
  readonly reason: RunDeletionReason;
  readonly mode: RunDeletionManifestMode;
  readonly status: RunDeletionJobStatus;
  readonly createdAt: string;
  readonly updatedAt?: string;
  readonly order: RunDeletionOrderProofV1;
  readonly candidate?: RunDeletionCandidateV1;
  readonly summary?: RunDeletionManifestSummaryV1;
}

export interface RunDeletionManifestWriteObject {
  readonly runId: string;
  readonly workspaceId: string;
  readonly contentType: typeof RUN_DELETION_MANIFEST_CONTENT_TYPE;
  readonly manifest: RunDeletionManifestV1;
}

export interface RunDeletionManifestObjectStore {
  putRunDeletionManifestObject(object: RunDeletionManifestWriteObject): Promise<void>;
}

export interface RunDeletionManifestWriteResult {
  readonly status: "written";
  readonly schemaVersion: typeof RUN_RETENTION_SCHEMA_VERSION;
  readonly runId: string;
  readonly workspaceId: string;
  readonly writtenAt: string;
  readonly mode: RunDeletionManifestMode;
}

export interface RunDeletionManifestWriter {
  writeRunDeletionManifest(input: RunDeletionManifestInput): Promise<RunDeletionManifestWriteResult>;
}

export type RunRetentionRedactionReason =
  | "forbidden_field_name"
  | "signed_url"
  | "object_store_key"
  | "vault_id"
  | "private_resource_handle"
  | "hash_like_value";

export interface RunRetentionRedactionFinding {
  readonly path: string;
  readonly reason: RunRetentionRedactionReason;
  readonly valueLength?: number;
}

export class RunRetentionValidationError extends Error {
  readonly code = "run_retention_contract_invalid";

  constructor(message: string) {
    super(message);
    this.name = "RunRetentionValidationError";
  }
}

export class RunRetentionRedactionError extends Error {
  readonly code = "run_retention_payload_not_public_safe";
  readonly findings: readonly RunRetentionRedactionFinding[];

  constructor(findings: readonly RunRetentionRedactionFinding[]) {
    super(`run retention payload contains non-public data at ${formatFindingPaths(findings)}`);
    this.name = "RunRetentionRedactionError";
    this.findings = Object.freeze([...findings]);
  }
}

export class FakeRunDeletionManifestObjectStore implements RunDeletionManifestObjectStore {
  #objects = new Map<string, RunDeletionManifestV1>();

  async putRunDeletionManifestObject(object: RunDeletionManifestWriteObject): Promise<void> {
    assertPublicSafeRunRetentionPayload(object.manifest);
    this.#objects.set(object.runId, cloneJson(object.manifest));
  }

  getByRunId(runId: string): RunDeletionManifestV1 | undefined {
    return this.get(runId);
  }

  get(runId: string): RunDeletionManifestV1 | undefined {
    const object = this.#objects.get(runId);
    return object ? cloneJson(object) : undefined;
  }

  listRunIds(): readonly string[] {
    return Object.freeze([...this.#objects.keys()].sort());
  }
}

export function createRunDeletionManifestWriter(
  store: RunDeletionManifestObjectStore
): RunDeletionManifestWriter {
  return {
    async writeRunDeletionManifest(
      input: RunDeletionManifestInput
    ): Promise<RunDeletionManifestWriteResult> {
      return writeRunDeletionManifest(store, input);
    }
  };
}

export async function writeRunDeletionManifest(
  store: RunDeletionManifestObjectStore,
  input: RunDeletionManifestInput
): Promise<RunDeletionManifestWriteResult> {
  const manifest = buildRunDeletionManifest(input);
  await store.putRunDeletionManifestObject({
    runId: manifest.run.runId,
    workspaceId: manifest.run.workspaceId,
    contentType: RUN_DELETION_MANIFEST_CONTENT_TYPE,
    manifest
  });
  return Object.freeze({
    status: "written" as const,
    schemaVersion: RUN_RETENTION_SCHEMA_VERSION,
    runId: manifest.run.runId,
    workspaceId: manifest.run.workspaceId,
    writtenAt: manifest.generatedAt,
    mode: manifest.mode
  });
}

export function buildRunRetentionPolicy(input: RunRetentionPolicyInput = {}): RunRetentionPolicyV1 {
  if (input.automaticDeletion) {
    if (input.retentionDays === undefined) {
      throw new RunRetentionValidationError(
        "automatic retention deletion requires an explicit positive retentionDays value"
      );
    }
    return Object.freeze({
      mode: "delete_after_days" as const,
      manualDelete: "enabled" as const,
      automaticDeletion: "enabled" as const,
      retentionDays: positiveInteger(input.retentionDays, "policy.retentionDays")
    });
  }

  if (input.retentionDays !== undefined) {
    throw new RunRetentionValidationError(
      "retentionDays is not allowed when automatic retention deletion is disabled"
    );
  }

  return Object.freeze({
    mode: "retain_indefinitely" as const,
    manualDelete: "enabled" as const,
    automaticDeletion: "disabled" as const
  });
}

export function evaluateRunDeletionCandidate(input: RunDeletionCandidateInput): RunDeletionCandidateV1 {
  const now = assertTimestamp(input.now, "candidate.now");
  const run = normalizeCandidateRun(input.run);
  const policy = normalizePolicy(input.policy);
  const blockers: RunDeletionBlockerV1[] = [];

  addRunBlockers(blockers, run, now);

  let eligibleAt: string | undefined;
  if (input.reason === "retention_gc") {
    if (policy.mode !== "delete_after_days" || policy.retentionDays === undefined) {
      blockers.push(blocker("retention_policy_disabled", now));
    } else if (run.terminalAt) {
      eligibleAt = addDaysIso(run.terminalAt, policy.retentionDays);
      if (Date.parse(now) < Date.parse(eligibleAt)) {
        blockers.push(blocker("unexpired", now));
      }
    }
  } else {
    eligibleAt = now;
  }

  return Object.freeze({
    status: blockers.length === 0 ? "selected" : "blocked",
    reason: input.reason,
    evaluatedAt: now,
    ...(eligibleAt ? { eligibleAt } : {}),
    blockers: Object.freeze(blockers)
  });
}

export function buildRunDeletionManifest(input: RunDeletionManifestInput): RunDeletionManifestV1 {
  const candidate =
    input.candidate ??
    evaluateRunDeletionCandidate({
      run: input.run,
      reason: input.request.reason,
      now: input.generatedAt,
      ...(input.policy ? { policy: input.policy } : {})
    });
  const normalizedCandidate = normalizeCandidate(candidate);
  const run = normalizeManifestRun(input.run, normalizedCandidate.eligibleAt);
  const summary = buildRunDeletionSummary(input.counts ?? [], normalizedCandidate);
  const manifest = Object.freeze({
    schemaVersion: RUN_RETENTION_SCHEMA_VERSION,
    kind: RUN_DELETION_MANIFEST_KIND,
    generatedAt: assertTimestamp(input.generatedAt, "manifest.generatedAt"),
    mode: input.mode,
    run,
    request: normalizeRequest(input.request),
    candidate: normalizedCandidate,
    summary,
    redaction: Object.freeze({
      policy: "counts_status_timestamps_only" as const,
      scannerVersion: RUN_RETENTION_REDACTION_SCANNER_VERSION,
      excludes: Object.freeze([...RUN_RETENTION_EXCLUDED_VALUE_CLASSES])
    })
  }) satisfies RunDeletionManifestV1;
  assertPublicSafeRunRetentionPayload(manifest);
  return manifest;
}

export function assertRunDeletionOrder(proof: RunDeletionOrderProofV1): void {
  const manifest = normalizeManifestProof(proof.manifest);
  const purge = normalizePurgeProof(proof.purge);
  const purgeStarted = purge.status === "running" || purge.status === "completed" || purge.status === "failed";

  if (purgeStarted && manifest.status !== "written") {
    throw new RunRetentionValidationError("run deletion cannot purge assets before the deletion manifest is written");
  }
  if (purgeStarted && manifest.mode !== "final") {
    throw new RunRetentionValidationError("run deletion cannot purge assets from a dry-run deletion manifest");
  }
}

export function buildRunDeletionJob(input: RunDeletionJobInput): RunDeletionJobV1 {
  assertRunDeletionOrder(input.order);
  const job = Object.freeze({
    schemaVersion: RUN_RETENTION_SCHEMA_VERSION,
    kind: RUN_DELETION_JOB_KIND,
    jobId: assertSafeIdentifier(input.jobId, "job.jobId"),
    runId: assertSafeIdentifier(input.runId, "job.runId"),
    workspaceId: assertSafeIdentifier(input.workspaceId, "job.workspaceId"),
    reason: input.reason,
    mode: input.mode,
    status: input.status,
    createdAt: assertTimestamp(input.createdAt, "job.createdAt"),
    ...(input.updatedAt ? { updatedAt: assertTimestamp(input.updatedAt, "job.updatedAt") } : {}),
    order: normalizeOrderProof(input.order),
    ...(input.candidate ? { candidate: normalizeCandidate(input.candidate) } : {}),
    ...(input.summary ? { summary: normalizeSummary(input.summary) } : {})
  }) satisfies RunDeletionJobV1;
  assertPublicSafeRunRetentionPayload(job);
  return job;
}

export function scanRunRetentionPayloadForSensitiveValues(
  input: unknown
): readonly RunRetentionRedactionFinding[] {
  const findings: RunRetentionRedactionFinding[] = [];
  visitRetentionValue(input, "$", findings);
  return Object.freeze(findings);
}

export function assertPublicSafeRunRetentionPayload(input: unknown): void {
  const findings = scanRunRetentionPayloadForSensitiveValues(input);
  if (findings.length > 0) {
    throw new RunRetentionRedactionError(findings);
  }
}

function normalizePolicy(input: RunRetentionPolicyV1 | RunRetentionPolicyInput | undefined): RunRetentionPolicyV1 {
  if (!input) {
    return buildRunRetentionPolicy();
  }
  if ("mode" in input) {
    if (input.mode === "delete_after_days") {
      if (input.retentionDays === undefined) {
        throw new RunRetentionValidationError(
          "delete_after_days retention policy requires an explicit retentionDays value"
        );
      }
      return buildRunRetentionPolicy({
        automaticDeletion: true,
        retentionDays: input.retentionDays
      });
    }
    if (input.retentionDays !== undefined) {
      throw new RunRetentionValidationError(
        "retain_indefinitely retention policy cannot include retentionDays"
      );
    }
    return buildRunRetentionPolicy();
  }
  return buildRunRetentionPolicy(input);
}

function normalizeCandidateRun(input: RunDeletionCandidateRunV1): RunDeletionCandidateRunV1 {
  return Object.freeze({
    runId: assertSafeIdentifier(input.runId, "run.runId"),
    workspaceId: assertSafeIdentifier(input.workspaceId, "run.workspaceId"),
    status: assertSafeMetadataString(input.status, "run.status"),
    ...(input.createdAt ? { createdAt: assertTimestamp(input.createdAt, "run.createdAt") } : {}),
    ...(input.terminalAt ? { terminalAt: assertTimestamp(input.terminalAt, "run.terminalAt") } : {}),
    ...(input.pendingDeleteAt ? { pendingDeleteAt: assertTimestamp(input.pendingDeleteAt, "run.pendingDeleteAt") } : {}),
    ...(input.deletedAt ? { deletedAt: assertTimestamp(input.deletedAt, "run.deletedAt") } : {}),
    ...(input.held !== undefined ? { held: input.held } : {}),
    ...(input.retentionExempt !== undefined ? { retentionExempt: input.retentionExempt } : {}),
    ...(input.unresolvedCleanup !== undefined ? { unresolvedCleanup: input.unresolvedCleanup } : {}),
    ...(input.unresolvedCustody !== undefined ? { unresolvedCustody: input.unresolvedCustody } : {})
  });
}

function normalizeManifestRun(
  input: RunDeletionCandidateRunV1,
  eligibleAt: string | undefined
): RunDeletionManifestRunV1 {
  const run = normalizeCandidateRun(input);
  return Object.freeze({
    runId: run.runId,
    workspaceId: run.workspaceId,
    status: run.status,
    ...(run.createdAt ? { createdAt: run.createdAt } : {}),
    ...(run.terminalAt ? { terminalAt: run.terminalAt } : {}),
    ...(eligibleAt ? { eligibleAt: assertTimestamp(eligibleAt, "run.eligibleAt") } : {}),
    ...(run.pendingDeleteAt ? { pendingDeleteAt: run.pendingDeleteAt } : {}),
    ...(run.deletedAt ? { deletedAt: run.deletedAt } : {})
  });
}

function normalizeRequest(input: RunDeletionManifestRequestV1): RunDeletionManifestRequestV1 {
  return Object.freeze({
    reason: input.reason,
    actorClass: input.actorClass
  });
}

function normalizeCandidate(input: RunDeletionCandidateV1): RunDeletionCandidateV1 {
  return Object.freeze({
    status: input.status,
    reason: input.reason,
    evaluatedAt: assertTimestamp(input.evaluatedAt, "candidate.evaluatedAt"),
    ...(input.eligibleAt ? { eligibleAt: assertTimestamp(input.eligibleAt, "candidate.eligibleAt") } : {}),
    blockers: Object.freeze(input.blockers.map(normalizeBlocker))
  });
}

function normalizeBlocker(input: RunDeletionBlockerV1): RunDeletionBlockerV1 {
  return Object.freeze({
    code: input.code,
    observedAt: assertTimestamp(input.observedAt, "candidate.blocker.observedAt")
  });
}

function normalizeCount(input: RunDeletionCountV1): RunDeletionCountV1 {
  return Object.freeze({
    class: assertSafeMetadataString(input.class, "summary.count.class"),
    count: nonNegativeInteger(input.count, "summary.count.count"),
    status: input.status,
    ...(input.countedAt ? { countedAt: assertTimestamp(input.countedAt, "summary.count.countedAt") } : {}),
    ...(input.errorClass ? { errorClass: assertSafeMetadataString(input.errorClass, "summary.count.errorClass") } : {})
  });
}

function buildRunDeletionSummary(
  input: readonly RunDeletionCountV1[],
  candidate: RunDeletionCandidateV1
): RunDeletionManifestSummaryV1 {
  const counts = Object.freeze(input.map(normalizeCount));
  return Object.freeze({
    totalCount: counts.reduce((sum, item) => sum + item.count, 0),
    failedCountClasses: counts.filter((item) => item.status === "failed").length,
    partialCountClasses: counts.filter((item) => item.status === "partial").length,
    blockerCount: candidate.blockers.length,
    counts
  });
}

function normalizeSummary(input: RunDeletionManifestSummaryV1): RunDeletionManifestSummaryV1 {
  return Object.freeze({
    totalCount: nonNegativeInteger(input.totalCount, "summary.totalCount"),
    failedCountClasses: nonNegativeInteger(input.failedCountClasses, "summary.failedCountClasses"),
    partialCountClasses: nonNegativeInteger(input.partialCountClasses, "summary.partialCountClasses"),
    blockerCount: nonNegativeInteger(input.blockerCount, "summary.blockerCount"),
    counts: Object.freeze(input.counts.map(normalizeCount))
  });
}

function normalizeOrderProof(input: RunDeletionOrderProofV1): RunDeletionOrderProofV1 {
  return Object.freeze({
    manifest: normalizeManifestProof(input.manifest),
    purge: normalizePurgeProof(input.purge)
  });
}

function normalizeManifestProof(input: RunDeletionManifestProofV1): RunDeletionManifestProofV1 {
  return Object.freeze({
    status: input.status,
    ...(input.mode ? { mode: input.mode } : {}),
    ...(input.writtenAt ? { writtenAt: assertTimestamp(input.writtenAt, "proof.manifest.writtenAt") } : {})
  });
}

function normalizePurgeProof(input: RunDeletionPurgeProofV1): RunDeletionPurgeProofV1 {
  return Object.freeze({
    status: input.status,
    ...(input.startedAt ? { startedAt: assertTimestamp(input.startedAt, "proof.purge.startedAt") } : {}),
    ...(input.completedAt ? { completedAt: assertTimestamp(input.completedAt, "proof.purge.completedAt") } : {}),
    ...(input.deletedObjectCount !== undefined
      ? { deletedObjectCount: nonNegativeInteger(input.deletedObjectCount, "proof.purge.deletedObjectCount") }
      : {})
  });
}

function addRunBlockers(
  blockers: RunDeletionBlockerV1[],
  run: RunDeletionCandidateRunV1,
  observedAt: string
): void {
  if (run.status === "deleted") {
    blockers.push(blocker("already_deleted", observedAt));
  } else if (run.status === "pending_delete" || run.pendingDeleteAt) {
    blockers.push(blocker("concurrent_delete", observedAt));
  } else if (!isTerminalStatusLike(run.status)) {
    blockers.push(blocker("non_terminal", observedAt));
  }

  if (run.held) {
    blockers.push(blocker("held", observedAt));
  }
  if (run.retentionExempt) {
    blockers.push(blocker("retention_exempt", observedAt));
  }
  if (run.unresolvedCleanup) {
    blockers.push(blocker("unresolved_cleanup", observedAt));
  }
  if (run.unresolvedCustody) {
    blockers.push(blocker("unresolved_custody", observedAt));
  }
}

function blocker(code: RunDeletionBlocker, observedAt: string): RunDeletionBlockerV1 {
  return Object.freeze({ code, observedAt });
}

function isTerminalStatusLike(status: string): boolean {
  return isTerminalRunStatus(status as RunStatus);
}

function addDaysIso(timestamp: string, days: number): string {
  const start = Date.parse(assertTimestamp(timestamp, "candidate.run.terminalAt"));
  const end = start + positiveInteger(days, "policy.retentionDays") * 24 * 60 * 60 * 1000;
  return new Date(end).toISOString();
}

function visitRetentionValue(
  input: unknown,
  path: string,
  findings: RunRetentionRedactionFinding[]
): void {
  if (typeof input === "string") {
    scanStringValue(input, path, findings);
    return;
  }
  if (Array.isArray(input)) {
    input.forEach((value, index) => visitRetentionValue(value, `${path}[${index}]`, findings));
    return;
  }
  if (!input || typeof input !== "object") {
    return;
  }
  for (const [key, value] of Object.entries(input as Record<string, unknown>)) {
    const childPath = `${path}.${key}`;
    if (isForbiddenRetentionFieldName(key)) {
      findings.push(Object.freeze({ path: childPath, reason: "forbidden_field_name" }));
    }
    visitRetentionValue(value, childPath, findings);
  }
}

function scanStringValue(
  value: string,
  path: string,
  findings: RunRetentionRedactionFinding[]
): void {
  for (const pattern of forbiddenStringPatterns) {
    if (pattern.regex.test(value)) {
      findings.push(Object.freeze({
        path,
        reason: pattern.reason,
        valueLength: value.length
      }));
    }
  }
}

const forbiddenStringPatterns: readonly {
  readonly reason: Exclude<RunRetentionRedactionReason, "forbidden_field_name">;
  readonly regex: RegExp;
}[] = Object.freeze([
  { reason: "signed_url", regex: /[?&](?:X-Amz-Signature|X-Amz-Credential|X-Amz-Algorithm|AWSAccessKeyId)=/i },
  { reason: "object_store_key", regex: /(^|[\s"'`])(?:runs|assets)\/[^?<#\s"'`]+/i },
  { reason: "vault_id", regex: /\b(?:vault|vlt|secret)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i },
  {
    reason: "private_resource_handle",
    regex: /\b(?:machine|session|resource|handle|provider|asset)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i
  },
  { reason: "hash_like_value", regex: /\b(?:sha256|hash)[:_-][A-Fa-f0-9]{16,}\b/ }
]);

function isForbiddenRetentionFieldName(key: string): boolean {
  return /^(path|paths|objectKey|objectKeys|objectStoreKey|objectStoreKeys|fileName|filename|filenames|size|sizes|bytes|byteCount|hash|hashes|providerId|providerIds|vaultId|vaultIds|resourceId|resourceIds|handle|handles|signedUrl|signedUrls|url|urls)$/i.test(
    key
  );
}

function assertSafeIdentifier(value: string, field: string): string {
  assertNonEmptyString(value, field);
  if (!/^[A-Za-z0-9._:-]+$/.test(value)) {
    throw new RunRetentionValidationError(`run retention ${field} must be an opaque identifier without path separators`);
  }
  assertPublicSafeRunRetentionPayload(value);
  return value;
}

function assertSafeMetadataString(value: string, field: string): string {
  assertNonEmptyString(value, field);
  assertPublicSafeRunRetentionPayload(value);
  return value;
}

function assertNonEmptyString(value: string, field: string): void {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new RunRetentionValidationError(`run retention ${field} must be a non-empty string`);
  }
}

function assertTimestamp(value: string, field: string): string {
  assertSafeMetadataString(value, field);
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) {
    throw new RunRetentionValidationError(`run retention ${field} must be an ISO timestamp string`);
  }
  return value;
}

function nonNegativeInteger(value: number, field: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RunRetentionValidationError(`run retention ${field} must be a non-negative safe integer`);
  }
  return value;
}

function positiveInteger(value: number, field: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new RunRetentionValidationError(`run retention ${field} must be a positive safe integer`);
  }
  return value;
}

function formatFindingPaths(findings: readonly RunRetentionRedactionFinding[]): string {
  return findings.map((finding) => `${finding.path} (${finding.reason})`).join(", ");
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

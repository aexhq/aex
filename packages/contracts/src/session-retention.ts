import type { SessionWorkflowStatus } from "./workflow-status.js";
import { isTerminalSessionWorkflowStatus } from "./workflow-status.js";

export const SESSION_RETENTION_SCHEMA_VERSION = 1;
export const SESSION_DELETION_MANIFEST_KIND = "aex.session_deletion_manifest.v1";
export const SESSION_DELETION_JOB_KIND = "aex.session_deletion_job.v1";
export const SESSION_DELETION_MANIFEST_CONTENT_TYPE = "application/json; charset=utf-8";
export const SESSION_RETENTION_REDACTION_SCANNER_VERSION = 1;

export const SESSION_DELETION_REASONS = ["manual_delete", "retention_gc"] as const;
export type SessionDeletionReason = (typeof SESSION_DELETION_REASONS)[number];

export const SESSION_DELETION_MANIFEST_MODES = ["dry_run", "final"] as const;
export type SessionDeletionManifestMode = (typeof SESSION_DELETION_MANIFEST_MODES)[number];

export const SESSION_DELETION_CANDIDATE_STATUSES = ["selected", "blocked"] as const;
export type SessionDeletionCandidateStatus = (typeof SESSION_DELETION_CANDIDATE_STATUSES)[number];

export const SESSION_DELETION_BLOCKERS = [
  "non_terminal",
  "retention_policy_disabled",
  "unexpired",
  "held",
  "retention_exempt",
  "unresolved_cleanup",
  "unresolved_custody"
] as const;
export type SessionDeletionBlocker = (typeof SESSION_DELETION_BLOCKERS)[number];

export const SESSION_DELETION_COUNT_CLASSES = [
  "object_store_objects",
  "files",
  "logs",
  "events",
  "assets",
  "db_event_rows",
  "db_file_rows",
  "capture_failures",
  "storage_samples",
  "custody_manifests"
] as const;
export type SessionDeletionCountClass = (typeof SESSION_DELETION_COUNT_CLASSES)[number];

export const SESSION_DELETION_COUNT_STATUSES = ["counted", "not_counted", "partial", "failed"] as const;
export type SessionDeletionCountStatus = (typeof SESSION_DELETION_COUNT_STATUSES)[number];

export const SESSION_DELETION_JOB_STATUSES = [
  "queued",
  "planning",
  "blocked",
  "manifest_written",
  "deleting",
  "delete_failed",
  "completed",
  "failed"
] as const;
export type SessionDeletionJobStatus = (typeof SESSION_DELETION_JOB_STATUSES)[number];

export const SESSION_DELETION_PROOF_STATUSES = [
  "not_started",
  "running",
  "completed",
  "failed"
] as const;
export type SessionDeletionProofStatus = (typeof SESSION_DELETION_PROOF_STATUSES)[number];

export const SESSION_DELETION_WRITE_STATUSES = ["not_written", "written", "write_failed"] as const;
export type SessionDeletionWriteStatus = (typeof SESSION_DELETION_WRITE_STATUSES)[number];

export const SESSION_RETENTION_EXCLUDED_VALUE_CLASSES = [
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
export type SessionRetentionExcludedValueClass = (typeof SESSION_RETENTION_EXCLUDED_VALUE_CLASSES)[number];

export interface SessionRetentionPolicyV1 {
  readonly mode: "retain_indefinitely" | "delete_after_days";
  readonly manualDelete: "enabled";
  readonly automaticDeletion: "disabled" | "enabled";
  readonly retentionDays?: number;
}

export interface SessionRetentionPolicyInput {
  readonly automaticDeletion?: boolean;
  readonly retentionDays?: number;
}

export interface SessionDeletionCandidateSessionV1 {
  readonly sessionId: string;
  readonly workspaceId: string;
  readonly status: string;
  readonly createdAt?: string;
  readonly terminalAt?: string;
  readonly held?: boolean;
  readonly retentionExempt?: boolean;
  readonly unresolvedCleanup?: boolean;
  readonly unresolvedCustody?: boolean;
}

export interface SessionDeletionBlockerV1 {
  readonly code: SessionDeletionBlocker;
  readonly observedAt: string;
}

export interface SessionDeletionCandidateV1 {
  readonly status: SessionDeletionCandidateStatus;
  readonly reason: SessionDeletionReason;
  readonly evaluatedAt: string;
  readonly eligibleAt?: string;
  readonly blockers: readonly SessionDeletionBlockerV1[];
}

export interface SessionDeletionCandidateInput {
  readonly session: SessionDeletionCandidateSessionV1;
  readonly reason: SessionDeletionReason;
  readonly policy?: SessionRetentionPolicyV1 | SessionRetentionPolicyInput;
  readonly now: string;
}

export interface SessionDeletionCountV1 {
  readonly class: SessionDeletionCountClass | string;
  readonly count: number;
  readonly status: SessionDeletionCountStatus;
  readonly countedAt?: string;
  readonly errorClass?: string;
}

export interface SessionDeletionManifestSessionV1 {
  readonly sessionId: string;
  readonly workspaceId: string;
  readonly status: string;
  readonly createdAt?: string;
  readonly terminalAt?: string;
  readonly eligibleAt?: string;
}

export interface SessionDeletionManifestRequestV1 {
  readonly reason: SessionDeletionReason;
  readonly actorClass: "user" | "api_key" | "system" | "operator";
}

export interface SessionDeletionManifestSummaryV1 {
  readonly totalCount: number;
  readonly failedCountClasses: number;
  readonly partialCountClasses: number;
  readonly blockerCount: number;
  readonly counts: readonly SessionDeletionCountV1[];
}

export interface SessionDeletionManifestRedactionV1 {
  readonly policy: "counts_status_timestamps_only";
  readonly scannerVersion: typeof SESSION_RETENTION_REDACTION_SCANNER_VERSION;
  readonly excludes: readonly SessionRetentionExcludedValueClass[];
}

export interface SessionDeletionManifestV1 {
  readonly schemaVersion: typeof SESSION_RETENTION_SCHEMA_VERSION;
  readonly kind: typeof SESSION_DELETION_MANIFEST_KIND;
  readonly generatedAt: string;
  readonly mode: SessionDeletionManifestMode;
  readonly session: SessionDeletionManifestSessionV1;
  readonly request: SessionDeletionManifestRequestV1;
  readonly candidate: SessionDeletionCandidateV1;
  readonly summary: SessionDeletionManifestSummaryV1;
  readonly redaction: SessionDeletionManifestRedactionV1;
}

export interface SessionDeletionManifestInput {
  readonly generatedAt: string;
  readonly mode: SessionDeletionManifestMode;
  readonly session: SessionDeletionCandidateSessionV1;
  readonly request: SessionDeletionManifestRequestV1;
  readonly candidate?: SessionDeletionCandidateV1;
  readonly policy?: SessionRetentionPolicyV1 | SessionRetentionPolicyInput;
  readonly counts?: readonly SessionDeletionCountV1[];
}

export interface SessionDeletionManifestProofV1 {
  readonly status: SessionDeletionWriteStatus;
  readonly mode?: SessionDeletionManifestMode;
  readonly writtenAt?: string;
}

export interface SessionDeletionPurgeProofV1 {
  readonly status: SessionDeletionProofStatus;
  readonly startedAt?: string;
  readonly completedAt?: string;
  readonly deletedObjectCount?: number;
}

export interface SessionDeletionOrderProofV1 {
  readonly manifest: SessionDeletionManifestProofV1;
  readonly purge: SessionDeletionPurgeProofV1;
}

export interface SessionDeletionJobV1 {
  readonly schemaVersion: typeof SESSION_RETENTION_SCHEMA_VERSION;
  readonly kind: typeof SESSION_DELETION_JOB_KIND;
  readonly jobId: string;
  readonly sessionId: string;
  readonly workspaceId: string;
  readonly reason: SessionDeletionReason;
  readonly mode: SessionDeletionManifestMode;
  readonly status: SessionDeletionJobStatus;
  readonly createdAt: string;
  readonly updatedAt?: string;
  readonly order: SessionDeletionOrderProofV1;
  readonly candidate?: SessionDeletionCandidateV1;
  readonly summary?: SessionDeletionManifestSummaryV1;
}

export interface SessionDeletionJobInput {
  readonly jobId: string;
  readonly sessionId: string;
  readonly workspaceId: string;
  readonly reason: SessionDeletionReason;
  readonly mode: SessionDeletionManifestMode;
  readonly status: SessionDeletionJobStatus;
  readonly createdAt: string;
  readonly updatedAt?: string;
  readonly order: SessionDeletionOrderProofV1;
  readonly candidate?: SessionDeletionCandidateV1;
  readonly summary?: SessionDeletionManifestSummaryV1;
}

export interface SessionDeletionManifestWriteObject {
  readonly sessionId: string;
  readonly workspaceId: string;
  readonly contentType: typeof SESSION_DELETION_MANIFEST_CONTENT_TYPE;
  readonly manifest: SessionDeletionManifestV1;
}

export interface SessionDeletionManifestObjectStore {
  putSessionDeletionManifestObject(object: SessionDeletionManifestWriteObject): Promise<void>;
}

export interface SessionDeletionManifestWriteResult {
  readonly status: "written";
  readonly schemaVersion: typeof SESSION_RETENTION_SCHEMA_VERSION;
  readonly sessionId: string;
  readonly workspaceId: string;
  readonly writtenAt: string;
  readonly mode: SessionDeletionManifestMode;
}

export interface SessionDeletionManifestWriter {
  writeSessionDeletionManifest(input: SessionDeletionManifestInput): Promise<SessionDeletionManifestWriteResult>;
}

export type SessionRetentionRedactionReason =
  | "forbidden_field_name"
  | "signed_url"
  | "object_store_key"
  | "vault_id"
  | "private_resource_handle"
  | "hash_like_value";

export interface SessionRetentionRedactionFinding {
  readonly path: string;
  readonly reason: SessionRetentionRedactionReason;
  readonly valueLength?: number;
}

export class SessionRetentionValidationError extends Error {
  readonly code = "session_retention_contract_invalid";

  constructor(message: string) {
    super(message);
    this.name = "SessionRetentionValidationError";
  }
}

export class SessionRetentionRedactionError extends Error {
  readonly code = "session_retention_payload_not_public_safe";
  readonly findings: readonly SessionRetentionRedactionFinding[];

  constructor(findings: readonly SessionRetentionRedactionFinding[]) {
    super(`session retention payload contains non-public data at ${formatFindingPaths(findings)}`);
    this.name = "SessionRetentionRedactionError";
    this.findings = Object.freeze([...findings]);
  }
}

export class FakeSessionDeletionManifestObjectStore implements SessionDeletionManifestObjectStore {
  #objects = new Map<string, SessionDeletionManifestV1>();

  async putSessionDeletionManifestObject(object: SessionDeletionManifestWriteObject): Promise<void> {
    assertPublicSafeSessionRetentionPayload(object.manifest);
    this.#objects.set(object.sessionId, cloneJson(object.manifest));
  }

  getBySessionId(sessionId: string): SessionDeletionManifestV1 | undefined {
    return this.get(sessionId);
  }

  get(sessionId: string): SessionDeletionManifestV1 | undefined {
    const object = this.#objects.get(sessionId);
    return object ? cloneJson(object) : undefined;
  }

  listSessionIds(): readonly string[] {
    return Object.freeze([...this.#objects.keys()].sort());
  }
}

export function createSessionDeletionManifestWriter(
  store: SessionDeletionManifestObjectStore
): SessionDeletionManifestWriter {
  return {
    async writeSessionDeletionManifest(
      input: SessionDeletionManifestInput
    ): Promise<SessionDeletionManifestWriteResult> {
      return writeSessionDeletionManifest(store, input);
    }
  };
}

export async function writeSessionDeletionManifest(
  store: SessionDeletionManifestObjectStore,
  input: SessionDeletionManifestInput
): Promise<SessionDeletionManifestWriteResult> {
  const manifest = buildSessionDeletionManifest(input);
  await store.putSessionDeletionManifestObject({
    sessionId: manifest.session.sessionId,
    workspaceId: manifest.session.workspaceId,
    contentType: SESSION_DELETION_MANIFEST_CONTENT_TYPE,
    manifest
  });
  return Object.freeze({
    status: "written" as const,
    schemaVersion: SESSION_RETENTION_SCHEMA_VERSION,
    sessionId: manifest.session.sessionId,
    workspaceId: manifest.session.workspaceId,
    writtenAt: manifest.generatedAt,
    mode: manifest.mode
  });
}

export function buildSessionRetentionPolicy(input: SessionRetentionPolicyInput = {}): SessionRetentionPolicyV1 {
  if (input.automaticDeletion) {
    if (input.retentionDays === undefined) {
      throw new SessionRetentionValidationError(
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
    throw new SessionRetentionValidationError(
      "retentionDays is not allowed when automatic retention deletion is disabled"
    );
  }

  return Object.freeze({
    mode: "retain_indefinitely" as const,
    manualDelete: "enabled" as const,
    automaticDeletion: "disabled" as const
  });
}

export function evaluateSessionDeletionCandidate(input: SessionDeletionCandidateInput): SessionDeletionCandidateV1 {
  const now = assertTimestamp(input.now, "candidate.now");
  const session = normalizeCandidateSession(input.session);
  const policy = normalizePolicy(input.policy);
  const blockers: SessionDeletionBlockerV1[] = [];

  addSessionBlockers(blockers, session, now);

  let eligibleAt: string | undefined;
  if (input.reason === "retention_gc") {
    if (policy.mode !== "delete_after_days" || policy.retentionDays === undefined) {
      blockers.push(blocker("retention_policy_disabled", now));
    } else if (session.terminalAt) {
      eligibleAt = addDaysIso(session.terminalAt, policy.retentionDays);
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

export function buildSessionDeletionManifest(input: SessionDeletionManifestInput): SessionDeletionManifestV1 {
  const candidate =
    input.candidate ??
    evaluateSessionDeletionCandidate({
      session: input.session,
      reason: input.request.reason,
      now: input.generatedAt,
      ...(input.policy ? { policy: input.policy } : {})
    });
  const normalizedCandidate = normalizeCandidate(candidate);
  const session = normalizeManifestSession(input.session, normalizedCandidate.eligibleAt);
  const summary = buildSessionDeletionSummary(input.counts ?? [], normalizedCandidate);
  const manifest = Object.freeze({
    schemaVersion: SESSION_RETENTION_SCHEMA_VERSION,
    kind: SESSION_DELETION_MANIFEST_KIND,
    generatedAt: assertTimestamp(input.generatedAt, "manifest.generatedAt"),
    mode: input.mode,
    session,
    request: normalizeRequest(input.request),
    candidate: normalizedCandidate,
    summary,
    redaction: Object.freeze({
      policy: "counts_status_timestamps_only" as const,
      scannerVersion: SESSION_RETENTION_REDACTION_SCANNER_VERSION,
      excludes: Object.freeze([...SESSION_RETENTION_EXCLUDED_VALUE_CLASSES])
    })
  }) satisfies SessionDeletionManifestV1;
  assertPublicSafeSessionRetentionPayload(manifest);
  return manifest;
}

export function assertSessionDeletionOrder(proof: SessionDeletionOrderProofV1): void {
  const manifest = normalizeManifestProof(proof.manifest);
  const purge = normalizePurgeProof(proof.purge);
  const purgeStarted = purge.status === "running" || purge.status === "completed" || purge.status === "failed";

  if (purgeStarted && manifest.status !== "written") {
    throw new SessionRetentionValidationError("session deletion cannot purge assets before the deletion manifest is written");
  }
  if (purgeStarted && manifest.mode !== "final") {
    throw new SessionRetentionValidationError("session deletion cannot purge assets from a dry-run deletion manifest");
  }
}

export function buildSessionDeletionJob(input: SessionDeletionJobInput): SessionDeletionJobV1 {
  assertSessionDeletionOrder(input.order);
  const job = Object.freeze({
    schemaVersion: SESSION_RETENTION_SCHEMA_VERSION,
    kind: SESSION_DELETION_JOB_KIND,
    jobId: assertSafeIdentifier(input.jobId, "job.jobId"),
    sessionId: assertSafeIdentifier(input.sessionId, "job.sessionId"),
    workspaceId: assertSafeIdentifier(input.workspaceId, "job.workspaceId"),
    reason: input.reason,
    mode: input.mode,
    status: input.status,
    createdAt: assertTimestamp(input.createdAt, "job.createdAt"),
    ...(input.updatedAt ? { updatedAt: assertTimestamp(input.updatedAt, "job.updatedAt") } : {}),
    order: normalizeOrderProof(input.order),
    ...(input.candidate ? { candidate: normalizeCandidate(input.candidate) } : {}),
    ...(input.summary ? { summary: normalizeSummary(input.summary) } : {})
  }) satisfies SessionDeletionJobV1;
  assertPublicSafeSessionRetentionPayload(job);
  return job;
}

export function scanSessionRetentionPayloadForSensitiveValues(
  input: unknown
): readonly SessionRetentionRedactionFinding[] {
  const findings: SessionRetentionRedactionFinding[] = [];
  visitRetentionValue(input, "$", findings);
  return Object.freeze(findings);
}

export function assertPublicSafeSessionRetentionPayload(input: unknown): void {
  const findings = scanSessionRetentionPayloadForSensitiveValues(input);
  if (findings.length > 0) {
    throw new SessionRetentionRedactionError(findings);
  }
}

function normalizePolicy(input: SessionRetentionPolicyV1 | SessionRetentionPolicyInput | undefined): SessionRetentionPolicyV1 {
  if (!input) {
    return buildSessionRetentionPolicy();
  }
  if ("mode" in input) {
    if (input.mode === "delete_after_days") {
      if (input.retentionDays === undefined) {
        throw new SessionRetentionValidationError(
          "delete_after_days retention policy requires an explicit retentionDays value"
        );
      }
      return buildSessionRetentionPolicy({
        automaticDeletion: true,
        retentionDays: input.retentionDays
      });
    }
    if (input.retentionDays !== undefined) {
      throw new SessionRetentionValidationError(
        "retain_indefinitely retention policy cannot include retentionDays"
      );
    }
    return buildSessionRetentionPolicy();
  }
  return buildSessionRetentionPolicy(input);
}

function normalizeCandidateSession(input: SessionDeletionCandidateSessionV1): SessionDeletionCandidateSessionV1 {
  return Object.freeze({
    sessionId: assertSafeIdentifier(input.sessionId, "session.sessionId"),
    workspaceId: assertSafeIdentifier(input.workspaceId, "session.workspaceId"),
    status: assertSafeMetadataString(input.status, "session.status"),
    ...(input.createdAt ? { createdAt: assertTimestamp(input.createdAt, "session.createdAt") } : {}),
    ...(input.terminalAt ? { terminalAt: assertTimestamp(input.terminalAt, "session.terminalAt") } : {}),
    ...(input.held !== undefined ? { held: input.held } : {}),
    ...(input.retentionExempt !== undefined ? { retentionExempt: input.retentionExempt } : {}),
    ...(input.unresolvedCleanup !== undefined ? { unresolvedCleanup: input.unresolvedCleanup } : {}),
    ...(input.unresolvedCustody !== undefined ? { unresolvedCustody: input.unresolvedCustody } : {})
  });
}

function normalizeManifestSession(
  input: SessionDeletionCandidateSessionV1,
  eligibleAt: string | undefined
): SessionDeletionManifestSessionV1 {
  const session = normalizeCandidateSession(input);
  return Object.freeze({
    sessionId: session.sessionId,
    workspaceId: session.workspaceId,
    status: session.status,
    ...(session.createdAt ? { createdAt: session.createdAt } : {}),
    ...(session.terminalAt ? { terminalAt: session.terminalAt } : {}),
    ...(eligibleAt ? { eligibleAt: assertTimestamp(eligibleAt, "session.eligibleAt") } : {})
  });
}

function normalizeRequest(input: SessionDeletionManifestRequestV1): SessionDeletionManifestRequestV1 {
  return Object.freeze({
    reason: input.reason,
    actorClass: input.actorClass
  });
}

function normalizeCandidate(input: SessionDeletionCandidateV1): SessionDeletionCandidateV1 {
  return Object.freeze({
    status: input.status,
    reason: input.reason,
    evaluatedAt: assertTimestamp(input.evaluatedAt, "candidate.evaluatedAt"),
    ...(input.eligibleAt ? { eligibleAt: assertTimestamp(input.eligibleAt, "candidate.eligibleAt") } : {}),
    blockers: Object.freeze(input.blockers.map(normalizeBlocker))
  });
}

function normalizeBlocker(input: SessionDeletionBlockerV1): SessionDeletionBlockerV1 {
  return Object.freeze({
    code: input.code,
    observedAt: assertTimestamp(input.observedAt, "candidate.blocker.observedAt")
  });
}

function normalizeCount(input: SessionDeletionCountV1): SessionDeletionCountV1 {
  return Object.freeze({
    class: assertSafeMetadataString(input.class, "summary.count.class"),
    count: nonNegativeInteger(input.count, "summary.count.count"),
    status: input.status,
    ...(input.countedAt ? { countedAt: assertTimestamp(input.countedAt, "summary.count.countedAt") } : {}),
    ...(input.errorClass ? { errorClass: assertSafeMetadataString(input.errorClass, "summary.count.errorClass") } : {})
  });
}

function buildSessionDeletionSummary(
  input: readonly SessionDeletionCountV1[],
  candidate: SessionDeletionCandidateV1
): SessionDeletionManifestSummaryV1 {
  const counts = Object.freeze(input.map(normalizeCount));
  return Object.freeze({
    totalCount: counts.reduce((sum, item) => sum + item.count, 0),
    failedCountClasses: counts.filter((item) => item.status === "failed").length,
    partialCountClasses: counts.filter((item) => item.status === "partial").length,
    blockerCount: candidate.blockers.length,
    counts
  });
}

function normalizeSummary(input: SessionDeletionManifestSummaryV1): SessionDeletionManifestSummaryV1 {
  return Object.freeze({
    totalCount: nonNegativeInteger(input.totalCount, "summary.totalCount"),
    failedCountClasses: nonNegativeInteger(input.failedCountClasses, "summary.failedCountClasses"),
    partialCountClasses: nonNegativeInteger(input.partialCountClasses, "summary.partialCountClasses"),
    blockerCount: nonNegativeInteger(input.blockerCount, "summary.blockerCount"),
    counts: Object.freeze(input.counts.map(normalizeCount))
  });
}

function normalizeOrderProof(input: SessionDeletionOrderProofV1): SessionDeletionOrderProofV1 {
  return Object.freeze({
    manifest: normalizeManifestProof(input.manifest),
    purge: normalizePurgeProof(input.purge)
  });
}

function normalizeManifestProof(input: SessionDeletionManifestProofV1): SessionDeletionManifestProofV1 {
  return Object.freeze({
    status: input.status,
    ...(input.mode ? { mode: input.mode } : {}),
    ...(input.writtenAt ? { writtenAt: assertTimestamp(input.writtenAt, "proof.manifest.writtenAt") } : {})
  });
}

function normalizePurgeProof(input: SessionDeletionPurgeProofV1): SessionDeletionPurgeProofV1 {
  return Object.freeze({
    status: input.status,
    ...(input.startedAt ? { startedAt: assertTimestamp(input.startedAt, "proof.purge.startedAt") } : {}),
    ...(input.completedAt ? { completedAt: assertTimestamp(input.completedAt, "proof.purge.completedAt") } : {}),
    ...(input.deletedObjectCount !== undefined
      ? { deletedObjectCount: nonNegativeInteger(input.deletedObjectCount, "proof.purge.deletedObjectCount") }
      : {})
  });
}

function addSessionBlockers(
  blockers: SessionDeletionBlockerV1[],
  session: SessionDeletionCandidateSessionV1,
  observedAt: string
): void {
  if (!isTerminalStatusLike(session.status)) {
    blockers.push(blocker("non_terminal", observedAt));
  }

  if (session.held) {
    blockers.push(blocker("held", observedAt));
  }
  if (session.retentionExempt) {
    blockers.push(blocker("retention_exempt", observedAt));
  }
  if (session.unresolvedCleanup) {
    blockers.push(blocker("unresolved_cleanup", observedAt));
  }
  if (session.unresolvedCustody) {
    blockers.push(blocker("unresolved_custody", observedAt));
  }
}

function blocker(code: SessionDeletionBlocker, observedAt: string): SessionDeletionBlockerV1 {
  return Object.freeze({ code, observedAt });
}

function isTerminalStatusLike(status: string): boolean {
  return isTerminalSessionWorkflowStatus(status as SessionWorkflowStatus);
}

function addDaysIso(timestamp: string, days: number): string {
  const start = Date.parse(assertTimestamp(timestamp, "candidate.session.terminalAt"));
  const end = start + positiveInteger(days, "policy.retentionDays") * 24 * 60 * 60 * 1000;
  return new Date(end).toISOString();
}

function visitRetentionValue(
  input: unknown,
  path: string,
  findings: SessionRetentionRedactionFinding[]
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
  findings: SessionRetentionRedactionFinding[]
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
  readonly reason: Exclude<SessionRetentionRedactionReason, "forbidden_field_name">;
  readonly regex: RegExp;
}[] = Object.freeze([
  { reason: "signed_url", regex: /[?&](?:X-Amz-Signature|X-Amz-Credential|X-Amz-Algorithm|AWSAccessKeyId)=/i },
  { reason: "object_store_key", regex: /(^|[\s"'`])(?:sessions|assets)\/[^?<#\s"'`]+/i },
  { reason: "vault_id", regex: /\b(?:vault|vlt|secret)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i },
  {
    reason: "private_resource_handle",
    regex: /\b(?:machine|resource|handle|provider|asset)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i
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
    throw new SessionRetentionValidationError(`session retention ${field} must be an opaque identifier without path separators`);
  }
  assertPublicSafeSessionRetentionPayload(value);
  return value;
}

function assertSafeMetadataString(value: string, field: string): string {
  assertNonEmptyString(value, field);
  assertPublicSafeSessionRetentionPayload(value);
  return value;
}

function assertNonEmptyString(value: string, field: string): void {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new SessionRetentionValidationError(`session retention ${field} must be a non-empty string`);
  }
}

function assertTimestamp(value: string, field: string): string {
  assertSafeMetadataString(value, field);
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) {
    throw new SessionRetentionValidationError(`session retention ${field} must be an ISO timestamp string`);
  }
  return value;
}

function nonNegativeInteger(value: number, field: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new SessionRetentionValidationError(`session retention ${field} must be a non-negative safe integer`);
  }
  return value;
}

function positiveInteger(value: number, field: string): number {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new SessionRetentionValidationError(`session retention ${field} must be a positive safe integer`);
  }
  return value;
}

function formatFindingPaths(findings: readonly SessionRetentionRedactionFinding[]): string {
  return findings.map((finding) => `${finding.path} (${finding.reason})`).join(", ");
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

import type { CredentialMode } from "./managed-key.js";
import type { RunStatus } from "./status.js";
import type { RunProvider, RuntimeKind } from "./submission.js";

export const CUSTODY_MANIFEST_SCHEMA_VERSION = 1;
export const CUSTODY_TOMBSTONE_SCHEMA_VERSION = 1;
export const CUSTODY_REDACTION_SCANNER_VERSION = 1;

export const CUSTODY_MANIFEST_KIND = "antpath.custody_manifest.v1";
export const CUSTODY_TOMBSTONE_KIND = "antpath.custody_tombstone.v1";
export const CUSTODY_MANIFEST_CONTENT_TYPE = "application/json; charset=utf-8";
export const CUSTODY_MANIFEST_RUN_REL_PATH = "metadata/custody.json";

export const CUSTODY_SECRET_CLASSES = [
  "provider_api_key",
  "mcp_credential",
  "proxy_endpoint_auth",
  "runner_bearer",
  "managed_system_credential"
] as const;
export type CustodySecretClass = (typeof CUSTODY_SECRET_CLASSES)[number];

export const CUSTODY_RESOURCE_CLASSES = [
  "runtime_machine",
  "native_provider_session",
  "native_provider_resource",
  "provider_asset",
  "proxy_token",
  "execution_secret",
  "event_archive",
  "run_output",
  "run_log",
  "run_asset"
] as const;
export type CustodyResourceClass = (typeof CUSTODY_RESOURCE_CLASSES)[number];

export const CUSTODY_EXPOSURE_SURFACES = [
  "antpath_vault",
  "antpath_kv",
  "fly_machine_env",
  "fly_machine_file",
  "provider_vault",
  "provider_session",
  "dashboard_proxy",
  "api_provider_proxy",
  "api_mcp_proxy",
  "run_artifact_store",
  "coordinator_event_archive"
] as const;
export type CustodyExposureSurface = (typeof CUSTODY_EXPOSURE_SURFACES)[number];

export const CUSTODY_EXPOSURE_ACCESS_KINDS = [
  "stored",
  "injected",
  "proxied",
  "replicated",
  "observed",
  "unknown"
] as const;
export type CustodyExposureAccessKind = (typeof CUSTODY_EXPOSURE_ACCESS_KINDS)[number];

export const CUSTODY_EXPOSURE_STATUSES = [
  "exposed",
  "revoked",
  "retained",
  "failed",
  "not_applicable"
] as const;
export type CustodyExposureStatus = (typeof CUSTODY_EXPOSURE_STATUSES)[number];

export const CUSTODY_DISPOSITION_STATUSES = [
  "destroyed",
  "revoked",
  "not_found",
  "provider_delete_confirmed",
  "retained_by_policy",
  "cleanup_failed",
  "parked_for_operator",
  "pending",
  "not_applicable"
] as const;
export type CustodyDispositionStatus = (typeof CUSTODY_DISPOSITION_STATUSES)[number];

export const CUSTODY_CLEANUP_STATUSES = [
  "succeeded",
  "partial",
  "failed",
  "retained",
  "pending"
] as const;
export type CustodyCleanupStatus = (typeof CUSTODY_CLEANUP_STATUSES)[number];

export const CUSTODY_EVIDENCE_SOURCES = [
  "run_row",
  "runtime_manifest",
  "terminal_event",
  "cleanup_step",
  "output_capture",
  "proxy_audit",
  "provider_cleanup_summary"
] as const;
export type CustodyEvidenceSource = (typeof CUSTODY_EVIDENCE_SOURCES)[number];

export const CUSTODY_MANIFEST_EXCLUDED_VALUE_CLASSES = [
  "raw_secret_values",
  "bearer_hashes",
  "provider_response_bodies",
  "signed_urls",
  "r2_object_keys",
  "vault_ids",
  "private_resource_handles"
] as const;
export type CustodyManifestExcludedValueClass =
  (typeof CUSTODY_MANIFEST_EXCLUDED_VALUE_CLASSES)[number];

export const CUSTODY_TOMBSTONE_MANIFEST_STATUSES = [
  "written",
  "not_written",
  "write_failed",
  "purged"
] as const;
export type CustodyTombstoneManifestStatus =
  (typeof CUSTODY_TOMBSTONE_MANIFEST_STATUSES)[number];

export const CUSTODY_TOMBSTONE_DELETION_STATUSES = [
  "not_deleted",
  "pending_delete",
  "deleted"
] as const;
export type CustodyTombstoneDeletionStatus =
  (typeof CUSTODY_TOMBSTONE_DELETION_STATUSES)[number];

export interface CustodyManifestRunV1 {
  readonly runId: string;
  readonly workspaceId: string;
  readonly provider: RunProvider | string;
  readonly runtime: RuntimeKind | string;
  readonly terminalStatus: RunStatus | string;
  readonly credentialMode?: CredentialMode;
  readonly createdAt?: string;
  readonly terminalAt?: string;
}

export interface CustodyManifestExposureV1 {
  readonly surface: CustodyExposureSurface | string;
  readonly access: CustodyExposureAccessKind;
  readonly status?: CustodyExposureStatus;
  readonly count?: number;
  readonly firstExposedAt?: string;
  readonly lastExposedAt?: string;
  readonly revokedAt?: string;
}

export interface CustodyManifestDispositionV1 {
  readonly status: CustodyDispositionStatus;
  readonly reason?: string;
  readonly decidedAt?: string;
  readonly errorClass?: string;
  readonly followUpRequired?: boolean;
}

export interface CustodyManifestEvidenceSummaryV1 {
  readonly source: CustodyEvidenceSource | string;
  readonly status?: string;
  readonly observedAt?: string;
  readonly count?: number;
  readonly errorClass?: string;
}

export interface CustodyManifestSecretClassV1 {
  readonly class: CustodySecretClass | string;
  readonly present: boolean;
  readonly count: number;
  readonly exposures: readonly CustodyManifestExposureV1[];
  readonly disposition: CustodyManifestDispositionV1;
  readonly evidence?: readonly CustodyManifestEvidenceSummaryV1[];
}

export interface CustodyManifestResourceClassV1 {
  readonly class: CustodyResourceClass | string;
  readonly count: number;
  readonly exposures: readonly CustodyManifestExposureV1[];
  readonly disposition: CustodyManifestDispositionV1;
  readonly evidence?: readonly CustodyManifestEvidenceSummaryV1[];
}

export interface CustodyManifestCleanupV1 {
  readonly status: CustodyCleanupStatus;
  readonly startedAt?: string;
  readonly finishedAt?: string;
  readonly errorClass?: string;
  readonly followUpRequiredCount?: number;
}

export interface CustodyManifestSummaryV1 {
  readonly secretClassCount: number;
  readonly secretInstanceCount: number;
  readonly resourceClassCount: number;
  readonly resourceInstanceCount: number;
  readonly exposureCount: number;
  readonly revokedExposureCount: number;
  readonly retainedCount: number;
  readonly cleanupFailedCount: number;
  readonly parkedForOperatorCount: number;
  readonly pendingCount: number;
  readonly followUpRequiredCount: number;
  readonly dispositionCounts: Readonly<Partial<Record<CustodyDispositionStatus, number>>>;
}

export interface CustodyManifestRedactionV1 {
  readonly policy: "metadata_only";
  readonly scannerVersion: typeof CUSTODY_REDACTION_SCANNER_VERSION;
  readonly excludes: readonly CustodyManifestExcludedValueClass[];
}

export interface CustodyManifestV1 {
  readonly schemaVersion: typeof CUSTODY_MANIFEST_SCHEMA_VERSION;
  readonly kind: typeof CUSTODY_MANIFEST_KIND;
  readonly generatedAt: string;
  readonly finalizedAt?: string;
  readonly run: CustodyManifestRunV1;
  readonly secrets: readonly CustodyManifestSecretClassV1[];
  readonly resources: readonly CustodyManifestResourceClassV1[];
  readonly cleanup: CustodyManifestCleanupV1;
  readonly summary: CustodyManifestSummaryV1;
  readonly redaction: CustodyManifestRedactionV1;
}

export type CustodyManifestRunInput = CustodyManifestRunV1;
export type CustodyManifestExposureInput = CustodyManifestExposureV1;
export type CustodyManifestDispositionInput = CustodyManifestDispositionV1;
export type CustodyManifestEvidenceSummaryInput = CustodyManifestEvidenceSummaryV1;
export type CustodyManifestCleanupInput = CustodyManifestCleanupV1;

export interface CustodyManifestSecretClassInput
  extends Omit<CustodyManifestSecretClassV1, "count" | "exposures"> {
  readonly count?: number;
  readonly exposures?: readonly CustodyManifestExposureInput[];
}

export interface CustodyManifestResourceClassInput
  extends Omit<CustodyManifestResourceClassV1, "count" | "exposures"> {
  readonly count?: number;
  readonly exposures?: readonly CustodyManifestExposureInput[];
}

export interface CustodyManifestInput {
  readonly generatedAt: string;
  readonly finalizedAt?: string;
  readonly run: CustodyManifestRunInput;
  readonly secrets?: readonly CustodyManifestSecretClassInput[];
  readonly resources?: readonly CustodyManifestResourceClassInput[];
  readonly cleanup?: CustodyManifestCleanupInput;
}

export interface CustodyTombstoneRunV1 {
  readonly runId: string;
  readonly workspaceId: string;
  readonly terminalStatus: RunStatus | string;
  readonly terminalAt?: string;
}

export interface CustodyTombstoneManifestV1 {
  readonly schemaVersion: typeof CUSTODY_MANIFEST_SCHEMA_VERSION;
  readonly status: CustodyTombstoneManifestStatus;
  readonly generatedAt?: string;
  readonly finalizedAt?: string;
  readonly tombstonedAt: string;
}

export interface CustodyTombstoneDeletionV1 {
  readonly status: CustodyTombstoneDeletionStatus;
  readonly pendingAt?: string;
  readonly deletedAt?: string;
}

export interface CustodyTombstoneRetentionV1 {
  readonly defaultPolicy: "retain_indefinitely";
  readonly userAction: "purge_or_anonymize_later";
}

export interface CustodyTombstoneV1 {
  readonly schemaVersion: typeof CUSTODY_TOMBSTONE_SCHEMA_VERSION;
  readonly kind: typeof CUSTODY_TOMBSTONE_KIND;
  readonly run: CustodyTombstoneRunV1;
  readonly manifest: CustodyTombstoneManifestV1;
  readonly summary: CustodyManifestSummaryV1;
  readonly deletion: CustodyTombstoneDeletionV1;
  readonly retention: CustodyTombstoneRetentionV1;
}

export interface CustodyTombstoneInput {
  readonly run: CustodyTombstoneRunV1;
  readonly manifest: Omit<CustodyTombstoneManifestV1, "schemaVersion">;
  readonly summary: CustodyManifestSummaryV1;
  readonly deletion?: CustodyTombstoneDeletionV1;
}

export interface CustodyTombstoneFromManifestInput {
  readonly manifestStatus?: CustodyTombstoneManifestStatus;
  readonly tombstonedAt: string;
  readonly deletion?: CustodyTombstoneDeletionV1;
}

export interface CustodyManifestWriteObject {
  readonly key: string;
  readonly contentType: typeof CUSTODY_MANIFEST_CONTENT_TYPE;
  readonly manifest: CustodyManifestV1;
}

export interface CustodyManifestObjectStore {
  putCustodyManifestObject(object: CustodyManifestWriteObject): Promise<void>;
}

export interface CustodyManifestWriteResult {
  readonly status: "written";
  readonly schemaVersion: typeof CUSTODY_MANIFEST_SCHEMA_VERSION;
  readonly runId: string;
  readonly workspaceId: string;
  readonly key: string;
  readonly writtenAt: string;
}

export interface CustodyManifestWriter {
  writeCustodyManifest(input: CustodyManifestInput): Promise<CustodyManifestWriteResult>;
}

export type CustodyRedactionReason =
  | "forbidden_field_name"
  | "bearer_token"
  | "provider_key"
  | "signed_url"
  | "r2_object_key"
  | "vault_id"
  | "private_resource_handle"
  | "high_entropy_token";

export interface CustodyRedactionFinding {
  readonly path: string;
  readonly reason: CustodyRedactionReason;
  readonly valueLength?: number;
}

export class CustodyManifestRedactionError extends Error {
  readonly code = "custody_manifest_not_public_safe";
  readonly findings: readonly CustodyRedactionFinding[];

  constructor(findings: readonly CustodyRedactionFinding[]) {
    super(`custody manifest contains non-public custody data at ${formatFindingPaths(findings)}`);
    this.name = "CustodyManifestRedactionError";
    this.findings = Object.freeze([...findings]);
  }
}

export class FakeCustodyManifestObjectStore implements CustodyManifestObjectStore {
  #objects = new Map<string, CustodyManifestV1>();

  async putCustodyManifestObject(object: CustodyManifestWriteObject): Promise<void> {
    assertPublicSafeCustodyPayload(object.manifest);
    this.#objects.set(object.key, cloneJson(object.manifest));
  }

  getByRunId(runId: string): CustodyManifestV1 | undefined {
    return this.get(custodyManifestObjectKey(runId));
  }

  get(key: string): CustodyManifestV1 | undefined {
    const object = this.#objects.get(key);
    return object ? cloneJson(object) : undefined;
  }

  listKeys(): readonly string[] {
    return Object.freeze([...this.#objects.keys()].sort());
  }
}

export function custodyManifestObjectKey(runId: string): string {
  assertSafeIdentifier(runId, "runId");
  return `runs/${runId}/${CUSTODY_MANIFEST_RUN_REL_PATH}`;
}

export function createCustodyManifestWriter(
  store: CustodyManifestObjectStore
): CustodyManifestWriter {
  return {
    async writeCustodyManifest(input: CustodyManifestInput): Promise<CustodyManifestWriteResult> {
      return writeCustodyManifest(store, input);
    }
  };
}

export async function writeCustodyManifest(
  store: CustodyManifestObjectStore,
  input: CustodyManifestInput
): Promise<CustodyManifestWriteResult> {
  const manifest = buildCustodyManifest(input);
  const key = custodyManifestObjectKey(manifest.run.runId);
  await store.putCustodyManifestObject({
    key,
    contentType: CUSTODY_MANIFEST_CONTENT_TYPE,
    manifest
  });
  return Object.freeze({
    status: "written",
    schemaVersion: CUSTODY_MANIFEST_SCHEMA_VERSION,
    runId: manifest.run.runId,
    workspaceId: manifest.run.workspaceId,
    key,
    writtenAt: manifest.finalizedAt ?? manifest.generatedAt
  });
}

export function buildCustodyManifest(input: CustodyManifestInput): CustodyManifestV1 {
  const secrets = Object.freeze((input.secrets ?? []).map(normalizeSecretClass));
  const resources = Object.freeze((input.resources ?? []).map(normalizeResourceClass));
  const cleanup = normalizeCleanup(input.cleanup);
  const manifest = Object.freeze({
    schemaVersion: CUSTODY_MANIFEST_SCHEMA_VERSION,
    kind: CUSTODY_MANIFEST_KIND,
    generatedAt: assertTimestamp(input.generatedAt, "generatedAt"),
    ...(input.finalizedAt ? { finalizedAt: assertTimestamp(input.finalizedAt, "finalizedAt") } : {}),
    run: normalizeRun(input.run),
    secrets,
    resources,
    cleanup,
    summary: buildCustodySummary(secrets, resources, cleanup),
    redaction: Object.freeze({
      policy: "metadata_only" as const,
      scannerVersion: CUSTODY_REDACTION_SCANNER_VERSION,
      excludes: Object.freeze([...CUSTODY_MANIFEST_EXCLUDED_VALUE_CLASSES])
    })
  }) satisfies CustodyManifestV1;
  assertPublicSafeCustodyPayload(manifest);
  return manifest;
}

export function buildCustodyTombstone(input: CustodyTombstoneInput): CustodyTombstoneV1 {
  const tombstone = Object.freeze({
    schemaVersion: CUSTODY_TOMBSTONE_SCHEMA_VERSION,
    kind: CUSTODY_TOMBSTONE_KIND,
    run: normalizeTombstoneRun(input.run),
    manifest: normalizeTombstoneManifest(input.manifest),
    summary: normalizeSummary(input.summary),
    deletion: normalizeTombstoneDeletion(input.deletion ?? { status: "not_deleted" }),
    retention: Object.freeze({
      defaultPolicy: "retain_indefinitely" as const,
      userAction: "purge_or_anonymize_later" as const
    })
  }) satisfies CustodyTombstoneV1;
  assertPublicSafeCustodyPayload(tombstone);
  return tombstone;
}

export function buildCustodyTombstoneFromManifest(
  manifest: CustodyManifestV1,
  input: CustodyTombstoneFromManifestInput
): CustodyTombstoneV1 {
  return buildCustodyTombstone({
    run: {
      runId: manifest.run.runId,
      workspaceId: manifest.run.workspaceId,
      terminalStatus: manifest.run.terminalStatus,
      ...(manifest.run.terminalAt ? { terminalAt: manifest.run.terminalAt } : {})
    },
    manifest: {
      status: input.manifestStatus ?? "written",
      generatedAt: manifest.generatedAt,
      ...(manifest.finalizedAt ? { finalizedAt: manifest.finalizedAt } : {}),
      tombstonedAt: input.tombstonedAt
    },
    summary: manifest.summary,
    ...(input.deletion ? { deletion: input.deletion } : {})
  });
}

export function scanCustodyPayloadForSensitiveValues(input: unknown): readonly CustodyRedactionFinding[] {
  const findings: CustodyRedactionFinding[] = [];
  visitCustodyValue(input, "$", findings);
  return Object.freeze(findings);
}

export function assertPublicSafeCustodyPayload(input: unknown): void {
  const findings = scanCustodyPayloadForSensitiveValues(input);
  if (findings.length > 0) {
    throw new CustodyManifestRedactionError(findings);
  }
}

function normalizeRun(input: CustodyManifestRunInput): CustodyManifestRunV1 {
  return Object.freeze({
    runId: assertSafeIdentifier(input.runId, "run.runId"),
    workspaceId: assertSafeIdentifier(input.workspaceId, "run.workspaceId"),
    provider: assertSafeMetadataString(input.provider, "run.provider"),
    runtime: assertSafeMetadataString(input.runtime, "run.runtime"),
    terminalStatus: assertSafeMetadataString(input.terminalStatus, "run.terminalStatus"),
    ...(input.credentialMode ? { credentialMode: input.credentialMode } : {}),
    ...(input.createdAt ? { createdAt: assertTimestamp(input.createdAt, "run.createdAt") } : {}),
    ...(input.terminalAt ? { terminalAt: assertTimestamp(input.terminalAt, "run.terminalAt") } : {})
  });
}

function normalizeSecretClass(input: CustodyManifestSecretClassInput): CustodyManifestSecretClassV1 {
  const count = nonNegativeInteger(input.count ?? (input.present ? 1 : 0), "secret.count");
  return Object.freeze({
    class: assertSafeMetadataString(input.class, "secret.class"),
    present: input.present,
    count,
    exposures: Object.freeze((input.exposures ?? []).map(normalizeExposure)),
    disposition: normalizeDisposition(input.disposition),
    ...(input.evidence ? { evidence: Object.freeze(input.evidence.map(normalizeEvidence)) } : {})
  });
}

function normalizeResourceClass(input: CustodyManifestResourceClassInput): CustodyManifestResourceClassV1 {
  const count = nonNegativeInteger(input.count ?? 1, "resource.count");
  return Object.freeze({
    class: assertSafeMetadataString(input.class, "resource.class"),
    count,
    exposures: Object.freeze((input.exposures ?? []).map(normalizeExposure)),
    disposition: normalizeDisposition(input.disposition),
    ...(input.evidence ? { evidence: Object.freeze(input.evidence.map(normalizeEvidence)) } : {})
  });
}

function normalizeExposure(input: CustodyManifestExposureInput): CustodyManifestExposureV1 {
  return Object.freeze({
    surface: assertSafeMetadataString(input.surface, "exposure.surface"),
    access: input.access,
    ...(input.status ? { status: input.status } : {}),
    ...(input.count !== undefined ? { count: nonNegativeInteger(input.count, "exposure.count") } : {}),
    ...(input.firstExposedAt
      ? { firstExposedAt: assertTimestamp(input.firstExposedAt, "exposure.firstExposedAt") }
      : {}),
    ...(input.lastExposedAt
      ? { lastExposedAt: assertTimestamp(input.lastExposedAt, "exposure.lastExposedAt") }
      : {}),
    ...(input.revokedAt ? { revokedAt: assertTimestamp(input.revokedAt, "exposure.revokedAt") } : {})
  });
}

function normalizeDisposition(input: CustodyManifestDispositionInput): CustodyManifestDispositionV1 {
  return Object.freeze({
    status: input.status,
    ...(input.reason ? { reason: assertSafeMetadataString(input.reason, "disposition.reason") } : {}),
    ...(input.decidedAt ? { decidedAt: assertTimestamp(input.decidedAt, "disposition.decidedAt") } : {}),
    ...(input.errorClass ? { errorClass: assertSafeMetadataString(input.errorClass, "disposition.errorClass") } : {}),
    ...(input.followUpRequired !== undefined ? { followUpRequired: input.followUpRequired } : {})
  });
}

function normalizeEvidence(input: CustodyManifestEvidenceSummaryInput): CustodyManifestEvidenceSummaryV1 {
  return Object.freeze({
    source: assertSafeMetadataString(input.source, "evidence.source"),
    ...(input.status ? { status: assertSafeMetadataString(input.status, "evidence.status") } : {}),
    ...(input.observedAt ? { observedAt: assertTimestamp(input.observedAt, "evidence.observedAt") } : {}),
    ...(input.count !== undefined ? { count: nonNegativeInteger(input.count, "evidence.count") } : {}),
    ...(input.errorClass ? { errorClass: assertSafeMetadataString(input.errorClass, "evidence.errorClass") } : {})
  });
}

function normalizeCleanup(input: CustodyManifestCleanupInput | undefined): CustodyManifestCleanupV1 {
  if (!input) {
    return Object.freeze({ status: "pending" });
  }
  return Object.freeze({
    status: input.status,
    ...(input.startedAt ? { startedAt: assertTimestamp(input.startedAt, "cleanup.startedAt") } : {}),
    ...(input.finishedAt ? { finishedAt: assertTimestamp(input.finishedAt, "cleanup.finishedAt") } : {}),
    ...(input.errorClass ? { errorClass: assertSafeMetadataString(input.errorClass, "cleanup.errorClass") } : {}),
    ...(input.followUpRequiredCount !== undefined
      ? {
          followUpRequiredCount: nonNegativeInteger(
            input.followUpRequiredCount,
            "cleanup.followUpRequiredCount"
          )
        }
      : {})
  });
}

function buildCustodySummary(
  secrets: readonly CustodyManifestSecretClassV1[],
  resources: readonly CustodyManifestResourceClassV1[],
  cleanup: CustodyManifestCleanupV1
): CustodyManifestSummaryV1 {
  const records = [...secrets, ...resources];
  const dispositionCounts: Partial<Record<CustodyDispositionStatus, number>> = {};
  let exposureCount = 0;
  let revokedExposureCount = 0;
  let followUpRequiredCount = cleanup.followUpRequiredCount ?? 0;

  for (const record of records) {
    dispositionCounts[record.disposition.status] = (dispositionCounts[record.disposition.status] ?? 0) + 1;
    exposureCount += record.exposures.length;
    revokedExposureCount += record.exposures.filter((exposure) => exposure.status === "revoked").length;
    if (record.disposition.followUpRequired) {
      followUpRequiredCount++;
    }
  }

  return Object.freeze({
    secretClassCount: secrets.length,
    secretInstanceCount: secrets.reduce((sum, record) => sum + record.count, 0),
    resourceClassCount: resources.length,
    resourceInstanceCount: resources.reduce((sum, record) => sum + record.count, 0),
    exposureCount,
    revokedExposureCount,
    retainedCount: dispositionCounts.retained_by_policy ?? 0,
    cleanupFailedCount: dispositionCounts.cleanup_failed ?? 0,
    parkedForOperatorCount: dispositionCounts.parked_for_operator ?? 0,
    pendingCount: dispositionCounts.pending ?? 0,
    followUpRequiredCount,
    dispositionCounts: Object.freeze({ ...dispositionCounts })
  });
}

function normalizeSummary(input: CustodyManifestSummaryV1): CustodyManifestSummaryV1 {
  return Object.freeze({
    secretClassCount: nonNegativeInteger(input.secretClassCount, "summary.secretClassCount"),
    secretInstanceCount: nonNegativeInteger(input.secretInstanceCount, "summary.secretInstanceCount"),
    resourceClassCount: nonNegativeInteger(input.resourceClassCount, "summary.resourceClassCount"),
    resourceInstanceCount: nonNegativeInteger(input.resourceInstanceCount, "summary.resourceInstanceCount"),
    exposureCount: nonNegativeInteger(input.exposureCount, "summary.exposureCount"),
    revokedExposureCount: nonNegativeInteger(input.revokedExposureCount, "summary.revokedExposureCount"),
    retainedCount: nonNegativeInteger(input.retainedCount, "summary.retainedCount"),
    cleanupFailedCount: nonNegativeInteger(input.cleanupFailedCount, "summary.cleanupFailedCount"),
    parkedForOperatorCount: nonNegativeInteger(
      input.parkedForOperatorCount,
      "summary.parkedForOperatorCount"
    ),
    pendingCount: nonNegativeInteger(input.pendingCount, "summary.pendingCount"),
    followUpRequiredCount: nonNegativeInteger(
      input.followUpRequiredCount,
      "summary.followUpRequiredCount"
    ),
    dispositionCounts: Object.freeze(
      Object.fromEntries(
        Object.entries(input.dispositionCounts).map(([key, value]) => [
          key,
          nonNegativeInteger(value, `summary.dispositionCounts.${key}`)
        ])
      ) as Partial<Record<CustodyDispositionStatus, number>>
    )
  });
}

function normalizeTombstoneRun(input: CustodyTombstoneRunV1): CustodyTombstoneRunV1 {
  return Object.freeze({
    runId: assertSafeIdentifier(input.runId, "tombstone.run.runId"),
    workspaceId: assertSafeIdentifier(input.workspaceId, "tombstone.run.workspaceId"),
    terminalStatus: assertSafeMetadataString(input.terminalStatus, "tombstone.run.terminalStatus"),
    ...(input.terminalAt ? { terminalAt: assertTimestamp(input.terminalAt, "tombstone.run.terminalAt") } : {})
  });
}

function normalizeTombstoneManifest(
  input: Omit<CustodyTombstoneManifestV1, "schemaVersion">
): CustodyTombstoneManifestV1 {
  return Object.freeze({
    schemaVersion: CUSTODY_MANIFEST_SCHEMA_VERSION,
    status: input.status,
    ...(input.generatedAt ? { generatedAt: assertTimestamp(input.generatedAt, "tombstone.manifest.generatedAt") } : {}),
    ...(input.finalizedAt ? { finalizedAt: assertTimestamp(input.finalizedAt, "tombstone.manifest.finalizedAt") } : {}),
    tombstonedAt: assertTimestamp(input.tombstonedAt, "tombstone.manifest.tombstonedAt")
  });
}

function normalizeTombstoneDeletion(input: CustodyTombstoneDeletionV1): CustodyTombstoneDeletionV1 {
  return Object.freeze({
    status: input.status,
    ...(input.pendingAt ? { pendingAt: assertTimestamp(input.pendingAt, "tombstone.deletion.pendingAt") } : {}),
    ...(input.deletedAt ? { deletedAt: assertTimestamp(input.deletedAt, "tombstone.deletion.deletedAt") } : {})
  });
}

function visitCustodyValue(
  input: unknown,
  path: string,
  findings: CustodyRedactionFinding[]
): void {
  if (typeof input === "string") {
    scanStringValue(input, path, findings);
    return;
  }
  if (Array.isArray(input)) {
    input.forEach((value, index) => visitCustodyValue(value, `${path}[${index}]`, findings));
    return;
  }
  if (!input || typeof input !== "object") {
    return;
  }
  for (const [key, value] of Object.entries(input as Record<string, unknown>)) {
    const childPath = `${path}.${key}`;
    if (isForbiddenCustodyFieldName(key)) {
      findings.push(Object.freeze({ path: childPath, reason: "forbidden_field_name" }));
    }
    visitCustodyValue(value, childPath, findings);
  }
}

function scanStringValue(
  value: string,
  path: string,
  findings: CustodyRedactionFinding[]
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
  readonly reason: Exclude<CustodyRedactionReason, "forbidden_field_name">;
  readonly regex: RegExp;
}[] = Object.freeze([
  { reason: "bearer_token", regex: /\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/i },
  {
    reason: "provider_key",
    regex: /\b(?:sk-(?:ant|proj|live|test|deepseek|openai)|xox[baprs]-|AIza)[A-Za-z0-9_-]{8,}/i
  },
  { reason: "signed_url", regex: /[?&](?:X-Amz-Signature|X-Amz-Credential|X-Amz-Algorithm|AWSAccessKeyId)=/i },
  { reason: "r2_object_key", regex: /(^|[\s"'`])(?:runs|assets)\/[^?<#\s"'`]+/i },
  { reason: "vault_id", regex: /\b(?:vault|vlt|secret)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i },
  {
    reason: "private_resource_handle",
    regex: /\b(?:machine|session|agent|file|skill|env|resource|handle|token_hash|bearer_hash)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i
  },
  { reason: "high_entropy_token", regex: /\b(?=[A-Za-z0-9_-]{40,}\b)(?=.*[A-Za-z])(?=.*\d)[A-Za-z0-9_-]{40,}\b/ }
]);

function isForbiddenCustodyFieldName(key: string): boolean {
  return /^(apiKey|secretValue|bearerHash|signedUrl|r2Key|objectKey|vaultId|providerResponseBody|responseBody|privateResourceHandle|resourceHandle|rawBody)$/i.test(
    key
  );
}

function assertSafeIdentifier(value: string, field: string): string {
  assertNonEmptyString(value, field);
  if (!/^[A-Za-z0-9._:-]+$/.test(value)) {
    throw new Error(`custody manifest ${field} must be an opaque identifier without path separators`);
  }
  assertPublicSafeCustodyPayload(value);
  return value;
}

function assertSafeMetadataString(value: string, field: string): string {
  assertNonEmptyString(value, field);
  assertPublicSafeCustodyPayload(value);
  return value;
}

function assertNonEmptyString(value: string, field: string): void {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`custody manifest ${field} must be a non-empty string`);
  }
}

function assertTimestamp(value: string, field: string): string {
  assertSafeMetadataString(value, field);
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) {
    throw new Error(`custody manifest ${field} must be an ISO timestamp string`);
  }
  return value;
}

function nonNegativeInteger(value: number, field: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`custody manifest ${field} must be a non-negative safe integer`);
  }
  return value;
}

function formatFindingPaths(findings: readonly CustodyRedactionFinding[]): string {
  return findings.map((finding) => `${finding.path} (${finding.reason})`).join(", ");
}

function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

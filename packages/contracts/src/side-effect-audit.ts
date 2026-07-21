import type { ProviderName } from "./submission.js";
import {
  createForbiddenFieldNamePredicate,
  isPlatformContentDigest,
  privateResourceHandlePattern,
  PUBLIC_SAFE_SECRET_PATTERNS,
  scanPublicSafePayload,
  underscoredHighEntropyPattern,
  type PublicSafeStringPattern
} from "./sdk-secrets.js";
import { assertAllowedKeys, defineAllowedKeys } from "./allowed-keys.js";

export const SIDE_EFFECT_AUDIT_SCHEMA_VERSION = 1;
export const SIDE_EFFECT_AUDIT_REDACTION_SCANNER_VERSION = 1;
export const SIDE_EFFECT_AUDIT_KIND = "aex.side_effect_audit.v1";

export const SIDE_EFFECT_AUDIT_ACTIONS = [
  "session.submit.accepted",
  "session.submit.rejected",
  "session.cancel.requested",
  "session.delete.requested",
  "session.delete.completed",
  "session.delete.failed",
  "session.download.requested",
  "session.file.downloaded",
  "session.log.downloaded",
  "session.event.downloaded",
  "workspace.asset.uploaded",
  "workspace.asset.deleted",
  "proxy.endpoint.called",
  "mcp.credential.accessed",
  "mcp.proxy.called",
  "provider.proxy.called",
  "custody.manifest.written",
  "custody.transition.recorded",
  "runtime.cleanup.completed",
  "runtime.cleanup.failed",
  "terminal_redrive.attempted",
  "terminal_redrive.completed",
  "api_key.created",
  "api_key.deleted",
  "api_key.used"
] as const;
export type SideEffectAuditAction = (typeof SIDE_EFFECT_AUDIT_ACTIONS)[number];

export const SIDE_EFFECT_AUDIT_ACTOR_PRINCIPAL_TYPES = [
  "user",
  "api_key",
  "system",
  "runtime"
] as const;
export type SideEffectAuditActorPrincipalType =
  (typeof SIDE_EFFECT_AUDIT_ACTOR_PRINCIPAL_TYPES)[number];

export const SIDE_EFFECT_AUDIT_SOURCE_PLANES = [
  "dashboard",
  "api",
  "runtime",
  "system"
] as const;
export type SideEffectAuditSourcePlane = (typeof SIDE_EFFECT_AUDIT_SOURCE_PLANES)[number];

export const SIDE_EFFECT_AUDIT_AUTHENTICATION_KINDS = [
  "dashboard_auth",
  "api_key",
  "runner_token",
  "system"
] as const;
export type SideEffectAuditAuthenticationKind =
  (typeof SIDE_EFFECT_AUDIT_AUTHENTICATION_KINDS)[number];

export const SIDE_EFFECT_AUDIT_TARGET_TYPES = [
  "workspace",
  "session",
  "proxy_endpoint",
  "mcp_credential",
  "mcp_proxy",
  "provider_proxy",
  "file_archive",
  "session_file",
  "session_log",
  "session_event_stream",
  "workspace_asset",
  "custody_manifest",
  "custody_transition",
  "cleanup",
  "deletion",
  "terminal_redrive",
  "api_key"
] as const;
export type SideEffectAuditTargetType = (typeof SIDE_EFFECT_AUDIT_TARGET_TYPES)[number];

export const SIDE_EFFECT_AUDIT_OUTCOMES = [
  "accepted",
  "rejected",
  "succeeded",
  "failed",
  "denied",
  "canceled",
  "pending"
] as const;
export type SideEffectAuditOutcome = (typeof SIDE_EFFECT_AUDIT_OUTCOMES)[number];

export const SIDE_EFFECT_AUDIT_COUNT_NAMES = [
  "requestBytes",
  "responseBytes",
  "durationMs",
  "attemptCount",
  "retryCount",
  "fileCount",
  "logCount",
  "eventCount",
  "assetCount",
  "proxyCallCount",
  "mcpCallCount",
  "providerProxyCallCount",
  "secretClassCount",
  "resourceClassCount",
  "deletedObjectCount",
  "retainedObjectCount",
  "failedObjectCount",
  "quotaRequestedUnits",
  "quotaRemainingUnits",
  "reservationCount"
] as const;
export type SideEffectAuditCountName = (typeof SIDE_EFFECT_AUDIT_COUNT_NAMES)[number];

export const SIDE_EFFECT_AUDIT_TIMESTAMP_NAMES = [
  "startedAt",
  "finishedAt",
  "observedAt",
  "decidedAt",
  "deletedAt",
  "terminalAt",
  "expiresAt"
] as const;
export type SideEffectAuditTimestampName = (typeof SIDE_EFFECT_AUDIT_TIMESTAMP_NAMES)[number];

export const SIDE_EFFECT_AUDIT_METADATA_EXCLUDED_VALUE_CLASSES = [
  "headers",
  "bodies",
  "raw_urls",
  "raw_paths",
  "query_strings",
  "provider_response_bodies",
  "signed_urls",
  "object_store_keys",
  "vault_ids",
  "resource_handles",
  "bearer_hashes",
  "secret_values",
  "customer_or_agent_identity",
  "private_pricing_or_provider_deployment"
] as const;
export type SideEffectAuditMetadataExcludedValueClass =
  (typeof SIDE_EFFECT_AUDIT_METADATA_EXCLUDED_VALUE_CLASSES)[number];

export interface SideEffectAuditPrincipalV1 {
  readonly type: SideEffectAuditActorPrincipalType;
  /**
   * Existing platform principal reference such as app-user id or API-token id.
   * This is not an agent id, customer end-user id, provider session id, or
   * stateful identity primitive.
   */
  readonly ref?: string;
}

export interface SideEffectAuditActorV1 {
  readonly principal: SideEffectAuditPrincipalV1;
  readonly sourcePlane: SideEffectAuditSourcePlane;
  readonly authenticatedBy?: SideEffectAuditAuthenticationKind;
}

export interface SideEffectAuditTargetV1 {
  readonly type: SideEffectAuditTargetType;
  readonly id?: string;
  readonly name?: string;
}

export interface SideEffectAuditCorrelationV1 {
  readonly requestId?: string;
  readonly operationId?: string;
  readonly eventId?: string;
  readonly parentAuditId?: string;
  readonly sourceAuditId?: string;
  readonly idempotencyKeyRef?: string;
}

export interface SideEffectAuditStatusMetadataV1 {
  readonly status?: string;
  readonly statusCode?: number;
  readonly errorClass?: string;
  readonly denialReason?: string;
  readonly followUpRequired?: boolean;
}

export interface SideEffectAuditDimensionsMetadataV1 {
  readonly provider?: ProviderName | string;
  readonly namespace?: "metadata" | "events" | "logs" | "files" | "archive";
  readonly method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  readonly surface?: string;
}

export interface SideEffectAuditRedactionMetadataV1 {
  readonly policy: "metadata_only";
  readonly scannerVersion: typeof SIDE_EFFECT_AUDIT_REDACTION_SCANNER_VERSION;
  readonly excludes: readonly SideEffectAuditMetadataExcludedValueClass[];
}

export interface SideEffectAuditMetadataV1 {
  readonly status?: SideEffectAuditStatusMetadataV1;
  readonly counts?: Readonly<Partial<Record<SideEffectAuditCountName, number>>>;
  readonly timestamps?: Readonly<Partial<Record<SideEffectAuditTimestampName, string>>>;
  readonly dimensions?: SideEffectAuditDimensionsMetadataV1;
  readonly redaction: SideEffectAuditRedactionMetadataV1;
}

export interface SideEffectAuditEventV1 {
  readonly schemaVersion: typeof SIDE_EFFECT_AUDIT_SCHEMA_VERSION;
  readonly kind: typeof SIDE_EFFECT_AUDIT_KIND;
  readonly auditId?: string;
  readonly workspaceId: string;
  readonly sessionId?: string;
  readonly action: SideEffectAuditAction;
  readonly outcome: SideEffectAuditOutcome;
  readonly observedAt: string;
  readonly actor: SideEffectAuditActorV1;
  readonly target: SideEffectAuditTargetV1;
  readonly correlation?: SideEffectAuditCorrelationV1;
  readonly metadata: SideEffectAuditMetadataV1;
}

export type SideEffectAuditPrincipalInput = SideEffectAuditPrincipalV1;
export type SideEffectAuditActorInput = SideEffectAuditActorV1;
export type SideEffectAuditTargetInput = SideEffectAuditTargetV1;
export type SideEffectAuditCorrelationInput = SideEffectAuditCorrelationV1;

export type SideEffectAuditMetadataInput = Omit<SideEffectAuditMetadataV1, "redaction"> & {
  readonly redaction?: never;
};

type SupportedSideEffectAuditMetadataInput = Omit<SideEffectAuditMetadataInput, "redaction">;
type DeletionSideEffectAuditMetadataInput = Pick<
  SupportedSideEffectAuditMetadataInput,
  "status" | "counts" | "timestamps"
>;

export type SideEffectAuditEventInput = Omit<
  SideEffectAuditEventV1,
  "schemaVersion" | "kind" | "metadata"
> & {
  readonly metadata?: SideEffectAuditMetadataInput;
};

export interface SideEffectAuditSessionScopedInput {
  readonly workspaceId: string;
  readonly sessionId: string;
  readonly observedAt: string;
  readonly actor: SideEffectAuditActorInput;
  readonly correlation?: SideEffectAuditCorrelationInput;
  readonly metadata?: SideEffectAuditMetadataInput;
}

export type SideEffectAuditRedactionReason =
  | "forbidden_field_name"
  | "uninspectable_value"
  | "bearer_token"
  | "provider_key"
  | "signed_url"
  | "object_store_key"
  | "vault_id"
  | "private_resource_handle"
  | "raw_url"
  | "raw_path"
  | "high_entropy_token";

export interface SideEffectAuditRedactionFinding {
  readonly path: string;
  readonly reason: SideEffectAuditRedactionReason;
  readonly valueLength?: number;
}

export class SideEffectAuditRedactionError extends Error {
  readonly code = "side_effect_audit_not_public_safe";
  readonly findings: readonly SideEffectAuditRedactionFinding[];

  constructor(findings: readonly SideEffectAuditRedactionFinding[]) {
    super(`side-effect audit payload contains non-public data at ${formatFindingPaths(findings)}`);
    this.name = "SideEffectAuditRedactionError";
    this.findings = Object.freeze([...findings]);
  }
}

export function buildSideEffectAuditEvent(
  input: SideEffectAuditEventInput
): SideEffectAuditEventV1 {
  const action = normalizeAction(input.action);
  const event = Object.freeze({
    schemaVersion: SIDE_EFFECT_AUDIT_SCHEMA_VERSION,
    kind: SIDE_EFFECT_AUDIT_KIND,
    ...(input.auditId ? { auditId: assertSafeIdentifier(input.auditId, "auditId") } : {}),
    workspaceId: assertSafeIdentifier(input.workspaceId, "workspaceId"),
    ...(input.sessionId ? { sessionId: assertSafeIdentifier(input.sessionId, "sessionId") } : {}),
    action,
    outcome: normalizeOutcome(input.outcome),
    observedAt: assertTimestamp(input.observedAt, "observedAt"),
    actor: normalizeActor(input.actor),
    target: normalizeTarget(input.target),
    ...(input.correlation ? { correlation: normalizeCorrelation(input.correlation) } : {}),
    metadata: redactSideEffectAuditMetadata(input.metadata ?? {}, action)
  }) satisfies SideEffectAuditEventV1;
  assertPublicSafeSideEffectAuditPayload(event);
  return event;
}

export function buildSessionDeletionRequestedAuditEvent(
  input: SideEffectAuditSessionScopedInput
): SideEffectAuditEventV1 {
  return buildSessionScopedAuditEvent(input, {
    action: "session.delete.requested",
    outcome: "accepted",
    targetType: "deletion"
  });
}

export function buildSessionDeletionCompletedAuditEvent(
  input: SideEffectAuditSessionScopedInput
): SideEffectAuditEventV1 {
  return buildSessionScopedAuditEvent(input, {
    action: "session.delete.completed",
    outcome: "succeeded",
    targetType: "deletion"
  });
}

export function buildSessionDeletionFailedAuditEvent(
  input: SideEffectAuditSessionScopedInput
): SideEffectAuditEventV1 {
  return buildSessionScopedAuditEvent(input, {
    action: "session.delete.failed",
    outcome: "failed",
    targetType: "deletion"
  });
}

export function buildSessionDownloadRequestedAuditEvent(
  input: SideEffectAuditSessionScopedInput
): SideEffectAuditEventV1 {
  return buildSessionScopedAuditEvent(input, {
    action: "session.download.requested",
    outcome: "accepted",
    targetType: "file_archive"
  });
}

export function buildCustodyManifestWrittenAuditEvent(
  input: SideEffectAuditSessionScopedInput
): SideEffectAuditEventV1 {
  return buildSessionScopedAuditEvent(input, {
    action: "custody.manifest.written",
    outcome: "succeeded",
    targetType: "custody_manifest"
  });
}

export function redactSideEffectAuditMetadata(
  input: SideEffectAuditMetadataInput,
  action?: SideEffectAuditAction
): SideEffectAuditMetadataV1 {
  assertPublicSafeSideEffectAuditPayload(input);
  assertSupportedMetadataKeys(input, action);
  const metadata = Object.freeze({
    ...(input.status ? { status: normalizeStatusMetadata(input.status) } : {}),
    ...(input.counts ? { counts: normalizeCounts(input.counts) } : {}),
    ...(input.timestamps ? { timestamps: normalizeTimestamps(input.timestamps) } : {}),
    ...(input.dimensions && !isDeletionAction(action)
      ? { dimensions: normalizeDimensions(input.dimensions) }
      : {}),
    redaction: Object.freeze({
      policy: "metadata_only" as const,
      scannerVersion: SIDE_EFFECT_AUDIT_REDACTION_SCANNER_VERSION,
      excludes: Object.freeze([...SIDE_EFFECT_AUDIT_METADATA_EXCLUDED_VALUE_CLASSES])
    })
  }) satisfies SideEffectAuditMetadataV1;
  assertPublicSafeSideEffectAuditPayload(metadata);
  return metadata;
}

export function scanSideEffectAuditPayloadForSensitiveValues(
  input: unknown
): readonly SideEffectAuditRedactionFinding[] {
  return scanPublicSafePayload(input, {
    patterns: auditStringPatterns,
    isForbiddenFieldName: isForbiddenAuditFieldName,
    isPatternExempt: ({ reason, path, match }) =>
      reason === "high_entropy_token" &&
      path === "$.target.id" &&
      isPlatformContentDigest(match)
  });
}

export function assertPublicSafeSideEffectAuditPayload(input: unknown): void {
  const findings = scanSideEffectAuditPayloadForSensitiveValues(input);
  if (findings.length > 0) {
    throw new SideEffectAuditRedactionError(findings);
  }
}

function normalizeActor(input: SideEffectAuditActorInput): SideEffectAuditActorV1 {
  if (!input || typeof input !== "object") {
    throw new Error("side-effect audit actor must be an object");
  }
  return Object.freeze({
    principal: normalizePrincipal(input.principal),
    sourcePlane: normalizeSourcePlane(input.sourcePlane),
    ...(input.authenticatedBy ? { authenticatedBy: normalizeAuthentication(input.authenticatedBy) } : {})
  });
}

function normalizePrincipal(input: SideEffectAuditPrincipalInput): SideEffectAuditPrincipalV1 {
  if (!input || typeof input !== "object") {
    throw new Error("side-effect audit actor principal must be an object");
  }
  const type = normalizePrincipalType(input.type);
  const ref = input.ref ? assertSafePrincipalRef(input.ref, "actor.principal.ref") : undefined;
  return Object.freeze({
    type,
    ...(ref ? { ref } : {})
  });
}

function normalizeTarget(input: SideEffectAuditTargetInput): SideEffectAuditTargetV1 {
  if (!input || typeof input !== "object") {
    throw new Error("side-effect audit target must be an object");
  }
  return Object.freeze({
    type: normalizeTargetType(input.type),
    ...(input.id ? { id: assertSafeIdentifier(input.id, "target.id") } : {}),
    ...(input.name ? { name: assertSafeMetadataString(input.name, "target.name") } : {})
  });
}

function normalizeCorrelation(input: SideEffectAuditCorrelationInput): SideEffectAuditCorrelationV1 {
  return Object.freeze({
    ...(input.requestId ? { requestId: assertSafeIdentifier(input.requestId, "correlation.requestId") } : {}),
    ...(input.operationId
      ? { operationId: assertSafeIdentifier(input.operationId, "correlation.operationId") }
      : {}),
    ...(input.eventId ? { eventId: assertSafeIdentifier(input.eventId, "correlation.eventId") } : {}),
    ...(input.parentAuditId
      ? { parentAuditId: assertSafeIdentifier(input.parentAuditId, "correlation.parentAuditId") }
      : {}),
    ...(input.sourceAuditId
      ? { sourceAuditId: assertSafeIdentifier(input.sourceAuditId, "correlation.sourceAuditId") }
      : {}),
    ...(input.idempotencyKeyRef
      ? {
          idempotencyKeyRef: assertSafeIdentifier(
            input.idempotencyKeyRef,
            "correlation.idempotencyKeyRef"
          )
        }
      : {})
  });
}

function normalizeStatusMetadata(
  input: SideEffectAuditStatusMetadataV1
): SideEffectAuditStatusMetadataV1 {
  const allowed = defineAllowedKeys<SideEffectAuditStatusMetadataV1>()(
    "status",
    "statusCode",
    "errorClass",
    "denialReason",
    "followUpRequired"
  );
  assertAllowedKeys(
    input,
    allowed,
    (key) => `side-effect audit metadata.status.${key} is not supported`
  );
  return Object.freeze({
    ...(input.status ? { status: assertSafeMetadataString(input.status, "metadata.status.status") } : {}),
    ...(input.statusCode !== undefined
      ? { statusCode: nonNegativeInteger(input.statusCode, "metadata.status.statusCode") }
      : {}),
    ...(input.errorClass
      ? { errorClass: assertSafeMetadataString(input.errorClass, "metadata.status.errorClass") }
      : {}),
    ...(input.denialReason
      ? { denialReason: assertSafeMetadataString(input.denialReason, "metadata.status.denialReason") }
      : {}),
    ...(input.followUpRequired !== undefined ? { followUpRequired: input.followUpRequired } : {})
  });
}

function normalizeCounts(
  input: Readonly<Partial<Record<SideEffectAuditCountName, number>>>
): Readonly<Partial<Record<SideEffectAuditCountName, number>>> {
  const out: Partial<Record<SideEffectAuditCountName, number>> = {};
  for (const [key, value] of Object.entries(input)) {
    if (!isStringIn(key, SIDE_EFFECT_AUDIT_COUNT_NAMES)) {
      throw new Error(`side-effect audit metadata.counts.${key} is not supported`);
    }
    if (value !== undefined) {
      out[key] = nonNegativeFinite(value, `metadata.counts.${key}`);
    }
  }
  return Object.freeze(out);
}

function normalizeTimestamps(
  input: Readonly<Partial<Record<SideEffectAuditTimestampName, string>>>
): Readonly<Partial<Record<SideEffectAuditTimestampName, string>>> {
  const out: Partial<Record<SideEffectAuditTimestampName, string>> = {};
  for (const [key, value] of Object.entries(input)) {
    if (!isStringIn(key, SIDE_EFFECT_AUDIT_TIMESTAMP_NAMES)) {
      throw new Error(`side-effect audit metadata.timestamps.${key} is not supported`);
    }
    if (value !== undefined) {
      out[key] = assertTimestamp(value, `metadata.timestamps.${key}`);
    }
  }
  return Object.freeze(out);
}

function normalizeDimensions(
  input: SideEffectAuditDimensionsMetadataV1
): SideEffectAuditDimensionsMetadataV1 {
  const allowed = defineAllowedKeys<SideEffectAuditDimensionsMetadataV1>()(
    "provider",
    "namespace",
    "method",
    "surface"
  );
  assertAllowedKeys(
    input,
    allowed,
    (key) => `side-effect audit metadata.dimensions.${key} is not supported`
  );
  return Object.freeze({
    ...(input.provider ? { provider: assertSafeMetadataString(input.provider, "metadata.dimensions.provider") } : {}),
    ...(input.namespace ? { namespace: input.namespace } : {}),
    ...(input.method ? { method: input.method } : {}),
    ...(input.surface ? { surface: assertSafeMetadataString(input.surface, "metadata.dimensions.surface") } : {})
  });
}

function assertSupportedMetadataKeys(
  input: SideEffectAuditMetadataInput,
  action: SideEffectAuditAction | undefined
): void {
  const allowed = isDeletionAction(action)
    ? defineAllowedKeys<DeletionSideEffectAuditMetadataInput>()("status", "counts", "timestamps")
    : defineAllowedKeys<SupportedSideEffectAuditMetadataInput>()("status", "counts", "timestamps", "dimensions");
  assertAllowedKeys(input, allowed, (key) => `side-effect audit metadata.${key} is not supported`);
}

function buildSessionScopedAuditEvent(
  input: SideEffectAuditSessionScopedInput,
  spec: {
    readonly action: SideEffectAuditAction;
    readonly outcome: SideEffectAuditOutcome;
    readonly targetType: SideEffectAuditTargetType;
  }
): SideEffectAuditEventV1 {
  return buildSideEffectAuditEvent({
    workspaceId: input.workspaceId,
    sessionId: input.sessionId,
    action: spec.action,
    outcome: spec.outcome,
    observedAt: input.observedAt,
    actor: input.actor,
    target: { type: spec.targetType, id: input.sessionId },
    ...(input.correlation ? { correlation: input.correlation } : {}),
    ...(input.metadata ? { metadata: input.metadata } : {})
  });
}

function normalizeAction(input: SideEffectAuditAction): SideEffectAuditAction {
  if (!isStringIn(input, SIDE_EFFECT_AUDIT_ACTIONS)) {
    throw new Error(`side-effect audit action ${String(input)} is not supported`);
  }
  return input;
}

function normalizeOutcome(input: SideEffectAuditOutcome): SideEffectAuditOutcome {
  if (!isStringIn(input, SIDE_EFFECT_AUDIT_OUTCOMES)) {
    throw new Error(`side-effect audit outcome ${String(input)} is not supported`);
  }
  return input;
}

function normalizePrincipalType(
  input: SideEffectAuditActorPrincipalType
): SideEffectAuditActorPrincipalType {
  if (!isStringIn(input, SIDE_EFFECT_AUDIT_ACTOR_PRINCIPAL_TYPES)) {
    throw new Error(`side-effect audit principal type ${String(input)} is not supported`);
  }
  return input;
}

function normalizeSourcePlane(input: SideEffectAuditSourcePlane): SideEffectAuditSourcePlane {
  if (!isStringIn(input, SIDE_EFFECT_AUDIT_SOURCE_PLANES)) {
    throw new Error(`side-effect audit source plane ${String(input)} is not supported`);
  }
  return input;
}

function normalizeAuthentication(
  input: SideEffectAuditAuthenticationKind
): SideEffectAuditAuthenticationKind {
  if (!isStringIn(input, SIDE_EFFECT_AUDIT_AUTHENTICATION_KINDS)) {
    throw new Error(`side-effect audit authentication kind ${String(input)} is not supported`);
  }
  return input;
}

function normalizeTargetType(input: SideEffectAuditTargetType): SideEffectAuditTargetType {
  if (!isStringIn(input, SIDE_EFFECT_AUDIT_TARGET_TYPES)) {
    throw new Error(`side-effect audit target type ${String(input)} is not supported`);
  }
  return input;
}

function isDeletionAction(action: SideEffectAuditAction | undefined): boolean {
  return action === "session.delete.requested" || action === "session.delete.completed" || action === "session.delete.failed";
}

const auditStringPatterns: readonly PublicSafeStringPattern<
  Exclude<SideEffectAuditRedactionReason, "forbidden_field_name" | "uninspectable_value">
>[] = Object.freeze([
  ...PUBLIC_SAFE_SECRET_PATTERNS,
  underscoredHighEntropyPattern(),
  privateResourceHandlePattern([
    "machine",
    "agent",
    "resource",
    "handle",
    "token_hash",
    "bearer_hash"
  ]),
  Object.freeze({ reason: "raw_url" as const, regex: /\bhttps?:\/\/\S+/i }),
  Object.freeze({
    reason: "raw_path" as const,
    regex: /(^|[\s"'`])\/[A-Za-z0-9._~!$&'()*+,;=:@%-]+(?:[/?#][^\s"'`]*)?/
  })
]);

const isForbiddenAuditFieldName = createForbiddenFieldNamePredicate([
  "authorization",
  "header",
  "headers",
  "requestHeader",
  "requestHeaders",
  "responseHeader",
  "responseHeaders",
  "body",
  "requestBody",
  "responseBody",
  "rawBody",
  "prompt",
  "url",
  "rawUrl",
  "href",
  "query",
  "queryString",
  "path",
  "rawPath",
  "signedUrl",
  "objectStoreKey",
  "objectKey",
  "vaultId",
  "providerResponseBody",
  "providerAccountId",
  "providerDeployment",
  "rateCard",
  "rateCardVersion",
  "margin",
  "discount",
  "calculator",
  "reconciliation",
  "resourceHandle",
  "privateResourceHandle",
  "bearerHash",
  "tokenHash",
  "apiKey",
  "apiKeys",
  "secretValue",
  "providerSessionId",
  "agentId",
  "customerId",
  "endUserId",
  "identity",
  "email"
]);

function assertSafeIdentifier(value: string, field: string): string {
  assertNonEmptyString(value, field);
  if (!/^[A-Za-z0-9._:-]+$/.test(value)) {
    throw new Error(`side-effect audit ${field} must be an opaque identifier without path separators`);
  }
  if (field === "target.id" && isPlatformContentDigest(value)) {
    return value;
  }
  assertPublicSafeSideEffectAuditPayload(value);
  return value;
}

function assertSafePrincipalRef(value: string, field: string): string {
  assertNonEmptyString(value, field);
  if (!/^[A-Za-z0-9._:-]+$/.test(value)) {
    throw new Error(`side-effect audit ${field} must be an opaque identifier without path separators`);
  }
  const ref = value;
  if (/^(?:agent|session|customer|end[_-]?user|provider[_-]?session)[_.:-]/i.test(ref)) {
    throw new Error(`side-effect audit ${field} must not introduce agent, session, or customer identity`);
  }
  assertPublicSafeSideEffectAuditPayload(ref);
  return ref;
}

function assertSafeMetadataString(value: string, field: string): string {
  assertNonEmptyString(value, field);
  assertPublicSafeSideEffectAuditPayload(value);
  return value;
}

function assertNonEmptyString(value: string, field: string): void {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`side-effect audit ${field} must be a non-empty string`);
  }
}

function assertTimestamp(value: string, field: string): string {
  assertSafeMetadataString(value, field);
  const ms = Date.parse(value);
  if (!Number.isFinite(ms)) {
    throw new Error(`side-effect audit ${field} must be an ISO timestamp string`);
  }
  return value;
}

function nonNegativeInteger(value: number, field: string): number {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`side-effect audit ${field} must be a non-negative safe integer`);
  }
  return value;
}

function nonNegativeFinite(value: number, field: string): number {
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`side-effect audit ${field} must be a non-negative finite number`);
  }
  return value;
}

function formatFindingPaths(findings: readonly SideEffectAuditRedactionFinding[]): string {
  return findings.map((finding) => `${finding.path} (${finding.reason})`).join(", ");
}

function isStringIn<T extends string>(value: unknown, allowed: readonly T[]): value is T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value);
}

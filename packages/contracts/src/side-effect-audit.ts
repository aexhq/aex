import type { CredentialMode, RunProvider, RuntimeKind } from "./submission.js";

export const SIDE_EFFECT_AUDIT_SCHEMA_VERSION = 1;
export const SIDE_EFFECT_AUDIT_REDACTION_SCANNER_VERSION = 1;
export const SIDE_EFFECT_AUDIT_KIND = "aex.side_effect_audit.v1";

export const SIDE_EFFECT_AUDIT_ACTIONS = [
  "run.submit.accepted",
  "run.submit.rejected",
  "run.cancel.requested",
  "run.delete.requested",
  "run.delete.completed",
  "run.delete.failed",
  "run.download.requested",
  "run.output.downloaded",
  "run.log.downloaded",
  "run.event.downloaded",
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
  "api_token.created",
  "api_token.deleted",
  "api_token.used"
] as const;
export type SideEffectAuditAction = (typeof SIDE_EFFECT_AUDIT_ACTIONS)[number];

export const SIDE_EFFECT_AUDIT_ACTOR_PRINCIPAL_TYPES = [
  "user",
  "api_token",
  "system",
  "runtime"
] as const;
export type SideEffectAuditActorPrincipalType =
  (typeof SIDE_EFFECT_AUDIT_ACTOR_PRINCIPAL_TYPES)[number];

export const SIDE_EFFECT_AUDIT_SOURCE_PLANES = [
  "dashboard",
  "worker",
  "runtime",
  "system"
] as const;
export type SideEffectAuditSourcePlane = (typeof SIDE_EFFECT_AUDIT_SOURCE_PLANES)[number];

export const SIDE_EFFECT_AUDIT_AUTHENTICATION_KINDS = [
  "dashboard_auth",
  "api_token",
  "runner_token",
  "system"
] as const;
export type SideEffectAuditAuthenticationKind =
  (typeof SIDE_EFFECT_AUDIT_AUTHENTICATION_KINDS)[number];

export const SIDE_EFFECT_AUDIT_TARGET_TYPES = [
  "workspace",
  "run",
  "proxy_endpoint",
  "mcp_credential",
  "mcp_proxy",
  "provider_proxy",
  "output_archive",
  "run_output",
  "run_log",
  "run_event_stream",
  "workspace_asset",
  "custody_manifest",
  "custody_transition",
  "cleanup",
  "deletion",
  "terminal_redrive",
  "api_token"
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
  "outputCount",
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
  readonly provider?: RunProvider | string;
  readonly runtime?: RuntimeKind | string;
  readonly credentialMode?: CredentialMode;
  readonly namespace?: "metadata" | "events" | "logs" | "outputs" | "archive";
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
  readonly runId?: string;
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

export type SideEffectAuditEventInput = Omit<
  SideEffectAuditEventV1,
  "schemaVersion" | "kind" | "metadata"
> & {
  readonly metadata?: SideEffectAuditMetadataInput;
};

export interface SideEffectAuditRunScopedInput {
  readonly workspaceId: string;
  readonly runId: string;
  readonly observedAt: string;
  readonly actor: SideEffectAuditActorInput;
  readonly correlation?: SideEffectAuditCorrelationInput;
  readonly metadata?: SideEffectAuditMetadataInput;
}

export type SideEffectAuditRedactionReason =
  | "forbidden_field_name"
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
    ...(input.runId ? { runId: assertSafeIdentifier(input.runId, "runId") } : {}),
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

export function buildRunDeletionRequestedAuditEvent(
  input: SideEffectAuditRunScopedInput
): SideEffectAuditEventV1 {
  return buildRunScopedAuditEvent(input, {
    action: "run.delete.requested",
    outcome: "accepted",
    targetType: "deletion"
  });
}

export function buildRunDeletionCompletedAuditEvent(
  input: SideEffectAuditRunScopedInput
): SideEffectAuditEventV1 {
  return buildRunScopedAuditEvent(input, {
    action: "run.delete.completed",
    outcome: "succeeded",
    targetType: "deletion"
  });
}

export function buildRunDeletionFailedAuditEvent(
  input: SideEffectAuditRunScopedInput
): SideEffectAuditEventV1 {
  return buildRunScopedAuditEvent(input, {
    action: "run.delete.failed",
    outcome: "failed",
    targetType: "deletion"
  });
}

export function buildRunDownloadRequestedAuditEvent(
  input: SideEffectAuditRunScopedInput
): SideEffectAuditEventV1 {
  return buildRunScopedAuditEvent(input, {
    action: "run.download.requested",
    outcome: "accepted",
    targetType: "output_archive"
  });
}

export function buildCustodyManifestWrittenAuditEvent(
  input: SideEffectAuditRunScopedInput
): SideEffectAuditEventV1 {
  return buildRunScopedAuditEvent(input, {
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
  const findings: SideEffectAuditRedactionFinding[] = [];
  visitAuditValue(input, "$", findings);
  return Object.freeze(findings);
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
  assertSupportedNestedKeys(input, ["status", "statusCode", "errorClass", "denialReason", "followUpRequired"], "metadata.status");
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
  assertSupportedNestedKeys(
    input,
    ["provider", "runtime", "credentialMode", "namespace", "method", "surface"],
    "metadata.dimensions"
  );
  return Object.freeze({
    ...(input.provider ? { provider: assertSafeMetadataString(input.provider, "metadata.dimensions.provider") } : {}),
    ...(input.runtime ? { runtime: assertSafeMetadataString(input.runtime, "metadata.dimensions.runtime") } : {}),
    ...(input.credentialMode ? { credentialMode: input.credentialMode } : {}),
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
    ? ["status", "counts", "timestamps"]
    : ["status", "counts", "timestamps", "dimensions"];
  assertSupportedNestedKeys(input, allowed, "metadata");
}

function assertSupportedNestedKeys(input: object, allowed: readonly string[], field: string): void {
  for (const key of Object.keys(input)) {
    if (!allowed.includes(key)) {
      throw new Error(`side-effect audit ${field}.${key} is not supported`);
    }
  }
}

function buildRunScopedAuditEvent(
  input: SideEffectAuditRunScopedInput,
  spec: {
    readonly action: SideEffectAuditAction;
    readonly outcome: SideEffectAuditOutcome;
    readonly targetType: SideEffectAuditTargetType;
  }
): SideEffectAuditEventV1 {
  return buildSideEffectAuditEvent({
    workspaceId: input.workspaceId,
    runId: input.runId,
    action: spec.action,
    outcome: spec.outcome,
    observedAt: input.observedAt,
    actor: input.actor,
    target: { type: spec.targetType, id: input.runId },
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
  return action === "run.delete.requested" || action === "run.delete.completed" || action === "run.delete.failed";
}

function visitAuditValue(
  input: unknown,
  path: string,
  findings: SideEffectAuditRedactionFinding[]
): void {
  if (typeof input === "string") {
    scanStringValue(input, path, findings);
    return;
  }
  if (Array.isArray(input)) {
    input.forEach((value, index) => visitAuditValue(value, `${path}[${index}]`, findings));
    return;
  }
  if (!input || typeof input !== "object") {
    return;
  }
  for (const [key, value] of Object.entries(input as Record<string, unknown>)) {
    const childPath = `${path}.${key}`;
    if (isForbiddenAuditFieldName(key)) {
      findings.push(Object.freeze({ path: childPath, reason: "forbidden_field_name" }));
    }
    visitAuditValue(value, childPath, findings);
  }
}

function scanStringValue(
  value: string,
  path: string,
  findings: SideEffectAuditRedactionFinding[]
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
  readonly reason: Exclude<SideEffectAuditRedactionReason, "forbidden_field_name">;
  readonly regex: RegExp;
}[] = Object.freeze([
  { reason: "bearer_token", regex: /\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/i },
  {
    reason: "provider_key",
    regex: /\b(?:sk-(?:ant|proj|live|test|deepseek|openai)|xox[baprs]-|AIza)[A-Za-z0-9_-]{8,}/i
  },
  { reason: "signed_url", regex: /[?&](?:X-Amz-Signature|X-Amz-Credential|X-Amz-Algorithm|AWSAccessKeyId)=/i },
  { reason: "object_store_key", regex: /(^|[\s"'`])(?:runs|assets)\/[^?<#\s"'`]+/i },
  { reason: "vault_id", regex: /\b(?:vault|vlt|secret)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i },
  {
    reason: "private_resource_handle",
    regex: /\b(?:machine|session|agent|file|skill|env|resource|handle|token_hash|bearer_hash)[_:-][A-Za-z0-9][A-Za-z0-9_-]{7,}\b/i
  },
  { reason: "raw_url", regex: /\bhttps?:\/\/\S+/i },
  { reason: "raw_path", regex: /(^|[\s"'`])\/[A-Za-z0-9._~!$&'()*+,;=:@%-]+(?:[/?#][^\s"'`]*)?/ },
  { reason: "high_entropy_token", regex: /\b(?=[A-Za-z0-9_-]{40,}\b)(?=.*[A-Za-z])(?=.*\d)[A-Za-z0-9_-]{40,}\b/ }
]);

function isForbiddenAuditFieldName(key: string): boolean {
  return /^(authorization|headers?|requestHeaders?|responseHeaders?|body|requestBody|responseBody|rawBody|prompt|url|rawUrl|href|query|queryString|path|rawPath|signedUrl|objectStoreKey|objectKey|vaultId|providerResponseBody|providerAccountId|providerDeployment|rateCard|rateCardVersion|margin|discount|calculator|reconciliation|resourceHandle|privateResourceHandle|bearerHash|tokenHash|apiKey|apiKeys|secretValue|sessionId|providerSessionId|agentId|customerId|endUserId|identity|email)$/i.test(
    key
  );
}

function assertSafeIdentifier(value: string, field: string): string {
  assertNonEmptyString(value, field);
  if (!/^[A-Za-z0-9._:-]+$/.test(value)) {
    throw new Error(`side-effect audit ${field} must be an opaque identifier without path separators`);
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

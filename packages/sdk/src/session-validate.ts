import {
  containsSecretLikeValue,
  redactString,
  SessionConfigValidationError,
  type ApprovalGate,
  type PlatformNetworking,
  type PlatformPackageInput,
  type ResponseFormat,
  type SessionRuntime,
  type SubmissionAssets,
  type WorkspaceFileRecord,
  type WorkspaceInstructionRecord,
  type WorkspaceSkillRecord,
  type WorkspaceToolRecord
} from "@aexhq/contracts";
import type {
  SessionCreateOptions,
  SessionEnvironmentOptions,
  SessionInput,
  SessionOverrides,
  SessionSendOptions,
  SessionStartOptions,
  StartSessionOptions
} from "./client-types.js";

type ExactKeys<Shape, Keys extends PropertyKey> =
  [Exclude<keyof Shape, Keys>] extends [never]
    ? [Exclude<Keys, keyof Shape>] extends [never]
      ? true
      : false
    : false;

/** Package-private compile-time proof that a runtime tuple's key set equals the public shape. */
export type ExactKeySet<Shape, Keys extends readonly PropertyKey[]> = ExactKeys<Shape, Keys[number]>;
type Assert<T extends true> = T;

const SESSION_CREATE_KEYS = [
  "model", "system", "assets", "mcpServers", "fileCapture",
  "builtinTools", "outputMode", "responseFormat", "approvalGate", "metadata",
  "idempotencyKey", "environment", "runtime", "overrides", "webhook"
] as const satisfies readonly (keyof SessionCreateOptions)[];
type SessionCreateKeysAreExact = Assert<ExactKeySet<SessionCreateOptions, typeof SESSION_CREATE_KEYS>>;

const SESSION_START_KEYS = [
  ...SESSION_CREATE_KEYS,
  "message", "deleteAfter", "messageIdempotencyKey", "stream"
] as const satisfies readonly (keyof SessionStartOptions)[];
type SessionStartKeysAreExact = Assert<ExactKeySet<SessionStartOptions, typeof SESSION_START_KEYS>>;

const START_CONTROL_KEYS = [
  "timeoutMs", "webSocketFactory", "idleTimeoutMs", "pingIntervalMs", "throwOnFailure"
] as const satisfies readonly (keyof StartSessionOptions)[];
type StartControlKeysAreExact = Assert<ExactKeySet<StartSessionOptions, typeof START_CONTROL_KEYS>>;

const SESSION_SEND_KEYS = [
  "webSocketFactory", "idleTimeoutMs", "pingIntervalMs", "idempotencyKey"
] as const satisfies readonly (keyof SessionSendOptions)[];
type SessionSendKeysAreExact = Assert<ExactKeySet<SessionSendOptions, typeof SESSION_SEND_KEYS>>;

type SessionStreamOptions = Omit<SessionSendOptions, "idempotencyKey">;
const SESSION_STREAM_KEYS = [
  "webSocketFactory", "idleTimeoutMs", "pingIntervalMs"
] as const satisfies readonly (keyof SessionStreamOptions)[];
type SessionStreamKeysAreExact = Assert<ExactKeySet<SessionStreamOptions, typeof SESSION_STREAM_KEYS>>;

const SESSION_OVERRIDE_KEYS = [
  "idleTtl", "timeout", "maxSpendUsd", "maxTurns"
] as const satisfies readonly (keyof SessionOverrides)[];
type SessionOverrideKeysAreExact = Assert<ExactKeySet<SessionOverrides, typeof SESSION_OVERRIDE_KEYS>>;

const SESSION_RUNTIME_KEYS = ["kind", "size"] as const satisfies readonly (keyof SessionRuntime)[];
type SessionRuntimeKeysAreExact = Assert<ExactKeySet<SessionRuntime, typeof SESSION_RUNTIME_KEYS>>;

type FileCaptureOptions = NonNullable<SessionCreateOptions["fileCapture"]>;
const FILE_CAPTURE_KEYS = [
  "allowedDirs", "deniedDirs", "captureTimeoutMs", "maxFileBytes", "maxTotalBytes", "maxFiles"
] as const satisfies readonly (keyof FileCaptureOptions)[];
type FileCaptureKeysAreExact = Assert<ExactKeySet<FileCaptureOptions, typeof FILE_CAPTURE_KEYS>>;

type SessionWebhookOptions = NonNullable<SessionCreateOptions["webhook"]>;
const SESSION_WEBHOOK_KEYS = ["url"] as const satisfies readonly (keyof SessionWebhookOptions)[];
type SessionWebhookKeysAreExact = Assert<ExactKeySet<SessionWebhookOptions, typeof SESSION_WEBHOOK_KEYS>>;

const SESSION_ENVIRONMENT_KEYS = [
  "networking", "packages", "variables", "secrets"
] as const satisfies readonly (keyof SessionEnvironmentOptions)[];
type SessionEnvironmentKeysAreExact = Assert<ExactKeySet<SessionEnvironmentOptions, typeof SESSION_ENVIRONMENT_KEYS>>;

const PLATFORM_NETWORKING_KEYS = [
  "mode", "allowedHosts"
] as const satisfies readonly (keyof PlatformNetworking)[];
type PlatformNetworkingKeysAreExact = Assert<ExactKeySet<PlatformNetworking, typeof PLATFORM_NETWORKING_KEYS>>;

const PLATFORM_PACKAGE_INPUT_KEYS = [
  "name", "version"
] as const satisfies readonly (keyof PlatformPackageInput)[];
type PlatformPackageInputKeysAreExact = Assert<ExactKeySet<PlatformPackageInput, typeof PLATFORM_PACKAGE_INPUT_KEYS>>;

type TextResponseFormat = Extract<ResponseFormat, { readonly kind: "text" }>;
const TEXT_RESPONSE_FORMAT_KEYS = ["kind"] as const satisfies readonly (keyof TextResponseFormat)[];
type TextResponseFormatKeysAreExact = Assert<ExactKeySet<TextResponseFormat, typeof TEXT_RESPONSE_FORMAT_KEYS>>;

type JsonSchemaResponseFormat = Extract<ResponseFormat, { readonly kind: "json_schema" }>;
const JSON_SCHEMA_RESPONSE_FORMAT_KEYS = [
  "kind", "schema", "strict", "name"
] as const satisfies readonly (keyof JsonSchemaResponseFormat)[];
type JsonSchemaResponseFormatKeysAreExact = Assert<ExactKeySet<JsonSchemaResponseFormat, typeof JSON_SCHEMA_RESPONSE_FORMAT_KEYS>>;

const APPROVAL_GATE_KEYS = ["tools"] as const satisfies readonly (keyof ApprovalGate)[];
type ApprovalGateKeysAreExact = Assert<ExactKeySet<ApprovalGate, typeof APPROVAL_GATE_KEYS>>;

const ASSET_CATEGORY_KEYS = [
  "files", "skills", "tools", "instructions"
] as const satisfies readonly (keyof SubmissionAssets)[];
type AssetCategoryKeysAreExact = Assert<ExactKeySet<SubmissionAssets, typeof ASSET_CATEGORY_KEYS>>;

const ASSET_ITEM_KEYS = {
  files: [
    "kind", "resourceId", "version", "assetId", "contentHash", "createdAt", "updatedAt",
    "sizeBytes", "contentType", "name", "mountPath"
  ],
  skills: [
    "kind", "resourceId", "version", "assetId", "contentHash", "createdAt", "updatedAt",
    "sizeBytes", "contentType", "name", "description"
  ],
  tools: [
    "kind", "resourceId", "version", "assetId", "contentHash", "createdAt", "updatedAt",
    "sizeBytes", "contentType", "name", "description", "input_schema", "entry"
  ],
  instructions: [
    "kind", "resourceId", "version", "assetId", "contentHash", "createdAt", "updatedAt",
    "sizeBytes", "contentType", "name"
  ]
} as const satisfies {
  readonly files: readonly (keyof WorkspaceFileRecord)[];
  readonly skills: readonly (keyof WorkspaceSkillRecord)[];
  readonly tools: readonly (keyof WorkspaceToolRecord)[];
  readonly instructions: readonly (keyof WorkspaceInstructionRecord)[];
};
type AssetFileKeysAreExact = Assert<ExactKeySet<WorkspaceFileRecord, typeof ASSET_ITEM_KEYS.files>>;
type AssetSkillKeysAreExact = Assert<ExactKeySet<WorkspaceSkillRecord, typeof ASSET_ITEM_KEYS.skills>>;
type AssetToolKeysAreExact = Assert<ExactKeySet<WorkspaceToolRecord, typeof ASSET_ITEM_KEYS.tools>>;
type AssetInstructionKeysAreExact = Assert<ExactKeySet<WorkspaceInstructionRecord, typeof ASSET_ITEM_KEYS.instructions>>;

export type SessionOptionKeyAssertions = readonly [
  SessionCreateKeysAreExact,
  SessionStartKeysAreExact,
  StartControlKeysAreExact,
  SessionSendKeysAreExact,
  SessionStreamKeysAreExact,
  SessionOverrideKeysAreExact,
  SessionRuntimeKeysAreExact,
  FileCaptureKeysAreExact,
  SessionWebhookKeysAreExact,
  SessionEnvironmentKeysAreExact,
  PlatformNetworkingKeysAreExact,
  PlatformPackageInputKeysAreExact,
  TextResponseFormatKeysAreExact,
  JsonSchemaResponseFormatKeysAreExact,
  ApprovalGateKeysAreExact,
  AssetCategoryKeysAreExact,
  AssetFileKeysAreExact,
  AssetSkillKeysAreExact,
  AssetToolKeysAreExact,
  AssetInstructionKeysAreExact
];

const VALIDATION_DIAGNOSTIC_MAX_LENGTH = 512;
const VALIDATION_DIAGNOSTIC_SOURCE_MAX_LENGTH = 4_096;
const VALIDATION_DIAGNOSTIC_MAX_TOKENS = 64;
const VALIDATION_DIAGNOSTIC_MAX_TOKEN_LENGTH = 1_024;

export type SessionConfigDiagnosticPolicy =
  | { readonly kind: "scalar"; readonly rejectedValues: readonly unknown[] }
  | { readonly kind: "redacted" };

function fallbackValidationDiagnostic(field: string): string {
  return `underlying validator rejected ${field}; diagnostic redacted`;
}

/**
 * Produce lossy diagnostic prose without retaining the caught value, its
 * properties, stack, or causal chain. Any uncertainty falls back to field-only
 * evidence rather than risking caller data in diagnostic-aware logging.
 */
function safeValidationDiagnostic(
  caught: unknown,
  field: string,
  policy: SessionConfigDiagnosticPolicy
): string {
  const fallback = fallbackValidationDiagnostic(field);
  if (policy.kind === "redacted") return fallback;
  try {
    const values = Array.from(policy.rejectedValues);
    if (values.length > VALIDATION_DIAGNOSTIC_MAX_TOKENS) return fallback;
    const tokens = values.filter((value): value is string => typeof value === "string" && value.length > 0);
    if (tokens.some((token) => token.length > VALIDATION_DIAGNOSTIC_MAX_TOKEN_LENGTH)) return fallback;

    const raw = typeof caught === "string"
      ? caught
      : caught instanceof Error
        ? caught.message
        : undefined;
    if (!raw) return fallback;
    let diagnostic = raw.slice(0, VALIDATION_DIAGNOSTIC_SOURCE_MAX_LENGTH);
    for (const token of tokens) {
      diagnostic = diagnostic.split(token).join("<redacted-value>");
    }
    diagnostic = diagnostic.replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ");
    diagnostic = redactString(diagnostic).trim();
    if (
      diagnostic.length === 0 ||
      containsSecretLikeValue(diagnostic) ||
      tokens.some((token) => diagnostic.includes(token))
    ) {
      return fallback;
    }
    return diagnostic.length <= VALIDATION_DIAGNOSTIC_MAX_LENGTH
      ? diagnostic
      : `${diagnostic.slice(0, VALIDATION_DIAGNOSTIC_MAX_LENGTH - 3)}...`;
  } catch {
    return fallback;
  }
}

function validationDiagnosticError(message: string): Error {
  const diagnostic = new Error(message);
  Object.defineProperty(diagnostic, "name", {
    configurable: true,
    value: "SessionConfigDiagnosticError"
  });
  return diagnostic;
}

/** Package-private catch-and-translate owner for canonical session validators. */
export function validatedSessionConfig<T>(
  surface: string,
  field: string,
  message: string,
  validate: () => T,
  policy: SessionConfigDiagnosticPolicy
): T {
  try {
    return validate();
  } catch (caught) {
    throw configError(
      surface,
      field,
      message,
      validationDiagnosticError(safeValidationDiagnostic(caught, field, policy))
    );
  }
}

/**
 * Package-private factory for SDK validation errors. `details.field` is the only
 * stable machine-readable payload; messages are human guidance and may change.
 */
export function configError(
  surface: string,
  field: string,
  message: string,
  cause?: Error
): SessionConfigValidationError {
  return new SessionConfigValidationError(
    `${surface}: ${message}`,
    { field },
    cause === undefined ? undefined : { cause }
  );
}

export function normaliseSessionInput(
  input: SessionInput,
  surface: string,
  field: string
): SessionInput {
  if (typeof input === "string") {
    if (!input) {
      throw configError(surface, field, `${field} must be a non-empty string`);
    }
    if (!input.trim()) {
      throw configError(surface, field, `${field} must contain non-whitespace text`);
    }
    return input;
  }
  if (!Array.isArray(input) || input.length === 0) {
    throw configError(surface, field, `${field} must be a non-empty string or string array`);
  }
  for (const segment of input) {
    if (typeof segment !== "string" || !segment) {
      throw configError(surface, field, `${field} segments must be non-empty strings`);
    }
  }
  if (input.every((segment) => !segment.trim())) {
    throw configError(surface, field, `${field} must contain non-whitespace text`);
  }
  return [...input];
}

export function assertSupportedSessionFields(
  options: SessionCreateOptions,
  surface: string,
  allowStartFields: boolean
): void {
  const record = options as unknown as Record<string, unknown>;
  const allowed = new Set<string>(allowStartFields ? SESSION_START_KEYS : SESSION_CREATE_KEYS);
  const guidance: Readonly<Record<string, string>> = {
    runtimeSize: "use runtime.size",
    runtimeKind: "use runtime.kind",
    secretEnv: "use environment.secrets",
    parentSessionId: "subagent lineage is assigned by the platform",
    message: "sessions are created without a first message; use Aex.start or session.messages.send",
    prompt: "use message",
    instructions: "publish Instructions through aex.workspace.instructions and pass the returned ref in assets.instructions"
  };
  for (const field of Object.keys(record)) {
    if (!allowed.has(field)) {
      const detail = guidance[field];
      throw configError(
        surface,
        field,
        `${field} is not a supported option${detail === undefined ? "" : `; ${detail}`}`
      );
    }
  }
  const overrides = record.overrides;
  if (overrides && typeof overrides === "object" && !Array.isArray(overrides)) {
    const overrideRecord = overrides as Record<string, unknown>;
    if (Object.prototype.hasOwnProperty.call(overrideRecord, "idleSuspendAfter")) {
      throw configError(
        surface,
        "overrides.idleSuspendAfter",
        "overrides.idleSuspendAfter is not a supported option; use overrides.idleTtl."
      );
    }
  }
  const runtime = record.runtime;
  if (runtime !== undefined) {
    if (typeof runtime !== "object" || runtime === null || Array.isArray(runtime)) {
      throw configError(surface, "runtime", "runtime must be an object like { kind, size }");
    }
    for (const key of Object.keys(runtime as Record<string, unknown>)) {
      if (!(SESSION_RUNTIME_KEYS as readonly string[]).includes(key)) {
        throw configError(surface, `runtime.${key}`, `runtime.${key} is not a supported option; use runtime.kind or runtime.size`);
      }
    }
  }
  assertStructuredSessionFields(record, surface, allowStartFields);
}

export function assertSupportedSessionSendOptions(
  options: unknown,
  surface: string,
  allowIdempotencyKey = true
): void {
  const record = options as Record<string, unknown> | undefined;
  if (!record || typeof record !== "object") return;
  const allowed = new Set<string>(allowIdempotencyKey ? SESSION_SEND_KEYS : SESSION_STREAM_KEYS);
  for (const field of Object.keys(record)) {
    if (allowed.has(field)) continue;
    const guidance = field === "from"
      ? "use session.events.list(), stream(), or streamEnvelopes() for replay"
      : field === "signal"
        ? "use session.cancel() / session.suspend() for remote control"
        : undefined;
    throw configError(
      surface,
      field,
      `${field} is not a supported option${guidance === undefined ? "" : `; ${guidance}`}`
    );
  }
}

export function assertStartSessionOptions(options: unknown, surface: string): void {
  assertAllowedObjectFields(options, surface, "options", START_CONTROL_KEYS);
}

function assertStructuredSessionFields(
  record: Record<string, unknown>,
  surface: string,
  allowStartFields: boolean
): void {
  const overrides = assertAllowedObjectFields(
    record.overrides,
    surface,
    "overrides",
    SESSION_OVERRIDE_KEYS
  );
  void overrides;
  assertAssetsFields(record.assets, surface);
  assertAllowedObjectFields(
    record.fileCapture,
    surface,
    "fileCapture",
    FILE_CAPTURE_KEYS
  );
  assertEnvironmentFields(record.environment, surface);
  assertAllowedObjectFields(record.webhook, surface, "webhook", SESSION_WEBHOOK_KEYS);

  const responseFormat = assertRecord(record.responseFormat, surface, "responseFormat");
  if (responseFormat !== undefined) {
    const allowed = responseFormat.kind === "text"
      ? TEXT_RESPONSE_FORMAT_KEYS
      : JSON_SCHEMA_RESPONSE_FORMAT_KEYS;
    assertAllowedKeys(responseFormat, surface, "responseFormat", allowed);
  }
  assertAllowedObjectFields(record.approvalGate, surface, "approvalGate", APPROVAL_GATE_KEYS);
  if (allowStartFields) {
    assertAllowedObjectFields(
      record.stream,
      surface,
      "stream",
      SESSION_STREAM_KEYS
    );
  }
}

function assertAssetsFields(value: unknown, surface: string): void {
  const assets = assertAllowedObjectFields(
    value,
    surface,
    "assets",
    ASSET_CATEGORY_KEYS
  );
  if (assets === undefined) return;
  // Workspace publish methods return records that extend the reusable ref with
  // immutable metadata. Accept those records directly so publish -> session is ergonomic.
  for (const category of ASSET_CATEGORY_KEYS) {
    const allowed = ASSET_ITEM_KEYS[category];
    const entries = assets[category];
    if (entries === undefined) continue;
    if (!Array.isArray(entries)) {
      throw configError(surface, `assets.${category}`, `assets.${category} must be an array`);
    }
    entries.forEach((entry, index) => {
      const field = `assets.${category}[${index}]`;
      const item = assertRecord(entry, surface, field);
      if (item !== undefined) assertAllowedKeys(item, surface, field, allowed);
    });
  }
}

function assertEnvironmentFields(value: unknown, surface: string): void {
  const environment = assertAllowedObjectFields(
    value,
    surface,
    "environment",
    SESSION_ENVIRONMENT_KEYS
  );
  if (environment === undefined) return;
  assertAllowedObjectFields(
    environment.networking,
    surface,
    "environment.networking",
    PLATFORM_NETWORKING_KEYS
  );
  for (const field of ["variables", "secrets"] as const) {
    assertRecord(environment[field], surface, `environment.${field}`);
  }
  const packages = environment.packages;
  if (packages === undefined) return;
  if (!Array.isArray(packages)) {
    throw configError(surface, "environment.packages", "environment.packages must be an array");
  }
  packages.forEach((entry, index) => {
    const field = `environment.packages[${index}]`;
    const item = assertRecord(entry, surface, field);
    if (item !== undefined) assertAllowedKeys(item, surface, field, PLATFORM_PACKAGE_INPUT_KEYS);
  });
}

export function assertAllowedObjectFields(
  value: unknown,
  surface: string,
  field: string,
  allowed: readonly string[]
): Record<string, unknown> | undefined {
  const record = assertRecord(value, surface, field);
  if (record !== undefined) assertAllowedKeys(record, surface, field, allowed);
  return record;
}

function assertRecord(
  value: unknown,
  surface: string,
  field: string
): Record<string, unknown> | undefined {
  if (value === undefined) return undefined;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw configError(surface, field, `${field} must be an object`);
  }
  return value as Record<string, unknown>;
}

function assertAllowedKeys(
  record: Record<string, unknown>,
  surface: string,
  field: string,
  allowed: readonly string[]
): void {
  const allowedSet = new Set(allowed);
  for (const key of Object.keys(record)) {
    if (allowedSet.has(key)) continue;
    const nested = `${field}.${key}`;
    throw configError(surface, nested, `${nested} is not a supported option`);
  }
}


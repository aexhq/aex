import {
  SessionConfigValidationError,
  type ProviderName
} from "@aexhq/contracts";
import type { SessionCreateOptions, SessionInput } from "./client-types.js";

/**
 * Package-private factory for SDK validation errors. `details.field` is the only
 * stable machine-readable payload; messages are human guidance and may change.
 */
export function configError(surface: string, field: string, message: string): SessionConfigValidationError {
  return new SessionConfigValidationError(`${surface}: ${message}`, { field });
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
  const allowed = new Set([
    "provider", "model", "system", "assets", "mcpServers", "fileCapture",
    "builtinTools", "outputMode", "responseFormat", "approvalGate", "metadata",
    "idempotencyKey", "apiKeys", "environment", "runtime", "overrides", "webhook",
    ...(allowStartFields ? ["message", "deleteAfter", "messageIdempotencyKey", "stream"] : [])
  ]);
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
      if (key !== "kind" && key !== "size") {
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
  const allowed = new Set([
    "webSocketFactory",
    "idleTimeoutMs",
    "pingIntervalMs",
    ...(allowIdempotencyKey ? ["idempotencyKey"] : [])
  ]);
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

function assertStructuredSessionFields(
  record: Record<string, unknown>,
  surface: string,
  allowStartFields: boolean
): void {
  const overrides = assertAllowedObjectFields(
    record.overrides,
    surface,
    "overrides",
    ["idleTtl", "timeout", "maxSpendUsd", "maxTurns"]
  );
  void overrides;
  assertAssetsFields(record.assets, surface);
  assertAllowedObjectFields(
    record.fileCapture,
    surface,
    "fileCapture",
    ["allowedDirs", "deniedDirs", "captureTimeoutMs", "maxFileBytes", "maxTotalBytes", "maxFiles"]
  );
  assertEnvironmentFields(record.environment, surface);
  assertAllowedObjectFields(record.webhook, surface, "webhook", ["url"]);

  const responseFormat = assertRecord(record.responseFormat, surface, "responseFormat");
  if (responseFormat !== undefined) {
    const allowed = responseFormat.kind === "text"
      ? ["kind"]
      : ["kind", "schema", "strict", "name"];
    assertAllowedKeys(responseFormat, surface, "responseFormat", allowed);
  }
  assertAllowedObjectFields(record.approvalGate, surface, "approvalGate", ["tools"]);
  if (allowStartFields) {
    assertAllowedObjectFields(
      record.stream,
      surface,
      "stream",
      ["webSocketFactory", "idleTimeoutMs", "pingIntervalMs"]
    );
  }
}

function assertAssetsFields(value: unknown, surface: string): void {
  const assets = assertAllowedObjectFields(
    value,
    surface,
    "assets",
    ["files", "skills", "tools", "instructions"]
  );
  if (assets === undefined) return;
  // Workspace publish methods return records that extend the reusable ref with
  // immutable metadata. Accept those records directly so publish -> session is ergonomic.
  const common = [
    "kind", "resourceId", "version", "assetId", "contentHash",
    "createdAt", "updatedAt", "sizeBytes", "contentType"
  ];
  const fields: Readonly<Record<string, readonly string[]>> = {
    files: [...common, "name", "mountPath"],
    skills: [...common, "name", "description"],
    tools: [...common, "name", "description", "input_schema", "entry"],
    instructions: [...common, "name"]
  };
  for (const [category, allowed] of Object.entries(fields)) {
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
    ["networking", "packages", "variables", "secrets"]
  );
  if (environment === undefined) return;
  assertAllowedObjectFields(
    environment.networking,
    surface,
    "environment.networking",
    ["mode", "allowedHosts"]
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
    if (item !== undefined) assertAllowedKeys(item, surface, field, ["name", "version"]);
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

export function validateApiKeys(
  apiKeys: Partial<Record<ProviderName, string>> | undefined,
  provider: ProviderName,
  surface: string
): void {
  const key = apiKeys?.[provider];
  if (typeof key !== "string" || key.length === 0) {
    throw configError(surface, `apiKeys.${provider}`, "a provider API key is required in apiKeys");
  }
}

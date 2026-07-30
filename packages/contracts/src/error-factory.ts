/**
 * The SINGLE wire→exception mapping. `apiErrorFromResponse` reads a non-2xx
 * body's stable `error` code, attaches it (typed) as `apiCode` plus the
 * `requestId`, and dispatches to the right {@link AexApiError} subclass. Both
 * `HttpClient.request` and `HttpClient.download` construct errors ONLY through
 * here, so the wire→exception mapping lives in one place — no inline
 * `new AexApiError(...)` with a hardcoded coarse code.
 */

import {
  AexApiError,
  AexAuthError,
  AexIdempotencyConflictError,
  AexNotFoundError,
  AexRateLimitError,
  type AexApiErrorInit
} from "./sdk-errors.js";
import { AEX_API_ERROR_MESSAGES, isAexApiErrorCode, type AexApiErrorCode } from "./error-codes.js";

export interface ApiErrorFromResponseInput {
  readonly status: number;
  readonly body: unknown;
  /**
   * Precomputed human message (e.g. `HttpClient`'s `extractErrorMessage`). When
   * absent the factory derives one from the stable code table.
   */
  readonly message?: string;
  readonly requestId?: string;
  readonly cause?: unknown;
}

type ApiErrorKind = "auth" | "idempotency" | "not_found" | "rate_limit" | "generic";

/**
 * EXHAUSTIVE stable-code → subclass-kind map. There is NO `default`: adding a
 * code to {@link AEX_API_ERROR_CODES} without a case here fails to compile
 * (the function would fall through and return `undefined`, which the
 * `ApiErrorKind` return type forbids). This is the whole-class guard that a new
 * server code can never silently collapse to the base error.
 */
export function apiErrorKindForCode(code: AexApiErrorCode): ApiErrorKind {
  switch (code) {
    case "unauthorized":
    case "forbidden":
    case "insufficient_scope":
    case "token_invalid":
    case "token_revoked":
    case "token_expired":
    case "malformed_token":
      return "auth";
    case "idempotency_conflict":
    case "operation_idempotency_conflict":
      return "idempotency";
    case "not_found":
    case "approval_not_found":
    case "file_not_found":
    case "export_not_found":
      return "not_found";
    case "rate_limited":
      return "rate_limit";
    case "invalid_cursor":
    case "invalid_file_selection":
    case "session_not_idle":
    case "workspace_activation_required":
    case "workspace_not_live":
    case "session_deleting":
    case "session_deleted":
    case "deletion_in_progress":
    case "operation_not_cancelable":
    case "approval_already_resolved":
    case "export_revoked":
    case "invalid_range":
    case "download_grant_expired":
    case "telemetry_payload_too_large":
    case "invalid_telemetry":
    case "invalid_query":
    case "invalid_metric_aggregation":
    case "unsupported_export_signal":
    case "telemetry_incomplete":
    case "export_not_ready":
    case "export_expired":
    case "invalid_network_policy":
    case "unsupported_package_ecosystem":
    case "package_resolution_failed":
    case "package_artifact_unavailable":
    case "package_integrity_mismatch":
    case "invalid_auto_topup_policy":
    case "payment_method_required":
    case "authentication_unavailable":
    case "account_paused":
    case "account_state_unavailable":
    case "precondition_failed":
    case "wrong_workspace_region":
    case "telemetry_quota_exceeded":
    case "observability_unavailable":
    case "upstream_error":
    case "internal_error":
      return "generic";
  }
}

function kindForStatus(status: number): ApiErrorKind {
  if (status === 401 || status === 403) return "auth";
  if (status === 404) return "not_found";
  if (status === 429) return "rate_limit";
  return "generic";
}

export function apiErrorFromResponse(input: ApiErrorFromResponseInput): AexApiError {
  const apiCode = apiCodeFromBody(input.body);
  const requestId = input.requestId ?? requestIdFromBody(input.body);
  const message = input.message ?? fallbackMessage(apiCode, input.body);
  const kind = apiCode !== undefined ? apiErrorKindForCode(apiCode) : kindForStatus(input.status);
  const init: AexApiErrorInit = {
    status: input.status,
    message,
    body: input.body,
    apiCode,
    requestId,
    cause: input.cause
  };
  switch (kind) {
    case "auth":
      return new AexAuthError({ ...init, requiredScope: requiredScopeFromBody(input.body) });
    case "idempotency":
      return new AexIdempotencyConflictError(init);
    case "not_found":
      return new AexNotFoundError(init);
    case "rate_limit":
      return new AexRateLimitError({ ...init, retryAfterMs: retryAfterMsFromBody(input.body) });
    case "generic":
      return new AexApiError(input.status, message, input.body, {
        apiCode,
        requestId,
        ...(input.cause !== undefined ? { cause: input.cause } : {})
      });
  }
}

function asRecord(body: unknown): Record<string, unknown> | undefined {
  return body !== null && typeof body === "object" && !Array.isArray(body)
    ? (body as Record<string, unknown>)
    : undefined;
}

function errorRecordFromBody(body: unknown): Record<string, unknown> | undefined {
  const record = asRecord(body);
  return asRecord(record?.error) ?? record;
}

function apiCodeFromBody(body: unknown): AexApiErrorCode | undefined {
  const record = errorRecordFromBody(body);
  if (!record) return undefined;
  if (isAexApiErrorCode(record.code)) return record.code;
  if (isAexApiErrorCode(record.error)) return record.error;
  return undefined;
}

function requestIdFromBody(body: unknown): string | undefined {
  const record = errorRecordFromBody(body);
  const value = record?.requestId;
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function requiredScopeFromBody(body: unknown): string | undefined {
  const error = errorRecordFromBody(body);
  const record = asRecord(error?.details) ?? error;
  if (!record) return undefined;
  for (const key of ["requiredScope", "required_scope", "scope"]) {
    const value = record[key];
    if (typeof value === "string" && value.length > 0) return value;
  }
  return undefined;
}

function retryAfterMsFromBody(body: unknown): number | undefined {
  const error = errorRecordFromBody(body);
  const record = asRecord(error?.details) ?? error;
  if (!record) return undefined;
  if (typeof record.retryAfterMs === "number" && Number.isFinite(record.retryAfterMs)) {
    return record.retryAfterMs;
  }
  if (typeof record.retryAfter === "number" && Number.isFinite(record.retryAfter)) {
    return record.retryAfter * 1000;
  }
  return undefined;
}

function fallbackMessage(apiCode: AexApiErrorCode | undefined, body: unknown): string {
  const record = errorRecordFromBody(body);
  const message = record?.message;
  if (typeof message === "string" && message.length > 0) return message;
  if (apiCode !== undefined) return AEX_API_ERROR_MESSAGES[apiCode];
  const error = record?.error;
  if (typeof error === "string" && error.length > 0) return error;
  return "aex API request failed";
}

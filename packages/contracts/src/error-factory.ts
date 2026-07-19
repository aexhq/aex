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
  ContentDeletedError,
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

type ApiErrorKind = "auth" | "idempotency" | "not_found" | "rate_limit" | "content_deleted" | "generic";

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
      return "idempotency";
    case "not_found":
      return "not_found";
    case "rate_limited":
    case "workspace_concurrency_exceeded":
    case "workspace_submit_rate_exceeded":
      return "rate_limit";
    case "content_deleted":
      return "content_deleted";
    case "session_busy":
    case "checkpoint_not_available":
    case "session_not_terminal":
    case "session_terminal":
    case "event_archive_too_large":
    case "event_archive_deadline_exceeded":
    case "unknown_workspace":
    case "workspace_inactive":
    case "workspace_spend_cap_exceeded":
    case "insufficient_balance":
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
    case "content_deleted":
      return new ContentDeletedError({
        ...init,
        sessionId: sessionIdFromBody(input.body),
        purgedAt: purgedAtFromBody(input.body),
        deletedBy: deletedByFromBody(input.body)
      });
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

function apiCodeFromBody(body: unknown): AexApiErrorCode | undefined {
  const record = asRecord(body);
  if (!record) return undefined;
  if (isAexApiErrorCode(record.error)) return record.error;
  if (isAexApiErrorCode(record.code)) return record.code;
  return undefined;
}

function requestIdFromBody(body: unknown): string | undefined {
  const record = asRecord(body);
  const value = record?.requestId;
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function requiredScopeFromBody(body: unknown): string | undefined {
  const record = asRecord(body);
  if (!record) return undefined;
  for (const key of ["requiredScope", "required_scope", "scope"]) {
    const value = record[key];
    if (typeof value === "string" && value.length > 0) return value;
  }
  return undefined;
}

function sessionIdFromBody(body: unknown): string | undefined {
  const value = asRecord(body)?.sessionId;
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function purgedAtFromBody(body: unknown): string | undefined {
  const value = asRecord(body)?.purgedAt;
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function deletedByFromBody(body: unknown): "retention" | "user" | undefined {
  const value = asRecord(body)?.deletedBy;
  return value === "retention" || value === "user" ? value : undefined;
}

function retryAfterMsFromBody(body: unknown): number | undefined {
  const record = asRecord(body);
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
  if (apiCode !== undefined) return AEX_API_ERROR_MESSAGES[apiCode];
  const record = asRecord(body);
  const message = record?.message;
  if (typeof message === "string" && message.length > 0) return message;
  const error = record?.error;
  if (typeof error === "string" && error.length > 0) return error;
  return "aex API request failed";
}

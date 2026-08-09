import { ERROR_METADATA, type ErrorClass } from "../generated/errors.js";
import { type RouteId } from "../generated/routes.js";

/**
 * A public error code as it arrives on the wire.
 *
 * Deliberately open rather than a union of the generated vocabulary: a deployed
 * platform may answer with a code newer than the installed SDK, and an unknown
 * code must reach the caller as data instead of failing to type-check.
 * `ERROR_METADATA` is the closed part — a lookup miss is what "newer than this
 * SDK" looks like at runtime.
 */
export type AexErrorCode = string;

export class AexError extends Error {
  constructor(message: string) {
    super(message);
    this.name = new.target.name;
  }
}

export class AexConfigError extends AexError {}

export interface ApiErrorInput {
  readonly status: number;
  readonly code: AexErrorCode;
  readonly requestId: string;
  readonly retryable: boolean;
  readonly message: string;
  readonly operationId?: string;
  readonly details?: unknown;
  readonly retryAfterMs?: number;
}

export class AexApiError extends AexError {
  readonly status: number;
  readonly code: AexErrorCode;
  readonly requestId: string;
  readonly retryable: boolean;
  readonly operationId: string | undefined;
  readonly details: unknown;
  readonly retryAfterMs: number | undefined;

  constructor(input: ApiErrorInput) {
    super(redact(input.message));
    this.status = input.status;
    this.code = input.code;
    this.requestId = input.requestId;
    this.retryable = input.retryable;
    this.operationId = input.operationId;
    this.details = input.details;
    this.retryAfterMs = input.retryAfterMs;
  }
}

export class AexAuthError extends AexApiError {}
export class AexNotFoundError extends AexApiError {}
export class AexConflictError extends AexApiError {}
export class AexPreconditionError extends AexApiError {}
export class AexValidationError extends AexApiError {}
export class AexQuotaError extends AexApiError {}
export class AexStateError extends AexApiError {}
export class AexUnavailableError extends AexApiError {}
export class AexInternalError extends AexApiError {}
export class AexGoneError extends AexApiError {}

export class AexStreamProtocolError extends AexError {}

interface ErrorEnvelope {
  readonly error?: {
    readonly code?: unknown;
    readonly class?: unknown;
    readonly message?: unknown;
    readonly requestId?: unknown;
    readonly retryable?: unknown;
    readonly operationId?: unknown;
    readonly details?: unknown;
  };
}

function constructorFor(errorClass: ErrorClass): typeof AexApiError {
  switch (errorClass) {
    case "auth": return AexAuthError;
    case "not_found": return AexNotFoundError;
    case "conflict": return AexConflictError;
    case "precondition": return AexPreconditionError;
    case "validation": return AexValidationError;
    case "quota": return AexQuotaError;
    case "state": return AexStateError;
    case "unavailable": return AexUnavailableError;
    case "internal": return AexInternalError;
  }
}

export function apiErrorFromResponse(
  _routeId: RouteId,
  status: number,
  body: unknown,
  headers: Headers,
): AexApiError {
  const envelope = body as ErrorEnvelope;
  const value = envelope.error;
  const code = typeof value?.code === "string" ? value.code : "invalid_error_envelope";
  const requestId = typeof value?.requestId === "string"
    ? value.requestId
    : headers.get("x-request-id");
  if (!requestId) throw new AexConfigError("error response omitted its request id");
  const retryAfterMs = parseRetryAfter(headers);
  const input: ApiErrorInput = {
    status,
    code,
    requestId,
    retryable: value?.retryable === true,
    message: typeof value?.message === "string" ? value.message : "request failed",
    ...(typeof value?.operationId === "string" ? { operationId: value.operationId } : {}),
    ...(value?.details === undefined ? {} : { details: value.details }),
    ...(retryAfterMs === undefined ? {} : { retryAfterMs }),
  };
  if (status === 410) return new AexGoneError(input);
  const metadata = ERROR_METADATA[code];
  if (!metadata) return new AexApiError(input);
  return new (constructorFor(metadata.class))(input);
}

function parseRetryAfter(headers: Headers): number | undefined {
  const value = headers.get("retry-after");
  if (!value) return undefined;
  const seconds = Number(value);
  return Number.isFinite(seconds) && seconds >= 0 ? Math.ceil(seconds * 1000) : undefined;
}

function redact(message: string): string {
  return message
    .replace(/aex_(?:wk|at)_[A-Za-z0-9_-]+/g, "[credential]")
    .replace(/https:\/\/[^\s?#]+\?[^\s]+/g, "[url]")
    .slice(0, 2_000);
}

export function isRetryable(error: unknown): boolean {
  return error instanceof AexApiError && error.retryable;
}

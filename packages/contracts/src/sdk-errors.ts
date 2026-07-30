import { redactSecrets } from "./sdk-secrets.js";
import { isAexApiErrorCode, type AexApiErrorCode } from "./error-codes.js";

export type AexErrorCode =
  | "CREDENTIAL_INVALID"
  | "API_ERROR"
  | "NETWORK_ERROR";

export class AexError extends Error {
  readonly code: AexErrorCode;
  readonly details?: unknown;

  constructor(code: AexErrorCode, message: string, details?: unknown, options?: { readonly cause?: unknown }) {
    super(redactSecrets(message), options?.cause === undefined ? undefined : { cause: options.cause });
    this.name = this.constructor.name;
    this.code = code;
    this.details = details === undefined ? undefined : redactSecrets(details);
  }
}

export class CredentialValidationError extends AexError {
  constructor(message: string, details?: unknown) {
    super("CREDENTIAL_INVALID", message, details);
  }
}

/**
 * Thrown by SDK and CLI operations when the hosted aex API returns a non-2xx
 * response. Carries the HTTP status, the redacted parsed body, the server's
 * STABLE {@link AexApiErrorCode} (when present), and a `requestId` for support.
 * Construct via {@link import("./error-factory.js").apiErrorFromResponse} — the
 * single wire→exception mapping — which dispatches to a typed subclass
 * ({@link AexAuthError} / {@link AexIdempotencyConflictError} /
 * {@link AexNotFoundError} / {@link AexRateLimitError}).
 */
export class AexApiError extends AexError {
  readonly status: number;
  readonly body: unknown;
  /** The server's stable error code, when the body carried a known one. */
  readonly apiCode: AexApiErrorCode | undefined;
  /** Request id (body `requestId` or a response header) for support correlation. */
  readonly requestId: string | undefined;

  constructor(
    status: number,
    message: string,
    body: unknown,
    options?: {
      readonly apiCode?: AexApiErrorCode | undefined;
      readonly requestId?: string | undefined;
      readonly cause?: unknown;
    }
  ) {
    super("API_ERROR", message, body, options?.cause === undefined ? undefined : { cause: options.cause });
    this.status = status;
    this.body = redactSecrets(body);
    this.apiCode = options?.apiCode;
    this.requestId = options?.requestId;
  }
}

/** Shared construction shape for the typed {@link AexApiError} subclasses. */
export interface AexApiErrorInit {
  readonly status: number;
  readonly message: string;
  readonly body: unknown;
  readonly apiCode?: AexApiErrorCode | undefined;
  readonly requestId?: string | undefined;
  readonly cause?: unknown;
}

/** 401/403 auth failure (token invalid/revoked/expired, forbidden, insufficient scope). */
export class AexAuthError extends AexApiError {
  /** The scope the endpoint required, when the server named it (insufficient_scope). */
  readonly requiredScope: string | undefined;
  constructor(init: AexApiErrorInit & { readonly requiredScope?: string | undefined }) {
    super(init.status, init.message, init.body, {
      apiCode: init.apiCode,
      requestId: init.requestId,
      cause: init.cause
    });
    this.requiredScope = init.requiredScope;
  }
}

/** 409 — the idempotency key was reused with a different request body. */
export class AexIdempotencyConflictError extends AexApiError {
  constructor(init: AexApiErrorInit) {
    super(init.status, init.message, init.body, {
      apiCode: init.apiCode,
      requestId: init.requestId,
      cause: init.cause
    });
  }
}

/** 404 — the requested resource was not found. */
export class AexNotFoundError extends AexApiError {
  constructor(init: AexApiErrorInit) {
    super(init.status, init.message, init.body, {
      apiCode: init.apiCode,
      requestId: init.requestId,
      cause: init.cause
    });
  }
}

/** Construction shape for {@link AexRateLimitError}. */
export type AexRateLimitErrorInit = Omit<AexApiErrorInit, "message" | "body"> & {
  /** Omitted when the retry policy builds the error: a fixed, non-leaky summary is generated. */
  readonly message?: string;
  readonly body?: unknown;
  /** Suggested backoff (ms), when the server advertised one (Retry-After). */
  readonly retryAfterMs?: number | undefined;
  /** How many transport attempts were made before giving up. Defaults to `1`. */
  readonly attempts?: number;
  readonly source?: "api";
};

/**
 * 429 / 503 / 529 — the workspace hit a rate/concurrency limit, or an upstream
 * provider said "slow down"; retry after a backoff.
 *
 * ONE class for both producers: the wire→exception factory
 * (`apiErrorFromResponse`) and the shared HTTP retry policy, so `isRateLimited`
 * recognises both with no split-brain. When the retry policy raises it, `message`
 * is a fixed non-leaky summary (never an echo of the raw body, which is still
 * available redacted on `.body`) and `attempts` / `source` / `providerFault`
 * carry the retry-layer detail.
 */
export class AexRateLimitError extends AexApiError {
  /** Suggested backoff (ms), when the server advertised one (Retry-After). */
  readonly retryAfterMs: number | undefined;
  /** How many attempts were made before giving up; `1` for a single-shot rejection. */
  readonly attempts: number;
  /** Whether the throttle came from the aex API plane or the upstream provider. */
  readonly source: "api";

  constructor(init: AexRateLimitErrorInit) {
    const attempts = init.attempts ?? 1;
    const source = init.source ?? "api";
    super(
      init.status,
      init.message ?? throttleSummary(init.status, attempts, source, init.retryAfterMs),
      init.body,
      {
        apiCode: init.apiCode,
        requestId: init.requestId,
        cause: init.cause
      }
    );
    this.retryAfterMs = init.retryAfterMs;
    this.attempts = attempts;
    this.source = source;
  }
}

/** Fixed, non-leaky throttle summary — never echoes the server's error body. */
function throttleSummary(
  status: number,
  attempts: number,
  source: "api",
  retryAfterMs: number | undefined
): string {
  const who = "aex API";
  const label = status === 529 ? "overloaded" : "rate limit reached";
  const attemptsLabel = `${attempts} attempt${attempts === 1 ? "" : "s"}`;
  const wait = retryAfterMs !== undefined ? `; retry after ~${Math.ceil(retryAfterMs / 1000)}s` : "";
  return `${who} ${label} (HTTP ${status}) after ${attemptsLabel}${wait}`;
}

/** True for a 401/403 authentication/authorization failure. */
export function isAuthError(err: unknown): err is AexAuthError {
  return err instanceof AexAuthError;
}
/** True for a 403 whose cause is a missing scope (`insufficient_scope`). */
export function isInsufficientScope(err: unknown): boolean {
  return err instanceof AexAuthError && err.apiCode === "insufficient_scope";
}
/** True for a 409 idempotency-key reuse conflict. */
export function isIdempotencyConflict(err: unknown): err is AexIdempotencyConflictError {
  return err instanceof AexIdempotencyConflictError;
}
/** True for a 404 not-found error. */
export function isNotFound(err: unknown): err is AexNotFoundError {
  return err instanceof AexNotFoundError;
}
/** True for a 429 rate/concurrency-limit error. */
export function isRateLimited(err: unknown): err is AexRateLimitError {
  return err instanceof AexRateLimitError;
}
/**
 * Thrown when a BFF-bound request fails BEFORE any HTTP response exists — DNS
 * failure, connection refused, TLS error, socket reset. Wraps the raw fetch
 * rejection (whose undici form is a bare `TypeError: fetch failed` with the
 * useful code hidden on `cause.code`) into a message that names the request
 * and the transport failure, e.g.
 * `POST api.aex.dev/api/assets/presign failed: ECONNREFUSED (connect ECONNREFUSED 127.0.0.1:443)`.
 * The original rejection is preserved on `cause`.
 */
export class AexNetworkError extends AexError {
  readonly method: string;
  /** Request host — never carries credentials or the query string. */
  readonly host: string;
  readonly path: string;
  /** Transport failure code (e.g. `ECONNREFUSED`), when detectable. */
  readonly causeCode: string | undefined;
  /** Attempts made when a retry layer exhausted its budget; `1` otherwise. */
  readonly attempts: number;
  /** Total elapsed time (ms) across all attempts, when the retry layer set it. */
  readonly elapsedMs: number | undefined;

  constructor(args: {
    readonly method: string;
    readonly host: string;
    readonly path: string;
    readonly cause: unknown;
    /** Set by the retry layer when it gave up: appended to the message. */
    readonly attempts?: number;
    readonly elapsedMs?: number;
  }) {
    const causeCode = extractErrorCode(args.cause);
    super(
      "NETWORK_ERROR",
      networkErrorMessage(args, causeCode),
      { method: args.method, host: args.host, path: args.path, ...(causeCode ? { code: causeCode } : {}) },
      { cause: args.cause }
    );
    this.method = args.method;
    this.host = args.host;
    this.path = args.path;
    this.causeCode = causeCode;
    this.attempts = args.attempts ?? 1;
    this.elapsedMs = args.elapsedMs;
  }
}

function networkErrorMessage(
  args: {
    readonly method: string;
    readonly host: string;
    readonly path: string;
    readonly cause: unknown;
    readonly attempts?: number;
    readonly elapsedMs?: number;
  },
  causeCode: string | undefined
): string {
  const target = args.host ? `${args.host}${args.path}` : "request";
  const detail = shortCauseMessage(args.cause, causeCode);
  const suffix =
    args.attempts === undefined
      ? ""
      : ` after ${args.attempts} attempt${args.attempts === 1 ? "" : "s"} over ${args.elapsedMs ?? 0}ms`;
  let message = `${args.method} ${target} failed`;
  if (causeCode) message += `: ${causeCode}`;
  if (detail) message += causeCode ? ` (${detail})` : `: ${detail}`;
  return message + suffix;
}

/** The innermost useful message off the rejection, URL-redacted and bounded. */
function shortCauseMessage(cause: unknown, causeCode: string | undefined): string | undefined {
  const nested = cause instanceof Error && cause.cause instanceof Error ? cause.cause : cause;
  const message =
    nested instanceof Error ? nested.message || nested.name : typeof nested === "string" ? nested : undefined;
  if (!message || message === causeCode) return undefined;
  return message.replace(/https?:\/\/[^\s<>"'`]+/g, (raw) => redactUrl(raw)).slice(0, 200);
}

/**
 * Best-effort transport error code (`ECONNREFUSED`, `ENOTFOUND`, …): checks
 * `err.code`, then a bounded native `cause` chain (where undici and database
 * drivers hide it), then falls back to an `E…`-shaped token in the outer
 * message. Five inspected values matches the platform diagnostic boundary and
 * prevents malformed/cyclic cause graphs from becoming unbounded work.
 */
export function extractErrorCode(err: unknown): string | undefined {
  let current: unknown = err;
  for (let depth = 0; depth < 5 && current && typeof current === "object"; depth += 1) {
    const code = stringProperty(current, "code");
    if (code) return code;
    current = (current as Record<string, unknown>).cause;
  }
  const match = /\bE[A-Z0-9_]+\b/.exec(errorMessageOf(err));
  return match?.[0];
}

/**
 * Redact a URL down to protocol + host + path: credentials become
 * `[redacted]@` and any query string becomes `?[redacted]` (presigned URLs
 * carry signatures there). Tolerates unparseable input.
 */
export function redactUrl(url: string): string {
  try {
    const parsed = new URL(url);
    const auth = parsed.username || parsed.password ? "[redacted]@" : "";
    const query = parsed.search ? "?[redacted]" : "";
    return `${parsed.protocol}//${auth}${parsed.host}${parsed.pathname}${query}`;
  } catch {
    const withoutAuth = url.replace(/\/\/[^/?#\s]+@/, "//[redacted]@");
    const queryStart = withoutAuth.indexOf("?");
    return queryStart === -1 ? withoutAuth : `${withoutAuth.slice(0, queryStart)}?[redacted]`;
  }
}

function errorMessageOf(err: unknown): string {
  if (err instanceof Error) return err.message || err.name;
  if (typeof err === "string") return err;
  return String(err);
}

function stringProperty(value: unknown, key: string): string | undefined {
  if (!value || typeof value !== "object") return undefined;
  const prop = (value as Record<string, unknown>)[key];
  return typeof prop === "string" && prop.length > 0 ? prop : undefined;
}

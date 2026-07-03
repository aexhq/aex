import { redactSecrets } from "./sdk-secrets.js";

export type AexErrorCode =
  | "RUN_CONFIG_INVALID"
  | "CREDENTIAL_INVALID"
  | "PROVIDER_ERROR"
  | "RUN_STATE_ERROR"
  | "CLEANUP_ERROR"
  | "RUNTIME_UNSUPPORTED"
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

export class RunConfigValidationError extends AexError {
  constructor(message: string, details?: unknown) {
    super("RUN_CONFIG_INVALID", message, details);
  }
}

export class CredentialValidationError extends AexError {
  constructor(message: string, details?: unknown) {
    super("CREDENTIAL_INVALID", message, details);
  }
}

export class ProviderError extends AexError {
  readonly status: number | undefined;

  constructor(message: string, options: { status?: number; details?: unknown } = {}) {
    super("PROVIDER_ERROR", message, options.details);
    this.status = options.status;
  }
}

export class RunStateError extends AexError {
  constructor(message: string, details?: unknown) {
    super("RUN_STATE_ERROR", message, details);
  }
}

export class CleanupError extends AexError {
  constructor(message: string, details?: unknown) {
    super("CLEANUP_ERROR", message, details);
  }
}

/**
 * Thrown by SDK and CLI operations when the dashboard BFF returns a non-2xx
 * response. Carries the HTTP status and parsed body for the caller to inspect.
 */
export class AexApiError extends AexError {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, message: string, body: unknown) {
    super("API_ERROR", message, body);
    this.status = status;
    this.body = redactSecrets(body);
  }
}

/**
 * Thrown when a BFF-bound request fails BEFORE any HTTP response exists — DNS
 * failure, connection refused, TLS error, socket reset. Wraps the raw fetch
 * rejection (whose undici form is a bare `TypeError: fetch failed` with the
 * useful code hidden on `cause.code`) into a message that names the request
 * and the transport failure, e.g.
 * `POST api.aex.dev/assets/presign failed: ECONNREFUSED (connect ECONNREFUSED 127.0.0.1:443)`.
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
 * `err.code`, then `err.cause.code` (where undici hides it), then falls back
 * to an `E…`-shaped token in the message.
 */
export function extractErrorCode(err: unknown): string | undefined {
  const code = stringProperty(err, "code");
  if (code) return code;
  const cause = objectProperty(err, "cause");
  const causeCode = stringProperty(cause, "code");
  if (causeCode) return causeCode;
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

function objectProperty(value: unknown, key: string): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object") return undefined;
  const prop = (value as Record<string, unknown>)[key];
  return prop && typeof prop === "object" ? (prop as Record<string, unknown>) : undefined;
}

function stringProperty(value: unknown, key: string): string | undefined {
  if (!value || typeof value !== "object") return undefined;
  const prop = (value as Record<string, unknown>)[key];
  return typeof prop === "string" && prop.length > 0 ? prop : undefined;
}

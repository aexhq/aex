import { AexError, AexNetworkError, extractErrorCode, redactUrl } from "./sdk-errors.js";
import { apiErrorFromResponse } from "./error-factory.js";
import { AEX_DEFAULT_BASE_URL } from "./stable.js";
import { reportWireResponse } from "./wire-observer.js";

export type FetchLike = (input: string | URL | Request, init?: RequestInit) => Promise<Response>;

/**
 * Sink for local debug traces. Receives one preformatted line per HTTP
 * round-trip (method, path, status, elapsed). NEVER carries the auth
 * header, request/response bodies, or query string — purely a local
 * diagnostic; nothing is uploaded. The SDK wires this to `console.error`
 * when `debug` is set; the CLI wires it to stderr under `--debug`.
 */
export type DebugSink = (line: string) => void;

export interface HttpClientOptions {
  /**
   * API plane root. Optional — defaults to `AEX_DEFAULT_BASE_URL`
   * (`https://api.aex.dev`). Self-hosted deployments override with their
   * own URL; no env var consults this value.
   */
  readonly baseUrl?: string;
  readonly apiKey: string;
  readonly fetch?: FetchLike;
  /** When set, every request emits a redacted one-line trace here. */
  readonly debug?: DebugSink;
  /**
   * Retry transient transport failures for idempotent requests.
   * Disabled by default; host CLIs enable this so a single dropped API
   * connection does not fail read-only commands or billable writes carrying
   * an `Idempotency-Key`.
   */
  readonly retryTransientGets?: boolean | TransientGetRetryOptions;
}

export interface TransientGetRetryOptions {
  readonly maxAttempts?: number;
  readonly baseDelayMs?: number;
  readonly sleep?: (ms: number) => Promise<void>;
}

/**
 * Thin transport used by every BFF-bound operation. The SDK class and
 * the CLI subcommands BOTH build an `HttpClient` and pass it to the
 * operations module — so they cannot drift in how they auth, encode
 * query parameters, or decode error responses.
 */
export class HttpClient {
  readonly #baseUrl: URL;
  readonly #apiKey: string;
  readonly #fetch: FetchLike;
  readonly #debug: DebugSink | undefined;
  readonly #retryTransientGets: ResolvedTransientGetRetry | null;

  constructor(options: HttpClientOptions) {
    if (!options.apiKey) {
      throw new Error("HttpClient: apiKey is required");
    }
    const raw = options.baseUrl ?? AEX_DEFAULT_BASE_URL;
    const normalized = raw.endsWith("/") ? raw : `${raw}/`;
    try {
      this.#baseUrl = new URL(normalized);
    } catch (err) {
      throw new Error(
        `HttpClient: invalid aex baseUrl ${JSON.stringify(redactUrl(raw))} — ` +
          `expected an absolute URL like "${AEX_DEFAULT_BASE_URL}"`,
        { cause: err }
      );
    }
    this.#apiKey = options.apiKey;
    this.#fetch = options.fetch ?? fetch;
    this.#debug = options.debug;
    this.#retryTransientGets = resolveTransientGetRetry(options.retryTransientGets);
  }

  /** Emit a redacted round-trip trace (no auth header, body, or query). */
  #trace(method: string | undefined, url: URL, status: number, startedMs: number): void {
    this.#debug?.(`[aex] ${(method ?? "GET").toUpperCase()} ${url.pathname} -> ${status} ${Date.now() - startedMs}ms`);
  }

  async request<T>(
    path: string,
    init: RequestInit = {},
    query: Record<string, string> = {}
  ): Promise<T> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    for (const [key, value] of Object.entries(query)) {
      url.searchParams.set(key, value);
    }
    const headers: Record<string, string> = {
      accept: "application/json",
      authorization: `Bearer ${this.#apiKey}`,
      ...normalizeHeaders(init.headers)
    };
    if (init.body !== undefined && init.body !== null && !headers["content-type"]) {
      // Default to JSON only for string-shaped bodies. FormData / Blob /
      // ArrayBuffer / streams set their own content-type (and FormData
      // specifically needs fetch to compute the multipart boundary), so
      // we leave content-type untouched for non-string bodies.
      if (typeof init.body === "string") {
        headers["content-type"] = "application/json";
      }
    }
    const method = methodOf(init.method);
    const retry = retryForRequest(method, headers, this.#retryTransientGets);
    const requestStartedMs = Date.now();
    for (let attempt = 1; attempt <= retry.maxAttempts; attempt += 1) {
      const startedMs = Date.now();
      try {
        const response = await this.#fetch(url, { ...init, headers });
        this.#trace(method, url, response.status, startedMs);
        const body = await readJson(response);
        if (!response.ok) {
          const errorBody = withResponseRequestId(body, response.headers);
          throw apiErrorFromResponse({
            status: response.status,
            body: errorBody,
            message: extractErrorMessage(errorBody)
          });
        }
        // C4: the harness validates real server bytes against the response
        // schemas. Every JSON response the SDK, the CLI and the user-test suites
        // receive passes through this one line, which is why the gate attaches
        // here instead of at each of ~120 call sites.
        reportWireResponse(() => ({
          method,
          path: url.pathname,
          status: response.status,
          body
        }));
        return body as T;
      } catch (err) {
        if (shouldRetryTransientRead(err, retry, attempt)) {
          await sleepBeforeRetry(this.#debug, method, url, err, retry, attempt);
          continue;
        }
        throw toNetworkError(method, url, err, retry.maxAttempts > 1 ? attempt : undefined, Date.now() - requestStartedMs);
      }
    }
    throw new Error("HttpClient.request retry loop exhausted");
  }

  async download(
    path: string,
    init: RequestInit = {},
    query: Record<string, string> = {}
  ): Promise<{ readonly response: Response }> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    for (const [key, value] of Object.entries(query)) {
      url.searchParams.set(key, value);
    }
    const headers: Record<string, string> = {
      authorization: `Bearer ${this.#apiKey}`,
      ...normalizeHeaders(init.headers)
    };
    const method = methodOf(init.method);
    const retry = retryForRequest(method, headers, this.#retryTransientGets);
    const requestStartedMs = Date.now();
    for (let attempt = 1; attempt <= retry.maxAttempts; attempt += 1) {
      const startedMs = Date.now();
      try {
        const response = await this.#fetch(url, { ...init, headers });
        this.#trace(method, url, response.status, startedMs);
        if (!response.ok) {
          const body = await readJson(response);
          const errorBody = withResponseRequestId(body, response.headers);
          throw apiErrorFromResponse({
            status: response.status,
            body: errorBody,
            message: extractErrorMessage(errorBody)
          });
        }
        return { response };
      } catch (err) {
        if (shouldRetryTransientRead(err, retry, attempt)) {
          await sleepBeforeRetry(this.#debug, method, url, err, retry, attempt);
          continue;
        }
        throw toNetworkError(method, url, err, retry.maxAttempts > 1 ? attempt : undefined, Date.now() - requestStartedMs);
      }
    }
    throw new Error("HttpClient.download retry loop exhausted");
  }
}

interface ResolvedTransientGetRetry {
  readonly maxAttempts: number;
  readonly baseDelayMs: number;
  readonly sleep: (ms: number) => Promise<void>;
}

const DEFAULT_TRANSIENT_GET_RETRY: ResolvedTransientGetRetry = {
  maxAttempts: 3,
  baseDelayMs: 250,
  sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms))
};

const TRANSIENT_READ_CODES = new Set([
  "ConnectionRefused",
  "FailedToOpenSocket",
  "ECONNRESET",
  "ECONNREFUSED",
  "ETIMEDOUT",
  "EAI_AGAIN",
  "UND_ERR_CONNECT_TIMEOUT",
  "UND_ERR_HEADERS_TIMEOUT",
  "UND_ERR_SOCKET",
  "UND_ERR_BODY_TIMEOUT"
]);

function resolveTransientGetRetry(
  retry: HttpClientOptions["retryTransientGets"]
): ResolvedTransientGetRetry | null {
  if (!retry) return null;
  if (retry === true) return DEFAULT_TRANSIENT_GET_RETRY;
  return {
    maxAttempts: Math.max(1, retry.maxAttempts ?? DEFAULT_TRANSIENT_GET_RETRY.maxAttempts),
    baseDelayMs: Math.max(0, retry.baseDelayMs ?? DEFAULT_TRANSIENT_GET_RETRY.baseDelayMs),
    sleep: retry.sleep ?? DEFAULT_TRANSIENT_GET_RETRY.sleep
  };
}

function methodOf(method: string | undefined): string {
  return (method ?? "GET").toUpperCase();
}

function retryForRequest(
  method: string,
  headers: Readonly<Record<string, string>>,
  retry: ResolvedTransientGetRetry | null
): ResolvedTransientGetRetry {
  if (!retry || (!isMethodIdempotent(method) && !hasIdempotencyKey(headers))) {
    return { ...DEFAULT_TRANSIENT_GET_RETRY, maxAttempts: 1 };
  }
  return retry;
}

function isMethodIdempotent(method: string): boolean {
  return method === "GET" || method === "HEAD";
}

function hasIdempotencyKey(headers: Readonly<Record<string, string>>): boolean {
  for (const [name, value] of Object.entries(headers)) {
    if (name.toLowerCase() === "idempotency-key" && value.trim().length > 0) {
      return true;
    }
  }
  return false;
}

function shouldRetryTransientRead(err: unknown, retry: ResolvedTransientGetRetry, attempt: number): boolean {
  return attempt < retry.maxAttempts && transientReadErrorCode(err) !== undefined;
}

async function sleepBeforeRetry(
  debug: DebugSink | undefined,
  method: string,
  url: URL,
  err: unknown,
  retry: ResolvedTransientGetRetry,
  attempt: number
): Promise<void> {
  const delayMs = retry.baseDelayMs * attempt;
  debug?.(
    `[aex] ${method} ${url.pathname} transient ${transientReadErrorCode(err) ?? "network"} ` +
      `attempt ${attempt}/${retry.maxAttempts}; retrying in ${delayMs}ms`
  );
  await retry.sleep(delayMs);
}

function transientReadErrorCode(err: unknown): string | undefined {
  if (err instanceof AexError) return undefined;
  if ((err as { readonly name?: unknown } | null | undefined)?.name === "AbortError") return undefined;
  const code = extractErrorCode(err);
  if (code && TRANSIENT_READ_CODES.has(code)) return code;
  const text = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
  if (/FailedToOpenSocket|fetch failed|socket hang up|other side closed|terminated|network.*reset|unable to connect/i.test(text)) return "fetch";
  return undefined;
}

/**
 * Wrap a fetch rejection into an {@link AexNetworkError} carrying the
 * request's method + redacted host/path. Caller-initiated aborts and
 * already-structured aex errors (e.g. a retry layer's AexRateLimitError or
 * AexNetworkError) pass through untouched.
 */
function toNetworkError(
  method: string | undefined,
  url: URL,
  err: unknown,
  attempts?: number,
  elapsedMs?: number
): unknown {
  if (err instanceof AexError) return err;
  // `DOMException` is not an `Error` subclass in every runtime, so match aborts by name.
  if ((err as { readonly name?: unknown } | null | undefined)?.name === "AbortError") return err;
  return new AexNetworkError({
    method: (method ?? "GET").toUpperCase(),
    host: url.host,
    path: url.pathname,
    cause: err,
    ...(attempts !== undefined ? { attempts } : {}),
    ...(elapsedMs !== undefined ? { elapsedMs } : {})
  });
}

function normalizeHeaders(headers: HeadersInit | undefined): Record<string, string> {
  if (!headers) return {};
  if (headers instanceof Headers) return Object.fromEntries(headers.entries());
  if (Array.isArray(headers)) return Object.fromEntries(headers);
  return headers;
}

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) return {};
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return { raw: text };
  }
}

function withResponseRequestId(body: unknown, headers: Headers): unknown {
  if (!body || typeof body !== "object" || Array.isArray(body)) return body;
  const record = body as Record<string, unknown>;
  if (typeof record.requestId === "string" && record.requestId.trim()) return body;
  const requestId = responseRequestId(headers);
  return requestId ? { ...record, requestId } : body;
}

function responseRequestId(headers: Headers): string | undefined {
  for (const name of ["x-request-id", "request-id"]) {
    const value = headers.get(name)?.trim();
    if (value) return value;
  }
  return undefined;
}

function extractErrorMessage(body: unknown): string {
  if (body && typeof body === "object") {
    const obj = body as { readonly error?: unknown; readonly message?: unknown };
    if (typeof obj.error === "string") {
      // A 409 `session_busy` body carries the session's CURRENT status.
      // Surface it: a send to a deleted (or cancelling/suspending) session
      // otherwise reads as merely "busy", which is misleading for a session
      // that will never accept a turn again.
      const status = (body as { readonly status?: unknown }).status;
      if (obj.error === "session_busy" && typeof status === "string") {
        return `session_busy (session status: ${status})`;
      }
      // Most aex API rejections are `{error: <code>, message: <human detail>}`.
      // Keep the stable code first, but don't drop the server's actionable
      // detail (e.g. asset_snapshot_source_missing's "upload and finalize it
      // before referencing it").
      if (typeof obj.message === "string" && obj.message.length > 0 && obj.message !== obj.error) {
        return `${obj.error}: ${obj.message}`;
      }
      return obj.error;
    }
    if (obj.error && typeof obj.error === "object" && "message" in obj.error) {
      const message = (obj.error as { readonly message?: unknown }).message;
      if (typeof message === "string") return message;
    }
    // aex API error envelope: `{ ok:false, code, message }`. Surface
    // the server's message so structured rejections (e.g. runtime support)
    // aren't flattened to the generic fallback below.
    if (typeof obj.message === "string") return obj.message;
  }
  return "aex API request failed";
}

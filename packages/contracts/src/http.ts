import { AexError, AexNetworkError, AexRateLimitError, extractErrorCode, redactUrl } from "./sdk-errors.js";
import { apiErrorFromResponse } from "./error-factory.js";
import { AEX_DEFAULT_BASE_URL } from "./stable.js";
import {
  abortableSleep,
  computeRetryDelayMs,
  isRateLimitHttpStatus,
  isRetryableHttpStatus,
  tryParseRetryAfterMs
} from "./retry-core.js";
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

// ---------------------------------------------------------------------------
// The ONE retry policy
//
// Every aex client — the SDK, the CLI, and the direct-to-storage asset
// uploader — retries through the values and the loop below. There is no second
// policy: status-awareness, `Retry-After`, full-jitter exponential backoff, and
// the wall-clock budget live here once, and the retryable-status predicate is
// sourced from the adjacent `retry-core.ts` rather than re-derived.
// ---------------------------------------------------------------------------

/**
 * Tunables for the shared retry policy. All fields are optional; omit the whole
 * object to accept {@link HTTP_RETRY_POLICY}, or pass `false` where a client
 * accepts it to turn retrying off entirely.
 */
export interface HttpRetryOptions {
  /**
   * Maximum attempts INCLUDING the first try. Default `4` (one try + three
   * retries). `1` performs a single attempt with no retries (but still maps a
   * final rate-limit status to {@link AexRateLimitError}).
   */
  readonly maxAttempts?: number;
  /**
   * Base delay (ms) for the exponential backoff — the nominal wait before the
   * first retry, doubling each subsequent retry. Default `500`.
   */
  readonly initialDelayMs?: number;
  /** Upper bound (ms) on any single backoff wait. Default `20_000`. */
  readonly maxDelayMs?: number;
  /**
   * Overall wall-clock budget (ms) across all attempts. Once the next backoff
   * would push past this, the loop stops and surfaces the last error. Default
   * `120_000`.
   */
  readonly maxElapsedMs?: number;
}

/** A fully resolved {@link HttpRetryOptions} — every field decided. */
export interface HttpRetryPolicy {
  readonly maxAttempts: number;
  readonly initialDelayMs: number;
  readonly maxDelayMs: number;
  readonly maxElapsedMs: number;
}

/**
 * The single default policy. Identity matters: every client that accepts the
 * defaults resolves to THIS object, which is what the SDK/CLI fitness test
 * asserts.
 */
export const HTTP_RETRY_POLICY: HttpRetryPolicy = Object.freeze({
  maxAttempts: 4,
  initialDelayMs: 500,
  maxDelayMs: 20_000,
  maxElapsedMs: 120_000
});

/** Resolve caller options over {@link HTTP_RETRY_POLICY}, clamping to sane bounds. */
export function resolveHttpRetryPolicy(options: HttpRetryOptions | undefined): HttpRetryPolicy {
  // Identity-preserving: an omitted policy, or the shared default handed back in,
  // resolves to the SAME object — so a host that accepts the defaults is
  // observably on the one policy, not on a private copy of its numbers.
  if (options === undefined || options === HTTP_RETRY_POLICY) return HTTP_RETRY_POLICY;
  const maxAttempts = Math.max(1, Math.floor(options.maxAttempts ?? HTTP_RETRY_POLICY.maxAttempts));
  const initialDelayMs = Math.max(0, options.initialDelayMs ?? HTTP_RETRY_POLICY.initialDelayMs);
  const maxDelayMs = Math.max(initialDelayMs, options.maxDelayMs ?? HTTP_RETRY_POLICY.maxDelayMs);
  const maxElapsedMs = Math.max(0, options.maxElapsedMs ?? HTTP_RETRY_POLICY.maxElapsedMs);
  return { maxAttempts, initialDelayMs, maxDelayMs, maxElapsedMs };
}

/** Hooks the retry policy needs, injectable so tests run without real timers. */
export interface HttpRetryDeps {
  readonly sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
  readonly random?: () => number;
  readonly now?: () => number;
  /** Optional redacted trace of each backoff decision (the CLI's `--debug`). */
  readonly debug?: DebugSink;
}

/** {@link HttpRetryDeps} with every hook decided. */
export interface ResolvedHttpRetryDeps {
  readonly sleep: (ms: number, signal?: AbortSignal) => Promise<void>;
  readonly random: () => number;
  readonly now: () => number;
}

export function resolveHttpRetryDeps(deps: HttpRetryDeps | undefined): ResolvedHttpRetryDeps {
  return {
    sleep: deps?.sleep ?? abortableSleep,
    random: deps?.random ?? Math.random,
    now: deps?.now ?? Date.now
  };
}

/**
 * The ONE scheduling decision: how long to wait before retrying the 1-based
 * `attempt` that just failed, or `undefined` when the attempt count or the
 * wall-clock budget is spent. Full-jitter exponential backoff with the server's
 * `Retry-After` as a floor, both from `retry-core.ts`.
 *
 * Every retry loop in the repo (API transport, direct upload PUT, multipart
 * part PUT) calls this instead of re-deriving the arithmetic.
 */
export function nextHttpRetryDelayMs(args: {
  readonly policy: HttpRetryPolicy;
  readonly attempt: number;
  readonly startedAtMs: number;
  readonly deps: ResolvedHttpRetryDeps;
  readonly retryAfterMs?: number | undefined;
}): number | undefined {
  if (args.attempt >= args.policy.maxAttempts) return undefined;
  const delayMs = computeRetryDelayMs(args.policy, args.attempt, args.deps.random, args.retryAfterMs);
  if (args.deps.now() - args.startedAtMs + delayMs > args.policy.maxElapsedMs) return undefined;
  return delayMs;
}

const SAFE_READ_METHODS: ReadonlySet<string> = new Set(["GET", "HEAD", "OPTIONS"]);

/**
 * Safe reads are retry-eligible directly; any mutation is eligible only when it
 * carries a stable `Idempotency-Key`, so a replayed write cannot double-bill.
 */
export function isHttpRetryEligible(input: Parameters<FetchLike>[0], init: Parameters<FetchLike>[1]): boolean {
  const request = typeof Request !== "undefined" && input instanceof Request ? input : undefined;
  const method = (init?.method ?? request?.method ?? "GET").toUpperCase();
  if (SAFE_READ_METHODS.has(method)) return true;
  const idempotencyKey = new Headers(init?.headers ?? request?.headers).get("idempotency-key");
  return typeof idempotencyKey === "string" && idempotencyKey.trim().length > 0;
}

/**
 * Wrap a {@link FetchLike} with the shared bounded-retry loop. `retry === false`
 * disables the layer entirely (the input fetch is returned unchanged). Otherwise
 * an eligible request is retried on a network error OR a retryable status
 * (429/500/502/503/504/529 per `retry-core.ts`), and an exhausted
 * rate-limit/overloaded status surfaces {@link AexRateLimitError}.
 */
export function withHttpRetry(
  fetchImpl: FetchLike,
  retry: HttpRetryOptions | false | undefined,
  deps: HttpRetryDeps = {}
): FetchLike {
  if (retry === false) return fetchImpl;
  const policy = resolveHttpRetryPolicy(retry);
  const resolved = resolveHttpRetryDeps(deps);
  const debug = deps.debug;

  return async (input, init) => {
    if (!isHttpRetryEligible(input, init)) return fetchImpl(input, init);
    const startedAtMs = resolved.now();
    const signal = init?.signal ?? undefined;

    for (let attempt = 1; ; attempt += 1) {
      let response: Response;
      try {
        response = await fetchImpl(input, init);
      } catch (err) {
        // A caller-initiated abort is terminal, never transient.
        if (isAbortError(err)) throw err;
        const delayMs = nextHttpRetryDelayMs({ policy, attempt, startedAtMs, deps: resolved });
        if (delayMs === undefined) {
          throw networkRetryExhausted(err, input, init, attempt, resolved.now() - startedAtMs);
        }
        traceRetry(debug, input, init, `transient ${extractErrorCode(err) ?? "network"}`, attempt, policy, delayMs);
        await resolved.sleep(delayMs, signal);
        continue;
      }

      // Success or a definitive (non-retryable) response — hand straight back so
      // the transport reads/throws exactly as it does without the retry layer.
      if (!isRetryableHttpStatus(response.status)) return response;

      const retryAfterMs = tryParseRetryAfterMs(response.headers.get("retry-after"), resolved.now());
      const delayMs = nextHttpRetryDelayMs({ policy, attempt, startedAtMs, deps: resolved, retryAfterMs });
      if (delayMs !== undefined) {
        await drain(response);
        traceRetry(debug, input, init, `status ${response.status}`, attempt, policy, delayMs);
        await resolved.sleep(delayMs, signal);
        continue;
      }

      // Retries exhausted (or budget spent). A rate-limit/overloaded status
      // becomes a structured throttle error; any other transient status falls
      // through to the transport's normal AexApiError.
      if (isRateLimitHttpStatus(response.status)) {
        const body = withResponseRequestId(await readJson(response).catch(() => ({})), response.headers);
        throw new AexRateLimitError({
          status: response.status,
          attempts: attempt,
          source: "api",
          ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
          body
        });
      }
      return response;
    }
  };
}

function traceRetry(
  debug: DebugSink | undefined,
  input: Parameters<FetchLike>[0],
  init: Parameters<FetchLike>[1],
  reason: string,
  attempt: number,
  policy: HttpRetryPolicy,
  delayMs: number
): void {
  if (!debug) return;
  const method = (init?.method ?? (typeof Request !== "undefined" && input instanceof Request ? input.method : "GET")).toUpperCase();
  const path = requestUrl(input)?.pathname ?? "";
  debug(`[aex] ${method} ${path} ${reason} attempt ${attempt}/${policy.maxAttempts}; retrying in ${delayMs}ms`);
}

function isAbortError(err: unknown): boolean {
  return (err as { readonly name?: unknown } | null | undefined)?.name === "AbortError";
}

/** Discard a retryable response body so the connection can be reused. */
async function drain(response: Response): Promise<void> {
  try {
    if (response.body && typeof (response.body as ReadableStream).cancel === "function") {
      await (response.body as ReadableStream).cancel();
      return;
    }
    await response.text();
  } catch {
    // Draining is best-effort; a discarded retryable response never surfaces.
  }
}

/**
 * Wrap the last network-error rejection once retries are exhausted, so the
 * surfaced error states how many attempts were made over how many ms and
 * preserves the raw rejection on `cause`. An {@link AexNetworkError} from an
 * inner transport is annotated — rebuilt with the same request context and its
 * original cause — rather than double-wrapped.
 */
function networkRetryExhausted(
  err: unknown,
  input: Parameters<FetchLike>[0],
  init: Parameters<FetchLike>[1],
  attempts: number,
  elapsedMs: number
): AexNetworkError {
  if (err instanceof AexNetworkError) {
    return new AexNetworkError({
      method: err.method,
      host: err.host,
      path: err.path,
      cause: err.cause ?? err,
      attempts,
      elapsedMs
    });
  }
  const url = requestUrl(input);
  const method = init?.method ?? (typeof Request !== "undefined" && input instanceof Request ? input.method : "GET");
  return new AexNetworkError({
    method: method.toUpperCase(),
    host: url?.host ?? "",
    path: url?.pathname ?? "",
    cause: err,
    attempts,
    elapsedMs
  });
}

function requestUrl(input: Parameters<FetchLike>[0]): URL | undefined {
  try {
    if (input instanceof URL) return input;
    return new URL(typeof input === "string" ? input : input.url);
  } catch {
    return undefined;
  }
}

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
   * Retry policy for this transport, applied through {@link withHttpRetry}.
   * OMITTED (or `false`) means one attempt per request: every host opts in
   * explicitly — the SDK and the CLI both pass {@link HTTP_RETRY_POLICY} — so
   * there is exactly one place the numbers live.
   */
  readonly retry?: HttpRetryOptions | false;
  /** Injectable clock/RNG/sleep for the retry loop; tests only. */
  readonly retryDeps?: HttpRetryDeps;
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
  readonly #retryPolicy: HttpRetryPolicy | null;

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
    this.#debug = options.debug;
    const retry = options.retry ?? false;
    this.#retryPolicy = retry === false ? null : resolveHttpRetryPolicy(retry);
    this.#fetch = withHttpRetry(options.fetch ?? fetch, retry, {
      ...options.retryDeps,
      ...(this.#debug ? { debug: this.#debug } : {})
    });
  }

  /**
   * The retry policy this transport resolved, or `null` when retrying is off.
   * Exposed so the SDK↔CLI fitness test can assert both hosts landed on the
   * SAME policy object rather than on two sets of equal-looking numbers.
   */
  get retryPolicy(): HttpRetryPolicy | null {
    return this.#retryPolicy;
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
      //
      // It stays on the INNER single attempt, not around the retry loop: the loop
      // now lives in `withHttpRetry`, and a retried request must report each
      // response it actually received, not just the last one.
      reportWireResponse(() => ({
        method,
        path: url.pathname,
        status: response.status,
        body
      }));
      return body as T;
    } catch (err) {
      throw toNetworkError(method, url, err, Date.now() - startedMs);
    }
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
      throw toNetworkError(method, url, err, Date.now() - startedMs);
    }
  }
}

function methodOf(method: string | undefined): string {
  return (method ?? "GET").toUpperCase();
}

/**
 * Wrap a fetch rejection into an {@link AexNetworkError} carrying the
 * request's method + redacted host/path. Caller-initiated aborts and
 * already-structured aex errors (the retry policy's AexRateLimitError or its
 * attempt-annotated AexNetworkError) pass through untouched.
 */
function toNetworkError(method: string | undefined, url: URL, err: unknown, elapsedMs: number): unknown {
  if (err instanceof AexError) return err;
  // `DOMException` is not an `Error` subclass in every runtime, so match aborts by name.
  if (isAbortError(err)) return err;
  return new AexNetworkError({
    method: (method ?? "GET").toUpperCase(),
    host: url.host,
    path: url.pathname,
    cause: err,
    elapsedMs
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

/**
 * Built-in transport resilience for the aex SDK.
 *
 * Every BFF-bound request the SDK makes goes through one {@link FetchLike}. This
 * module wraps that fetch so a transient failure — an HTTP 429 (rate limited),
 * a 500/502/503/504 (server hiccup), a 529 (upstream overloaded), or a network
 * error — is retried with BOUNDED exponential backoff + full jitter, honoring
 * the server's `Retry-After` header when present. Non-retryable 4xx responses
 * (400/401/403/404/…) fail fast — retrying them only wastes the caller's time.
 *
 * Retries are SAFE to enable by default because the billable submits
 * (`createSession` / `sendSessionMessage`) carry a stable `Idempotency-Key`
 * header: re-sending the identical request de-duplicates server-side, so a retry
 * never creates a duplicate billable turn.
 *
 * When retries are exhausted on a rate-limit / overloaded status the wrapper
 * surfaces an {@link AexRateLimitError} — a structured, non-leaky throttle error
 * carrying the parsed `retryAfterMs`, the attempt count, and (when the runtime
 * supplies it) an upstream {@link ProviderFault}. All other exhausted retries
 * fall through to the transport's usual `AexApiError` / network rejection.
 */

import { AexApiError, AexNetworkError, type FetchLike } from "@aexhq/contracts";

/**
 * HTTP statuses that are transient and worth retrying. The billable submits
 * carry an idempotency key, so re-issuing them is safe. Everything not in this
 * set (400/401/403/404/409/422/…) is a definitive client error and fails fast.
 */
export const RETRYABLE_STATUS: readonly number[] = [429, 500, 502, 503, 504, 529];

/**
 * The subset of {@link RETRYABLE_STATUS} the platform / upstream provider uses to
 * say "slow down": 429 rate-limit, 503 unavailable, 529 overloaded. When retries
 * for one of these run out, the wrapper raises an {@link AexRateLimitError}.
 */
export const RATE_LIMIT_STATUS: readonly number[] = [429, 503, 529];

/**
 * Tunes the built-in retry loop. All fields are optional; omit the whole
 * `retry` option (or pass `retry: false` on the client) to accept the defaults
 * or turn the loop off entirely.
 */
export interface RetryOptions {
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

interface ResolvedRetryConfig {
  readonly maxAttempts: number;
  readonly initialDelayMs: number;
  readonly maxDelayMs: number;
  readonly maxElapsedMs: number;
}

const DEFAULT_RETRY: ResolvedRetryConfig = {
  maxAttempts: 4,
  initialDelayMs: 500,
  maxDelayMs: 20_000,
  maxElapsedMs: 120_000
};

/** Resolve caller options over the defaults, clamping to sane bounds. */
export function resolveRetryConfig(options: RetryOptions | undefined): ResolvedRetryConfig {
  const maxAttempts = Math.max(1, Math.floor(options?.maxAttempts ?? DEFAULT_RETRY.maxAttempts));
  const initialDelayMs = Math.max(0, options?.initialDelayMs ?? DEFAULT_RETRY.initialDelayMs);
  const maxDelayMs = Math.max(initialDelayMs, options?.maxDelayMs ?? DEFAULT_RETRY.maxDelayMs);
  const maxElapsedMs = Math.max(0, options?.maxElapsedMs ?? DEFAULT_RETRY.maxElapsedMs);
  return { maxAttempts, initialDelayMs, maxDelayMs, maxElapsedMs };
}

export function isRetryableStatus(status: number): boolean {
  return RETRYABLE_STATUS.includes(status);
}

export function isRateLimitStatus(status: number): boolean {
  return RATE_LIMIT_STATUS.includes(status);
}

/**
 * Parse an HTTP `Retry-After` header into milliseconds. Per RFC 7231 the value
 * is either a non-negative integer number of seconds or an HTTP-date; both are
 * handled. Returns `undefined` for a missing or unparseable value.
 */
export function parseRetryAfterMs(headerValue: string | null | undefined, now: number = Date.now()): number | undefined {
  if (headerValue === null || headerValue === undefined) return undefined;
  const trimmed = headerValue.trim();
  if (trimmed.length === 0) return undefined;
  if (/^\d+$/.test(trimmed)) {
    return Number(trimmed) * 1000;
  }
  const dateMs = Date.parse(trimmed);
  if (!Number.isNaN(dateMs)) {
    return Math.max(0, dateMs - now);
  }
  return undefined;
}

/**
 * Full-jitter exponential backoff (AWS-style): the nominal wait doubles per
 * retry up to `maxDelayMs`, and the actual wait is a uniform sample in
 * `[0, nominal]` to de-correlate concurrent clients. `attemptNumber` is the
 * 1-based number of the attempt that just failed.
 */
export function computeBackoffDelayMs(
  config: ResolvedRetryConfig,
  attemptNumber: number,
  random: () => number
): number {
  const exponent = Math.max(0, attemptNumber - 1);
  const nominal = Math.min(config.maxDelayMs, config.initialDelayMs * 2 ** exponent);
  return Math.round(random() * nominal);
}

/** Combine the server's `Retry-After` (a floor) with our jittered backoff. */
function nextDelayMs(
  config: ResolvedRetryConfig,
  attemptNumber: number,
  random: () => number,
  retryAfterMs: number | undefined
): number {
  const backoff = computeBackoffDelayMs(config, attemptNumber, random);
  return retryAfterMs === undefined ? backoff : Math.max(retryAfterMs, backoff);
}

/**
 * A structured, redaction-safe description of an UPSTREAM provider fault the aex
 * runtime surfaces on a failed turn (a rate limit, an overloaded provider, a
 * quota exhaustion, or a generic provider error). It is a sibling of the
 * API-plane throttle: the container/runtime emits this shape on the terminal
 * error and the SDK re-exposes it on {@link AexRateLimitError.providerFault} so
 * callers get one place to read "the model provider throttled us".
 */
export interface ProviderFault {
  /** Upstream provider id, e.g. `"anthropic"`, when the runtime reports one. */
  readonly provider?: string;
  /** Coarse fault class. */
  readonly kind: "rate_limit" | "overloaded" | "quota_exceeded" | "provider_error";
  /** Upstream HTTP status when the provider surfaced one (e.g. `429`, `529`). */
  readonly status?: number;
  /** Milliseconds the upstream asked the caller to wait, when it supplied one. */
  readonly retryAfterMs?: number;
  /** Short, already-redacted upstream message. */
  readonly message?: string;
}

const THROTTLE_KINDS: ReadonlySet<ProviderFault["kind"]> = new Set(["rate_limit", "overloaded", "quota_exceeded"]);

/** True when a {@link ProviderFault} represents a "back off and retry" signal. */
export function isThrottleFault(fault: ProviderFault): boolean {
  return THROTTLE_KINDS.has(fault.kind);
}

/**
 * Structured throttle error. Extends {@link AexApiError} so existing
 * `catch (err instanceof AexApiError)` sites keep working, while callers that
 * want the details narrow with {@link isRateLimited} and read `retryAfterMs`,
 * `attempts`, `source`, and `providerFault`. The `message` is a fixed,
 * non-leaky summary — it never echoes the raw error body (which is still
 * available, redacted, on `.body`).
 */
export class AexRateLimitError extends AexApiError {
  /** Milliseconds the server/provider asked us to wait, when known. */
  readonly retryAfterMs?: number;
  /** How many attempts were made before giving up. */
  readonly attempts: number;
  /** Whether the throttle came from the aex API plane or the upstream provider. */
  readonly source: "api" | "provider";
  /** The upstream provider fault, when the throttle originated there. */
  readonly providerFault?: ProviderFault;

  constructor(args: {
    readonly status: number;
    readonly attempts: number;
    readonly retryAfterMs?: number;
    readonly source?: "api" | "provider";
    readonly providerFault?: ProviderFault;
    readonly body?: unknown;
    readonly message?: string;
  }) {
    super(args.status, args.message ?? defaultThrottleMessage(args), args.body);
    this.attempts = args.attempts;
    this.source = args.source ?? "api";
    if (args.retryAfterMs !== undefined) this.retryAfterMs = args.retryAfterMs;
    if (args.providerFault !== undefined) this.providerFault = args.providerFault;
  }
}

/** Type guard for {@link AexRateLimitError}. */
export function isRateLimited(err: unknown): err is AexRateLimitError {
  return err instanceof AexRateLimitError;
}

function defaultThrottleMessage(args: {
  readonly status: number;
  readonly attempts: number;
  readonly retryAfterMs?: number;
  readonly source?: "api" | "provider";
}): string {
  const who = args.source === "provider" ? "upstream provider" : "aex API";
  const label = args.status === 529 ? "overloaded" : "rate limit reached";
  const attempts = `${args.attempts} attempt${args.attempts === 1 ? "" : "s"}`;
  const wait =
    args.retryAfterMs !== undefined
      ? `; retry after ~${Math.ceil(args.retryAfterMs / 1000)}s`
      : "";
  return `${who} ${label} (HTTP ${args.status}) after ${attempts}${wait}`;
}

/**
 * Best-effort parse of an unknown value into a {@link ProviderFault}. Tolerant
 * of two shapes so the SDK consumes the runtime fault the moment it starts
 * emitting one, without a contracts change:
 *
 *   1. The canonical `{ provider?, kind, status?, retryAfterMs?, message? }`
 *      (optionally nested under a `providerFault` key), OR
 *   2. A raw upstream error `{ type: "rate_limit_error" | "overloaded_error"
 *      | ..., message?, retry_after? | retryAfter? }` — `type` maps to `kind`
 *      and `retry_after` (seconds) maps to `retryAfterMs`.
 *
 * Returns `undefined` when the value carries no recognizable fault.
 */
export function parseProviderFault(value: unknown): ProviderFault | undefined {
  if (value === null || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  const nested = record.providerFault ?? record.provider_fault;
  if (nested !== undefined && nested !== value) {
    const fromNested = parseProviderFault(nested);
    if (fromNested) return fromNested;
  }

  const kind = coerceFaultKind(record.kind ?? record.type ?? record.code);
  if (kind === undefined) return undefined;

  const provider = typeof record.provider === "string" ? record.provider : undefined;
  const status = coerceStatus(record.status ?? record.statusCode ?? record.httpStatus);
  const retryAfterMs = coerceRetryAfterMs(record.retryAfterMs ?? record.retry_after_ms ?? record.retryAfter ?? record.retry_after);
  const message = typeof record.message === "string" ? record.message : undefined;

  return {
    kind,
    ...(provider !== undefined ? { provider } : {}),
    ...(status !== undefined ? { status } : {}),
    ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
    ...(message !== undefined ? { message } : {})
  };
}

function coerceFaultKind(raw: unknown): ProviderFault["kind"] | undefined {
  if (typeof raw !== "string") return undefined;
  const value = raw.toLowerCase();
  if (value.includes("rate_limit") || value.includes("rate limit") || value === "429") return "rate_limit";
  if (value.includes("overload") || value === "529") return "overloaded";
  if (value.includes("quota") || value.includes("insufficient")) return "quota_exceeded";
  if (value.includes("provider_error") || value.includes("provider error") || value.includes("api_error")) {
    return "provider_error";
  }
  return undefined;
}

function coerceStatus(raw: unknown): number | undefined {
  if (typeof raw === "number" && Number.isFinite(raw)) return raw;
  if (typeof raw === "string" && /^\d+$/.test(raw.trim())) return Number(raw.trim());
  return undefined;
}

/** Accept a ms number, a `<digits>` string, or seconds under a `retry_after` alias. */
function coerceRetryAfterMs(raw: unknown): number | undefined {
  if (typeof raw === "number" && Number.isFinite(raw)) {
    // Heuristic: small integers are seconds (the upstream convention), large
    // ones are already milliseconds.
    return raw > 0 && raw < 1000 ? raw * 1000 : raw;
  }
  if (typeof raw === "string" && /^\d+$/.test(raw.trim())) {
    const n = Number(raw.trim());
    return n > 0 && n < 1000 ? n * 1000 : n;
  }
  return undefined;
}

/**
 * Wrap the last network-error rejection once retries are exhausted, so the
 * surfaced error states how many attempts were made over how many ms and
 * preserves the raw rejection on `cause` (mirrors the {@link AexRateLimitError}
 * path for exhausted throttle statuses). An {@link AexNetworkError} from the
 * transport is annotated — rebuilt with the same request context and its
 * original cause — rather than double-wrapped.
 */
function networkRetryExhausted(
  err: unknown,
  input: string | URL | Request,
  init: RequestInit | undefined,
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

function requestUrl(input: string | URL | Request): URL | undefined {
  try {
    if (input instanceof URL) return input;
    return new URL(typeof input === "string" ? input : input.url);
  } catch {
    return undefined;
  }
}

/** Hooks the retry loop needs, injectable so tests run without real timers. */
export interface RetryDeps {
  readonly sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
  readonly random?: () => number;
  readonly now?: () => number;
}

const defaultSleep = (ms: number, signal?: AbortSignal): Promise<void> =>
  new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason instanceof Error ? signal.reason : new DOMException("Aborted", "AbortError"));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = (): void => {
      clearTimeout(timer);
      reject(signal?.reason instanceof Error ? signal.reason : new DOMException("Aborted", "AbortError"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });

function isAbortError(err: unknown): boolean {
  return err instanceof Error && err.name === "AbortError";
}

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

async function readBodyForError(response: Response): Promise<unknown> {
  try {
    const text = await response.text();
    if (text.length === 0) return {};
    try {
      return JSON.parse(text) as unknown;
    } catch {
      return { raw: text };
    }
  } catch {
    return {};
  }
}

/**
 * Wrap a {@link FetchLike} with the bounded-retry loop. `retry === false`
 * disables the layer entirely (the input fetch is returned unchanged). Otherwise
 * the returned fetch retries transient failures per {@link RetryOptions} and, on
 * an exhausted rate-limit/overloaded status, throws {@link AexRateLimitError}.
 */
export function withRetry(
  fetchImpl: FetchLike,
  retry: RetryOptions | false | undefined,
  deps: RetryDeps = {}
): FetchLike {
  if (retry === false) return fetchImpl;
  const config = resolveRetryConfig(retry);
  const sleep = deps.sleep ?? defaultSleep;
  const random = deps.random ?? Math.random;
  const now = deps.now ?? Date.now;

  return async (input, init) => {
    const startedAt = now();
    const signal = init?.signal ?? undefined;
    let attempt = 0;

    for (;;) {
      attempt += 1;

      let response: Response | undefined;
      try {
        response = await fetchImpl(input, init);
      } catch (err) {
        // A caller-initiated abort is terminal, never transient.
        if (isAbortError(err)) throw err;
        if (attempt >= config.maxAttempts) {
          throw networkRetryExhausted(err, input, init, attempt, now() - startedAt);
        }
        const delay = computeBackoffDelayMs(config, attempt, random);
        if (now() - startedAt + delay > config.maxElapsedMs) {
          throw networkRetryExhausted(err, input, init, attempt, now() - startedAt);
        }
        await sleep(delay, signal ?? undefined);
        continue;
      }

      // Success or a definitive (non-retryable) response — hand straight back so
      // the transport reads/throws exactly as it does without the retry layer.
      if (!isRetryableStatus(response.status)) {
        return response;
      }

      const retryAfterMs = parseRetryAfterMs(response.headers.get("retry-after"), now());
      const willRetry =
        attempt < config.maxAttempts &&
        now() - startedAt + nextDelayMs(config, attempt, random, retryAfterMs) <= config.maxElapsedMs;

      if (willRetry) {
        await drain(response);
        await sleep(nextDelayMs(config, attempt, random, retryAfterMs), signal ?? undefined);
        continue;
      }

      // Retries exhausted (or budget spent). A rate-limit/overloaded status
      // becomes a structured throttle error; any other transient status falls
      // through to the transport's normal AexApiError.
      if (isRateLimitStatus(response.status)) {
        const body = await readBodyForError(response);
        const errorBody = withResponseRequestId(body, response.headers);
        throw new AexRateLimitError({
          status: response.status,
          attempts: attempt,
          source: "api",
          ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
          body: errorBody
        });
      }
      return response;
    }
  };
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

/**
 * Public-safe retry primitives shared by public packages through the
 * `@aexhq/contracts/internal` subpath. Keep this module free of hosted
 * implementation details and do not export it from the public contracts root.
 */

export const RETRYABLE_HTTP_STATUS: readonly number[] = [429, 500, 502, 503, 504, 529];
export const RATE_LIMIT_HTTP_STATUS: readonly number[] = [429, 503, 529];

const RETRYABLE_HTTP_STATUS_SET: ReadonlySet<number> = new Set(RETRYABLE_HTTP_STATUS);
const RATE_LIMIT_HTTP_STATUS_SET: ReadonlySet<number> = new Set(RATE_LIMIT_HTTP_STATUS);

export interface RetryBackoffConfig {
  readonly initialDelayMs: number;
  readonly maxDelayMs: number;
}

export type RetryRandom = () => number;

export function isRetryableHttpStatus(status: number): boolean {
  return RETRYABLE_HTTP_STATUS_SET.has(status);
}

export function isRateLimitHttpStatus(status: number): boolean {
  return RATE_LIMIT_HTTP_STATUS_SET.has(status);
}

/**
 * Parse an HTTP `Retry-After` header into milliseconds. RFC 7231 permits either
 * a non-negative integer number of seconds or an HTTP-date.
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
 * Full-jitter exponential backoff: the nominal wait doubles per failed attempt
 * and the actual delay is sampled uniformly in `[0, nominal]`.
 */
export function computeRetryBackoffDelayMs(
  config: RetryBackoffConfig,
  attemptNumber: number,
  random: RetryRandom
): number {
  const exponent = Math.max(0, attemptNumber - 1);
  const nominal = Math.min(config.maxDelayMs, config.initialDelayMs * 2 ** exponent);
  return Math.round(random() * nominal);
}

/** Combine a server `Retry-After` floor with the jittered backoff delay. */
export function computeRetryDelayMs(
  config: RetryBackoffConfig,
  attemptNumber: number,
  random: RetryRandom,
  retryAfterMs: number | undefined
): number {
  const backoff = computeRetryBackoffDelayMs(config, attemptNumber, random);
  return retryAfterMs === undefined ? backoff : Math.max(retryAfterMs, backoff);
}

export function abortableSleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortReason(signal));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = (): void => {
      clearTimeout(timer);
      reject(signal ? abortReason(signal) : makeAbortError());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

function abortReason(signal: AbortSignal): unknown {
  return signal.reason instanceof Error ? signal.reason : makeAbortError();
}

function makeAbortError(): unknown {
  if (typeof DOMException !== "undefined") return new DOMException("Aborted", "AbortError");
  const err = new Error("Aborted");
  err.name = "AbortError";
  return err;
}

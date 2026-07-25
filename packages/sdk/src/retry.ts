/**
 * Built-in transport resilience for the aex SDK.
 *
 * There is ONE retry policy in this repo and it lives in `@aexhq/contracts`
 * (`HTTP_RETRY_POLICY` / `withHttpRetry` in `http.ts`), shared verbatim by the
 * SDK, the `aex` CLI, and the direct-to-storage asset uploader. This module is
 * the SDK's naming of that policy plus the provider-fault decoding that has no
 * home in the transport.
 *
 * The policy: an eligible request that fails with an HTTP 429 (rate limited), a
 * 500/502/503/504 (server hiccup), a 529 (upstream overloaded), or a network
 * error is retried with BOUNDED exponential backoff + full jitter, honoring the
 * server's `Retry-After` header when present, inside a wall-clock budget.
 * Non-retryable 4xx responses (400/401/403/404/…) fail fast.
 *
 * Safe reads (GET/HEAD/OPTIONS) are eligible directly. Any mutation is eligible
 * only when it carries a stable `Idempotency-Key`; all other mutations get
 * exactly one transport attempt.
 *
 * When retries are exhausted on a rate-limit / overloaded status the wrapper
 * surfaces an {@link AexRateLimitError} — a structured, non-leaky throttle error
 * carrying the parsed `retryAfterMs`, the attempt count, and (when the runtime
 * supplies it) an upstream {@link ProviderFault}. All other exhausted retries
 * fall through to the transport's usual `AexApiError` / network rejection.
 */

import {
  isRateLimited,
  parseProviderFault as parseCanonicalProviderFault,
  resolveHttpRetryPolicy,
  withHttpRetry,
  type KnownProviderFaultKind,
  type ProviderFault
} from "@aexhq/contracts";

// The rate-limit guard AND the rate-limit error are SINGLE-SOURCED in
// `@aexhq/contracts`, so the wire→exception factory (`apiErrorFromResponse`) and
// the retry policy raise the SAME class recognised by the SAME `isRateLimited` —
// no split-brain, no subclass mirror.
export { isRateLimited };
export { AexRateLimitError } from "@aexhq/contracts";
export type { ProviderFault } from "@aexhq/contracts";

/**
 * `RETRYABLE_STATUS` — statuses that are transient and worth retrying for an
 * eligible request. `RATE_LIMIT_STATUS` — the subset the platform / upstream
 * provider uses to say "slow down" (429 rate-limit, 503 unavailable, 529
 * overloaded); when retries for one of these run out the wrapper raises an
 * `AexRateLimitError`. Both are `retry-core.ts`'s sets under the SDK's names.
 */
export {
  RATE_LIMIT_HTTP_STATUS as RATE_LIMIT_STATUS,
  RETRYABLE_HTTP_STATUS as RETRYABLE_STATUS
} from "@aexhq/contracts/internal";

/**
 * Tunes the built-in retry loop. All fields are optional; omit the whole
 * `retry` option (or pass `retry: false` on the client) to accept the defaults
 * or turn the loop off entirely. The SDK's name for the repo-wide
 * `HttpRetryOptions`.
 */
export type { HttpRetryOptions as RetryOptions } from "@aexhq/contracts";

/** Resolve caller options over the one shared policy, clamping to sane bounds. */
export const resolveRetryConfig = resolveHttpRetryPolicy;

/**
 * Wrap a `FetchLike` with the one shared bounded-retry loop. `retry === false`
 * disables the layer entirely.
 */
export const withRetry = withHttpRetry;

export type { HttpRetryDeps as RetryDeps } from "@aexhq/contracts";

// The retryable/rate-limit status predicates, the RFC 7231 `Retry-After` parse
// (integer seconds OR HTTP-date), and the AWS-style full-jitter exponential
// backoff are all `retry-core.ts`'s. Re-exported under the SDK's names — never
// re-implemented, which is how the CLI and the SDK drifted in the first place.
export {
  computeRetryBackoffDelayMs as computeBackoffDelayMs,
  isRateLimitHttpStatus as isRateLimitStatus,
  isRetryableHttpStatus as isRetryableStatus,
  tryParseRetryAfterMs as parseRetryAfterMs
} from "@aexhq/contracts/internal";

const THROTTLE_KINDS: ReadonlySet<string> = new Set([
  "rate_limit",
  "overloaded",
  "quota_exceeded",
  "unavailable"
]);

/** True when a {@link ProviderFault} represents a "back off and retry" signal. */
export function isThrottleFault(fault: ProviderFault): boolean {
  return THROTTLE_KINDS.has(fault.kind);
}

/**
 * Best-effort compatibility parse of an unknown provider error. Canonical
 * Session and RUN_ERROR fields are validated by `@aexhq/contracts`; this helper
 * remains tolerant only for callers decoding documented historical raw-provider
 * shapes:
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

  try {
    return parseCanonicalProviderFault(record);
  } catch {
    // Continue into the deliberately narrow historical raw-provider decoder.
  }

  const kind = legacyFaultKind(record.kind ?? record.type ?? record.code);
  if (kind === undefined) return undefined;

  const provider = typeof record.provider === "string" ? record.provider : undefined;
  const status = coerceStatus(record.status ?? record.statusCode ?? record.httpStatus);
  const retryAfterMs =
    record.retryAfterMs !== undefined
      ? coerceRetryDelay(record.retryAfterMs, 1)
      : record.retry_after_ms !== undefined
        ? coerceRetryDelay(record.retry_after_ms, 1)
        : record.retryAfter !== undefined
          ? coerceRetryDelay(record.retryAfter, 1000)
          : coerceRetryDelay(record.retry_after, 1000);
  const message = typeof record.message === "string" ? record.message : undefined;

  return {
    kind,
    ...(provider !== undefined ? { provider } : {}),
    ...(status !== undefined ? { status } : {}),
    ...(retryAfterMs !== undefined ? { retryAfterMs } : {}),
    ...(message !== undefined ? { message } : {})
  };
}

const LEGACY_FAULT_KINDS: Readonly<Record<string, KnownProviderFaultKind>> = {
  rate_limit: "rate_limit",
  rate_limit_error: "rate_limit",
  overloaded: "overloaded",
  overloaded_error: "overloaded",
  quota_exceeded: "quota_exceeded",
  quota_exceeded_error: "quota_exceeded",
  insufficient_quota: "quota_exceeded",
  unavailable: "unavailable",
  api_error: "unavailable",
  timeout_error: "unavailable",
  provider_error: "provider_error",
  authentication_error: "provider_error",
  invalid_request_error: "provider_error",
  "429": "rate_limit",
  "529": "overloaded"
};

function legacyFaultKind(raw: unknown): KnownProviderFaultKind | undefined {
  if (typeof raw !== "string") return undefined;
  return LEGACY_FAULT_KINDS[raw.trim().toLowerCase()];
}

function coerceStatus(raw: unknown): number | undefined {
  if (typeof raw === "number" && Number.isFinite(raw)) return raw;
  if (typeof raw === "string" && /^\d+$/.test(raw.trim())) return Number(raw.trim());
  return undefined;
}

/** Coerce a numeric delay using the unit declared by its field name. */
function coerceRetryDelay(raw: unknown, multiplier: 1 | 1000): number | undefined {
  const value =
    typeof raw === "number" && Number.isFinite(raw)
      ? raw
      : typeof raw === "string" && /^\d+$/.test(raw.trim())
        ? Number(raw.trim())
        : undefined;
  if (value === undefined || value < 0) return undefined;
  const milliseconds = value * multiplier;
  return Number.isSafeInteger(milliseconds) ? milliseconds : undefined;
}


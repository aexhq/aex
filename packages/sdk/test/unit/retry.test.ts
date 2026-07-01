import { describe, expect, it } from "vitest";
import { AexApiError, type FetchLike } from "@aexhq/contracts";
import {
  AexRateLimitError,
  computeBackoffDelayMs,
  isRateLimited,
  isRateLimitStatus,
  isRetryableStatus,
  isThrottleFault,
  parseProviderFault,
  parseRetryAfterMs,
  resolveRetryConfig,
  withRetry,
  RETRYABLE_STATUS,
  RATE_LIMIT_STATUS,
  type RetryDeps
} from "../../src/retry.js";

function json(body: unknown, status = 200, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers }
  });
}

/**
 * A fetch mock that plays a scripted list of outcomes (a Response or a thrown
 * error) in order, recording the `init` it was invoked with each time so tests
 * can assert that the SAME request (headers/body) is re-issued on retry.
 */
function scriptedFetch(
  outcomes: ReadonlyArray<Response | Error | (() => Response | Error)>
): { readonly fetch: FetchLike; readonly calls: RequestInit[] } {
  const calls: RequestInit[] = [];
  let i = 0;
  const fetch: FetchLike = async (_input, init) => {
    calls.push(init ?? {});
    const outcome = outcomes[Math.min(i, outcomes.length - 1)]!;
    i += 1;
    const resolved = typeof outcome === "function" ? outcome() : outcome;
    if (resolved instanceof Error) throw resolved;
    return resolved;
  };
  return { fetch, calls };
}

/** Deterministic deps: a virtual clock the injected sleep advances. */
function deterministicDeps(random = 0.5): {
  readonly deps: RetryDeps;
  readonly slept: number[];
  now(): number;
} {
  let clock = 0;
  const slept: number[] = [];
  return {
    deps: {
      random: () => random,
      now: () => clock,
      sleep: async (ms: number) => {
        slept.push(ms);
        clock += ms;
      }
    },
    slept,
    now: () => clock
  };
}

describe("retry: constants + classification", () => {
  it("marks the documented transient statuses retryable and nothing else", () => {
    for (const status of [429, 500, 502, 503, 504, 529]) {
      expect(isRetryableStatus(status)).toBe(true);
    }
    for (const status of [200, 201, 400, 401, 403, 404, 409, 422, 501]) {
      expect(isRetryableStatus(status)).toBe(false);
    }
    expect([...RETRYABLE_STATUS].sort((a, b) => a - b)).toEqual([429, 500, 502, 503, 504, 529]);
  });

  it("marks 429/503/529 as rate-limit statuses", () => {
    expect(RATE_LIMIT_STATUS.every(isRateLimitStatus)).toBe(true);
    expect(isRateLimitStatus(500)).toBe(false);
    expect(isRateLimitStatus(502)).toBe(false);
    expect(isRateLimitStatus(504)).toBe(false);
  });
});

describe("retry: parseRetryAfterMs", () => {
  it("parses delay-seconds into milliseconds", () => {
    expect(parseRetryAfterMs("0")).toBe(0);
    expect(parseRetryAfterMs("1")).toBe(1000);
    expect(parseRetryAfterMs("30")).toBe(30_000);
    expect(parseRetryAfterMs("  12  ")).toBe(12_000);
  });

  it("parses an HTTP-date relative to now", () => {
    const now = Date.UTC(2026, 0, 1, 0, 0, 0);
    const inFive = new Date(now + 5_000).toUTCString();
    expect(parseRetryAfterMs(inFive, now)).toBe(5_000);
    // A past date clamps to 0 rather than going negative.
    const past = new Date(now - 10_000).toUTCString();
    expect(parseRetryAfterMs(past, now)).toBe(0);
  });

  it("returns undefined for missing / empty / garbage", () => {
    expect(parseRetryAfterMs(null)).toBeUndefined();
    expect(parseRetryAfterMs(undefined)).toBeUndefined();
    expect(parseRetryAfterMs("")).toBeUndefined();
    expect(parseRetryAfterMs("   ")).toBeUndefined();
    expect(parseRetryAfterMs("soon")).toBeUndefined();
  });
});

describe("retry: computeBackoffDelayMs + resolveRetryConfig", () => {
  it("applies defaults and clamps", () => {
    expect(resolveRetryConfig(undefined)).toEqual({
      maxAttempts: 4,
      initialDelayMs: 500,
      maxDelayMs: 20_000,
      maxElapsedMs: 120_000
    });
    // maxDelayMs is floored to initialDelayMs; maxAttempts floors to >= 1.
    const cfg = resolveRetryConfig({ maxAttempts: 0.9, initialDelayMs: 1000, maxDelayMs: 100 });
    expect(cfg.maxAttempts).toBe(1);
    expect(cfg.maxDelayMs).toBe(1000);
  });

  it("doubles per attempt with full jitter and caps at maxDelayMs", () => {
    const cfg = resolveRetryConfig({ initialDelayMs: 100, maxDelayMs: 1000 });
    // random=1 -> full nominal: 100, 200, 400, 800, then capped at 1000.
    expect(computeBackoffDelayMs(cfg, 1, () => 1)).toBe(100);
    expect(computeBackoffDelayMs(cfg, 2, () => 1)).toBe(200);
    expect(computeBackoffDelayMs(cfg, 3, () => 1)).toBe(400);
    expect(computeBackoffDelayMs(cfg, 4, () => 1)).toBe(800);
    expect(computeBackoffDelayMs(cfg, 5, () => 1)).toBe(1000);
    expect(computeBackoffDelayMs(cfg, 6, () => 1)).toBe(1000);
    // Full jitter: random=0 -> 0; random=0.5 -> half nominal.
    expect(computeBackoffDelayMs(cfg, 3, () => 0)).toBe(0);
    expect(computeBackoffDelayMs(cfg, 3, () => 0.5)).toBe(200);
  });
});

describe("retry: parseProviderFault", () => {
  it("parses the canonical shape", () => {
    expect(
      parseProviderFault({ provider: "anthropic", kind: "rate_limit", status: 429, retryAfterMs: 2000, message: "slow down" })
    ).toEqual({ provider: "anthropic", kind: "rate_limit", status: 429, retryAfterMs: 2000, message: "slow down" });
  });

  it("unwraps a nested providerFault", () => {
    expect(parseProviderFault({ providerFault: { kind: "overloaded", status: 529 } })).toEqual({
      kind: "overloaded",
      status: 529
    });
  });

  it("maps a raw upstream error shape (type + retry_after seconds)", () => {
    expect(parseProviderFault({ type: "rate_limit_error", message: "rate limited", retry_after: 3 })).toEqual({
      kind: "rate_limit",
      retryAfterMs: 3000,
      message: "rate limited"
    });
    expect(parseProviderFault({ type: "overloaded_error" })).toEqual({ kind: "overloaded" });
  });

  it("returns undefined for non-faults", () => {
    expect(parseProviderFault(undefined)).toBeUndefined();
    expect(parseProviderFault(null)).toBeUndefined();
    expect(parseProviderFault("nope")).toBeUndefined();
    expect(parseProviderFault({ status: 200 })).toBeUndefined();
  });

  it("classifies throttle vs non-throttle kinds", () => {
    expect(isThrottleFault({ kind: "rate_limit" })).toBe(true);
    expect(isThrottleFault({ kind: "overloaded" })).toBe(true);
    expect(isThrottleFault({ kind: "quota_exceeded" })).toBe(true);
    expect(isThrottleFault({ kind: "provider_error" })).toBe(false);
  });
});

describe("retry: AexRateLimitError", () => {
  it("is an AexApiError with structured throttle detail and a non-leaky message", () => {
    const err = new AexRateLimitError({ status: 429, attempts: 3, retryAfterMs: 2000, source: "api" });
    expect(err).toBeInstanceOf(AexApiError);
    expect(isRateLimited(err)).toBe(true);
    expect(err.status).toBe(429);
    expect(err.attempts).toBe(3);
    expect(err.retryAfterMs).toBe(2000);
    expect(err.source).toBe("api");
    expect(err.message).toBe("aex API rate limit reached (HTTP 429) after 3 attempts; retry after ~2s");
    expect(err.name).toBe("AexRateLimitError");
  });

  it("names the overloaded case and carries a provider fault", () => {
    const err = new AexRateLimitError({
      status: 529,
      attempts: 1,
      source: "provider",
      providerFault: { provider: "anthropic", kind: "overloaded", status: 529 }
    });
    expect(err.message).toBe("upstream provider overloaded (HTTP 529) after 1 attempt");
    expect(err.providerFault?.provider).toBe("anthropic");
    expect(isRateLimited(err)).toBe(true);
  });

  it("isRateLimited rejects unrelated errors", () => {
    expect(isRateLimited(new AexApiError(429, "x", {}))).toBe(false);
    expect(isRateLimited(new Error("nope"))).toBe(false);
  });
});

describe("withRetry: transient handling", () => {
  it("retries a 429 then returns the eventual success", async () => {
    const { fetch, calls } = scriptedFetch([json({ error: "slow" }, 429), json({ ok: true }, 200)]);
    const { deps, slept } = deterministicDeps(1);
    const wrapped = withRetry(fetch, { initialDelayMs: 100 }, deps);
    const res = await wrapped("https://x/api/sessions", { method: "POST", headers: { "Idempotency-Key": "k1" } });
    expect(res.status).toBe(200);
    expect(calls).toHaveLength(2);
    expect(slept).toEqual([100]);
    // The SAME request (idempotency key) is re-issued — no duplicate billable run.
    expect((calls[0]!.headers as Record<string, string>)["Idempotency-Key"]).toBe("k1");
    expect((calls[1]!.headers as Record<string, string>)["Idempotency-Key"]).toBe("k1");
  });

  it("honors Retry-After as a floor over the jittered backoff", async () => {
    const { fetch } = scriptedFetch([json({}, 429, { "retry-after": "5" }), json({ ok: true }, 200)]);
    const { deps, slept } = deterministicDeps(1); // backoff nominal = 100
    const wrapped = withRetry(fetch, { initialDelayMs: 100 }, deps);
    await wrapped("https://x", { method: "POST" });
    // Retry-After (5s) dominates the 100ms backoff.
    expect(slept).toEqual([5000]);
  });

  it("throws a structured AexRateLimitError once 429 retries are exhausted", async () => {
    const { fetch, calls } = scriptedFetch([json({ error: "limit" }, 429, { "retry-after": "2" })]);
    const { deps } = deterministicDeps(1);
    const wrapped = withRetry(fetch, { maxAttempts: 3, initialDelayMs: 1 }, deps);
    await expect(wrapped("https://x", { method: "POST" })).rejects.toMatchObject({
      name: "AexRateLimitError",
      status: 429,
      attempts: 3,
      retryAfterMs: 2000,
      source: "api"
    });
    expect(calls).toHaveLength(3);
  });

  it("surfaces 529 overloaded as a rate-limit error after exhaustion", async () => {
    const { fetch } = scriptedFetch([json({ type: "overloaded_error" }, 529)]);
    const { deps } = deterministicDeps(1);
    const wrapped = withRetry(fetch, { maxAttempts: 2, initialDelayMs: 1 }, deps);
    const err = await wrapped("https://x", { method: "POST" }).catch((e) => e);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).status).toBe(529);
  });

  it("returns the final response for an exhausted 5xx so the transport throws its normal AexApiError", async () => {
    const { fetch, calls } = scriptedFetch([json({ error: "boom" }, 503), json({ error: "boom" }, 500)]);
    const { deps } = deterministicDeps(1);
    const wrapped = withRetry(fetch, { maxAttempts: 2, initialDelayMs: 1 }, deps);
    const res = await wrapped("https://x", { method: "GET" });
    // 503 is a rate-limit status, but here the FIRST (503) is retried and the
    // FINAL attempt is a plain 500 -> handed back, not thrown.
    expect(res.status).toBe(500);
    expect(calls).toHaveLength(2);
  });

  it("does not retry non-retryable 4xx — fail fast", async () => {
    for (const status of [400, 401, 403, 404, 409, 422]) {
      const { fetch, calls } = scriptedFetch([json({ error: "no" }, status), json({ ok: true }, 200)]);
      const { deps, slept } = deterministicDeps(1);
      const wrapped = withRetry(fetch, undefined, deps);
      const res = await wrapped("https://x", { method: "POST" });
      expect(res.status).toBe(status);
      expect(calls).toHaveLength(1);
      expect(slept).toEqual([]);
    }
  });

  it("retries a network error then succeeds, and rethrows the original when exhausted", async () => {
    const netErr = new TypeError("fetch failed");
    const okAfter = scriptedFetch([netErr, json({ ok: true }, 200)]);
    const okDeps = deterministicDeps(1);
    const ok = await withRetry(okAfter.fetch, { initialDelayMs: 1 }, okDeps.deps)("https://x", { method: "POST" });
    expect(ok.status).toBe(200);
    expect(okAfter.calls).toHaveLength(2);

    const alwaysDown = scriptedFetch([netErr]);
    const downDeps = deterministicDeps(1);
    await expect(
      withRetry(alwaysDown.fetch, { maxAttempts: 3, initialDelayMs: 1 }, downDeps.deps)("https://x", { method: "POST" })
    ).rejects.toBe(netErr);
    expect(alwaysDown.calls).toHaveLength(3);
  });

  it("never retries an AbortError", async () => {
    const abort = new DOMException("Aborted", "AbortError");
    const { fetch, calls } = scriptedFetch([abort, json({ ok: true }, 200)]);
    const { deps } = deterministicDeps(1);
    await expect(withRetry(fetch, undefined, deps)("https://x", { method: "POST" })).rejects.toBe(abort);
    expect(calls).toHaveLength(1);
  });

  it("stops retrying once the wall-clock budget is spent", async () => {
    // Always 429, backoff 1000ms each, budget 1500ms: attempt1 (sleep 1000),
    // attempt2 would need another 1000 -> 2000 > 1500, so give up at 2 attempts.
    const { fetch, calls } = scriptedFetch([json({}, 429)]);
    const { deps, slept } = deterministicDeps(1);
    const wrapped = withRetry(fetch, { maxAttempts: 10, initialDelayMs: 1000, maxDelayMs: 1000, maxElapsedMs: 1500 }, deps);
    await expect(wrapped("https://x", { method: "POST" })).rejects.toBeInstanceOf(AexRateLimitError);
    expect(slept).toEqual([1000]);
    expect(calls).toHaveLength(2);
  });

  it("retry:false is a pass-through — no wrapping, no retries", async () => {
    const { fetch, calls } = scriptedFetch([json({}, 429), json({ ok: true }, 200)]);
    const wrapped = withRetry(fetch, false);
    expect(wrapped).toBe(fetch);
    const res = await wrapped("https://x", { method: "POST" });
    expect(res.status).toBe(429);
    expect(calls).toHaveLength(1);
  });

  it("maxAttempts:1 makes a single attempt but still maps a rate-limit to AexRateLimitError", async () => {
    const { fetch, calls } = scriptedFetch([json({}, 429, { "retry-after": "7" })]);
    const { deps, slept } = deterministicDeps(1);
    const wrapped = withRetry(fetch, { maxAttempts: 1 }, deps);
    const err = await wrapped("https://x", { method: "POST" }).catch((e) => e);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).attempts).toBe(1);
    expect((err as AexRateLimitError).retryAfterMs).toBe(7000);
    expect(calls).toHaveLength(1);
    expect(slept).toEqual([]);
  });
});

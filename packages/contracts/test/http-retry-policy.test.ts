/**
 * The ONE retry policy (`http.ts`).
 *
 * Pins the B2 defect and its fix. Before this, the contracts transport matched
 * only network error CODES and a message regex — it NEVER inspected
 * `response.status` — so `aex sessions list` failed immediately on a 429 while
 * `sdk.sessions.list()` retried transparently. Now both go through
 * `withHttpRetry`, which is status-aware, honours `Retry-After`, applies
 * full-jitter exponential backoff, and stops at a wall-clock budget.
 *
 * Every delay here comes from an injected RNG + clock, so the suite is
 * deterministic and never sleeps.
 */
import { describe, expect, it } from "bun:test";
import {
  HTTP_RETRY_POLICY,
  HttpClient,
  isHttpRetryEligible,
  nextHttpRetryDelayMs,
  resolveHttpRetryDeps,
  resolveHttpRetryPolicy,
  withHttpRetry,
  type HttpRetryPolicy
} from "../src/http.js";
import { RETRYABLE_HTTP_STATUS } from "../src/retry-core.js";
import { AexApiError, AexNetworkError, AexRateLimitError, isRateLimited } from "../src/sdk-errors.js";

interface Harness {
  readonly slept: number[];
  readonly deps: { random: () => number; now: () => number; sleep: (ms: number) => Promise<void> };
}

/** Deterministic clock/RNG/sleep. `random` defaults to 1 — jitter at its ceiling. */
function harness(random = 1, startMs = 0): Harness {
  const slept: number[] = [];
  let clock = startMs;
  return {
    slept,
    deps: {
      random: () => random,
      now: () => clock,
      sleep: async (ms: number) => {
        slept.push(ms);
        clock += ms;
      }
    }
  };
}

function jsonResponse(status: number, headers: Record<string, string> = {}, body: unknown = { error: "rate_limited" }): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers }
  });
}

async function rejectionOf(promise: Promise<unknown>): Promise<unknown> {
  return promise.then(
    () => {
      throw new Error("expected the promise to reject");
    },
    (err) => err as unknown
  );
}

describe("HTTP_RETRY_POLICY: one set of numbers", () => {
  it("resolves omitted options to the SAME policy object every caller shares", () => {
    expect(resolveHttpRetryPolicy(undefined)).toBe(HTTP_RETRY_POLICY);
    expect(HTTP_RETRY_POLICY).toEqual({
      maxAttempts: 4,
      initialDelayMs: 500,
      maxDelayMs: 20_000,
      maxElapsedMs: 120_000
    });
  });

  it("clamps caller overrides instead of trusting them", () => {
    expect(resolveHttpRetryPolicy({ maxAttempts: 0.9, initialDelayMs: 1000, maxDelayMs: 100 })).toEqual({
      maxAttempts: 1,
      initialDelayMs: 1000,
      // maxDelayMs can never sit below initialDelayMs.
      maxDelayMs: 1000,
      maxElapsedMs: 120_000
    });
    expect(resolveHttpRetryPolicy({ maxAttempts: -5, initialDelayMs: -1, maxElapsedMs: -1 })).toEqual({
      maxAttempts: 1,
      initialDelayMs: 0,
      maxDelayMs: 20_000,
      maxElapsedMs: 0
    });
  });
});

describe("nextHttpRetryDelayMs: the single scheduling decision", () => {
  const policy: HttpRetryPolicy = resolveHttpRetryPolicy({
    maxAttempts: 5,
    initialDelayMs: 100,
    maxDelayMs: 1000,
    maxElapsedMs: 10_000
  });

  it("applies full-jitter exponential backoff from the injected RNG", () => {
    const ceiling = resolveHttpRetryDeps({ random: () => 1, now: () => 0 });
    const floor = resolveHttpRetryDeps({ random: () => 0, now: () => 0 });
    const half = resolveHttpRetryDeps({ random: () => 0.5, now: () => 0 });
    const base = { policy, startedAtMs: 0 };

    // Nominal doubles per failed attempt and is capped by maxDelayMs.
    expect(nextHttpRetryDelayMs({ ...base, attempt: 1, deps: ceiling })).toBe(100);
    expect(nextHttpRetryDelayMs({ ...base, attempt: 2, deps: ceiling })).toBe(200);
    expect(nextHttpRetryDelayMs({ ...base, attempt: 3, deps: ceiling })).toBe(400);
    expect(nextHttpRetryDelayMs({ ...base, attempt: 4, deps: ceiling })).toBe(800);

    // Jitter is a uniform sample in [0, nominal] — not the nominal itself.
    expect(nextHttpRetryDelayMs({ ...base, attempt: 3, deps: floor })).toBe(0);
    expect(nextHttpRetryDelayMs({ ...base, attempt: 3, deps: half })).toBe(200);
  });

  it("treats Retry-After as a floor under the jittered backoff", () => {
    const deps = resolveHttpRetryDeps({ random: () => 0, now: () => 0 });
    expect(nextHttpRetryDelayMs({ policy, attempt: 1, startedAtMs: 0, deps, retryAfterMs: 3_000 })).toBe(3_000);
    // A jittered backoff LARGER than Retry-After wins.
    const ceiling = resolveHttpRetryDeps({ random: () => 1, now: () => 0 });
    expect(nextHttpRetryDelayMs({ policy, attempt: 4, startedAtMs: 0, deps: ceiling, retryAfterMs: 10 })).toBe(800);
  });

  it("stops on the attempt ceiling and on the elapsed budget", () => {
    const deps = resolveHttpRetryDeps({ random: () => 1, now: () => 0 });
    expect(nextHttpRetryDelayMs({ policy, attempt: 5, startedAtMs: 0, deps })).toBeUndefined();
    // Elapsed budget: the next delay would push past maxElapsedMs.
    expect(
      nextHttpRetryDelayMs({ policy, attempt: 1, startedAtMs: -9_950, deps, retryAfterMs: undefined })
    ).toBeUndefined();
  });
});

describe("withHttpRetry: status awareness (the B2 defect)", () => {
  it("retries EVERY retryable status — the old predicate never read response.status", async () => {
    for (const status of RETRYABLE_HTTP_STATUS) {
      const h = harness(0);
      let calls = 0;
      const wrapped = withHttpRetry(
        async () => {
          calls += 1;
          return calls === 1 ? jsonResponse(status) : jsonResponse(200, {}, { ok: true });
        },
        { initialDelayMs: 100, maxDelayMs: 100 },
        h.deps
      );
      const response = await wrapped("https://api.example.test/api/sessions", { method: "GET" });
      expect(response.status, `status ${status}`).toBe(200);
      expect(calls, `status ${status}`).toBe(2);
    }
  });

  it("fails fast on a definitive 4xx", async () => {
    const h = harness();
    let calls = 0;
    const wrapped = withHttpRetry(
      async () => {
        calls += 1;
        return jsonResponse(404, {}, { error: "not_found" });
      },
      undefined,
      h.deps
    );
    expect((await wrapped("https://api.example.test/api/sessions/x")).status).toBe(404);
    expect(calls).toBe(1);
    expect(h.slept).toEqual([]);
  });

  it("only retries mutations that carry an Idempotency-Key", () => {
    expect(isHttpRetryEligible("https://x/y", { method: "GET" })).toBe(true);
    expect(isHttpRetryEligible("https://x/y", { method: "HEAD" })).toBe(true);
    expect(isHttpRetryEligible("https://x/y", { method: "OPTIONS" })).toBe(true);
    expect(isHttpRetryEligible("https://x/y", { method: "POST" })).toBe(false);
    expect(isHttpRetryEligible("https://x/y", { method: "POST", headers: { "Idempotency-Key": "  " } })).toBe(false);
    expect(isHttpRetryEligible("https://x/y", { method: "POST", headers: { "Idempotency-Key": "k-1" } })).toBe(true);
  });

  it("never retries a caller-initiated abort", async () => {
    const h = harness();
    const abort = new DOMException("The operation was aborted", "AbortError");
    let calls = 0;
    const wrapped = withHttpRetry(
      async () => {
        calls += 1;
        throw abort;
      },
      undefined,
      h.deps
    );
    expect(await rejectionOf(wrapped("https://api.example.test/api/whoami"))).toBe(abort);
    expect(calls).toBe(1);
  });

  it("returns the input fetch untouched when retrying is disabled", () => {
    const fetchImpl = async (): Promise<Response> => jsonResponse(200, {}, { ok: true });
    expect(withHttpRetry(fetchImpl, false)).toBe(fetchImpl);
  });
});

describe("HttpClient under the shared policy", () => {
  it("honours Retry-After on a 429 and surfaces AexRateLimitError once exhausted", async () => {
    const h = harness();
    let attempts = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "aexw_token",
      retry: HTTP_RETRY_POLICY,
      retryDeps: h.deps,
      fetch: async () => {
        attempts += 1;
        return jsonResponse(429, { "retry-after": "7" });
      }
    });

    const err = await rejectionOf(client.request("/api/sessions"));

    expect(attempts).toBe(HTTP_RETRY_POLICY.maxAttempts);
    // Retry-After (7s) dominates the 500/1000/2000ms jittered backoff.
    expect(h.slept).toEqual([7_000, 7_000, 7_000]);
    expect(err).toBeInstanceOf(AexRateLimitError);
    expect(err).toBeInstanceOf(AexApiError);
    expect(isRateLimited(err)).toBe(true);
    const throttle = err as AexRateLimitError;
    expect(throttle.status).toBe(429);
    expect(throttle.attempts).toBe(HTTP_RETRY_POLICY.maxAttempts);
    expect(throttle.retryAfterMs).toBe(7_000);
    expect(throttle.source).toBe("api");
    // Fixed, non-leaky summary — never an echo of the server body.
    expect(throttle.message).toBe("aex API rate limit reached (HTTP 429) after 4 attempts; retry after ~7s");
  });

  it("terminates retries when the elapsed budget cannot fit the next backoff", async () => {
    const h = harness();
    let attempts = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "aexw_token",
      // Budget fits ONE 1000ms wait (1000 <= 1500) but not a second (1000+1000 > 1500).
      retry: { maxAttempts: 9, initialDelayMs: 1000, maxDelayMs: 1000, maxElapsedMs: 1500 },
      retryDeps: h.deps,
      fetch: async () => {
        attempts += 1;
        return jsonResponse(503, {}, { error: "upstream_error" });
      }
    });

    const err = await rejectionOf(client.request("/api/sessions"));

    expect(attempts).toBe(2);
    expect(h.slept).toEqual([1_000]);
    // 503 is a rate-limit-family status, so exhaustion is still structured.
    expect(err).toBeInstanceOf(AexRateLimitError);
    expect((err as AexRateLimitError).attempts).toBe(2);
  });

  it("applies jitter from the injected RNG rather than a fixed linear ramp", async () => {
    const runs: Array<readonly number[]> = [];
    for (const random of [0, 0.25, 1]) {
      const h = harness(random);
      const client = new HttpClient({
        baseUrl: "https://api.example.test",
        apiKey: "aexw_token",
        retry: { maxAttempts: 4, initialDelayMs: 400, maxDelayMs: 20_000 },
        retryDeps: h.deps,
        fetch: async () => jsonResponse(500, {}, { error: "internal_error" })
      });
      await rejectionOf(client.request("/api/sessions"));
      runs.push([...h.slept]);
    }
    // random=0 → no wait; random=0.25 → a quarter of each nominal;
    // random=1 → the full nominal, doubling per attempt. The pre-fix CLI policy
    // was `baseDelayMs * attempt` with no jitter at all.
    expect(runs).toEqual([
      [0, 0, 0],
      [100, 200, 400],
      [400, 800, 1_600]
    ]);
  });

  it("annotates an exhausted network failure with the attempt count and elapsed time", async () => {
    const h = harness();
    const raw = new TypeError("fetch failed", { cause: Object.assign(new Error("boom"), { code: "ECONNRESET" }) });
    let attempts = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "aexw_token",
      retry: { maxAttempts: 3, initialDelayMs: 100, maxDelayMs: 100 },
      retryDeps: h.deps,
      fetch: async () => {
        attempts += 1;
        throw raw;
      }
    });

    const err = (await rejectionOf(client.request("/api/sessions"))) as AexNetworkError;

    expect(attempts).toBe(3);
    expect(h.slept).toEqual([100, 100]);
    expect(err).toBeInstanceOf(AexNetworkError);
    expect(err.attempts).toBe(3);
    expect(err.method).toBe("GET");
    expect(err.host).toBe("api.example.test");
    expect(err.message).toContain("after 3 attempts");
  });

  it("reports no policy at all when retrying is not requested", async () => {
    let attempts = 0;
    const client = new HttpClient({
      baseUrl: "https://api.example.test",
      apiKey: "aexw_token",
      fetch: async () => {
        attempts += 1;
        return jsonResponse(429, { "retry-after": "1" });
      }
    });
    expect(client.retryPolicy).toBeNull();
    expect(await rejectionOf(client.request("/api/sessions"))).toBeInstanceOf(AexRateLimitError);
    expect(attempts).toBe(1);
  });
});

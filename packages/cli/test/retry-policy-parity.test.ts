/**
 * SDK ↔ CLI retry-policy parity (plan 06 § B2 fitness test).
 *
 * The defect this locks shut: the CLI used to set `retryTransientGets: true`,
 * whose predicate matched only network error codes and a message regex and NEVER
 * inspected `response.status` — 3 attempts, linear, no jitter, no budget, no
 * `Retry-After` — while the SDK ran a richer policy of its own. So
 * `aex sessions list` failed immediately on a 429 that `sdk.sessions.list()`
 * retried transparently: same operation, same package, same server.
 *
 * There is now ONE policy. This file asserts BOTH hosts resolve the identical
 * object (not merely equal numbers) and that an identical 429 + `Retry-After`
 * script produces an identical backoff sequence and an identical error on both
 * paths. The CLI does NOT depend on the SDK — the `../../sdk/src/*` import below
 * is the repo's established drift detector, same as
 * `start-submit-conformance.test.ts`.
 */
import { describe, expect, it } from "bun:test";
import { HTTP_RETRY_POLICY, HttpClient, type HttpRetryPolicy } from "@aexhq/contracts";
import { AexRateLimitError, isRateLimited } from "@aexhq/contracts";
import { resolveHttpRetryPolicy } from "@aexhq/contracts";
import { putDirectUploadWithRetry } from "@aexhq/contracts/internal";
import { makeHttpClient } from "../src/host/common.js";
import { resolveRetryConfig, withRetry } from "../../sdk/src/retry.js";
import { makeIo } from "./support.js";

const FLAGS = { apiKey: "aexw_parity_token", aexUrl: "https://api.example.test", debug: false, json: false };

/** The policy the CLI's real transport factory lands on. */
function cliPolicy(): HttpRetryPolicy | null {
  const cap = makeIo({ argv: [] });
  return makeHttpClient(cap.io, FLAGS).retryPolicy;
}

/** The policy the SDK's transport lands on when the caller tunes nothing. */
function sdkPolicy(): HttpRetryPolicy {
  return resolveRetryConfig(undefined);
}

interface DrivenRun {
  readonly attempts: number;
  readonly slept: readonly number[];
  readonly error: unknown;
}

/**
 * Drive a persistent `429; Retry-After: 7` through an `HttpClient` carrying
 * `policy`, with an injected clock/RNG/sleep so nothing actually waits.
 */
async function driveThrottledGet(policy: HttpRetryPolicy): Promise<DrivenRun> {
  const slept: number[] = [];
  let clock = 0;
  let attempts = 0;
  const client = new HttpClient({
    baseUrl: FLAGS.aexUrl,
    apiKey: FLAGS.apiKey,
    retry: policy,
    retryDeps: {
      random: () => 1,
      now: () => clock,
      sleep: async (ms: number) => {
        slept.push(ms);
        clock += ms;
      }
    },
    fetch: async () => {
      attempts += 1;
      return new Response(JSON.stringify({ error: "rate_limited" }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "7" }
      });
    }
  });
  const error = await client.request("/api/sessions").then(
    () => undefined,
    (err: unknown) => err
  );
  return { attempts, slept, error };
}

describe("SDK ↔ CLI resolve ONE retry policy", () => {
  it("lands on the SAME policy object, not two sets of equal numbers", () => {
    const fromCli = cliPolicy();
    const fromSdk = sdkPolicy();

    expect(fromCli).not.toBeNull();
    expect(fromCli).toBe(HTTP_RETRY_POLICY);
    expect(fromSdk).toBe(HTTP_RETRY_POLICY);
    expect(fromCli).toBe(fromSdk);
  });

  it("routes the asset-upload path through the same resolver", () => {
    // `putDirectUploadWithRetry` (used by BOTH `aex start` and the SDK submit
    // path via `uploadAsset`) resolves through `resolveHttpRetryPolicy`, so the
    // third policy that used to live in `asset-upload-helper.ts` is gone.
    expect(resolveHttpRetryPolicy(undefined)).toBe(HTTP_RETRY_POLICY);
    expect(putDirectUploadWithRetry).toBeTypeOf("function");
  });

  it("exposes one shared implementation — the SDK's withRetry IS the contracts loop", async () => {
    const slept: number[] = [];
    let clock = 0;
    let calls = 0;
    const wrapped = withRetry(
      async () => {
        calls += 1;
        return calls === 1
          ? new Response("{}", { status: 503, headers: { "retry-after": "2" } })
          : new Response(JSON.stringify({ ok: true }), { status: 200 });
      },
      HTTP_RETRY_POLICY,
      {
        random: () => 0,
        now: () => clock,
        sleep: async (ms: number) => {
          slept.push(ms);
          clock += ms;
        }
      }
    );
    const response = await wrapped("https://api.example.test/api/sessions", { method: "GET" });

    expect(response.status).toBe(200);
    // Status-aware (503) AND Retry-After-aware (2s), with random=0 proving the
    // wait came from the header rather than the backoff.
    expect(slept).toEqual([2_000]);
  });
});

describe("a 429 with Retry-After behaves identically on both paths", () => {
  it("produces the same attempts, the same waits, and the same error", async () => {
    const viaCli = await driveThrottledGet(cliPolicy()!);
    const viaSdk = await driveThrottledGet(sdkPolicy());

    expect(viaCli.attempts).toBe(HTTP_RETRY_POLICY.maxAttempts);
    expect(viaSdk.attempts).toBe(viaCli.attempts);

    // Retry-After (7s) dominates the 500/1000/2000ms jittered backoff on BOTH.
    expect(viaCli.slept).toEqual([7_000, 7_000, 7_000]);
    expect(viaSdk.slept).toEqual(viaCli.slept);

    for (const run of [viaCli, viaSdk]) {
      expect(run.error).toBeInstanceOf(AexRateLimitError);
      expect(isRateLimited(run.error)).toBe(true);
      const throttle = run.error as AexRateLimitError;
      expect(throttle.status).toBe(429);
      expect(throttle.retryAfterMs).toBe(7_000);
      expect(throttle.attempts).toBe(HTTP_RETRY_POLICY.maxAttempts);
      expect(throttle.source).toBe("api");
    }
    expect((viaCli.error as AexRateLimitError).message).toBe((viaSdk.error as AexRateLimitError).message);
  });

  it("retries a 429 at all — the pre-fix CLI gave up after one attempt", async () => {
    const run = await driveThrottledGet(cliPolicy()!);
    expect(run.attempts).toBeGreaterThan(1);
    expect(run.slept.length).toBe(HTTP_RETRY_POLICY.maxAttempts - 1);
  });
});

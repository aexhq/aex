/**
 * OFFLINE edge-case coverage of the SDK's transport error/retry surface through a
 * clean installed package, using an INJECTED fetch (no network, no secrets). This
 * complements `sdk-retry.test.ts` (which proves the happy retry path) by probing
 * the ADVERSARIAL corners a real customer hits under throttling / server faults:
 *
 *   - `Retry-After` is honored as a delay FLOOR, and its ms surfaces on the error;
 *   - 503 / 529 exhaustion → structured `AexRateLimitError` (like 429), while
 *     500 / 502 / 504 exhaustion → a PLAIN `AexApiError` (NOT rate-limited);
 *   - a malformed / oversized error body never crashes the SDK — it still throws a
 *     typed `AexApiError` with a string message and captured body;
 *   - `parseProviderFault` / `isThrottleFault` parse raw upstream fault shapes;
 *   - `session.messages.replayLast()` before any send throws a typed `SessionStateError`.
 *
 * Run offline (no dev network):
 *   cd aex/apps/user-tests
 *   bun test --isolate --timeout=180000 test/offline/edge-retry-injected.test.ts
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { strictEqual, ok } from "node:assert/strict";

// A fetch that plays a scripted list of outcomes for POST /api/sessions.
// Each outcome is one of:
//   - a status number (>=400 → error Response, else a 201 session)
//   - "network"                → throw a transient network error
//   - { status, retryAfter?, body? } → an error Response with a custom
//     retry-after header and/or raw body text (to fuzz the error decoder)
function makeFetch(outcomes) {
  const attempts = [];
  let i = 0;
  const fetch = async (input, init = {}) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method = String(init.method ?? "GET").toUpperCase();
    if (url.endsWith("/api/sessions") && method === "POST") {
      const outcome = outcomes[Math.min(i, outcomes.length - 1)];
      i += 1;
      attempts.push({ at: Date.now() });
      if (outcome === "network") throw new TypeError("fetch failed");
      const status = typeof outcome === "number" ? outcome : outcome.status;
      if (status >= 400) {
        const headers = { "content-type": "application/json" };
        const retryAfter = typeof outcome === "object" ? outcome.retryAfter : "0";
        if (retryAfter !== undefined && retryAfter !== null) headers["retry-after"] = String(retryAfter);
        const body = typeof outcome === "object" && outcome.body !== undefined
          ? outcome.body
          : JSON.stringify({ error: "slow down" });
        return new Response(body, { status, headers });
      }
      return new Response(JSON.stringify({ session: { id: "sess_edge", status: "idle", acceptsMessages: true } }), {
        status: 201,
        headers: { "content-type": "application/json" }
      });
    }
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
  };
  return { fetch, attempts };
}
`;

describe("SDK transport edge cases (installed package, injected fetch)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, fileName: string, timeoutMs = 120_000): Promise<Record<string, unknown>> {
    const scriptPath = join(install.installDir, fileName);
    writeFileSync(scriptPath, script);
    const child = await runCommand(getBunCommand(), [scriptPath], { cwd: install.installDir, timeoutMs });
    if (child.exitCode !== 0) {
      throw new Error(`${fileName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
    }
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  }

  it("honors Retry-After, distinguishes throttle vs server faults, and tolerates malformed error bodies", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const {
  Aex, isRateLimited, AexRateLimitError, AexApiError,
  parseProviderFault, isThrottleFault, SessionStateError
} = await import("@aexhq/sdk");

function client(fetch, retry) {
  return new Aex({
    apiKey: "aex_edge_token",
    baseUrl: "https://example.invalid",
    fetch,
    retry
  });
}
const create = (fetch, retry) =>
  client(fetch, retry).sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });

// (1) Retry-After is honored as a delay FLOOR. Server says "wait 1s" (retry-after: 1)
//     while our jittered backoff is ~1ms; the actual wait must be >= ~1s, and the
//     surfaced AexRateLimitError must carry retryAfterMs === 1000.
const a = makeFetch([{ status: 429, retryAfter: 1 }]);
// performance.now() (monotonic): the SDK sleeps on setTimeout's monotonic
// clock, so a wall-clock step (e.g. WSL2 timesyncd correction mid-sleep)
// can deflate a Date.now() delta below the real wait.
const t0 = performance.now();
let throttle;
try {
  await create(a.fetch, { maxAttempts: 2, initialDelayMs: 1, maxDelayMs: 2 });
} catch (err) { throttle = err; }
const retryAfterElapsedMs = Math.round(performance.now() - t0);
ok(throttle, "persistent 429 should reject");
strictEqual(isRateLimited(throttle), true);
strictEqual(throttle.status, 429);
strictEqual(throttle.retryAfterMs, 1000, "retryAfterMs parsed from Retry-After: 1 second");
strictEqual(throttle.attempts, 2);
ok(retryAfterElapsedMs >= 800, "Retry-After floored the wait to ~1s, got " + retryAfterElapsedMs + "ms");

// (2) 503 and 529 exhaustion ALSO surface a structured throttle (source "api"),
//     just like 429 — the whole RATE_LIMIT_STATUS set is covered.
const rateStatuses = {};
for (const status of [503, 529]) {
  const f = makeFetch([status]);
  let e;
  try { await create(f.fetch, { maxAttempts: 2, initialDelayMs: 1, maxDelayMs: 2 }); } catch (err) { e = err; }
  rateStatuses[status] = {
    isRateLimited: isRateLimited(e),
    isApiError: e instanceof AexApiError,
    status: e && e.status,
    source: e && e.source,
    attempts: e && e.attempts
  };
}

// (3) A pure 500 storm is transient (retried) but is NOT a "slow down" signal:
//     after exhaustion it must be a PLAIN AexApiError, isRateLimited === false.
const s500 = makeFetch([500]);
let serverErr;
try { await create(s500.fetch, { maxAttempts: 2, initialDelayMs: 1, maxDelayMs: 2 }); } catch (err) { serverErr = err; }
ok(serverErr, "persistent 500 should reject");
strictEqual(serverErr instanceof AexApiError, true);
strictEqual(isRateLimited(serverErr), false, "500 is a server fault, not a rate limit");
strictEqual(serverErr.status, 500);
strictEqual(s500.attempts.length, 2, "500 is retried up to maxAttempts");

// (4) A malformed / oversized error body must never crash the decoder. A
//     non-retryable 400 with a 200KB non-JSON body still yields a typed
//     AexApiError with a string message; the SDK does not hang or throw raw.
const junk = "x".repeat(200_000) + "\u{1F525}{not:json"; // huge + unicode + NUL + broken JSON
const bad = makeFetch([{ status: 400, retryAfter: "0", body: junk }]);
let malformed;
try { await create(bad.fetch, { maxAttempts: 3, initialDelayMs: 1, maxDelayMs: 2 }); } catch (err) { malformed = err; }
ok(malformed, "400 should reject");
strictEqual(malformed instanceof AexApiError, true);
strictEqual(malformed.status, 400);
strictEqual(typeof malformed.message, "string");
strictEqual(bad.attempts.length, 1, "400 is non-retryable: exactly one attempt");
const malformedMessage = String(malformed.message);

// A valid-but-oversized JSON error envelope: the server message is surfaced verbatim.
const bigMsg = "E".repeat(50_000);
const big = makeFetch([{ status: 400, retryAfter: "0", body: JSON.stringify({ error: bigMsg }) }]);
let bigErr;
try { await create(big.fetch); } catch (err) { bigErr = err; }
const bigMessageMatched = bigErr instanceof AexApiError && String(bigErr.message).includes(bigMsg);

// (5) parseProviderFault / isThrottleFault decode raw upstream fault shapes.
const overloaded = parseProviderFault({ type: "overloaded_error", message: "Overloaded" });
const rlFault = parseProviderFault({ type: "rate_limit_error", retry_after: 3 });
const authFault = parseProviderFault({ type: "authentication_error", message: "bad key" });
const faultChecks = {
  overloadedKind: overloaded && overloaded.kind,
  overloadedIsThrottle: overloaded ? isThrottleFault(overloaded) : null,
  rlKind: rlFault && rlFault.kind,
  rlRetryAfterMs: rlFault && rlFault.retryAfterMs, // 3 seconds → 3000 ms
  authFaultKind: authFault && authFault.kind // legacy auth errors normalize to generic provider_error
};

// (6) replayLast() before any send is a typed SessionStateError (not a bare throw).
const ok201 = makeFetch([201]);
const handle = await client(ok201.fetch).sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
let replayErr;
try { handle.messages.replayLast(); } catch (err) { replayErr = err; }
const replayCheck = {
  isSessionStateError: replayErr instanceof SessionStateError,
  name: replayErr && replayErr.name,
  code: replayErr && replayErr.code,
  hasMessage: typeof (replayErr && replayErr.message) === "string" && replayErr.message.length > 0
};

console.log(JSON.stringify({
  ok: true,
  retryAfter: { retryAfterMs: throttle.retryAfterMs, attempts: throttle.attempts, elapsedMs: retryAfterElapsedMs },
  rateStatuses,
  server500: { isRateLimited: isRateLimited(serverErr), status: serverErr.status, attempts: s500.attempts.length },
  malformed: { isApiError: malformed instanceof AexApiError, status: malformed.status, attempts: bad.attempts.length, messageType: typeof malformedMessage, bigMessageMatched },
  faultChecks,
  replayCheck
}));
`;
    const result = await runChild(script, "edge-retry-injected.mjs", 120_000);

    expect(result.ok).toBe(true);
    // (1) Retry-After honored as a floor + surfaced on the error.
    expect(result.retryAfter).toMatchObject({ retryAfterMs: 1000, attempts: 2 });
    expect((result.retryAfter as { elapsedMs: number }).elapsedMs).toBeGreaterThanOrEqual(800);
    // (2) 503 / 529 behave like 429 — structured throttle from the API plane.
    for (const status of [503, 529]) {
      expect(result.rateStatuses).toHaveProperty(String(status));
      expect((result.rateStatuses as Record<string, unknown>)[String(status)]).toMatchObject({
        isRateLimited: true,
        isApiError: true,
        status,
        source: "api",
        attempts: 2
      });
    }
    // (3) 500 is a server fault, NOT a rate limit.
    expect(result.server500).toMatchObject({ isRateLimited: false, status: 500, attempts: 2 });
    // (4) Malformed / oversized error bodies are tolerated, still typed.
    expect(result.malformed).toMatchObject({ isApiError: true, status: 400, attempts: 1, messageType: "string", bigMessageMatched: true });
    // (5) Provider-fault parsing.
    expect(result.faultChecks).toMatchObject({
      overloadedKind: "overloaded",
      overloadedIsThrottle: true,
      rlKind: "rate_limit",
      rlRetryAfterMs: 3000,
      authFaultKind: "provider_error"
    });
    // (6) replayLast → typed SessionStateError.
    expect(result.replayCheck).toMatchObject({
      isSessionStateError: true,
      name: "SessionStateError",
      code: "SESSION_STATE_ERROR",
      hasMessage: true
    });
  });
});

/**
 * Blackbox coverage of the SDK's built-in transport resilience through a clean
 * installed package. A fake fetch injects transient failures (429 / 5xx /
 * network) and records every attempt, so the assertions live at the user
 * boundary: `import "@aexhq/sdk"` resolves from the packed artifact, and we prove
 *
 *   - a throttled submit is retried with bounded backoff,
 *   - every retry re-issues the SAME Idempotency-Key (no duplicate billable run),
 *   - a persistent throttle surfaces a structured `AexRateLimitError`,
 *   - non-retryable 4xx fail fast, and `retry:false` disables the layer,
 *   - `session.replayLast(...)` exists for replaying a throttled turn.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { strictEqual, ok } from "node:assert/strict";

function headersToObject(headers) {
  const out = {};
  if (!headers) return out;
  if (headers instanceof Headers) {
    for (const [k, v] of headers.entries()) out[k.toLowerCase()] = v;
    return out;
  }
  if (Array.isArray(headers)) {
    for (const [k, v] of headers) out[String(k).toLowerCase()] = String(v);
    return out;
  }
  for (const [k, v] of Object.entries(headers)) out[String(k).toLowerCase()] = String(v);
  return out;
}

// A fetch that plays a scripted list of outcomes for POST /api/sessions and
// records the headers of every attempt. Each outcome is a status number, or
// "network" to throw a transient network error.
function makeFetch(outcomes) {
  const attempts = [];
  let i = 0;
  const fetch = async (input, init = {}) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const method = String(init.method ?? "GET").toUpperCase();
    if (url.endsWith("/api/sessions") && method === "POST") {
      const outcome = outcomes[Math.min(i, outcomes.length - 1)];
      i += 1;
      attempts.push({ headers: headersToObject(init.headers) });
      if (outcome === "network") throw new TypeError("fetch failed");
      if (outcome >= 400) {
        return new Response(JSON.stringify({ error: "slow down" }), {
          status: outcome,
          headers: { "content-type": "application/json", "retry-after": "0" }
        });
      }
      return new Response(JSON.stringify({ session: { id: "sess_retry", status: "idle", turnSeq: 0 } }), {
        status: 201,
        headers: { "content-type": "application/json" }
      });
    }
    return new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } });
  };
  return { fetch, attempts };
}
`;

describe("SDK built-in retry (installed package)", () => {
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

  it("retries transient failures with a stable idempotency key and structured throttle errors", async () => {
    const script =
      CHILD_HARNESS +
      String.raw`
const { Aex, isRateLimited, AexRateLimitError, AexApiError } = await import("@aexhq/sdk");

function client(fetch) {
  return new Aex({
    apiToken: "aex_retry_token",
    baseUrl: "https://example.invalid",
    fetch,
    retry: { maxAttempts: 4, initialDelayMs: 1, maxDelayMs: 2 }
  });
}

// 1) A 429 then a 500 then success — retried, and EVERY attempt reuses the one
//    Idempotency-Key, so a retry never creates a duplicate billable run.
const a = makeFetch([429, 500, 201]);
const session = await client(a.fetch).sessions.create({
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: "sk-ant" },
  idempotencyKey: "stable-key"
});
strictEqual(session.id, "sess_retry");
strictEqual(a.attempts.length, 3);
const keys = a.attempts.map((x) => x.headers["idempotency-key"]);
strictEqual(keys.every((k) => k === "stable-key"), true, "every retry reuses the same idempotency key");

// 2) A transient network error is retried, then succeeds.
const b = makeFetch(["network", 201]);
const s2 = await client(b.fetch).sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
strictEqual(s2.id, "sess_retry");
strictEqual(b.attempts.length, 2);

// 3) A persistent 429 surfaces a structured, non-leaky AexRateLimitError.
const c = makeFetch([429]);
let throttle;
try {
  await client(c.fetch).sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
} catch (err) {
  throttle = err;
}
ok(throttle, "expected the persistent throttle to reject");
strictEqual(isRateLimited(throttle), true);
strictEqual(throttle instanceof AexApiError, true, "AexRateLimitError is an AexApiError");
strictEqual(throttle.status, 429);
strictEqual(throttle.attempts, 4);
ok(!/token|secret|sk-ant|workspace|margin|cost/i.test(throttle.message), "throttle message is non-leaky");

// 4) A non-retryable 4xx fails fast — one attempt, not a rate-limit error.
const d = makeFetch([400]);
let badRequest;
try {
  await client(d.fetch).sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
} catch (err) {
  badRequest = err;
}
ok(badRequest, "400 should reject");
strictEqual(d.attempts.length, 1);
strictEqual(isRateLimited(badRequest), false);

// 5) retry:false disables the layer — a 429 is a single plain AexApiError.
const e = makeFetch([429]);
const off = new Aex({
  apiToken: "aex_retry_token",
  baseUrl: "https://example.invalid",
  fetch: e.fetch,
  retry: false
});
let raw;
try {
  await off.sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
} catch (err) {
  raw = err;
}
ok(raw, "429 should reject with retry disabled");
strictEqual(e.attempts.length, 1);
strictEqual(isRateLimited(raw), false);
strictEqual(raw instanceof AexApiError, true);

// 6) A handle exposes replayLast for replaying a throttled turn.
const f = makeFetch([201]);
const handle = await client(f.fetch).openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
strictEqual(typeof handle.replayLast, "function");

console.log(JSON.stringify({
  ok: true,
  retriedAttempts: a.attempts.length,
  networkAttempts: b.attempts.length,
  throttleAttempts: throttle.attempts,
  failFastAttempts: d.attempts.length,
  disabledAttempts: e.attempts.length,
  hasReplayLast: typeof handle.replayLast === "function"
}));
`;
    const result = await runChild(script, "sdk-retry-behaviors.mjs", 120_000);
    expect(result).toMatchObject({
      ok: true,
      retriedAttempts: 3,
      networkAttempts: 2,
      throttleAttempts: 4,
      failFastAttempts: 1,
      disabledAttempts: 1,
      hasReplayLast: true
    });
  });
});

/**
 * Live edge-case sweep: one-shot `client.run(...)` submission, idempotency, and
 * terminal lifecycle — hammered as a real customer against the DEV plane.
 *
 * Surface area: `Aex.run` / `SessionRunOptions` in packages/sdk/src/client.ts.
 * Each `it` installs the packed SDK (shared per worker) and drives a real run in
 * a child Bun process that `import { Aex } from "@aexhq/sdk"`, then asserts on the
 * printed JSON. Prompts are tiny and the model is deepseek-v4-flash to keep spend
 * and time low. Cases that only exercise CLIENT-side validation make no HTTP call.
 *
 * Required env (wired by the shared live runner):
 *   AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, AEX_USER_TEST_TARBALL
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`edge-run-lifecycle: required env ${name} is missing (needs a real dev-plane URL + gate-provider key).`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const providerKey = requireGateKey("edge-run-lifecycle");
const apiKey = requireEnv("AEX_API_KEY");
const model = gateModel();

// Shared in-child preamble: build the client + tiny helpers. No dynamic values
// are interpolated in — everything unique (idempotency keys, probes) is minted
// in-child so the embedded JS carries no `${...}` and every scenario script is
// self-contained and uniquely named on disk.
const PREAMBLE = `
import { Aex } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
const MODEL = process.env.MODEL;
const KEY = process.env.PROVIDER_KEY;
const PROVIDER = process.env.PROVIDER;
const gateKeys = { [PROVIDER]: KEY };
const WAIT = Number(process.env.WAIT_MS || "240000");
function uid(p){ return p + "-" + Date.now() + "-" + Math.random().toString(36).slice(2,8); }
function dense(s){ return (s || "").replace(/\\s+/g, ""); }
function errInfo(e){
  return {
    name: e && e.name ? e.name : null,
    message: e && e.message ? String(e.message) : String(e),
    status: (e && typeof e.status === "number") ? e.status : null,
    code: (e && e.code) ? e.code : null,
    apiCode: e && e.apiCode ? String(e.apiCode) : null,
    requestId: e && e.requestId ? String(e.requestId) : null
  };
}
function leaks(obj){ try { return JSON.stringify(obj).includes(KEY); } catch { return false; } }
function print(o){ process.stdout.write(JSON.stringify(o)); }
`;

function buildEnv(waitMs: number): Record<string, string> {
  const passEnv: Record<string, string> = {
    AEX_API_URL: apiUrl,
    AEX_API_KEY: apiKey,
    PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey,
    MODEL: model,
    WAIT_MS: String(waitMs)
  };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const k of carry) if (process.env[k]) passEnv[k] = process.env[k]!;
  return passEnv;
}

describe("live dev-plane — edge cases for client.run submission + idempotency + lifecycle", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(
    fileName: string,
    body: string,
    opts: { childTimeoutMs: number; waitMs: number }
  ): Promise<Record<string, unknown>> {
    const scriptPath = join(install.installDir, fileName);
    writeFileSync(scriptPath, PREAMBLE + "\n" + body);
    const child = await runCommand(getBunCommand(), [scriptPath], {
      cwd: install.installDir,
      timeoutMs: opts.childTimeoutMs,
      env: buildEnv(opts.waitMs)
    });
    if (child.exitCode !== 0) {
      throw new Error(
        `${fileName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
      );
    }
    const parsed = JSON.parse(child.stdout.trim()) as Record<string, unknown>;
    // Echo raw evidence to stderr so passing cases still surface their JSON.
    console.error(`[edge-evidence] ${fileName}: ${JSON.stringify(parsed)}`);
    return parsed;
  }

  // ── Case 0: pure CLIENT-side validation (no HTTP, no billable run) ──────────
  it(
    "rejects malformed run options at the SDK boundary before any HTTP call",
    async () => {
      const body = `
        async function rej(fn){ try { await fn(); return "__RESOLVED__"; } catch(e){ return e && e.message ? e.message : String(e); } }
        const emptyMsg     = await rej(() => client.run({ provider:PROVIDER, model:MODEL, message:"", apiKeys:gateKeys }));
        const emptyArr     = await rej(() => client.run({ provider:PROVIDER, model:MODEL, message:[], apiKeys:gateKeys }));
        const emptySegment = await rej(() => client.run({ provider:PROVIDER, model:MODEL, message:["ok",""], apiKeys:gateKeys }));
        const missingKey   = await rej(() => client.run({ provider:PROVIDER, model:MODEL, message:"hi" }));
        const badProvider  = await rej(() => client.run({ provider:"acme", model:MODEL, message:"hi", apiKeys:{ acme:"k" } }));
        const legacyPrompt = await rej(() => client.run({ provider:PROVIDER, model:MODEL, message:"hi", apiKeys:gateKeys, prompt:"x" }));
        print({ emptyMsg, emptyArr, emptySegment, missingKey, badProvider, legacyPrompt });
      `;
      const r = await runChild("edge-clientside-validation.mjs", body, { childTimeoutMs: 120_000, waitMs: 60_000 });
      expect(r.emptyMsg).toMatch(/message must be a non-empty string/);
      expect(r.emptyArr).toMatch(/non-empty string or string array/);
      expect(r.emptySegment).toMatch(/segments must be non-empty strings/);
      expect(r.missingKey).toMatch(/provider API key is required/);
      expect(r.badProvider).toMatch(/not available for (?:model|provider)/);
      expect(r.legacyPrompt).toMatch(/prompt is not a supported option/);
    },
    150_000
  );

  // ── Case 1: baseline success + RunResult.ok/status/text + unknown-field ignore ─
  it(
    "one-shot run succeeds: ok=true, parked status, verbatim text, and an unknown option is ignored gracefully",
    async () => {
      const body = `
        const probe = "OK-" + Math.random().toString(36).slice(2,8);
        const r = await client.run({
          provider:PROVIDER, model:MODEL,
          message:"Output verbatim: " + probe,
          idempotencyKey: uid("edge-baseline"),
          apiKeys: gateKeys,
          totallyUnknownOption: { nope: 1 } // must be ignored, not rejected
        }, { timeoutMs: WAIT });
        const out = {
          runId: r.runId, ok: r.ok, status: r.status,
          denseText: dense(r.text), probe,
          hasUsage: !!r.usage, usageTotal: r.usage && r.usage.totalTokens,
          costUsd: (typeof r.costUsd === "number") ? r.costUsd : null,
          eventCount: Array.isArray(r.events) ? r.events.length : null,
          error: r.error || null
        };
        print({ ...out, leaked: leaks(out) });
      `;
      const r = await runChild("edge-baseline.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ok).toBe(true);
      expect(["idle", "suspended", "succeeded"]).toContain(r.status);
      expect(String(r.denseText)).toContain(String(r.probe));
      expect(Number(r.eventCount)).toBeGreaterThan(0);
      expect(r.leaked).toBe(false);
    },
    330_000
  );

  // ── Case 2: idempotencyKey SEQUENTIAL reuse → same run, no duplicate ─────────
  it(
    "reusing one idempotencyKey sequentially replays the same run (no duplicate billable run)",
    async () => {
      const body = `
        const key = uid("edge-idem-seq");
        const opts = () => ({ provider:PROVIDER, model:MODEL, message:"Output verbatim: SEQ", idempotencyKey:key, apiKeys:gateKeys });
        const first = await client.run(opts(), { timeoutMs: WAIT });
        const second = await client.run(opts(), { timeoutMs: WAIT });
        print({ runId1:first.runId, runId2:second.runId, same:(first.runId === second.runId), ok1:first.ok, ok2:second.ok, status1:first.status, status2:second.status });
      `;
      const r = await runChild("edge-idem-seq.mjs", body, { childTimeoutMs: 420_000, waitMs: 200_000 });
      expect(r.same).toBe(true);
      expect(r.ok1).toBe(true);
      expect(r.ok2).toBe(true);
    },
    450_000
  );

  // ── Case 3: idempotencyKey CONCURRENT reuse → race handled, same run ─────────
  it(
    "two concurrent runs with the same idempotencyKey resolve to the same run (create race is deduped)",
    async () => {
      const body = `
        const key = uid("edge-idem-conc");
        const opts = () => ({ provider:PROVIDER, model:MODEL, message:"Output verbatim: CONC", idempotencyKey:key, apiKeys:gateKeys });
        const settled = await Promise.allSettled([
          client.run(opts(), { timeoutMs: WAIT }),
          client.run(opts(), { timeoutMs: WAIT })
        ]);
        const view = settled.map(s => s.status === "fulfilled"
          ? { ok:true, runId:s.value.runId, runOk:s.value.ok, status:s.value.status }
          : { ok:false, err: errInfo(s.reason) });
        const ids = view.filter(v => v.ok).map(v => v.runId);
        print({ view, bothFulfilled: view.every(v => v.ok), sameId: (ids.length === 2 && ids[0] === ids[1]), distinctIds: [...new Set(ids)] });
      `;
      const r = await runChild("edge-idem-conc.mjs", body, { childTimeoutMs: 300_000, waitMs: 200_000 });
      // Contract: reusing an idempotencyKey must NOT create a duplicate billable
      // run. Whether concurrent, both calls must converge on ONE run id.
      expect(Array.isArray(r.distinctIds) ? (r.distinctIds as unknown[]).length : 99).toBeLessThanOrEqual(1);
    },
    330_000
  );

  // ── Case 4: deleteAfter → session gone (open should 404) ─────────────────────
  it(
    "deleteAfter removes the session: opening it afterward 404s (or reports a deleted record)",
    async () => {
      const body = `
        const r = await client.run({
          provider:PROVIDER, model:MODEL,
          message:"Output verbatim: DEL",
          idempotencyKey: uid("edge-del"),
          apiKeys: gateKeys,
          deleteAfter: true
        }, { timeoutMs: WAIT });
        let openOutcome, recordStatus=null, err=null;
        try {
          const s = await client.sessions.open(r.runId);
          openOutcome = "resolved";
          recordStatus = s && s.record ? s.record.status : null;
        } catch(e) {
          openOutcome = "threw";
          err = errInfo(e);
        }
        const deleted = (openOutcome === "threw" && err && err.status === 404)
          || (openOutcome === "resolved" && (recordStatus === "deleted" || recordStatus === "expired"));
        print({ runId:r.runId, ranOk:r.ok, openOutcome, recordStatus, err, deleted });
      `;
      const r = await runChild("edge-delete-after.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ranOk).toBe(true);
      expect(r.deleted).toBe(true);
    },
    330_000
  );

  // ── Case 5: whitespace-only message (passes client validation, server runs it) ─
  it(
    "whitespace-only message is accepted client-side and terminates without hanging",
    async () => {
      const body = `
        const t0 = Date.now();
        let outcome, res=null, err=null;
        try {
          const r = await client.run({ provider:PROVIDER, model:MODEL, message:"   ", idempotencyKey: uid("edge-ws"), apiKeys:gateKeys }, { timeoutMs: WAIT });
          outcome = "resolved";
          const streamErrs = Array.isArray(r.events) ? r.events.filter(e => e && e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error").length : 0;
          res = { ok:r.ok, status:r.status, hasError: !!r.error, error: (r.error||"").slice(0,300), denseTextLen: dense(r.text).length, streamErrs };
        } catch(e) { outcome = "threw"; err = errInfo(e); }
        print({ scenario:"whitespace", outcome, res, err, elapsedMs: Date.now()-t0, leaked: leaks({res, err}) });
      `;
      const r = await runChild("edge-whitespace-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 200_000 });
      // Robust invariant: it must TERMINATE (resolve or throw), not hang, and never leak the key.
      expect(["resolved", "threw"]).toContain(r.outcome);
      expect(r.leaked).toBe(false);
    },
    330_000
  );

  // ── Case 6: unicode / emoji / CJK / newline message (wire-encoding round-trip) ─
  it(
    "unicode + emoji + CJK + newline message submits and completes cleanly",
    async () => {
      const body = `
        const probe = "u" + Math.random().toString(36).slice(2,6);
        const msg = "Output this token verbatim then stop: [[" + probe + "-café-\\uD83D\\uDE80-\\u65E5\\u672C\\u8A9E]]\\nSecond line.";
        const r = await client.run({ provider:PROVIDER, model:MODEL, message: msg, idempotencyKey: uid("edge-unicode"), apiKeys:gateKeys }, { timeoutMs: WAIT });
        const out = { runId:r.runId, ok:r.ok, status:r.status, probe, denseText: dense(r.text).slice(0,200), textLen: (r.text||"").length };
        print({ ...out, leaked: leaks(out) });
      `;
      const r = await runChild("edge-unicode-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ok).toBe(true);
      expect(["idle", "suspended", "succeeded"]).toContain(r.status);
      expect(Number(r.textLen)).toBeGreaterThan(0);
      expect(String(r.denseText)).toContain(String(r.probe));
      expect(r.leaked).toBe(false);
    },
    330_000
  );

  // ── Case 7: very long (~50KB) message (large-payload submission) ─────────────
  it(
    "a ~50KB message submits and completes without a payload-size failure",
    async () => {
      const body = `
        const filler = "A".repeat(50 * 1024);
        const msg = "Reply with exactly the single word DONE and nothing else. Ignore this padding: " + filler;
        const t0 = Date.now();
        let outcome, res=null, err=null;
        try {
          const r = await client.run({ provider:PROVIDER, model:MODEL, message: msg, idempotencyKey: uid("edge-big"), apiKeys:gateKeys }, { timeoutMs: WAIT });
          outcome = "resolved";
          res = { ok:r.ok, status:r.status, denseText: dense(r.text).slice(0,80), textLen:(r.text||"").length, error:r.error||null };
        } catch(e) { outcome="threw"; err = errInfo(e); }
        print({ scenario:"bigmsg", bytes: msg.length, outcome, res, err, elapsedMs: Date.now()-t0, leaked: leaks({res, err}) });
      `;
      const r = await runChild("edge-big-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.leaked).toBe(false);
      // Observed behavior (pinned): a ~50KB message is accepted and succeeds —
      // no payload-size failure, no hang.
      expect(r.outcome).toBe("resolved");
      expect((r.res as Record<string, unknown>).ok).toBe(true);
    },
    330_000
  );

  // ── Case 8: message as string ARRAY (SessionInput union) ─────────────────────
  it(
    "message given as a string array (SessionInput union) submits and completes",
    async () => {
      const body = `
        const probe = "arr" + Math.random().toString(36).slice(2,6);
        const r = await client.run({ provider:PROVIDER, model:MODEL, message:["Output verbatim:", probe], idempotencyKey: uid("edge-arr"), apiKeys:gateKeys }, { timeoutMs: WAIT });
        const out = { runId:r.runId, ok:r.ok, status:r.status, probe, denseText: dense(r.text).slice(0,200), textLen:(r.text||"").length };
        print({ ...out, leaked: leaks(out) });
      `;
      const r = await runChild("edge-array-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ok).toBe(true);
      expect(Number(r.textLen)).toBeGreaterThan(0);
      expect(r.leaked).toBe(false);
    },
    330_000
  );

  // ── Case 9: invalid model → server rejects at submit (EXPECTED) ──────────────
  it(
    "an invalid model id is rejected (server 4xx), never accepted as a successful run",
    async () => {
      const body = `
        let outcome, res=null, err=null;
        try {
          const r = await client.run({ provider:PROVIDER, model:"not-a-model", message:"hi", idempotencyKey: uid("edge-badmodel"), apiKeys:gateKeys }, { timeoutMs: 90_000 });
          outcome = "resolved";
          res = { ok:r.ok, status:r.status, error:r.error||null };
        } catch(e) { outcome = "threw"; err = errInfo(e); }
        print({ scenario:"badmodel", outcome, res, err });
      `;
      const r = await runChild("edge-invalid-model.mjs", body, { childTimeoutMs: 150_000, waitMs: 90_000 });
      // Admission validation rejects an unknown model before provisioning. This
      // is preferable to accepting a doomed billable run and surfacing the
      // model error only after the runner starts.
      expect(r.outcome).toBe("threw");
      const err = r.err as Record<string, unknown>;
      expect(err.status).toBe(400);
      expect(String(err.message ?? "")).toMatch(/invalid_model|model/i);
    },
    180_000
  );

  // ── Case 10: tiny client timeoutMs → abort behavior (no hang, no leak) ───────
  it(
    "a tiny timeoutMs aborts the wait quickly without hanging or leaking (documents partial-result semantics)",
    async () => {
      const body = `
        const t0 = Date.now();
        let outcome, res=null, err=null;
        try {
          const r = await client.run({ provider:PROVIDER, model:MODEL, message:"Output verbatim: TINY", idempotencyKey: uid("edge-tiny"), apiKeys:gateKeys }, { timeoutMs: 1 });
          outcome = "resolved";
          res = { ok:r.ok, status:r.status, eventCount: Array.isArray(r.events)?r.events.length:null, hasError: !!r.error, error:r.error||null };
        } catch(e) { outcome = "threw"; err = errInfo(e); }
        print({ scenario:"tiny-timeout", outcome, res, err, elapsedMs: Date.now()-t0, leaked: leaks({res, err}) });
      `;
      const r = await runChild("edge-tiny-timeout.mjs", body, { childTimeoutMs: 120_000, waitMs: 5_000 });
      // The one hard requirement from the brief: no hang, no leak. A hang would
      // be caught by the 120s child timeout above and fail this test.
      expect(["resolved", "threw"]).toContain(r.outcome);
      expect(r.leaked).toBe(false);
      expect(Number(r.elapsedMs)).toBeLessThan(90_000);
    },
    150_000
  );
});

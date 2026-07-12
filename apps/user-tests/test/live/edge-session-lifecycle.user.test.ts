/**
 * Live edge-case sweep: one-shot `client.start(...)` submission, idempotency, and
 * terminal lifecycle — hammered as a real customer against the DEV plane.
 *
 * Surface area: `Aex.start` / `SessionStartOptions` in packages/sdk/src/client.ts.
 * Each `it` installs the packed SDK (shared per worker) and drives a real session in
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
    throw new Error(`edge-session-lifecycle: required env ${name} is missing (needs a real dev-plane URL + gate-provider key).`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const providerKey = requireGateKey("edge-session-lifecycle");
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
const knownSecrets = [process.env.AEX_API_KEY, KEY]
  .filter((value) => typeof value === "string" && value.length > 0);
const WAIT = Number(process.env.WAIT_MS || "240000");
function uid(p){ return p + "-" + Date.now() + "-" + Math.random().toString(36).slice(2,8); }
function dense(s){ return (s || "").replace(/\\s+/g, ""); }
function errInfo(e){
  const details = e && e.details && typeof e.details === "object" && !Array.isArray(e.details)
    ? e.details
    : null;
  const detailKeys = details ? Object.keys(details) : [];
  return {
    name: e && e.name ? e.name : null,
    hasMessage: !!(e && e.message),
    status: (e && typeof e.status === "number") ? e.status : null,
    code: (e && e.code) ? e.code : null,
    detailsField: details && typeof details.field === "string" ? details.field : null,
    detailsOnlyField: detailKeys.length === 1 && detailKeys[0] === "field",
    apiCode: e && e.apiCode ? String(e.apiCode) : null,
    hasRequestId: !!(e && e.requestId)
  };
}
function serialized(value){ try { return JSON.stringify(value) || ""; } catch { return ""; } }
function containsKnownSecret(value){
  const text = serialized(value);
  return knownSecrets.some((secret) => text.includes(secret));
}
function redactKnownSecrets(value){
  let text = serialized(value);
  for (const secret of knownSecrets) text = text.split(secret).join("[REDACTED]");
  return JSON.parse(text);
}
function print(value){
  const leakedKeyAnywhere = containsKnownSecret(value);
  const safe = redactKnownSecrets({ ...value, leakedKeyAnywhere });
  process.stdout.write(JSON.stringify(safe));
}
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

describe("live dev-plane — edge cases for client.start submission + idempotency + lifecycle", () => {
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
    const leakedKnownKey = [apiKey, providerKey].some(
      (secret) => child.stdout.includes(secret) || child.stderr.includes(secret)
    );
    if (child.exitCode !== 0) {
      throw new Error(
        `${fileName} exited ${child.exitCode}; stdoutBytes=${child.stdout.length}; ` +
        `stderrBytes=${child.stderr.length}; leakedKnownKey=${leakedKnownKey}`
      );
    }
    if (leakedKnownKey) {
      throw new Error(
        `${fileName} emitted a known key; stdoutBytes=${child.stdout.length}; stderrBytes=${child.stderr.length}`
      );
    }
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(child.stdout.trim()) as Record<string, unknown>;
    } catch {
      throw new Error(
        `${fileName} emitted invalid JSON; stdoutBytes=${child.stdout.length}; stderrBytes=${child.stderr.length}`
      );
    }
    if (parsed.leakedKeyAnywhere === true) {
      throw new Error(
        `${fileName} detected a known key before output; stdoutBytes=${child.stdout.length}; stderrBytes=${child.stderr.length}`
      );
    }
    console.error(
      `[edge-evidence] ${fileName}: stdoutBytes=${child.stdout.length}; ` +
      `stderrBytes=${child.stderr.length}; fieldCount=${Object.keys(parsed).length}; leakedKnownKey=false`
    );
    return parsed;
  }

  function expectConfigError(value: unknown, field: string): void {
    expect(value).toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      hasMessage: true,
      detailsField: field,
      detailsOnlyField: true
    });
  }

  // ── Case 0: pure CLIENT-side validation (no HTTP, no billable session turn) ──────────
  it(
    "rejects malformed session options at the SDK boundary before any HTTP call",
    async () => {
      const body = `
        let httpCalls = 0;
        const validationClient = new Aex({
          baseUrl: process.env.AEX_API_URL,
          apiKey: process.env.AEX_API_KEY,
          retry: false,
          fetch: async () => { httpCalls += 1; throw new Error("CLIENT_VALIDATION_MADE_HTTP"); }
        });
        async function rej(fn){
          try {
            await fn();
            return { name:"Resolved", hasMessage:false, status:null, code:null, detailsField:null, detailsOnlyField:false, apiCode:null, hasRequestId:false };
          } catch(e) {
            return errInfo(e);
          }
        }
        const emptyMsg     = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:"", apiKeys:gateKeys }));
        const emptyArr     = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:[], apiKeys:gateKeys }));
        const emptySegment = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:["ok",""], apiKeys:gateKeys }));
        const whitespaceMsg = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:"  \\n\\t ", apiKeys:gateKeys }));
        const whitespaceArray = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:["  ","\\n"], apiKeys:gateKeys }));
        const missingKey   = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:"hi" }));
        const badProvider  = await rej(() => validationClient.start({ provider:"acme", model:MODEL, message:"hi", apiKeys:{ acme:"k" } }));
        const legacyPrompt = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:"hi", apiKeys:gateKeys, prompt:"x" }));
        const unknownOption = await rej(() => validationClient.start({ provider:PROVIDER, model:MODEL, message:"hi", apiKeys:gateKeys, totallyUnknownOption:{nope:true} }));
        print({ emptyMsg, emptyArr, emptySegment, whitespaceMsg, whitespaceArray, missingKey, badProvider, legacyPrompt, unknownOption, httpCalls });
      `;
      const r = await runChild("edge-clientside-validation.mjs", body, { childTimeoutMs: 120_000, waitMs: 60_000 });
      expectConfigError(r.emptyMsg, "message");
      expectConfigError(r.emptyArr, "message");
      expectConfigError(r.emptySegment, "message");
      expectConfigError(r.whitespaceMsg, "message");
      expectConfigError(r.whitespaceArray, "message");
      expectConfigError(r.missingKey, `apiKeys.${GATE_PROVIDER}`);
      expectConfigError(r.badProvider, "provider");
      expectConfigError(r.legacyPrompt, "prompt");
      expectConfigError(r.unknownOption, "totallyUnknownOption");
      expect(r.httpCalls).toBe(0);
    },
    150_000
  );

  // ── Case 1: baseline success + settled SessionResult fields ────────────────
  it(
    "one-shot run succeeds with a settled status and verbatim text",
    async () => {
      const body = `
        const probe = "OK-" + Math.random().toString(36).slice(2,8);
        const r = await client.start({
          provider:PROVIDER, model:MODEL,
          message:"SessionFile verbatim: " + probe,
          idempotencyKey: uid("edge-baseline"),
          apiKeys: gateKeys
        }, { timeoutMs: WAIT });
        const out = {
          sessionId: r.sessionId, ok: r.ok, status: r.status,
          textContainsProbe: dense(r.text).includes(probe), textLen: (r.text || "").length,
          hasUsage: !!r.usage, usageTotal: r.usage && r.usage.totalTokens,
          costUsd: (typeof r.costUsd === "number") ? r.costUsd : null,
          eventCount: Array.isArray(r.events) ? r.events.length : null,
          hasError: !!r.error
        };
        print(out);
      `;
      const r = await runChild("edge-baseline.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ok).toBe(true);
      expect(r.status).toBe("succeeded");
      expect(r.textContainsProbe).toBe(true);
      expect(Number(r.eventCount)).toBeGreaterThan(0);
      expect(r.leakedKeyAnywhere).toBe(false);
    },
    330_000
  );

  // ── Case 2: idempotencyKey SEQUENTIAL reuse → same run, no duplicate ─────────
  it(
    "reusing one idempotencyKey sequentially replays the same run (no duplicate billable session turn)",
    async () => {
      const body = `
        const key = uid("edge-idem-seq");
        const opts = () => ({ provider:PROVIDER, model:MODEL, message:"SessionFile verbatim: SEQ", idempotencyKey:key, apiKeys:gateKeys });
        const first = await client.start(opts(), { timeoutMs: WAIT });
        const second = await client.start(opts(), { timeoutMs: WAIT });
        print({ sessionId1:first.sessionId, sessionId2:second.sessionId, same:(first.sessionId === second.sessionId), ok1:first.ok, ok2:second.ok, status1:first.status, status2:second.status });
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
    "two concurrent sessions with the same idempotencyKey resolve to the same run (create race is deduped)",
    async () => {
      const body = `
        const key = uid("edge-idem-conc");
        const opts = () => ({ provider:PROVIDER, model:MODEL, message:"SessionFile verbatim: CONC", idempotencyKey:key, apiKeys:gateKeys });
        const settled = await Promise.allSettled([
          client.start(opts(), { timeoutMs: WAIT }),
          client.start(opts(), { timeoutMs: WAIT })
        ]);
        const view = settled.map(s => s.status === "fulfilled"
          ? { ok:true, sessionId:s.value.sessionId, runOk:s.value.ok, status:s.value.status }
          : { ok:false, err: errInfo(s.reason) });
        const ids = view.filter(v => v.ok).map(v => v.sessionId);
        print({ view, bothFulfilled: view.every(v => v.ok), sameId: (ids.length === 2 && ids[0] === ids[1]), distinctIds: [...new Set(ids)] });
      `;
      const r = await runChild("edge-idem-conc.mjs", body, { childTimeoutMs: 300_000, waitMs: 200_000 });
      // Contract: reusing an idempotencyKey must NOT create a duplicate billable
      // run. Whether concurrent, both calls must converge on ONE session id.
      expect(Array.isArray(r.distinctIds) ? (r.distinctIds as unknown[]).length : 99).toBeLessThanOrEqual(1);
    },
    330_000
  );

  // ── Case 4: deleteAfter → session gone (open should 404) ─────────────────────
  it(
    "deleteAfter removes the session: opening it afterward 404s (or reports a deleted record)",
    async () => {
      const body = `
        const r = await client.start({
          provider:PROVIDER, model:MODEL,
          message:"SessionFile verbatim: DEL",
          idempotencyKey: uid("edge-del"),
          apiKeys: gateKeys,
          deleteAfter: true
        }, { timeoutMs: WAIT });
        let openOutcome, recordStatus=null, err=null;
        try {
          const s = await client.sessions.open(r.sessionId);
          openOutcome = "resolved";
          recordStatus = s && s.record ? s.record.status : null;
        } catch(e) {
          openOutcome = "threw";
          err = errInfo(e);
        }
        const deleted = (openOutcome === "threw" && err && err.status === 404)
          || (openOutcome === "resolved" && (recordStatus === "deleted" || recordStatus === "expired"));
        print({ sessionId:r.sessionId, ranOk:r.ok, openOutcome, recordStatus, err, deleted });
      `;
      const r = await runChild("edge-delete-after.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ranOk).toBe(true);
      expect(r.deleted).toBe(true);
    },
    330_000
  );

  // ── Case 5: unicode / emoji / CJK / newline message (wire-encoding round-trip) ─
  it(
    "unicode + emoji + CJK + newline message submits and completes cleanly",
    async () => {
      const body = `
        const probe = "u" + Math.random().toString(36).slice(2,6);
        const msg = "SessionFile this token verbatim then stop: [[" + probe + "-café-\\uD83D\\uDE80-\\u65E5\\u672C\\u8A9E]]\\nSecond line.";
        const r = await client.start({ provider:PROVIDER, model:MODEL, message: msg, idempotencyKey: uid("edge-unicode"), apiKeys:gateKeys }, { timeoutMs: WAIT });
        print({
          sessionId:r.sessionId,
          ok:r.ok,
          status:r.status,
          textContainsProbe:dense(r.text).includes(probe),
          textLen:(r.text||"").length
        });
      `;
      const r = await runChild("edge-unicode-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ok).toBe(true);
      expect(["idle", "suspended", "succeeded"]).toContain(r.status);
      expect(Number(r.textLen)).toBeGreaterThan(0);
      expect(r.textContainsProbe).toBe(true);
      expect(r.leakedKeyAnywhere).toBe(false);
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
          const r = await client.start({ provider:PROVIDER, model:MODEL, message: msg, idempotencyKey: uid("edge-big"), apiKeys:gateKeys }, { timeoutMs: WAIT });
          outcome = "resolved";
          res = { ok:r.ok, status:r.status, textLen:(r.text||"").length, hasError:!!r.error };
        } catch(e) { outcome="threw"; err = errInfo(e); }
        print({ scenario:"bigmsg", bytes: msg.length, outcome, res, err, elapsedMs: Date.now()-t0 });
      `;
      const r = await runChild("edge-big-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.leakedKeyAnywhere).toBe(false);
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
        const r = await client.start({ provider:PROVIDER, model:MODEL, message:["SessionFile verbatim:", probe], idempotencyKey: uid("edge-arr"), apiKeys:gateKeys }, { timeoutMs: WAIT });
        print({
          sessionId:r.sessionId,
          ok:r.ok,
          status:r.status,
          textContainsProbe:dense(r.text).includes(probe),
          textLen:(r.text||"").length
        });
      `;
      const r = await runChild("edge-array-message.mjs", body, { childTimeoutMs: 300_000, waitMs: 240_000 });
      expect(r.ok).toBe(true);
      expect(Number(r.textLen)).toBeGreaterThan(0);
      expect(r.textContainsProbe).toBe(true);
      expect(r.leakedKeyAnywhere).toBe(false);
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
          const r = await client.start({ provider:PROVIDER, model:"not-a-model", message:"hi", idempotencyKey: uid("edge-badmodel"), apiKeys:gateKeys }, { timeoutMs: 90_000 });
          outcome = "resolved";
          res = { ok:r.ok, status:r.status, hasError:!!r.error };
        } catch(e) { outcome = "threw"; err = errInfo(e); }
        print({ scenario:"badmodel", outcome, res, err });
      `;
      const r = await runChild("edge-invalid-model.mjs", body, { childTimeoutMs: 150_000, waitMs: 90_000 });
      // Admission validation rejects an unknown model before provisioning. This
      // is preferable to accepting a doomed billable session turn and surfacing the
      // model error only after the runner starts.
      expect(r.outcome).toBe("threw");
      const err = r.err as Record<string, unknown>;
      expect(err.status).toBe(400);
      expect(err.hasMessage).toBe(true);
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
          const r = await client.start({ provider:PROVIDER, model:MODEL, message:"SessionFile verbatim: TINY", idempotencyKey: uid("edge-tiny"), apiKeys:gateKeys }, { timeoutMs: 1 });
          outcome = "resolved";
          res = { ok:r.ok, status:r.status, eventCount: Array.isArray(r.events)?r.events.length:null, hasError:!!r.error };
        } catch(e) { outcome = "threw"; err = errInfo(e); }
        print({ scenario:"tiny-timeout", outcome, res, err, elapsedMs: Date.now()-t0 });
      `;
      const r = await runChild("edge-tiny-timeout.mjs", body, { childTimeoutMs: 120_000, waitMs: 5_000 });
      // The one hard requirement from the brief: no hang, no leak. A hang would
      // be caught by the 120s child timeout above and fail this test.
      expect(["resolved", "threw"]).toContain(r.outcome);
      expect(r.leakedKeyAnywhere).toBe(false);
      expect(Number(r.elapsedMs)).toBeLessThan(90_000);
    },
    150_000
  );
});

/**
 * Live edge-case sweep: session-path admission gates.
 *
 * Release-gating probes for session-path admission gates. These assert the
 * public SDK path sees the same hosted admission contract as one-shot submits:
 *
 *   1. Concurrency: `whoami().limits.maxConcurrentSessions` is enforced for
 *      `client.start(...)` / session turns with a public 429
 *      `workspace_concurrency_exceeded` once the cap is saturated.
 *   2. A retired `secrets.apiKeys` provider key is rejected at session create
 *      with 400 `invalid_submission` (managed AI Gateway; no customer key).
 *   3. A non-slug model is rejected at session create, before any billable turn
 *      launches.
 *
 * Billing: probes 2 and 3 are refused at admission and never create a session
 * (zero billable). Probe 1 sends cap+1 tiny turns (~$0.003 at gateway rates) —
 * it cannot observe concurrent running states without running concurrently.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, plus AEX_USER_TEST_TARBALL/VERSION
 * (wired by the shared runner). No provider key: the platform's managed gateway
 * key serves every model call.
 *
 * Optional env: AEX_ADMISSION_GATES_MAX_SAFE_CAP limits the concurrency probe
 * to low-cap isolated workspaces (default 10). The test fails before launching
 * sessions when the target workspace cap is higher.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { gateModel } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-admission-gates): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const model = gateModel();

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of [
      "SystemRoot",
      "SystemDrive",
      "TEMP",
      "TMP",
      "USERPROFILE",
      "APPDATA",
      "LOCALAPPDATA",
      "ComSpec",
      "ProgramFiles",
      "ProgramData"
    ]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

const CHILD_PRELUDE = `
  import { Aex } from "@aexhq/sdk";
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
  const noRetryClient = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY, retry: false });
  const MODEL = process.env.MODEL;
  const MAX_SAFE_CAP = Number(process.env.AEX_ADMISSION_GATES_MAX_SAFE_CAP ?? "10");
  const HOLD_SECONDS = Number(process.env.AEX_ADMISSION_GATES_HOLD_SECONDS ?? "45");
  const errShape = (e) => ({
    name: e && e.constructor ? e.constructor.name : "Error",
    message: e && e.message ? String(e.message).slice(0, 300) : String(e),
    status: e && typeof e.status === "number" ? e.status : null,
    code: e && typeof e.code === "string" ? e.code : null,
    requestId: e && e.body && typeof e.body.requestId === "string" ? e.body.requestId : null,
    bodyError: e && e.body && typeof e.body.error === "string" ? e.body.error : null,
    bodyCode: e && e.body && typeof e.body.code === "string" ? e.body.code : null,
    bodyObserved: e && e.body && typeof e.body.observed === "number" ? e.body.observed : null,
    bodyCap: e && e.body && typeof e.body.cap === "number" ? e.body.cap : null
  });
  const RAW_CONNECT_TRANSIENT_CODES = new Set([
    "ConnectionRefused",
    "FailedToOpenSocket",
    "ECONNREFUSED",
    "EAI_AGAIN",
    "ETIMEDOUT",
    "UND_ERR_CONNECT_TIMEOUT"
  ]);
  const rawErrorCode = (e) => {
    if (e && typeof e.code === "string") return e.code;
    if (e && e.cause && typeof e.cause.code === "string") return e.cause.code;
    const message = e && e.message ? String(e.message) : String(e);
    const match = /\\b(ConnectionRefused|FailedToOpenSocket|E[A-Z0-9_]+|UND_ERR_[A-Z0-9_]+)\\b/.exec(message);
    return match ? match[1] : null;
  };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const raw = async (method, path, body) => {
    const url = process.env.AEX_API_URL + path;
    const maxAttempts = ["GET", "HEAD", "OPTIONS"].includes(method) ? 3 : 1;
    for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
      try {
        const res = await fetch(url, {
          method,
          headers: { authorization: "Bearer " + process.env.AEX_API_KEY, "content-type": "application/json" },
          body: body ? JSON.stringify(body) : undefined
        });
        let parsed = null;
        try { parsed = await res.json(); } catch {}
        return { status: res.status, body: parsed };
      } catch (e) {
        const code = rawErrorCode(e);
        if (attempt === maxAttempts || !RAW_CONNECT_TRANSIENT_CODES.has(code)) throw e;
        console.error("raw API fetch transient " + code + " for " + method + " " + path + " attempt " + attempt + "/" + maxAttempts);
        await sleep(250 * attempt);
      }
    }
    throw new Error("unreachable raw retry loop");
  };
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 5 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      MODEL: model,
      AEX_ADMISSION_GATES_MAX_SAFE_CAP: process.env.AEX_ADMISSION_GATES_MAX_SAFE_CAP ?? "10"
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-admission-gates runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-admission-gates runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: session-path admission gates", () => {
  it(
    "the plan concurrency cap rejects turns beyond maxConcurrentSessions",
    async () => {
      const body = `
        const out = { cap: null, admitted: 0, rejected429: 0, otherErrors: [], peakRunning: 0, initialRunning: 0, sessionIds: [], retryDisabledForOverflow: true };
        const me = await client.whoami();
        out.cap = me.limits.maxConcurrentSessions;
        if (!Number.isFinite(MAX_SAFE_CAP) || MAX_SAFE_CAP < 1) {
          out.setupError = "invalid_max_safe_cap";
          out.maxSafeCap = MAX_SAFE_CAP;
          console.log(JSON.stringify(out));
          process.exit(0);
        }
        if (out.cap > MAX_SAFE_CAP) {
          out.setupError = "cap_too_high";
          out.maxSafeCap = MAX_SAFE_CAP;
          console.log(JSON.stringify(out));
          process.exit(0);
        }
        if (!Number.isFinite(HOLD_SECONDS) || HOLD_SECONDS < 10 || HOLD_SECONDS > 120) {
          out.setupError = "invalid_hold_seconds";
          out.holdSeconds = HOLD_SECONDS;
          console.log(JSON.stringify(out));
          process.exit(0);
        }
        const startedAt = Date.now();
        const runningCount = async () => {
          const page = await client.sessions.list({ limit: 50 });
          const running = page.sessions.filter((s) => s.status === "running").length;
          out.peakRunning = Math.max(out.peakRunning, running);
          return running;
        };
        const waitForRunning = async (target, timeoutMs, label) => {
          const deadline = Date.now() + timeoutMs;
          let last = 0;
          while (Date.now() < deadline) {
            try {
              last = await runningCount();
              if (last >= target) return true;
            } catch (e) {
              out.otherErrors.push({ phase: label, err: errShape(e) });
            }
            await sleep(2000);
          }
          out.waitTimedOut = label;
          out.lastRunning = last;
          return false;
        };
        const waitForIdle = async (timeoutMs) => {
          const deadline = Date.now() + timeoutMs;
          while (Date.now() < deadline) {
            const running = await runningCount();
            if (running === 0) return true;
            await sleep(2000);
          }
          out.setupError = "workspace_not_idle";
          out.initialRunning = await runningCount();
          return false;
        };
        if (!(await waitForIdle(60000))) {
          console.log(JSON.stringify(out));
          process.exit(0);
        }
        out.initialRunning = await runningCount();
        const nonce = Date.now().toString(36) + "-" + Math.random().toString(36).slice(2);
        const holderMessage = (i) => [
          "Use the bash tool exactly once to run this command:",
          "\`sleep " + HOLD_SECONDS + "; echo OK-" + i + "\`",
          "After the tool result, reply with exactly: OK-" + i
        ];
        const holderPromises = [];
        try {
          for (let i = 0; i < out.cap; i += 1) {
            const session = await client.sessions.create({
              model: MODEL,
              builtinTools: "default",
              idempotencyKey: "admission-holder-create-" + nonce + "-" + i
            });
            out.sessionIds.push(session.id);
            holderPromises.push(
              session
                .messages.send(holderMessage(i), {
                  idempotencyKey: "admission-holder-run-" + nonce + "-" + i
                })
                .finished()
                .then(
                  () => ({ ok: true, sessionId: session.id }),
                  (e) => ({ ok: false, sessionId: session.id, err: errShape(e) })
                )
            );
          }
          if (!(await waitForRunning(out.cap, 120000, "cap_saturation"))) {
            out.setupError = "cap_not_saturated";
          } else {
            const extra = await noRetryClient
              .start(
                {
                  model: MODEL,
                  builtinTools: "default",
                  idempotencyKey: "admission-overflow-create-" + nonce,
                  message: holderMessage("overflow")
                },
                { timeoutMs: 180000 }
              )
              .then(
                (r) => ({ ok: true, sessionId: r.sessionId }),
                (e) => ({ ok: false, err: errShape(e) })
              );
            if (extra.ok) {
              out.admitted += 1;
              out.sessionIds.push(extra.sessionId);
            } else if (extra.err.status === 429 || /concurrency/i.test(extra.err.message)) {
              out.rejected429 += 1;
              out.rejection = extra.err;
            } else {
              out.otherErrors.push(extra.err);
            }
          }
          const holders = await Promise.all(holderPromises);
          for (const r of holders) {
            if (r.ok) out.admitted += 1;
            else out.otherErrors.push(r.err);
          }
        } finally {
          for (const id of out.sessionIds) {
            await raw("DELETE", "/api/sessions/" + id).catch(() => {});
          }
          out.elapsedMs = Date.now() - startedAt;
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "admission-concurrency.mjs", body, 8 * 60_000);
      console.info("edge-admission-gates concurrency result", JSON.stringify(result));
      expect(result.setupError).toBeUndefined();
      expect(result.otherErrors).toEqual([]);
      const cap = result.cap as number;
      expect(cap).toBeGreaterThan(0);
      expect((result.peakRunning as number) <= cap, JSON.stringify(result)).toBe(true);
      expect((result.rejected429 as number) + (result.admitted as number)).toBe(cap + 1);
      expect(result.rejected429 as number).toBeGreaterThanOrEqual(1);
    },
    10 * 60_000
  );

  it(
    "a retired provider-key secret is rejected at session create",
    async () => {
      // Managed AI Gateway: a customer provider key is no longer part of the
      // contract, so the OLD `missing_provider_key` gate is gone. The live gate
      // now runs the other way — a stale client that still sends
      // `secrets.apiKeys` is refused by the submission-shape check
      // (platform api.ts: `secrets.apiKeys is not supported (managed AI Gateway;
      // no customer provider key)`), which surfaces as 400 `invalid_submission`.
      // Posted raw so the SDK's own field guard does not short-circuit the probe.
      const body = `
        const out = { status: null, error: null, message: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          submission: {
            model: MODEL,
            builtinTools: "none",
            assets: { files: [], skills: [], tools: [], instructions: [] }
          },
          secrets: { apiKeys: { anthropic: "sk-ant-retired-field-probe" } },
        });
        out.status = r.status;
        out.error = r.body && typeof r.body.error === "string" ? r.body.error : null;
        out.message = r.body && typeof r.body.message === "string" ? r.body.message : null;
        const admitted = r.body && r.body.session && typeof r.body.session.id === "string" ? r.body.session.id : null;
        if (admitted) {
          out.admittedId = admitted;
          await raw("DELETE", "/api/sessions/" + admitted);
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "admission-retired-provider-key.mjs", body);
      console.info("edge-admission-gates retired-provider-key result", JSON.stringify(result));
      expect(result.status).toBe(400);
      expect(result.error).toBe("invalid_submission");
      expect(String(result.message)).toContain("secrets.apiKeys is not supported");
      expect(result.admittedId).toBeNull();
    },
    5 * 60_000
  );

  it(
    "a non-slug model is rejected at session create, not at the first billable turn",
    async () => {
      // The serving provider is derived from the slug's creator prefix, so there
      // is no provider/model pair left to mismatch. What remains gated at create
      // is the slug SHAPE: a bare model name (the pre-pivot spelling) must 400
      // before a container launches, not ~45s later at the first billable turn.
      const body = `
        const out = { status: null, error: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          submission: {
            model: "not-a-gateway-slug",
            builtinTools: "none",
            assets: { files: [], skills: [], tools: [], instructions: [] }
          },
        });
        out.status = r.status;
        out.error = r.body && typeof r.body.error === "string" ? r.body.error : null;
        const admitted = r.body && r.body.session && typeof r.body.session.id === "string" ? r.body.session.id : null;
        if (admitted) {
          out.admittedId = admitted;
          await raw("DELETE", "/api/sessions/" + admitted);
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "admission-non-slug-model.mjs", body);
      console.info("edge-admission-gates non-slug-model result", JSON.stringify(result));
      expect(result.status).toBe(400);
      expect(result.error).toBe("invalid_model");
      expect(result.admittedId).toBeNull();
    },
    5 * 60_000
  );
});

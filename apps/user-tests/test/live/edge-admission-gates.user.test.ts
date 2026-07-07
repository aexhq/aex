/**
 * Live edge-case sweep: session-path admission gates.
 *
 * Release-gating probes for session-path admission gates. These assert the
 * public SDK path sees the same hosted admission contract as one-shot submits:
 *
 *   1. Concurrency: `whoami().limits.maxConcurrentRuns` is enforced for
 *      `client.run(...)` / session turns with a public 429
 *      `workspace_concurrency_exceeded` once the cap is saturated.
 *   2. A whitespace-only provider key is rejected at session create.
 *   3. A provider/model mismatch is rejected at session create, before any
 *      billable turn launches.
 *
 * Billing: probes 2 and 3 create born-empty idle sessions and delete them
 * without a turn (zero billable). Probe 1 sends cap+1 tiny turns (~$0.003
 * at deepseek rates) — it cannot observe concurrent running states without
 * running concurrently.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 *
 * Optional env: AEX_ADMISSION_GATES_MAX_SAFE_CAP limits the concurrency probe
 * to low-cap isolated workspaces (default 10). The test fails before launching
 * runs when the target workspace cap is higher.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-admission-gates): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-admission-gates");
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
  const PROVIDER = process.env.PROVIDER;
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
  const MAX_SAFE_CAP = Number(process.env.AEX_ADMISSION_GATES_MAX_SAFE_CAP ?? "10");
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
    "ECONNREFUSED",
    "EAI_AGAIN",
    "ETIMEDOUT",
    "UND_ERR_CONNECT_TIMEOUT"
  ]);
  const rawErrorCode = (e) => {
    if (e && typeof e.code === "string") return e.code;
    if (e && e.cause && typeof e.cause.code === "string") return e.cause.code;
    const message = e && e.message ? String(e.message) : String(e);
    const match = /\\b(ConnectionRefused|E[A-Z0-9_]+|UND_ERR_[A-Z0-9_]+)\\b/.exec(message);
    return match ? match[1] : null;
  };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const raw = async (method, path, body) => {
    const url = process.env.AEX_API_URL + path;
    const maxAttempts = 3;
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
      PROVIDER: GATE_PROVIDER,
      PROVIDER_KEY: providerKey,
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
    "the plan concurrency cap rejects turns beyond maxConcurrentRuns",
    async () => {
      const body = `
        const out = { cap: null, admitted: 0, rejected429: 0, otherErrors: [], peakRunning: 0, initialRunning: 0, runIds: [] };
        const me = await client.whoami();
        out.cap = me.limits.maxConcurrentRuns;
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
        const n = out.cap + 1;
        try {
          const initialPage = await client.sessions.list({ limit: 50 });
          out.initialRunning = initialPage.sessions.filter((s) => s.status === "running").length;
          out.peakRunning = Math.max(out.peakRunning, out.initialRunning);
        } catch {}
        const done = { flag: false };
        const poller = (async () => {
          while (!done.flag) {
            await new Promise((r) => setTimeout(r, 5000));
            try {
              const page = await client.sessions.list({ limit: 50 });
              const running = page.sessions.filter((s) => s.status === "running").length;
              out.peakRunning = Math.max(out.peakRunning, running);
            } catch {}
          }
        })();
        const results = await Promise.all(
          Array.from({ length: n }, (_, i) =>
            client
              .run(
                {
                  provider: PROVIDER,
                  model: MODEL,
                  includeBuiltinTools: false,
                  apiKeys: { [PROVIDER]: PROVIDER_KEY },
                  message: "Reply with exactly: OK-" + i
                },
                { timeoutMs: 240000 }
              )
              .then(
                (r) => ({ ok: true, runId: r.runId }),
                (e) => ({ ok: false, err: errShape(e) })
              )
          )
        );
        done.flag = true;
        await poller;
        for (const r of results) {
          if (r.ok) {
            out.admitted += 1;
            out.runIds.push(r.runId);
          } else if (r.err.status === 429 || /concurrency/i.test(r.err.message)) {
            out.rejected429 += 1;
          } else {
            out.otherErrors.push(r.err);
          }
        }
        for (const id of out.runIds) {
          await raw("DELETE", "/api/sessions/" + id).catch(() => {});
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
    "a whitespace-only provider key is rejected at session create",
    async () => {
      const body = `
        const out = { status: null, error: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          provider: PROVIDER,
          submission: { model: MODEL, includeBuiltinTools: false },
          secrets: { apiKeys: { [PROVIDER]: "   " } }
        });
        out.status = r.status;
        out.error = r.body && typeof r.body.error === "string" ? r.body.error : null;
        const admitted = r.body && (r.body.session?.id ?? r.body.id ?? r.body.runId);
        if (admitted) {
          out.admittedId = admitted;
          await raw("DELETE", "/api/sessions/" + admitted);
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "admission-whitespace-key.mjs", body);
      console.info("edge-admission-gates whitespace-key result", JSON.stringify(result));
      expect(result.status).toBe(400);
      expect(result.error).toBe("missing_provider_key");
    },
    5 * 60_000
  );

  it(
    "a provider/model mismatch is rejected at session create, not at the first billable turn",
    async () => {
      const body = `
        const out = { status: null, error: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          provider: "anthropic",
          submission: { model: MODEL, includeBuiltinTools: false },
          secrets: { apiKeys: { anthropic: "sk-ant-probe-invalid-000000000000" } }
        });
        out.status = r.status;
        out.error = r.body && typeof r.body.error === "string" ? r.body.error : null;
        const admitted = r.body && (r.body.session?.id ?? r.body.id ?? r.body.runId);
        if (admitted) {
          out.admittedId = admitted;
          await raw("DELETE", "/api/sessions/" + admitted);
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "admission-provider-mismatch.mjs", body);
      console.info("edge-admission-gates provider-mismatch result", JSON.stringify(result));
      expect(result.status).toBe(400);
      expect(["invalid_submission", "invalid_model"]).toContain(result.error);
    },
    5 * 60_000
  );
});

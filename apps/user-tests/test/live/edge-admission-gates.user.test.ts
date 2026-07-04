/**
 * Live edge-case sweep: session-path admission gates.
 *
 * DEFECT PROBE — on the dev plane the workspace admission gates
 * (concurrency cap, and strict BYOK/provider validation at create) are only
 * wired on the legacy `POST /api/runs` submit path. The session surface —
 * the ONLY path the SDK uses — skips them:
 *
 *   1. Concurrency: `whoami().limits.maxConcurrentRuns` (plan cap) is never
 *      consulted by `POST /sessions` or `POST /sessions/:id/messages`; N >
 *      cap turns all run simultaneously with zero 429
 *      (`workspace_concurrency_exceeded` is unreachable from the SDK path).
 *   2. A whitespace-only provider key passes the `missing_provider_key`
 *      gate (`sub.apiKey === ""` presence check only) and is admitted.
 *   3. A provider/model mismatch (provider that does not serve the model)
 *      is admitted at create (201) and only fails at the first BILLABLE
 *      turn (`invalid_submission` at dispatch) — the validation exists but
 *      runs one turn too late.
 *
 * Billing: probes 2 and 3 create born-empty idle sessions and delete them
 * without a turn (zero billable). Probe 1 sends cap+1 tiny turns (~$0.003
 * at deepseek rates) — it cannot observe concurrent running states without
 * running concurrently.
 *
 * Required env: AEX_API_URL, AEX_API_TOKEN, DEEPSEEK_API_KEY, +
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
const apiToken = requireEnv("AEX_API_TOKEN");
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
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
  const PROVIDER = process.env.PROVIDER;
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
  const MAX_SAFE_CAP = Number(process.env.AEX_ADMISSION_GATES_MAX_SAFE_CAP ?? "10");
  const errShape = (e) => ({
    name: e && e.constructor ? e.constructor.name : "Error",
    message: e && e.message ? String(e.message).slice(0, 300) : String(e),
    status: e && typeof e.status === "number" ? e.status : null,
    code: e && typeof e.code === "string" ? e.code : null
  });
  const raw = async (method, path, body) => {
    const res = await fetch(process.env.AEX_API_URL + path, {
      method,
      headers: { authorization: "Bearer " + process.env.AEX_API_TOKEN, "content-type": "application/json" },
      body: body ? JSON.stringify(body) : undefined
    });
    let parsed = null;
    try { parsed = await res.json(); } catch {}
    return { status: res.status, body: parsed };
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
      AEX_API_TOKEN: apiToken,
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
        const out = { cap: null, admitted: 0, rejected429: 0, otherErrors: [], peakRunning: 0, runIds: [] };
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
      expect(result.setupError).toBeUndefined();
      expect(result.otherErrors).toEqual([]);
      const cap = result.cap as number;
      expect(cap).toBeGreaterThan(0);
      // DEFECT (dev): all cap+1 turns are admitted and run simultaneously
      // (peakRunning > cap, rejected429 === 0) — the session path never
      // consults the concurrency gate.
      expect((result.peakRunning as number) <= cap).toBe(true);
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
      // DEFECT (dev): 201 — the missing_provider_key gate is a bare === ""
      // check, so a whitespace key is admitted and only fails (billed) at
      // the first turn as a provider 401.
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
      // DEFECT (dev): 201 — validateRunModelProvider only runs on the
      // in-process child submit path; the session create admits the
      // mismatch and the customer discovers it via a billed turn that
      // errors `invalid_submission`.
      expect(result.status).toBe(400);
      expect(["invalid_submission", "invalid_model"]).toContain(result.error);
    },
    5 * 60_000
  );
});

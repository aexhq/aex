/**
 * Live edge-case sweep: public type-contract gaps found by the iter-8
 * SDK-shape audit (2026-07-04).
 *
 * REGRESSION PROBES — fixed by the platform after the 2026-07-04 dev sweep:
 *
 *   1. Token usage is unavailable on EVERY public surface. `SessionResult.usage`
 *      / `Session.usage` document aggregate token counts "when
 *      the deployment exposes it", but the managed plane emits no `aex.usage`
 *      events, never populates record `usage`, and the served
 *      `costTelemetry` (GET /api/sessions/:id) carries no `providerUsage` block —
 *      a customer cannot see input/output token counts for any run.
 *   2. Prompt size is unbounded at session create: a multi-MiB prompt is
 *      admitted (201) with no server-side cap (same family as the unbounded
 *      instructions finding — the only ceiling is the API gateway body limit).
 *
 * Billing: probe 1 runs ONE tiny billable turn (~$0.0004); probe 2 creates a
 * born-empty idle session and deletes it (zero billable).
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { gateModel } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-type-contract): required env ${name} is missing.`);
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
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
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
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-type-contract runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-type-contract runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: public type-contract gaps", () => {
  it(
    "token usage is exposed on at least one public surface for a finished run",
    async () => {
      const body = `
        const out = { sessionId: null, resultUsage: null, sessionUsage: null, providerUsage: null, usageEvents: 0 };
        const result = await client.start(
          {
            model: MODEL,
            builtinTools: "none",
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            message: "Reply with exactly: USAGE-PROBE-OK"
          },
          { timeoutMs: 240000 }
        );
        out.sessionId = result.sessionId;
        out.resultUsage = result.usage ?? null;
        out.usageEvents = result.events.filter(
          (e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.usage"
        ).length;
        const record = await client.sessions.get(result.sessionId);
        out.sessionUsage = record.usage ?? null;
        const unit = await raw("GET", "/api/sessions/" + result.sessionId);
        out.providerUsage = unit.body && unit.body.session && unit.body.session.costTelemetry && unit.body.session.costTelemetry.providerUsage
          ? unit.body.session.costTelemetry.providerUsage
          : null;
        await raw("DELETE", "/api/sessions/" + result.sessionId);
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "type-contract-usage.mjs", body, 6 * 60_000);
      const anyUsage =
        result.resultUsage !== null ||
        result.sessionUsage !== null ||
        result.providerUsage !== null ||
        (result.usageEvents as number) > 0;
      const dump = JSON.stringify(result).slice(0, 1200);
      expect(anyUsage, `no public usage surface was populated; diagnostics: ${dump}`).toBe(true);
    },
    8 * 60_000
  );

  it(
    "a multi-MiB prompt is rejected at session create",
    async () => {
      const body = `
        const out = { status: null, error: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          submission: {
            model: MODEL,
            builtinTools: "none",
            assets: { files: [], skills: [], tools: [], instructions: [] },
            prompt: ["x".repeat(2 * 1024 * 1024)]
          },
          secrets: { apiKeys: { [PROVIDER]: "sk-probe-fake-key" } }
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
      const result = await runChild(install, "type-contract-prompt-cap.mjs", body);
      const dump = JSON.stringify(result).slice(0, 1200);
      expect([400, 413], `oversized prompt was not rejected; diagnostics: ${dump}`).toContain(Number(result.status));
    },
    5 * 60_000
  );
});

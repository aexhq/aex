/**
 * Live edge-case sweep: SDK `runtime` must be honored, validated, and visible.
 *
 * DEFECT PROBE — the public contract offers five runtime-size presets
 * (packages/contracts/src/runtime-sizes.ts): 0.25cpu-1gb, 0.5cpu-4gb,
 * 1cpu-6gb, 2cpu-8gb, 4cpu-12gb. If the platform's token list knows only a
 * subset + internal tier names (lite/standard/standard-2/standard-4), the
 * other public sizes silently fall back to the 0.25 vCPU / 1 GB default task
 * definition — a customer asking for a 4-vCPU/12 GB box gets the smallest
 * box with no error and no visible signal. Two observable defects:
 *   1. The SDK session record must expose the requested size as typed `runtime`.
 *   2. The server accepts a GARBAGE `runtimeSize` (raw wire, 201) instead of
 *      rejecting it — only the SDK's client-side validation catches typos.
 *
 * ZERO billable turns: both probes create born-empty idle sessions and
 * delete them without sending a turn.
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
    throw new Error(`user-tests live (edge-runtime-size): required env ${name} is missing.`);
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
  const errShape = (e) => ({
    name: e && e.constructor ? e.constructor.name : "Error",
    message: e && e.message ? String(e.message).slice(0, 300) : String(e),
    status: e && typeof e.status === "number" ? e.status : null,
    code: e && typeof e.code === "string" ? e.code : null
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
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-runtime-size runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-runtime-size runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: runtime honored, validated, and visible", () => {
  it(
    "the SDK session record echoes the requested public runtime",
    async () => {
      const body = `
        const out = { created: null, recordRuntime: null, leakedRuntimeSize: false, rawRuntimeSize: null, error: null };
        try {
          const session = await client.sessions.create({
            model: MODEL,
            builtinTools: "none",
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            runtime: { size: "1cpu-6gb" }
          });
          out.created = session.id;
          const rec = (await client.sessions.open(session.id)).record;
          out.recordRuntime = rec.runtime ?? null;
          out.leakedRuntimeSize = Object.prototype.hasOwnProperty.call(rec, "runtimeSize");
          const rawRec = await raw("GET", "/api/sessions/" + session.id);
          out.rawRuntimeSize = rawRec.body && rawRec.body.session && rawRec.body.session.runtimeSize !== undefined
            ? rawRec.body.session.runtimeSize
            : null;
          const h = await client.sessions.open(session.id);
          await h.delete().catch(() => {});
        } catch (e) {
          // Post-finish session reads must fail the child loudly; the shape
          // still ships as evidence (the parent asserts result.error is null).
          out.error = errShape(e);
          process.exitCode = 1;
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "runtime-size-echo.mjs", body);
      expect(result.error).toBeNull();
      expect(result.created).toBeTruthy();
      expect(result.recordRuntime).toEqual({ kind: "container", size: "1cpu-6gb" });
      expect(result.leakedRuntimeSize).toBe(false);
      expect(result.rawRuntimeSize).toBe("1cpu-6gb");
    },
    5 * 60_000
  );

  it(
    "the server rejects an unknown runtimeSize on the raw wire",
    async () => {
      const body = `
        const out = { status: null, error: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          runtimeSize: "shared-99x-1tb",
          submission: {
            model: MODEL,
            builtinTools: "none",
            assets: { files: [], skills: [], tools: [], instructions: [] }
          },
          secrets: { apiKeys: { [PROVIDER]: PROVIDER_KEY } }
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
      const result = await runChild(install, "runtime-size-invalid.mjs", body);
      // DEFECT (dev): returns 201 and admits the session — unknown sizes
      // silently fall back to the default 0.25x/1gb task definition.
      expect(result.status).toBeGreaterThanOrEqual(400);
      expect(result.status).toBeLessThan(500);
    },
    5 * 60_000
  );
});

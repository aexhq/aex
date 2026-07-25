/**
 * Live edge-case sweep: idempotencyKey BODY-MISMATCH semantics.
 *
 * The public contract (apps/docs/content/docs/concepts/sessions.md) promises:
 *   "aex hashes the normalized non-secret submission, so a retry with the
 *    same key and same body returns the existing session while a mismatched
 *    body fails with an idempotency conflict."
 *
 * The existing lifecycle tests cover the SAME-body replay half (same sessionId,
 * no dup billable session turn). This covers the mismatched-body half: a same-key retry
 * with a different message, model, or runtimeSize must return 409
 * rather than silently replaying the wrong run.
 *
 * ONE billable session turn total (tiny prompt); the mismatch probes replay/conflict
 * against that session and never start a second billable turn.
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
    throw new Error(`user-tests live (edge-idempotency): required env ${name} is missing.`);
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
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;

  // Instrument fetch so the parent can assert on the RAW HTTP statuses the
  // plane returned for each createSession POST (201 create / 200 replay / 409
  // conflict), independent of how the SDK surfaces them.
  const httpLog = [];
  const instrumentedFetch = async (url, init) => {
    const resp = await fetch(url, init);
    const u = String(url);
    if (init && init.method === "POST" && /\\/api\\/sessions$/.test(u.replace(/\\?.*$/, ""))) {
      httpLog.push(resp.status);
    }
    return resp;
  };
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY, fetch: instrumentedFetch });

  const errShape = (e) => ({
    name: e && e.constructor ? e.constructor.name : "Error",
    message: e && e.message ? String(e.message).slice(0, 400) : String(e),
    status: e && typeof e.status === "number" ? e.status : null,
    code: e && typeof e.code === "string" ? e.code : null
  });
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 8 * 60_000
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
      `edge-idempotency runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-idempotency runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

interface MismatchResult {
  readonly errored: boolean;
  readonly error: { name: string; message: string; status: number | null; code: string | null } | null;
  readonly sessionId: string | null;
  readonly sameRun: boolean;
}

describe("edge: idempotencyKey body-mismatch is a conflict, not a silent replay", () => {
  it(
    "same key + same body replays; same key + different message/model/runtimeSize must CONFLICT (409), never silently replay the wrong run",
    async () => {
      const body = `
        const KEY = "edge-idem-" + Date.now() + "-" + Math.random().toString(36).slice(2, 8);
        const base = {
          model: MODEL,
          message: "Reply with the single word done. Do not use any tools.",
          builtinTools: "none",
          idempotencyKey: KEY
        };

        // 1. the ONE billable session turn
        const first = await client.start(base, { timeoutMs: 5 * 60_000 });

        // 2. same key + same body: contract says replay of the SAME run.
        let replay;
        try {
          const r = await client.start(base, { timeoutMs: 5 * 60_000 });
          replay = { errored: false, error: null, sessionId: r.sessionId, sameRun: r.sessionId === first.sessionId };
        } catch (e) {
          replay = { errored: true, error: errShape(e), sessionId: null, sameRun: false };
        }

        // 3-5. same key + MISMATCHED body: contract says idempotency conflict.
        async function mismatch(overrides) {
          try {
            const r = await client.start({ ...base, ...overrides }, { timeoutMs: 5 * 60_000 });
            return { errored: false, error: null, sessionId: r.sessionId, sameRun: r.sessionId === first.sessionId };
          } catch (e) {
            return { errored: true, error: errShape(e), sessionId: null, sameRun: false };
          }
        }
        const diffMessage = await mismatch({ message: "Reply with the single word OTHER. Do not use any tools." });
        const alternateModel = MODEL === "deepseek-v4-flash" ? "deepseek-v4-pro" : "deepseek-v4-flash";
        const diffModel = await mismatch({ model: alternateModel });
        const diffRuntime = await mismatch({ runtime: { size: "0.25cpu-1gb" } });

        process.stdout.write(JSON.stringify({
          firstSessionId: first.sessionId,
          firstOk: first.ok,
          replay,
          diffMessage,
          diffModel,
          diffRuntime,
          httpLog
        }));
        process.exit(0);
      `;
      const out = (await runChild(install, "edge-idem-A.mjs", body, 9 * 60_000)) as {
        firstSessionId: string;
        firstOk: boolean;
        replay: MismatchResult;
        diffMessage: MismatchResult;
        diffModel: MismatchResult;
        diffRuntime: MismatchResult;
        httpLog: number[];
      };

      expect(typeof out.firstSessionId).toBe("string");
      expect(out.firstSessionId.length).toBeGreaterThan(0);

      // Same key + same body: replay of the SAME run, no error, and the raw
      // createSession POST for it must not be another 201 (no second create).
      expect(out.replay.errored, `same-body replay errored: ${JSON.stringify(out.replay.error)}`).toBe(false);
      expect(
        out.replay.sameRun,
        `same-body replay returned a DIFFERENT run (${out.replay.sessionId} vs ${out.firstSessionId}) — duplicate billable submit`
      ).toBe(true);
      const creates = out.httpLog.filter((s) => s === 201).length;
      expect(creates, `expected exactly one 201 create, raw createSession statuses: ${JSON.stringify(out.httpLog)}`).toBe(1);

      // The docs promise "a mismatched body fails with an idempotency conflict".
      for (const [label, probe] of [
        ["different message", out.diffMessage],
        ["different model", out.diffModel],
        ["different runtimeSize", out.diffRuntime]
      ] as const) {
        expect(
          probe.errored,
          `same idempotencyKey + ${label} did NOT conflict — server silently replayed run ` +
            `${probe.sessionId ?? "?"} (sameRun=${probe.sameRun}); contract requires an idempotency conflict`
        ).toBe(true);
        const status = probe.error?.status ?? 0;
        expect(
          status,
          `same idempotencyKey + ${label} should surface the 409 idempotency conflict, got: ${JSON.stringify(probe.error)}`
        ).toBe(409);
      }
    },
    12 * 60_000
  );
});

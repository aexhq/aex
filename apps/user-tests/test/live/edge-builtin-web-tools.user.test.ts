/**
 * Live edge-case sweep: built-in web tools (`web_search`, `web_fetch`).
 *
 * DEFECT PROBE — two defects observed on the dev plane (2026-07-04):
 *
 *   1. `web_search` is dead on every plane: the platform-side SERPER key is
 *      intentionally empty (infra/terraform/modules/region/ecs.tf — populating
 *      it in the brain env would be a Class-B secret leak via /proc environ;
 *      the fix path is server-side injection at the byok-inject egress proxy).
 *      Yet the docs advertise `web_search` as a default builtin. The agent sees
 *      an opaque tool error "web_search → Serper HTTP 403". This probe asserts
 *      the tool RESULT succeeds — red until platform web_search is enabled.
 *
 *   2. `web_fetch` to an SSRF-blocked target (link-local metadata IP) is
 *      correctly denied and the run survives (tool-level error, not a run
 *      crash) — but the denial surfaces as a bare "HTTP 407" (Smokescreen
 *      CONNECT deny has no typed body; container-runtime aws-entry.ts
 *      installWebEgressInterceptor dispatches through the proxy with no denial
 *      classification), not the "blocked by egress policy" wording the tool's
 *      own typed-denial path (agent-tool-web-fetch parseEgressDenial) was
 *      built to produce. The run-survival half is a REGRESSION GUARD (green);
 *      the message-quality half is the defect (red).
 *
 * Cost: two tiny billable turns.
 *
 * Required env: AEX_API_URL, AEX_API_TOKEN, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-builtin-web-tools): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const providerKey = requireGateKey("edge-builtin-web-tools");
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
  const toolResults = (events, name) =>
    events
      .filter((e) => e.type === "TOOL_CALL_RESULT")
      .map((e) => e.data ?? {})
      .filter((d) => JSON.stringify(d).includes(name) || true);
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 6 * 60_000
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
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-builtin-web-tools runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-builtin-web-tools runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: built-in web tools work and fail honestly", () => {
  it(
    "web_search returns a successful tool result (DEFECT: platform SERPER backend disabled — Serper HTTP 403)",
    async () => {
      const body = `
        const out = { status: null, text: null, searchResult: null, error: null };
        let runId = null;
        try {
          const r = await client.run({
            model: MODEL,
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            message: "Use the web_search tool to search for: Eiffel Tower height meters. If the tool errors, reply 'SEARCH-ERR: ' plus the error text; otherwise reply 'SEARCH-OK'.",
          }, { timeoutMs: 240000 });
          runId = r.runId;
          out.status = r.status;
          out.text = (r.text ?? "").slice(0, 200);
          const results = r.events.filter((e) => e.type === "TOOL_CALL_RESULT");
          out.searchResult = results.length > 0 ? { isError: results[0].data?.isError === true, text: JSON.stringify(results[0].data?.content ?? []).slice(0, 300) } : null;
        } catch (e) {
          out.error = String(e).slice(0, 400);
        } finally {
          if (runId) { try { await client.sessions.delete(runId); } catch {} }
        }
        console.log(JSON.stringify(out));
      `;
      const out = await runChild(install, "web-search-probe.mjs", body);
      expect(out.error).toBeNull();
      const searchResult = out.searchResult as { isError: boolean; text: string } | null;
      expect(searchResult).not.toBeNull();
      // DEFECT (red on dev): the platform search backend is disabled, so the
      // tool result is an error ("web_search → Serper HTTP 403").
      expect(searchResult!.isError).toBe(false);
    },
    8 * 60_000
  );

  it(
    "web_fetch to an SSRF-blocked host is a tool-level denial that names the egress policy (run must survive)",
    async () => {
      const body = `
        const out = { status: null, text: null, fetchResult: null, error: null };
        let runId = null;
        try {
          const r = await client.run({
            model: MODEL,
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            message: "Use the web_fetch tool to fetch http://169.254.169.254/latest/meta-data/ exactly once. Reply 'FETCH-ERR: ' plus the tool's error text, or 'FETCH-OK' if it worked. Do not retry.",
          }, { timeoutMs: 240000 });
          runId = r.runId;
          out.status = r.status;
          out.text = (r.text ?? "").slice(0, 200);
          const results = r.events.filter((e) => e.type === "TOOL_CALL_RESULT");
          out.fetchResult = results.length > 0 ? { isError: results[0].data?.isError === true, text: JSON.stringify(results[0].data?.content ?? []).slice(0, 300) } : null;
        } catch (e) {
          out.error = String(e).slice(0, 400);
        } finally {
          if (runId) { try { await client.sessions.delete(runId); } catch {} }
        }
        console.log(JSON.stringify(out));
      `;
      const out = await runChild(install, "web-fetch-ssrf-probe.mjs", body);
      // REGRESSION GUARD (green today): the run survives — SSRF denial is a
      // tool-level error, never a run-level crash.
      expect(out.error).toBeNull();
      expect(out.status).toBe("idle");
      const fetchResult = out.fetchResult as { isError: boolean; text: string } | null;
      expect(fetchResult).not.toBeNull();
      expect(fetchResult!.isError).toBe(true);
      // DEFECT (red on dev): the denial reads "web_fetch http://169.254.169.254/…
      // → HTTP 407" — a bare proxy status instead of the typed "blocked by
      // egress policy" wording the tool was built to surface.
      expect(fetchResult!.text).toMatch(/blocked by egress policy/i);
    },
    8 * 60_000
  );
});

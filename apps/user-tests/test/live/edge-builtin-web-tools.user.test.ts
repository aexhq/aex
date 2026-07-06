/**
 * Live edge-case sweep: built-in web tools (`web_search`, `web_fetch`).
 *
 * REGRESSION PROBES — two defects fixed after the dev sweep (2026-07-04):
 *
 *   1. `web_search` must work without placing the platform Serper key in the
 *      brain task env. The key is injected at the managed byok-inject egress
 *      boundary, and this probe asserts the tool RESULT succeeds.
 *
 *   2. `web_fetch` to an SSRF-blocked target (link-local metadata IP) must be a
 *      tool-level denial, not a run crash, and the denial must name the egress
 *      policy instead of surfacing a bare proxy status.
 *
 * Cost: two tiny billable turns.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
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
const apiKey = requireEnv("AEX_API_KEY");
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
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
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
      AEX_API_KEY: apiKey,
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
    "web_search returns a successful tool result",
    async () => {
      const body = `
        const out = { runId: null, status: null, text: null, searchResult: null, error: null };
        let runId = null;
        try {
          const r = await client.run({
            model: MODEL,
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            message: "Use the web_search tool to search for: Eiffel Tower height meters. If the tool errors, reply 'SEARCH-ERR: ' plus the error text; otherwise reply 'SEARCH-OK'.",
          }, { timeoutMs: 240000 });
          runId = r.runId;
          out.runId = runId;
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
      const dump = JSON.stringify(out).slice(0, 1200);
      expect(out.error, `web_search run threw before returning diagnostics: ${dump}`).toBeNull();
      const searchResult = out.searchResult as { isError: boolean; text: string } | null;
      expect(searchResult, `web_search did not emit a tool result: ${dump}`).not.toBeNull();
      expect(searchResult!.isError, `web_search returned a tool error: ${dump}`).toBe(false);
    },
    8 * 60_000
  );

  it(
    "web_fetch to an SSRF-blocked host is a tool-level denial that names the egress policy (run must survive)",
    async () => {
      const body = `
        const out = { runId: null, status: null, text: null, fetchResult: null, error: null };
        let runId = null;
        try {
          const r = await client.run({
            model: MODEL,
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            message: "Use the web_fetch tool to fetch http://169.254.169.254/latest/meta-data/ exactly once. Reply 'FETCH-ERR: ' plus the tool's error text, or 'FETCH-OK' if it worked. Do not retry.",
          }, { timeoutMs: 240000 });
          runId = r.runId;
          out.runId = runId;
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
      const dump = JSON.stringify(out).slice(0, 1200);
      expect(out.error, `web_fetch run threw before returning diagnostics: ${dump}`).toBeNull();
      expect(["idle", "succeeded"], `web_fetch run did not survive cleanly: ${dump}`).toContain(out.status);
      const fetchResult = out.fetchResult as { isError: boolean; text: string } | null;
      expect(fetchResult, `web_fetch did not emit a tool result: ${dump}`).not.toBeNull();
      expect(fetchResult!.isError, `web_fetch unexpectedly succeeded for a private target: ${dump}`).toBe(true);
      expect(fetchResult!.text, `web_fetch denial text was not policy-classified: ${dump}`).toMatch(
        /blocked by egress policy/i
      );
    },
    8 * 60_000
  );
});

/**
 * Live edge-case sweep: subagent mid-tree failure + inline-MCP host reachability.
 *
 * These assertions cover edge regressions observed on the dev plane
 * (2026-07-04). They do not self-skip: each case either proves the current
 * fixed contract or continues documenting an unfixed defect.
 *
 * -- Finding 1 (FIXED): invalid-model subagent must never zombie -------------
 * The in-process child admit path now validates model/provider before writing a
 * child row. The parent should see a subagent tool error (400 invalid_model) and
 * no queued child run should remain. If a child run id is ever returned, it must
 * reach a terminal status within the poll window.
 *
 * ── Finding 2 (PLATFORM): non-allowlisted inline MCP host → boot_failed ──────
 * `McpServer.remote({url})` advertises arbitrary remote MCP servers, but the
 * per-run egress allowlist (R12, the `aex-{plane}-{region}-egress-allowlist`
 * DDB table) is provisioned + read-wired yet NEVER WRITTEN by the submit Lambda
 * (infra/lambdas/src/api.ts). So the ONLY reachable MCP hosts are the static
 * union baked into the egress ACL (mcp.deepwiki.com, example.com, provider
 * hosts). Any other host — INCLUDING the documented public baseline
 * `mcp.context7.com` — is refused by the egress default-deny (HTTP 407), and
 * because MCP discovery runs at brain boot ("a server that fails discovery
 * throws an honest terminal"), the WHOLE run crashes `boot_failed` before any
 * agent work — one unreachable MCP server kills the entire run.
 * EXPECTED AFTER FIX: a declared MCP host is written to the per-run allowlist so
 * the dial is permitted (or, at minimum, an unreachable MCP server degrades to a
 * tool-level error rather than a boot crash of the whole run).
 *
 * Cost: two tiny deepseek runs (Finding 1 parent + Finding 2 boot-fail run,
 * which invokes no LLM). The Finding-1 parent is cancelled after the probe to
 * release its container promptly.
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
    throw new Error(`user-tests live (edge-subagent-mcp-failmodes): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL").replace(/\/$/, "");
const apiToken = requireEnv("AEX_API_TOKEN");
const providerKey = requireGateKey("edge-subagent-mcp-failmodes");
const model = gateModel();

function buildPassEnv(extra: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extra };
  if (process.platform === "win32") {
    for (const k of ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]) {
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
  import { Aex, McpServer } from "@aexhq/sdk";
  const PROVIDER = process.env.PROVIDER;
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
  const errShape = (e) => ({
    name: e && e.constructor ? e.constructor.name : "Error",
    message: e && e.message ? String(e.message).slice(0, 400) : String(e),
    status: e && typeof e.status === "number" ? e.status : null,
    code: e && typeof e.code === "string" ? e.code : null
  });
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
`;

async function runChild(install: InstallResult, scriptName: string, body: string, timeoutMs = 10 * 60_000): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({ AEX_API_URL: apiUrl, AEX_API_TOKEN: apiToken, PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey, MODEL: model })
  });
  if (child.exitCode !== 0) {
    throw new Error(`edge-subagent-mcp-failmodes runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-subagent-mcp-failmodes runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("live DEV — subagent + MCP failure modes (DEFECT PROBES)", () => {
  it(
    "Finding 1 fixed: a subagent with an invalid model is rejected instead of creating a zombie child",
    async () => {
      const out = await runChild(
        install,
        "probe-subagent-invalid-model.mjs",
        `
        const prompt =
          'You have a tool named "subagent" that delegates a task to a child agent run. ' +
          'Call the subagent tool EXACTLY ONCE with: model set to "totally-invalid-model-zzz9", and ' +
          'prompt set to "say hi". Report the tool result you got, then STOP. Do not retry.';
        // Create first so diagnostics always include the parent run id, even if
        // the turn throws or times out.
        const parent = await client.sessions.create({
          provider: PROVIDER,
          model: MODEL,
          includeBuiltinTools: true,
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          overrides: { maxSpendUsd: 0.10, idleTtl: "3m" }
        });
        const parentRunId = parent.id;

        let sendResult = null, sendThrown = null;
        try {
          const r = await parent.send(prompt, { idleTimeoutMs: 6 * 60_000 }).done();
          sendResult = { status: r.status, text: String(r.text || "").slice(0, 500) };
        } catch (e) {
          sendThrown = errShape(e);
        }

        let childId = null, subResults = [], subStarts = 0;
        if (parentRunId) {
          const h = await client.sessions.open(parentRunId);
          const evs = await h.events().list();
          const subStartIds = new Set(
            evs
              .filter((e) => e.type === "TOOL_CALL_START" && e.data && e.data.name === "subagent")
              .map((e) => e.data && typeof e.data.id === "string" ? e.data.id : null)
              .filter((id) => id !== null)
          );
          subStarts = subStartIds.size;
          subResults = evs
            .filter((e) => e.type === "TOOL_CALL_RESULT" && e.data && typeof e.data.id === "string" && subStartIds.has(e.data.id))
            .map((e) => {
              const c = e.data && e.data.content;
              const t = Array.isArray(c) ? c.map((b) => (b && b.text) || "").join(" ") : typeof c === "string" ? c : "";
              return { isError: e.data && e.data.isError === true, text: t };
            });
          const m = JSON.stringify(subResults).match(/\\brun_[0-9a-f]{32}\\b/i);
          childId = m ? m[0] : null;
        }

        // Poll any returned child for ~2 min. Fixed admit should usually return
        // no child id, but if one appears it must not remain queued forever.
        let childStatus = null, childUpdatedEqualsCreated = null, childTerminal = false;
        if (childId) {
          const terminal = new Set(["failed", "error", "cancelled", "succeeded", "timed_out", "idle", "suspended"]);
          for (let i = 0; i < 24; i++) {
            const c = await client.sessions.get(childId).catch((e) => ({ __err: errShape(e) }));
            if (c && !c.__err) {
              childStatus = c.status;
              childUpdatedEqualsCreated = c.createdAt === c.updatedAt;
              if (terminal.has(c.status)) { childTerminal = true; break; }
            }
            await sleep(5000);
          }
          // Clean up: cancel the zombie child + the parent to release compute.
          await client.sessions.open(childId).then((h) => h.cancel()).catch(() => {});
        }
        if (parentRunId) await client.sessions.open(parentRunId).then((h) => h.cancel()).catch(() => {});

        console.log(JSON.stringify({
          parentRunId,
          childId,
          childStatus,
          childTerminal,
          childUpdatedEqualsCreated,
          subStarts,
          subResults: subResults.slice(0, 4),
          sendResult,
          sendThrown
        }));
        `
      );

      const dump = JSON.stringify(out).slice(0, 1200);
      expect(out.parentRunId, `parent run id was not captured: ${dump}`).toBeTruthy();
      expect(out.subStarts, `model did not call the subagent tool; diagnostics: ${dump}`).toBeGreaterThan(0);
      const subResults = Array.isArray(out.subResults) ? out.subResults as Array<{ isError?: boolean; text?: string }> : [];
      const joined = JSON.stringify(subResults);
      expect(joined, `subagent tool result did not surface invalid-model admission error: ${dump}`).toMatch(/invalid_model|POST|400|model/i);
      if (out.childId) {
        expect(out.childTerminal, `child run was created but did not terminalize: ${dump}`).toBe(true);
      }
    },
    12 * 60_000
  );

  it(
    "DEFECT PROBE (Finding 2): a non-allowlisted inline MCP host boot-fails the whole run",
    async () => {
      const out = await runChild(
        install,
        "probe-mcp-nonallowlisted-host.mjs",
        `
        // mcp.context7.com is the DOCUMENTED public MCP baseline, but it is NOT in
        // the static egress union and the per-run allowlist is never written, so the
        // egress boundary refuses it (407) and the brain boot throws terminal.
        const r = await client.run(
          { provider: PROVIDER, model: MODEL, message: "List your MCP tools and stop.", includeBuiltinTools: false,
            mcpServers: [McpServer.remote({ name: "probe", url: "https://mcp.context7.com/mcp" })],
            apiKeys: { [PROVIDER]: PROVIDER_KEY }, overrides: { maxSpendUsd: 0.05, idleTtl: "3m" } },
          { timeoutMs: 6 * 60_000 }
        ).catch((e) => ({ __thrown: errShape(e), runId: null }));

        let record = null;
        const runId = r && r.runId ? r.runId : null;
        if (runId) {
          const c = await client.sessions.get(runId).catch(() => null);
          if (c) record = { status: c.status, failureClass: c.failureClass, errorMessage: (c.errorMessage || "").slice(0, 300) };
        }
        console.log(JSON.stringify({ runId, ok: !!(r && r.ok), status: r && r.status, thrown: r && r.__thrown ? r.__thrown.name : null, record }));
        `
      );

      const dump = JSON.stringify(out).slice(0, 1200);
      const record = out.record as { status?: string; failureClass?: string; errorMessage?: string } | null;
      // The run must not succeed on an unreachable MCP host.
      expect(out.ok, `MCP run unexpectedly succeeded — Finding 2 may be FIXED: ${dump}`).not.toBe(true);
      // DEFECT: the failure is a whole-run boot crash (not a tool-level degrade),
      // and the message names the egress default-deny. When the per-run allowlist
      // is written (or MCP failure degrades gracefully), this flips.
      expect(
        record?.failureClass === "boot_failed" || /egress|denied|not allowed|407/i.test(record?.errorMessage ?? ""),
        `expected a boot_failed egress-denied terminal for a non-allowlisted MCP host: ${dump}`
      ).toBe(true);
    },
    10 * 60_000
  );
});

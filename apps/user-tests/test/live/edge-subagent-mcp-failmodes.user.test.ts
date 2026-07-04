/**
 * Live edge-case sweep: subagent mid-tree failure + inline-MCP host reachability.
 *
 * Both assertions are DEFECT PROBES — they encode the CURRENT broken behaviour
 * observed on the dev plane (2026-07-04). They FLIP to the correct expectation
 * once the platform fixes land; until then they document the defect without
 * self-skipping (the live suite never skips).
 *
 * ── Finding 1 (PLATFORM): invalid-model subagent → ZOMBIE child ──────────────
 * The `subagent` tool spawns a CHILD run via the internal in-process admit
 * (host=in-process): the API returns 202 with the child runId and hands the
 * parent a writer-token, but performs NO model validation. When the child model
 * is unknown, the parent container's in-process spawn never produces a terminal
 * for it, so the child row is stuck `submitted`/`queued` FOREVER (never
 * `failed`), `updatedAt == createdAt`, no SFN execution, no brain logs. The
 * parent, meanwhile, reads `{"status":"queued","terminal":false}` from
 * subagent_result on every poll and never learns the child is dead — it either
 * loops on `wait`/`subagent_result` until its own idle timeout or gives up.
 * A normal top-level submit with the same bad model would fail_closed at boot;
 * the lineage/in-process path skips that gate.
 * EXPECTED AFTER FIX: the child reaches a terminal failed status
 * (invalid_submission / boot_failed) within a couple of minutes, AND that
 * failure is visible to the parent via subagent_result (terminal:true).
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
    "DEFECT PROBE (Finding 1): a subagent with an invalid model becomes a zombie child (never terminal)",
    async () => {
      const out = await runChild(
        install,
        "probe-subagent-invalid-model.mjs",
        `
        const prompt =
          'You have a tool named "subagent" that delegates a task to a child agent run. ' +
          'Call the subagent tool EXACTLY ONCE with: model set to "totally-invalid-model-zzz9", and ' +
          'prompt set to "say hi". Report the tool result you got, then STOP. Do not retry.';
        // Bound the parent so the run cannot linger: a short idle TTL + a modest wait budget.
        const r = await client.run(
          { provider: PROVIDER, model: MODEL, message: prompt, includeBuiltinTools: true,
            apiKeys: { [PROVIDER]: PROVIDER_KEY }, overrides: { maxSpendUsd: 0.10, idleTtl: "3m" } },
          { timeoutMs: 6 * 60_000 }
        ).catch((e) => ({ __thrown: errShape(e) }));

        const parentRunId = r && r.runId ? r.runId : null;
        let childId = null, subResults = [];
        if (parentRunId) {
          const h = await client.sessions.open(parentRunId);
          const evs = await h.events().list();
          subResults = evs
            .filter((e) => e.type === "TOOL_CALL_RESULT")
            .map((e) => {
              const c = e.data && e.data.content;
              const t = Array.isArray(c) ? c.map((b) => (b && b.text) || "").join(" ") : typeof c === "string" ? c : "";
              return t;
            });
          const m = JSON.stringify(subResults).match(/\\brun_[0-9a-f]{32}\\b/i);
          childId = m ? m[0] : null;
        }

        // Poll the child for ~2 min: the DEFECT is that it never reaches terminal.
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

        console.log(JSON.stringify({ parentRunId, childId, childStatus, childTerminal, childUpdatedEqualsCreated, subResults: subResults.slice(0, 4) }));
        `
      );

      const dump = JSON.stringify(out).slice(0, 1200);
      // The subagent tool DID spawn a child (the parent got a runId back).
      expect(out.childId, `no child runId spawned — parent flow changed: ${dump}`).toBeTruthy();
      // DEFECT: the child never reaches terminal within the poll window.
      // When the platform validates the child model (or the in-process spawn
      // surfaces the failure), childTerminal flips true and THIS assertion fails —
      // delete the probe and assert the terminal-failure expectation instead.
      expect(out.childTerminal, `child reached terminal — Finding 1 appears FIXED, update this probe: ${dump}`).toBe(false);
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

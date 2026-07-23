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
 * no queued child session should remain. If a child session id is ever returned, it must
 * reach a terminal status within the poll window.
 *
 * -- Finding 2 (FIXED): documented inline MCP host must not hit egress deny ----
 * `McpServer.remote({url})` advertises remote MCP servers. The documented
 * Context7 endpoint (`https://mcp.context7.com/mcp`) used to be absent from the
 * managed egress ceiling, so discovery failed at the boundary with HTTP 407 and
 * the whole run crashed `boot_failed` before any agent work. The regression now
 * asserts that Context7 discovery reaches the server and the session survives.
 *
 * Cost: two tiny deepseek sessions. The Finding-1 parent is cancelled after the
 * probe to release its container promptly.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
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
const apiKey = requireEnv("AEX_API_KEY");
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
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
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
    env: buildPassEnv({ AEX_API_URL: apiUrl, AEX_API_KEY: apiKey, PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey, MODEL: model })
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

describe("live DEV — subagent + MCP failure modes", () => {
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
        // Create first so diagnostics always include the parent session id, even if
        // the turn throws or times out.
        const parent = await client.sessions.create({
          provider: PROVIDER,
          model: MODEL,
          builtinTools: "default",
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          overrides: { maxSpendUsd: 0.10, idleTtl: "3m" }
        });
        const parentSessionId = parent.id;

        let sendResult = null, sendThrown = null;
        try {
          const r = await parent.messages.send(prompt, { idleTimeoutMs: 6 * 60_000 }).finished();
          sendResult = { status: r.status, text: String(r.text || "").slice(0, 500) };
        } catch (e) {
          sendThrown = errShape(e);
        }

        let childId = null, subResults = [], subStarts = 0;
        if (parentSessionId) {
          const h = await client.sessions.open(parentSessionId);
          const evs = await h.events.list();
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
          const m = JSON.stringify(subResults).match(/\\bses_[0-9a-f]{32}\\b/i);
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
        if (parentSessionId) await client.sessions.open(parentSessionId).then((h) => h.cancel()).catch(() => {});

        console.log(JSON.stringify({
          parentSessionId,
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
      expect(out.parentSessionId, `parent session id was not captured: ${dump}`).toBeTruthy();
      expect(out.subStarts, `model did not call the subagent tool; diagnostics: ${dump}`).toBeGreaterThan(0);
      const subResults = Array.isArray(out.subResults) ? out.subResults as Array<{ isError?: boolean; text?: string }> : [];
      const joined = JSON.stringify(subResults);
      expect(joined, `subagent tool result did not surface invalid-model admission error: ${dump}`).toMatch(/invalid_model|POST|400|model/i);
      expect(!out.childId || out.childTerminal, `child session was created but did not terminalize: ${dump}`).toBe(true);
    },
    12 * 60_000
  );

  it(
    "Finding 2 fixed: a documented public inline MCP host is reachable through managed egress",
    async () => {
      const out = await runChild(
        install,
        "probe-mcp-nonallowlisted-host.mjs",
        `
        // mcp.context7.com is a documented public MCP baseline. Discovery must not
        // fail at the managed egress ceiling with a proxy/allowlist denial.
        const out = {
          sessionId: null,
          createThrown: null,
          sendThrown: null,
          sendResult: null,
          record: null,
          recentEvents: []
        };
        let session = null;
        try {
          session = await client.sessions.create(
            { provider: PROVIDER, model: MODEL, builtinTools: "none",
            mcpServers: [McpServer.remote({ name: "probe", url: "https://mcp.context7.com/mcp" })],
            apiKeys: { [PROVIDER]: PROVIDER_KEY }, overrides: { maxSpendUsd: 0.05, idleTtl: "3m" } }
          );
          out.sessionId = session.id;
          try {
            const r = await session.messages.send("List your MCP tools and stop.", { idleTimeoutMs: 6 * 60_000 }).finished();
            out.sendResult = { status: r.status, text: String(r.text || "").slice(0, 300) };
          } catch (e) {
            out.sendThrown = errShape(e);
          }

          const terminal = new Set(["failed", "error", "cancelled", "succeeded", "timed_out", "idle", "suspended"]);
          const deadline = Date.now() + 2 * 60_000;
          while (Date.now() < deadline) {
            const c = await client.sessions.get(session.id).catch(() => null);
            if (c) {
              out.record = { status: c.status, failureClass: c.failureClass, errorMessage: (c.errorMessage || "").slice(0, 300) };
              if (terminal.has(c.status)) break;
            }
            await sleep(5000);
          }
          const h = await client.sessions.open(session.id).catch(() => null);
          if (h) {
            const evs = await h.events.list().catch(() => []);
            out.recentEvents = evs.slice(-5).map((e) => ({
              type: e.type,
              data: e.data ? JSON.stringify(e.data).slice(0, 240) : null
            }));
          }
        } catch (e) {
          // Session create/read failures must fail the child loudly; the shape
          // still ships as evidence (the parent asserts createThrown is null).
          out.createThrown = errShape(e);
          process.exitCode = 1;
        } finally {
          if (session && session.id) {
            await session.delete().catch(() => {});
          }
        }
        console.log(JSON.stringify(out));
        `
      );

      const dump = JSON.stringify(out).slice(0, 1200);
      expect(out.createThrown, `MCP session create failed before a session id was captured: ${dump}`).toBeNull();
      expect(out.sessionId, `MCP session id was not captured: ${dump}`).toBeTruthy();
      const record = out.record as { status?: string; failureClass?: string; errorMessage?: string } | null;
      expect(record?.failureClass, `MCP discovery still failed at boot: ${dump}`).not.toBe("boot_failed");
      expect(record?.errorMessage ?? "", `MCP discovery still hit egress policy: ${dump}`).not.toMatch(
        /egress|denied|not allowed|407/i
      );
      expect((out.sendResult as { status?: string } | null)?.status, `MCP run did not finish cleanly: ${dump}`)
        .toBe("succeeded");
    },
    10 * 60_000
  );
});

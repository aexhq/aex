/**
 * Live edge sweep — SUBAGENT LINEAGE OBSERVABILITY (Wave 1)
 *
 * Customer POV, blackbox against the installed public `@aexhq/sdk` on DEV.
 * A parent DeepSeek run is prompted to delegate a tiny task to the builtin
 * `subagent` tool (spawns a CHILD run). We then probe — using ONLY the public
 * SDK read surface a customer actually has (`Aex` + `client.sessions.*`) —
 * whether the child is observable, and whether its work / cost flows back into
 * the parent.
 *
 * The public 0.36.0 SDK is session-based: there is NO `getRun` / `AgentExecutor`.
 * The only child-observation paths a customer has are:
 *   - the parent's `subagent` TOOL_CALL_RESULT text (carries the child runId),
 *   - `client.sessions.get(childId)` / `client.sessions.open(childId)`,
 *   - `client.sessions.list()`.
 *
 * Hard invariants asserted here (the rest is characterized to a scratchpad JSON
 * for the human sweep): parent succeeds, subagent was actually called, a child
 * runId was returned, and the DeepSeek key never leaks into ANY SDK-visible
 * surface (parent OR child events).
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY,
 * MODEL. Cost-safe: one parent + one tiny child, DeepSeek, tiny prompts.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (lineage): required env ${name} is missing.`);
  }
  return value;
}

function requireAnyEnv(...names: string[]): string {
  for (const name of names) {
    const value = process.env[name];
    if (value && value.length > 0) return value;
  }
  throw new Error(`user-tests live (lineage): required env ${names.join(" or ")} is missing.`);
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireAnyEnv("DEEPSEEK_API_KEY", "DEEPSEEK_KEY");
const model = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

interface Wave1Result {
  readonly parentRunId: string;
  readonly parentStatus: string;
  readonly parentOk: boolean;
  readonly parentCostUsd: number | null;
  readonly parentEventKinds: readonly string[];
  readonly subagentStartCount: number;
  readonly subagentResults: ReadonlyArray<{ readonly isError: boolean; readonly text: string }>;
  readonly childId: string | null;
  readonly parentMarker: string;
  readonly childMarker: string;
  readonly childMarkerInParentEvents: boolean;
  readonly parentOutputCount: number;
  // observability probes on the child id (public read surface)
  readonly childGetResolved: boolean;
  readonly childGetStatus: string | null;
  readonly childGetError: string | null;
  readonly childOpenResolved: boolean;
  readonly childOpenError: string | null;
  readonly childEventKinds: readonly string[];
  readonly childMarkerInChildEvents: boolean;
  readonly childOutputCount: number;
  readonly childInSessionsList: boolean;
  readonly parentInSessionsList: boolean;
  readonly sessionsListCount: number;
  readonly leakedKeyAnywhere: boolean;
}

describe("live DEV — subagent lineage observability (Wave 1)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "parent spawns a subagent; probe whether the child is observable via the public SDK",
    async () => {
      const parentMarker = "PARENT-" + Math.random().toString(36).slice(2, 8).toUpperCase();
      const childMarker = "CHILD-" + Math.random().toString(36).slice(2, 8).toUpperCase();

      const script = `
        import { Aex } from "@aexhq/sdk";

        const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;
        const PARENT_MARKER = process.env.PARENT_MARKER;
        const CHILD_MARKER = process.env.CHILD_MARKER;

        const childInstruction = "Reply with exactly this one line and nothing else: " + CHILD_MARKER;
        const prompt =
          "You have a tool named subagent that delegates a task to a child agent run. " +
          "Call the subagent tool EXACTLY ONCE with these arguments: " +
          'model set to "' + model + '", and prompt set to the text between BEGIN and END, verbatim:\\n' +
          "BEGIN " + childInstruction + " END\\n" +
          "After the tool returns, reply with exactly one line: " + PARENT_MARKER;

        const runResult = await client.run({
          provider: "deepseek",
          model,
          message: prompt,
          includeBuiltinTools: true,
          apiKeys: { deepseek: deepseekKey },
          idempotencyKey: "edge-lineage-w1-" + Date.now()
        }, { timeoutMs: 8 * 60 * 1000 });

        const parentRunId = runResult.runId;
        const parentOk = runResult.ok === true;
        const parentStatus = parentOk ? "succeeded" : (typeof runResult.status === "string" && runResult.status ? runResult.status : "failed");
        const parentCostUsd = typeof runResult.costUsd === "number" ? runResult.costUsd : null;

        // Prefer freshly-listed events (journal forwards async).
        let events = Array.isArray(runResult.events) ? runResult.events : [];
        try {
          const s = await client.sessions.open(parentRunId);
          const listed = await s.events().list();
          if (Array.isArray(listed) && listed.length > 0) events = listed;
        } catch {}

        const parentEventKinds = events.map((e) => e.type);

        function toolText(e) {
          const c = e && e.data ? e.data.content : undefined;
          if (Array.isArray(c)) return c.map((b) => (b && typeof b.text === "string" ? b.text : "")).join(" ");
          if (typeof c === "string") return c;
          return "";
        }
        const subagentStartIds = new Set(
          events.filter((e) => e.type === "TOOL_CALL_START" && e.data && e.data.name === "subagent")
            .map((e) => (e.data && typeof e.data.id === "string" ? e.data.id : null))
            .filter((id) => id !== null)
        );
        const subagentStartCount = subagentStartIds.size;
        const subagentResults = events
          .filter((e) => e.type === "TOOL_CALL_RESULT" && e.data && typeof e.data.id === "string" && subagentStartIds.has(e.data.id))
          .map((e) => ({ isError: e.data.isError === true, text: toolText(e) }));

        let childId = null;
        for (const r of subagentResults) {
          const m = r.text.match(/\\brun_[0-9a-f]{32}\\b/i);
          if (m) { childId = m[0]; break; }
        }

        const parentEventsStr = JSON.stringify(events);
        const childMarkerInParentEvents = parentEventsStr.includes(CHILD_MARKER);

        let parentOutputCount = 0;
        try {
          const s = await client.sessions.open(parentRunId);
          const outs = await s.outputs().list();
          parentOutputCount = Array.isArray(outs) ? outs.length : 0;
        } catch {}

        // ---- child observability probes (public read surface only) ----
        let childGetResolved = false, childGetStatus = null, childGetError = null;
        let childOpenResolved = false, childOpenError = null;
        let childEventKinds = [], childMarkerInChildEvents = false, childOutputCount = 0;
        let childEventsStr = "";
        if (childId) {
          try {
            const rec = await client.sessions.get(childId);
            childGetResolved = true;
            childGetStatus = rec && typeof rec.status === "string" ? rec.status : null;
          } catch (e) { childGetError = e instanceof Error ? e.message : String(e); }
          try {
            const cs = await client.sessions.open(childId);
            childOpenResolved = true;
            const cev = await cs.events().list();
            const cevArr = Array.isArray(cev) ? cev : [];
            childEventKinds = cevArr.map((e) => e.type);
            childEventsStr = JSON.stringify(cevArr);
            childMarkerInChildEvents = childEventsStr.includes(CHILD_MARKER);
            const cout = await cs.outputs().list();
            childOutputCount = Array.isArray(cout) ? cout.length : 0;
          } catch (e) { childOpenError = e instanceof Error ? e.message : String(e); }
        }

        // ---- sessions.list() lineage probe ----
        let childInSessionsList = false, parentInSessionsList = false, sessionsListCount = 0;
        try {
          const page = await client.sessions.list();
          const ids = Array.isArray(page.sessions) ? page.sessions.map((s) => s.sessionId ?? s.id) : [];
          sessionsListCount = ids.length;
          parentInSessionsList = ids.includes(parentRunId);
          if (childId) childInSessionsList = ids.includes(childId);
        } catch {}

        const leakedKeyAnywhere = parentEventsStr.includes(deepseekKey) || childEventsStr.includes(deepseekKey);

        const result = {
          parentRunId, parentStatus, parentOk, parentCostUsd,
          parentEventKinds, subagentStartCount, subagentResults, childId,
          parentMarker: PARENT_MARKER, childMarker: CHILD_MARKER,
          childMarkerInParentEvents, parentOutputCount,
          childGetResolved, childGetStatus, childGetError,
          childOpenResolved, childOpenError, childEventKinds,
          childMarkerInChildEvents, childOutputCount,
          childInSessionsList, parentInSessionsList, sessionsListCount,
          leakedKeyAnywhere
        };
        process.stdout.write(JSON.stringify(result));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "edge-lineage-w1-runner.mjs");
      writeFileSync(scriptPath, script);

      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_KEY: apiKey,
        DEEPSEEK_API_KEY: deepseekKey,
        DEEPSEEK_KEY: deepseekKey,
        MODEL: model,
        PARENT_MARKER: parentMarker,
        CHILD_MARKER: childMarker
      };
      const pathKey = process.platform === "win32" ? "Path" : "PATH";
      if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
      const carry =
        process.platform === "win32"
          ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
          : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
      for (const k of carry) if (process.env[k]) passEnv[k] = process.env[k]!;

      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 9 * 60 * 1000,
        env: passEnv
      });
      if (child.exitCode !== 0) {
        throw new Error(
          `lineage runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
        );
      }

      const result = JSON.parse(child.stdout.trim()) as Wave1Result;
      writeFileSync(join(install.installDir, "lineage-wave1-out.json"), JSON.stringify(result, null, 2));

      const dump = JSON.stringify(result, null, 2);

      // ---- HARD INVARIANTS (unconditional) ----
      // Parent completed cleanly.
      expect(result.parentStatus, dump).toBe("succeeded");
      // The agent actually reached for the subagent tool.
      expect(result.subagentStartCount, dump).toBeGreaterThan(0);
      // The subagent tool returned a child runId (async "started" path).
      expect(result.childId, dump).not.toBeNull();
      // The customer's DeepSeek key MUST NOT appear in ANY SDK-visible surface
      // (parent OR child events).
      expect(result.leakedKeyAnywhere, dump).toBe(false);
    },
    9.7 * 60 * 1000
  );
});

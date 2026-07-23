/**
 * Live edge sweep — SUBAGENT LINEAGE OBSERVABILITY (Wave 1)
 * @aex-reliability-audit: post-finish-read
 *
 * Customer POV, blackbox against the installed public `@aexhq/sdk` on DEV.
 * A parent DeepSeek run is prompted to delegate a tiny task to the builtin
 * `subagent` tool (spawns a CHILD session). We then probe — using ONLY the public
 * SDK read surface a customer actually has (`Aex` + parent session handles) —
 * whether the child is observable, and whether its work / cost flows back into
 * the parent.
 *
 * Child observation follows the current read-only lineage API:
 *   - the parent's `subagent` TOOL_CALL_RESULT text (carries the child sessionId),
 *   - `await parent.children()` for child state, events, files, and descendants,
 *   - `client.sessions.list()` for top-level sessions only.
 *
 * Hard invariants asserted here (the rest is characterized to a scratchpad JSON
 * for the human sweep): parent succeeds, subagent was actually called, a child
 * sessionId was returned, and neither the AEX nor DeepSeek key leaks into ANY
 * SDK-visible surface (parent events, child events, tool output, or errors).
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
  readonly stage: string;
  readonly error: { readonly name: string; readonly message: string; readonly code: string | null; readonly status: number | null } | null;
  readonly parentSessionId: string | null;
  readonly parentStatus: string;
  readonly parentOk: boolean;
  readonly parentCostUsd: number | null;
  readonly parentEventKinds: readonly string[];
  readonly subagentStartCount: number;
  readonly subagentResults: ReadonlyArray<{ readonly isError: boolean; readonly text: string }>;
  readonly childId: string | null;
  readonly parentOutputCount: number;
  readonly childFound: boolean;
  readonly childParentSessionId: string | null;
  readonly childDepth: number | null;
  readonly childStatus: string | null;
  readonly childLastRunOutcome: string | null;
  readonly childEventKinds: readonly string[];
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
      const script = `
        import { Aex } from "@aexhq/sdk";

        const aexApiKey = process.env.AEX_API_KEY;
        const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: aexApiKey });
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const knownSecrets = [aexApiKey, deepseekKey].filter((value) => typeof value === "string" && value.length > 0);
        const model = process.env.MODEL;
        const startedAt = Date.now();
        let stage = "starting_parent";
        let parentSessionId = null;
        let childId = null;
        let observedSecretLeak = false;
        let observation = {
          stage, error:null, parentSessionId, parentStatus:"not_started", parentOk:false, parentCostUsd:null,
          parentEventKinds:[], subagentStartCount:0, subagentResults:[], childId,
          parentOutputCount:0,
          childFound:false, childParentSessionId:null, childDepth:null, childStatus:null, childLastRunOutcome:null,
          childEventKinds:[], childOutputCount:0,
          childInSessionsList:false, parentInSessionsList:false, sessionsListCount:0,
          leakedKeyAnywhere:false
        };

        function errorInfo(error) {
          return {
            name: error && error.name ? String(error.name) : "Error",
            message: error && error.message ? String(error.message) : String(error),
            code: error && error.code ? String(error.code) : null,
            status: error && typeof error.status === "number" ? error.status : null
          };
        }
        function serialized(value) {
          try { return JSON.stringify(value) ?? ""; } catch { return String(value); }
        }
        function containsKnownSecret(value) {
          const text = serialized(value);
          return knownSecrets.some((secret) => text.includes(secret));
        }
        function redactKnownSecrets(value) {
          let text = serialized(value);
          for (const secret of knownSecrets) text = text.split(secret).join("[REDACTED]");
          return JSON.parse(text);
        }
        function toolText(event) {
          const content = event && event.data ? event.data.content : undefined;
          if (Array.isArray(content)) return content.map((block) => block && typeof block.text === "string" ? block.text : "").join(" ");
          return typeof content === "string" ? content : "";
        }

        try {
          const prompt =
            "Use the subagent tool to delegate one short task to a child agent using model " + model + ". " +
            "Ask the child to explain briefly why parent-child run lineage is useful. " +
            "After the tool returns, finish your response briefly.";

          const sessionResult = await client.start({
            provider: "deepseek",
            model,
            message: prompt,
            builtinTools: "default",
            apiKeys: { deepseek: deepseekKey },
            idempotencyKey: "edge-lineage-w1-" + Date.now()
          }, { timeoutMs: 8 * 60 * 1000 });
          parentSessionId = sessionResult.sessionId;
          observation = { ...observation, parentSessionId };

          stage = "reading_parent";
          const parentSession = await client.sessions.open(parentSessionId);
          const events = await parentSession.events.list();
          const parentEventKinds = events.map((event) => event.type);
          const subagentStartIds = new Set(
            events.filter((event) => event.type === "TOOL_CALL_START" && event.data && event.data.name === "subagent")
              .map((event) => event.data && typeof event.data.id === "string" ? event.data.id : null)
              .filter((id) => id !== null)
          );
          const subagentResults = events
            .filter((event) => event.type === "TOOL_CALL_RESULT" && event.data && typeof event.data.id === "string" && subagentStartIds.has(event.data.id))
            .map((event) => ({ isError: event.data.isError === true, text: toolText(event) }));
          for (const result of subagentResults) {
            const match = result.text.match(/\\bses_[0-9a-f]{32}\\b/i);
            if (match) { childId = match[0]; break; }
          }
          observation = { ...observation, childId };

          const parentEventsStr = JSON.stringify(events);
          const parentSnapshot = await parentSession.files.list();

          stage = "reading_child";
          const children = await parentSession.children();
          const child = childId ? children.find((candidate) => candidate.id === childId) : undefined;
          let childEventKinds = [];
          let childOutputCount = 0;
          let childEventsStr = "";
          if (child) {
            const childEvents = await child.events.list();
            childEventKinds = childEvents.map((event) => event.type);
            childEventsStr = JSON.stringify(childEvents);
            childOutputCount = (await child.files.list()).files.length;
          }

          stage = "listing_root_sessions";
          const ids = [];
          let cursor;
          let pages = 0;
          do {
            const page = await client.sessions.list({
              since: new Date(startedAt - 60_000).toISOString(),
              limit: 100,
              ...(cursor ? { cursor } : {})
            });
            ids.push(...page.sessions.map((session) => session.id));
            cursor = page.nextCursor;
            pages += 1;
            if (pages > 100) throw new Error("sessions.list pagination did not terminate");
          } while (cursor);

          observation = {
            stage:"complete", error:null,
            parentSessionId,
            parentStatus:sessionResult.status,
            parentOk:sessionResult.ok === true,
            parentCostUsd:typeof sessionResult.costUsd === "number" ? sessionResult.costUsd : null,
            parentEventKinds,
            subagentStartCount:subagentStartIds.size,
            subagentResults,
            childId,
            parentOutputCount:parentSnapshot.files.length,
            childFound:child !== undefined,
            childParentSessionId:child?.parentSessionId ?? null,
            childDepth:child?.depth ?? null,
            childStatus:child?.status ?? null,
            childLastRunOutcome:child?.ref.lastRun?.outcome ?? null,
            childEventKinds,
            childOutputCount,
            childInSessionsList:childId ? ids.includes(childId) : false,
            parentInSessionsList:ids.includes(parentSessionId),
            sessionsListCount:ids.length,
            leakedKeyAnywhere:false
          };
          observedSecretLeak = containsKnownSecret({ parentEventsStr, childEventsStr, subagentResults });
        } catch (error) {
          const info = errorInfo(error);
          observedSecretLeak = observedSecretLeak || containsKnownSecret(info);
          observation = { ...observation, stage, parentSessionId, childId, error:info };
          process.exitCode = 1;
        }
        const leakedKeyAnywhere = observedSecretLeak || containsKnownSecret(observation);
        process.stdout.write(JSON.stringify(redactKnownSecrets({ ...observation, leakedKeyAnywhere })));
      `;
      const scriptPath = join(install.installDir, "edge-lineage-w1-runner.mjs");
      writeFileSync(scriptPath, script);

      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_KEY: apiKey,
        DEEPSEEK_API_KEY: deepseekKey,
        DEEPSEEK_KEY: deepseekKey,
        MODEL: model
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
      let result: Wave1Result;
      try {
        result = JSON.parse(child.stdout.trim()) as Wave1Result;
      } catch {
        throw new Error(`lineage runner returned invalid evidence; exitCode=${child.exitCode}; stdoutBytes=${child.stdout.length}; stderrBytes=${child.stderr.length}`);
      }
      const evidence = {
        stage: result.stage,
        hasError: result.error !== null,
        errorName: result.error?.name ?? null,
        errorCode: result.error?.code ?? null,
        errorStatus: result.error?.status ?? null,
        parentSessionId: result.parentSessionId,
        parentStatus: result.parentStatus,
        parentOk: result.parentOk,
        parentEventCount: result.parentEventKinds.length,
        subagentStartCount: result.subagentStartCount,
        subagentResultCount: result.subagentResults.length,
        subagentErrorCount: result.subagentResults.filter((item) => item.isError).length,
        childId: result.childId,
        childFound: result.childFound,
        childParentSessionId: result.childParentSessionId,
        childDepth: result.childDepth,
        childStatus: result.childStatus,
        childLastRunOutcome: result.childLastRunOutcome,
        childEventCount: result.childEventKinds.length,
        childInSessionsList: result.childInSessionsList,
        parentInSessionsList: result.parentInSessionsList,
        leakedKeyAnywhere: result.leakedKeyAnywhere
      };
      const evidenceJson = JSON.stringify(evidence);
      expect(evidenceJson.includes(apiKey) || evidenceJson.includes(deepseekKey), "metadata-only lineage evidence contains a known secret").toBe(false);
      writeFileSync(join(install.installDir, "lineage-wave1-out.json"), JSON.stringify(evidence, null, 2));
      console.error(`[edge-evidence] edge-lineage-w1-runner.mjs: ${evidenceJson}`);
      if (child.exitCode !== 0) {
        throw new Error(`lineage runner failed (${child.exitCode}): ${evidenceJson}`);
      }

      const dump = JSON.stringify(evidence, null, 2);

      // ---- HARD INVARIANTS (unconditional) ----
      expect(result.error === null, dump).toBe(true);
      expect(result.stage, dump).toBe("complete");
      // Parent completed cleanly.
      expect(result.parentOk, dump).toBe(true);
      expect(result.parentStatus, dump).toBe("succeeded");
      expect(result.parentEventKinds, dump).toContain("RUN_FINISHED");
      // The agent actually reached for the subagent tool.
      expect(result.subagentStartCount, dump).toBeGreaterThanOrEqual(1);
      // The subagent tool returned a child sessionId (async "started" path).
      expect(result.childId, dump).not.toBeNull();
      expect(result.childFound, dump).toBe(true);
      expect(result.childParentSessionId, dump).toBe(result.parentSessionId);
      expect(result.childDepth, dump).toBe(1);
      expect(result.childLastRunOutcome, dump).toBe("succeeded");
      expect(result.childEventKinds, dump).toContain("RUN_FINISHED");
      expect(result.childInSessionsList, dump).toBe(false);
      expect(result.parentInSessionsList, dump).toBe(true);
      // Neither customer credential may appear in an SDK-visible surface.
      expect(result.leakedKeyAnywhere, dump).toBe(false);
    },
    9.7 * 60 * 1000
  );
});

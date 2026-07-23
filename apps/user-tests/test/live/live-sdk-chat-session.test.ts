/**
 * Live scenario: live-sdk-chat-session.test.ts
 *
 * Exercises the resumable chat/session API through a freshly installed SDK
 * against the hosted API:
 *   - create a session, send a run over the coordinator stream, and stop on
 *     the canonical `RUN_FINISHED` consistency barrier.
 *   - suspend an idle session, resume it, then send a follow-up turn that uses
 *     the prior turn's conversational context.
 *   - assert raw message idempotency and busy-session rejection on the same
 *     public `/api/sessions` routes.
 *
 * Required env:
 *   AEX_API_URL
 *   AEX_API_KEY
 *   DEEPSEEK_API_KEY
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { formatChildFailure, LIVE_REQUEST_TRACE_SOURCE } from "../_fixtures/live-diagnostics.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (chat-session): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL").replace(/\/$/, "");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const deepseekModel = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const key of carry) if (process.env[key]) env[key] = process.env[key]!;
  return env;
}

interface ChatSessionLiveResult {
  readonly sessionId: string;
  readonly probe: string;
  readonly idleTtl: string;
  readonly firstStatus: string;
  readonly firstSessionStatus: string;
  readonly suspendedStatus: string;
  readonly resumedStatus: string;
  readonly secondStatus: string;
  readonly secondSessionStatus: string;
  readonly listed: boolean;
  readonly firstText: string;
  readonly secondText: string;
  readonly snapshotTypes: readonly string[];
  readonly customNames: readonly string[];
  readonly turnFinishedCount: number;
  readonly leakedKey: boolean;
}

interface RawSessionLiveResult {
  readonly sessionId: string;
  readonly createStatus: string;
  readonly firstStatus: number;
  readonly replayStatus: number;
  readonly conflictStatus: number;
  readonly conflictError: string | null;
  readonly busyStatus: number;
  readonly firstTurnSeq: number;
  readonly replayTurnSeq: number;
  readonly finalStatus: string;
  readonly customNames: readonly string[];
  readonly turnFinishedCount: number;
  readonly terminalKind: string;
  readonly terminalOutcome: string;
  readonly leakedKey: boolean;
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

describe("live hosted API — resumable chat sessions via installed SDK", () => {
  it(
    "SDK chat session: finished runs, suspend/resume, follow-up message, delete",
    async () => {
      const script = `
        import { Aex } from "@aexhq/sdk";

        ${LIVE_REQUEST_TRACE_SOURCE}

        const baseUrl = process.env.AEX_API_URL.replace(/\\/$/, "");
        const apiKey = process.env.AEX_API_KEY;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;
        const probe = "REF.chat." + Math.random().toString(36).slice(2, 10);
        const client = new Aex({ baseUrl, apiKey, debug: aexDebug });

        const session = await client.sessions.create({
          provider: "deepseek",
          model,
          system:
            "You are in a multi-turn verification session. When the user asks what reference was remembered, answer with the exact reference only.",
          builtinTools: "none",
          apiKeys: { deepseek: deepseekKey },
          idempotencyKey: "chat-session-create-" + Date.now(),
          overrides: { idleTtl: "1d" }
        });

        const first = await session.messages.send(
          "Remember this reference for later: " + probe + ". Reply with exactly: ready " + probe,
          { idempotencyKey: "chat-session-first-" + Date.now(), idleTimeoutMs: 240000 }
        ).finished();
        const firstSession = first.session;

        const suspended = await session.suspend();
        const suspendedRecord = suspended.session;

        const resumed = await session.resume();
        const resumedRecord = resumed.session;

        const second = await session.messages.send(
          "What reference did I ask you to remember? Reply with exactly the reference and no other words.",
          { idempotencyKey: "chat-session-second-" + Date.now(), idleTimeoutMs: 240000 }
        ).finished();
        const secondSession = second.session;
        const idleTtl = secondSession.idleTtl;
        const events = await session.events.list();
        // The suite runs many shards against one shared workspace and the list is
        // newest-first, so with a "since" lower bound this session is the LAST
        // item in the range — concurrent shards' newer sessions fill page one.
        // Follow nextCursor pages; "since" keeps the page space small and finite.
        const listedIn = (p) => Array.isArray(p.sessions) && p.sessions.some((s) => s.id === session.id);
        let listed = false;
        let page;
        let cursor;
        const listStatus = secondSession.status;
        do {
          page = await client.sessions.list({ ...(listStatus ? { status: listStatus } : {}), limit: 25, since: session.record.createdAt, ...(cursor ? { cursor } : {}) });
          if (listedIn(page)) { listed = true; break; }
          cursor = page.nextCursor;
        } while (cursor);
        await session.delete();

        const serialized = JSON.stringify({ first, second, firstSession, secondSession, events, page });
        process.stdout.write(JSON.stringify({
          sessionId: session.id,
          probe,
          idleTtl,
          firstStatus: first.status,
          firstSessionStatus: firstSession.status,
          suspendedStatus: suspendedRecord.status,
          resumedStatus: resumedRecord.status,
          secondStatus: second.status,
          secondSessionStatus: secondSession.status,
          listed,
          firstText: first.text,
          secondText: second.text,
          snapshotTypes: [...new Set(events.map((e) => e.type))],
          customNames: [...new Set(events.filter((e) => e.type === "CUSTOM").map((e) => e.data && e.data.name).filter(Boolean))],
          turnFinishedCount: events.filter((e) => e.type === "RUN_FINISHED").length,
          leakedKey: serialized.includes(deepseekKey)
        }));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-chat-session-sdk.mjs");
      writeFileSync(scriptPath, script);
      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 12 * 60_000,
        env: buildPassEnv({
          AEX_API_URL: apiUrl,
          AEX_API_KEY: apiKey,
          DEEPSEEK_KEY: deepseekKey,
          MODEL: deepseekModel
        })
      });
      if (child.exitCode !== 0) {
        throw new Error(formatChildFailure("chat-session SDK runner", child, [apiKey, deepseekKey]));
      }
      const result = JSON.parse(child.stdout.trim()) as ChatSessionLiveResult;
      const dump = (): string => JSON.stringify(result, null, 2);

      expect(result.firstStatus, dump()).toBe("succeeded");
      expect(result.firstSessionStatus, dump()).toBe("idle");
      expect(result.idleTtl, dump()).toBe("1d");
      expect(result.suspendedStatus, dump()).toBe("suspended");
      expect(result.resumedStatus, dump()).toBe("idle");
      expect(result.secondStatus, dump()).toBe("succeeded");
      expect(result.secondSessionStatus, dump()).toBe("idle");
      expect(result.listed, dump()).toBe(true);
      expect(result.firstText.replace(/\s+/g, ""), dump()).toContain(result.probe);
      expect(result.secondText.replace(/\s+/g, ""), dump()).toContain(result.probe);
      expect(result.snapshotTypes, dump()).toContain("RUN_FINISHED");
      expect(result.turnFinishedCount, dump()).toBe(2);
      expect(result.leakedKey, dump()).toBe(false);
    },
    13 * 60_000
  );

  it(
    "raw session messages: idempotent replay returns the same turn and concurrent different message is busy",
    async () => {
      const script = `
        import { Aex } from "@aexhq/sdk";

        ${LIVE_REQUEST_TRACE_SOURCE}

        const baseUrl = process.env.AEX_API_URL.replace(/\\/$/, "");
        const apiKey = process.env.AEX_API_KEY;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;
        const client = new Aex({ baseUrl, apiKey, debug: aexDebug });

        async function api(path, init = {}) {
          const method = init.method || "GET";
          const startedMs = Date.now();
          let res;
          try {
            res = await fetch(baseUrl + path, {
              ...init,
              headers: {
                authorization: "Bearer " + apiKey,
                "content-type": "application/json",
                ...(init.headers || {})
              }
            });
            writeRequestTrace(method, path, res.status, startedMs);
          } catch (error) {
            writeRequestTrace(method, path, undefined, startedMs);
            throw error;
          }
          const text = await res.text();
          let body = null;
          try { body = text ? JSON.parse(text) : null; } catch {}
          return { status: res.status, body, text };
        }

        const session = await client.sessions.create({
          provider: "deepseek",
          model,
          builtinTools: "none",
          apiKeys: { deepseek: deepseekKey },
          idempotencyKey: "chat-raw-create-" + Date.now()
        });
        const createStatus = session.record.status;

        const key = "chat-raw-message-" + Date.now();
        const firstInput = "Reply with exactly one word: ok.";
        const first = await api("/api/sessions/" + encodeURIComponent(session.id) + "/messages", {
          method: "POST",
          headers: { "Idempotency-Key": key },
          body: JSON.stringify({ input: firstInput })
        });
        const replay = await api("/api/sessions/" + encodeURIComponent(session.id) + "/messages", {
          method: "POST",
          headers: { "Idempotency-Key": key },
          body: JSON.stringify({ input: firstInput })
        });
        const conflict = await api("/api/sessions/" + encodeURIComponent(session.id) + "/messages", {
          method: "POST",
          headers: { "Idempotency-Key": key },
          body: JSON.stringify({ input: "This mismatched replay body must conflict." })
        });
        const busy = await api("/api/sessions/" + encodeURIComponent(session.id) + "/messages", {
          method: "POST",
          headers: { "Idempotency-Key": "chat-raw-busy-" + Date.now() },
          body: JSON.stringify({ input: "This should be rejected while the prior turn is running." })
        });

        const acceptedRunId = first.body && first.body.run ? first.body.run.runId : null;
        if (!acceptedRunId) throw new Error("message acceptance omitted run.runId");
        let terminal = null;
        for await (const event of session.events.stream({ from: first.body.eventCursor ?? first.body.run.eventCursor ?? 0 })) {
          if (event.runId !== acceptedRunId) continue;
          if (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") {
            terminal = event;
            break;
          }
        }
        if (!terminal) throw new Error("accepted run ended without a terminal event");
        const finalSession = await session.refresh();
        const events = await session.events.list();
        await session.delete();

        const serialized = JSON.stringify({ first, replay, busy, finalSession, events });
        process.stdout.write(JSON.stringify({
          sessionId: session.id,
          createStatus,
          firstStatus: first.status,
          replayStatus: replay.status,
          conflictStatus: conflict.status,
          conflictError: conflict.body && typeof conflict.body.error === "string" ? conflict.body.error : null,
          busyStatus: busy.status,
          firstTurnSeq: first.body && first.body.run ? first.body.run.turnSeq : -1,
          replayTurnSeq: replay.body && replay.body.run ? replay.body.run.turnSeq : -2,
          finalStatus: finalSession.status,
          customNames: [...new Set(events.filter((e) => e.type === "CUSTOM").map((e) => e.data && e.data.name).filter(Boolean))],
          turnFinishedCount: events.filter((e) => e.type === "RUN_FINISHED").length,
          terminalKind: terminal.type,
          terminalOutcome: terminal.data && terminal.data.outcome,
          leakedKey: serialized.includes(deepseekKey)
        }));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-chat-session-raw.mjs");
      writeFileSync(scriptPath, script);
      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 9 * 60_000,
        env: buildPassEnv({
          AEX_API_URL: apiUrl,
          AEX_API_KEY: apiKey,
          DEEPSEEK_KEY: deepseekKey,
          MODEL: deepseekModel
        })
      });
      if (child.exitCode !== 0) {
        throw new Error(formatChildFailure("chat-session raw runner", child, [apiKey, deepseekKey]));
      }
      const result = JSON.parse(child.stdout.trim()) as RawSessionLiveResult;
      const dump = (): string => JSON.stringify(result, null, 2);

      expect(result.createStatus, dump()).toBe("idle");
      expect(result.firstStatus, dump()).toBeGreaterThanOrEqual(200);
      expect(result.firstStatus, dump()).toBeLessThan(300);
      expect(result.replayStatus, dump()).toBeGreaterThanOrEqual(200);
      expect(result.replayStatus, dump()).toBeLessThan(300);
      expect(result.firstTurnSeq, dump()).toBeGreaterThan(0);
      expect(result.replayTurnSeq, dump()).toBe(result.firstTurnSeq);
      expect(result.conflictStatus, dump()).toBe(409);
      expect(result.conflictError, dump()).toBe("idempotency_conflict");
      expect(result.busyStatus, dump()).toBe(409);
      expect(result.finalStatus, dump()).toBe("idle");
      expect(result.terminalKind, dump()).toBe("RUN_FINISHED");
      expect(result.terminalOutcome, dump()).toBe("succeeded");
      expect(result.turnFinishedCount, dump()).toBe(1);
      expect(result.leakedKey, dump()).toBe(false);
    },
    10 * 60_000
  );
});

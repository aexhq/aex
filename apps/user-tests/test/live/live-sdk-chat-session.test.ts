/**
 * Live scenario: live-sdk-chat-session.test.ts
 *
 * Exercises the resumable chat/session API through a freshly installed SDK
 * against the hosted API:
 *   - create a session, send a turn over the coordinator stream, and stop on
 *     a clean `aex.session.*` turn terminal instead of `RUN_FINISHED`.
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
import { afterAll, beforeAll, describe, expect, it } from "vitest";
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
  readonly firstPolledStatus: string;
  readonly suspendedStatus: string;
  readonly resumedStatus: string;
  readonly secondStatus: string;
  readonly secondPolledStatus: string;
  readonly listed: boolean;
  readonly firstText: string;
  readonly secondText: string;
  readonly snapshotTypes: readonly string[];
  readonly customNames: readonly string[];
  readonly runFinishedCount: number;
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
  readonly runFinishedCount: number;
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
    "SDK chat session: idle event, suspend/resume, follow-up message, delete",
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

        const CLEAN_SESSION_STATUSES = ["idle", "succeeded"];
        const CLEAN_SESSION_TERMINAL_NAMES = ["aex.session.idle", "aex.session.succeeded"];

        async function pollSession(id, wanted, timeoutMs = 240000) {
          const deadline = Date.now() + timeoutMs;
          let session = null;
          while (Date.now() < deadline) {
            session = await client.sessions.get(id);
            if (wanted.includes(session.status)) return session;
            await new Promise((r) => setTimeout(r, 2000));
          }
          throw new Error("session " + id + " did not reach " + wanted.join("/") + ": " + JSON.stringify(session));
        }

        async function listEventsSettled(session, requiredNames, timeoutMs = 90000) {
          const deadline = Date.now() + timeoutMs;
          const wanted = Array.isArray(requiredNames) ? requiredNames : [requiredNames];
          let events = [];
          while (Date.now() < deadline) {
            events = await session.events().list();
            const names = events
              .filter((e) => e.type === "CUSTOM")
              .map((e) => e.data && e.data.name)
              .filter(Boolean);
            if (names.some((name) => wanted.includes(name))) return events;
            await new Promise((r) => setTimeout(r, 1000));
          }
          return events;
        }

        function textOf(events) {
          return events
            .filter((e) => e.type === "TEXT_MESSAGE_CONTENT")
            .map((e) => e.data && typeof e.data.text === "string" ? e.data.text : "")
            .join(" ");
        }

        const session = await client.openSession({
          provider: "deepseek",
          model,
          system:
            "You are in a multi-turn verification session. When the user asks what reference was remembered, answer with the exact reference only.",
          includeBuiltinTools: false,
          apiKeys: { deepseek: deepseekKey },
          idempotencyKey: "chat-session-create-" + Date.now(),
          overrides: { idleTtl: "1d" }
        });

        const first = await session.send(
          "Remember this reference for later: " + probe + ". Reply with exactly: ready " + probe,
          { idempotencyKey: "chat-session-first-" + Date.now(), idleTimeoutMs: 240000 }
        ).done();
        const firstIdle = await pollSession(session.id, CLEAN_SESSION_STATUSES);

        const suspended = await session.suspend({ idempotencyKey: "chat-session-suspend-" + Date.now() });
        const suspendedRecord = suspended.session;
        const suspendedEvents = await listEventsSettled(session, "aex.session.suspended");

        const resumed = await session.resume({ idempotencyKey: "chat-session-resume-" + Date.now() });
        const resumedRecord = resumed.session;

        const second = await session.send(
          "What reference did I ask you to remember? Reply with exactly the reference and no other words.",
          { idempotencyKey: "chat-session-second-" + Date.now(), idleTimeoutMs: 240000 }
        ).done();
        const secondIdle = await pollSession(session.id, CLEAN_SESSION_STATUSES);
        const idleEvents = await listEventsSettled(session, CLEAN_SESSION_TERMINAL_NAMES);
        // The suite runs 11 shards against one shared workspace and the list is
        // newest-first, so with a "since" lower bound this session is the LAST
        // item in the range — concurrent shards' newer sessions fill page one.
        // Follow nextCursor pages; "since" keeps the page space small and finite.
        const listedIn = (p) => Array.isArray(p.sessions) && p.sessions.some((s) => s.id === session.id || s.sessionId === session.id);
        let listed = false;
        let page;
        let cursor;
        const listStatus = CLEAN_SESSION_STATUSES.includes(secondIdle.status) ? secondIdle.status : undefined;
        do {
          page = await client.sessions.list({ ...(listStatus ? { status: listStatus } : {}), limit: 25, since: session.record.createdAt, ...(cursor ? { cursor } : {}) });
          if (listedIn(page)) { listed = true; break; }
          cursor = page.nextCursor;
        } while (cursor);
        await session.delete({ idempotencyKey: "chat-session-delete-" + Date.now() });

        const events = [...suspendedEvents, ...idleEvents];
        const serialized = JSON.stringify({ first, second, firstIdle, secondIdle, events, page });
        process.stdout.write(JSON.stringify({
          sessionId: session.id,
          probe,
          idleTtl: session.record.idleTtl,
          firstStatus: first.status,
          firstPolledStatus: firstIdle.status,
          suspendedStatus: suspendedRecord.status,
          resumedStatus: resumedRecord.status,
          secondStatus: second.status,
          secondPolledStatus: secondIdle.status,
          listed,
          firstText: first.text,
          secondText: second.text,
          snapshotTypes: [...new Set(events.map((e) => e.type))],
          customNames: [...new Set(events.filter((e) => e.type === "CUSTOM").map((e) => e.data && e.data.name).filter(Boolean))],
          runFinishedCount: events.filter((e) => e.type === "RUN_FINISHED").length,
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

      expect(["idle", "succeeded"], dump()).toContain(result.firstStatus);
      expect(["idle", "succeeded"], dump()).toContain(result.firstPolledStatus);
      expect(result.idleTtl, dump()).toBe("1d");
      expect(result.suspendedStatus, dump()).toBe("suspended");
      expect(["idle", "succeeded"], dump()).toContain(result.resumedStatus);
      expect(["idle", "succeeded"], dump()).toContain(result.secondStatus);
      expect(["idle", "succeeded"], dump()).toContain(result.secondPolledStatus);
      expect(result.listed, dump()).toBe(true);
      expect(result.firstText.replace(/\s+/g, ""), dump()).toContain(result.probe);
      expect(result.secondText.replace(/\s+/g, ""), dump()).toContain(result.probe);
      expect(result.snapshotTypes, dump()).toContain("CUSTOM");
      expect(result.customNames.some((name) => name === "aex.session.idle" || name === "aex.session.succeeded"), dump()).toBe(true);
      expect(result.customNames, dump()).toContain("aex.session.suspended");
      expect(result.runFinishedCount, dump()).toBe(0);
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

        const CLEAN_SESSION_STATUSES = ["idle", "succeeded"];
        const CLEAN_SESSION_TERMINAL_NAMES = ["aex.session.idle", "aex.session.succeeded"];

        async function pollSession(id, wanted, timeoutMs = 300000) {
          const deadline = Date.now() + timeoutMs;
          let session = null;
          while (Date.now() < deadline) {
            session = await client.sessions.get(id);
            if (wanted.includes(session.status)) return session;
            await new Promise((r) => setTimeout(r, 2000));
          }
          throw new Error("session " + id + " did not reach " + wanted.join("/") + ": " + JSON.stringify(session));
        }

        async function listEventsSettled(session, timeoutMs = 90000) {
          const deadline = Date.now() + timeoutMs;
          let events = [];
          while (Date.now() < deadline) {
            events = await session.events().list();
            if (events.some((e) => e.type === "CUSTOM" && e.data && CLEAN_SESSION_TERMINAL_NAMES.includes(e.data.name))) return events;
            await new Promise((r) => setTimeout(r, 1000));
          }
          return events;
        }

        const session = await client.openSession({
          provider: "deepseek",
          model,
          includeBuiltinTools: false,
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

        const finalSession = await pollSession(session.id, [...CLEAN_SESSION_STATUSES, "error", "failed", "timed_out", "cancelled"]);
        const events = await listEventsSettled(session);
        await session.delete({ idempotencyKey: "chat-raw-delete-" + Date.now() });

        const serialized = JSON.stringify({ first, replay, busy, finalSession, events });
        process.stdout.write(JSON.stringify({
          sessionId: session.id,
          createStatus,
          firstStatus: first.status,
          replayStatus: replay.status,
          conflictStatus: conflict.status,
          conflictError: conflict.body && typeof conflict.body.error === "string" ? conflict.body.error : null,
          busyStatus: busy.status,
          firstTurnSeq: first.body && first.body.turn ? first.body.turn.turnSeq : -1,
          replayTurnSeq: replay.body && replay.body.turn ? replay.body.turn.turnSeq : -2,
          finalStatus: finalSession.status,
          customNames: [...new Set(events.filter((e) => e.type === "CUSTOM").map((e) => e.data && e.data.name).filter(Boolean))],
          runFinishedCount: events.filter((e) => e.type === "RUN_FINISHED").length,
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

      expect(["idle", "succeeded"], dump()).toContain(result.createStatus);
      expect(result.firstStatus, dump()).toBeGreaterThanOrEqual(200);
      expect(result.firstStatus, dump()).toBeLessThan(300);
      expect(result.replayStatus, dump()).toBeGreaterThanOrEqual(200);
      expect(result.replayStatus, dump()).toBeLessThan(300);
      expect(result.firstTurnSeq, dump()).toBeGreaterThan(0);
      expect(result.replayTurnSeq, dump()).toBe(result.firstTurnSeq);
      expect(result.conflictStatus, dump()).toBe(409);
      expect(result.conflictError, dump()).toBe("idempotency_conflict");
      expect(result.busyStatus, dump()).toBe(409);
      expect(["idle", "succeeded"], dump()).toContain(result.finalStatus);
      expect(result.customNames.some((name) => name === "aex.session.idle" || name === "aex.session.succeeded"), dump()).toBe(true);
      expect(result.runFinishedCount, dump()).toBe(0);
      expect(result.leakedKey, dump()).toBe(false);
    },
    10 * 60_000
  );
});

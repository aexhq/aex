/**
 * Live scenario: live-sdk-event-stream.test.ts
 *
 * Exercises the unified event coordinator end-to-end through the installed
 * SDK, the way a user listening to a session would:
 *
 *   1. run (DeepSeek Managed) — a one-shot session via `client.start(...)`
 *   2. LISTEN over the coordinator WebSocket via `session.events.streamEnvelopes(...)`
 *      (ticket broker → coordinator WS, exactly-once cursor resume).
 *   3. SNAPSHOT the same log after the `RUN_FINISHED` consistency barrier.
 *   4. FETCH the hosted durable NDJSON archive through
 *      `session.events.archiveLink()`, proving the server-side archive agrees.
 *
 * Asserts the expected unified-envelope events exist (AG-UI vocabulary):
 * RUN_STARTED, ≥1 TEXT_MESSAGE_CONTENT, RUN_FINISHED — over both the live WS
 * and the snapshot — and that the public archive contains the same durable log.
 *
 * Required env:
 *   AEX_API_URL              live hosted API URL (local or prod)
 *   AEX_API_KEY             workspace API key
 *   DEEPSEEK_API_KEY    customer DeepSeek API key
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (event-stream): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const model = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";
const runtimeKind = requireRuntimeKind();

function requireRuntimeKind(): "container" | "spot_container" | "lambda" {
  const value = requireEnv("AEX_USER_TEST_RUNTIME_KIND");
  if (value !== "container" && value !== "spot_container" && value !== "lambda") {
    throw new Error(`user-tests live (event-stream): invalid AEX_USER_TEST_RUNTIME_KIND ${JSON.stringify(value)}.`);
  }
  return value;
}

interface StreamResult {
  readonly sessionId: string;
  readonly runId: string;
  readonly runStatus: string;
  readonly requestedRuntimeKind: string;
  readonly observedRuntimeKind: string | null;
  readonly cleanupPassed: boolean;
  readonly streamedCount: number;
  readonly streamedTypes: readonly string[];
  readonly streamedCustomNames: readonly string[];
  readonly snapshotTypes: readonly string[];
  readonly snapshotCustomNames: readonly string[];
  readonly snapshotCount: number;
  readonly archiveEventCount: number;
  readonly archiveTypes: readonly string[];
  readonly archiveMatchesSnapshot: boolean;
  readonly lifecycle: {
    readonly streamed: LifecycleCounts;
    readonly snapshot: LifecycleCounts;
    readonly archive: LifecycleCounts;
  };
  readonly leakedKey: boolean;
  readonly terminalOutcome: string | null;
  readonly liveDeltaCount: number;
  readonly liveSequenceOrdered: boolean;
  readonly liveDeltasNonReplayable: boolean;
  readonly liveDeltasHaveNoSequence: boolean;
  readonly coalescedBeforeTerminalCount: number;
  readonly resultCoalescedCount: number;
  readonly resultDeltaCount: number;
}

interface LifecycleCounts {
  readonly runStarted: number;
  readonly terminal: number;
  readonly runFinished: number;
  readonly runError: number;
}

describe("live api.aex.dev — event stream: listen (WS) + snapshot + hosted archive", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "streams the unified envelope live, snapshots it, and verifies the hosted durable archive",
    async () => {
      const probe = "evt-stream-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { Aex } from "@aexhq/sdk";

        const ARCHIVE_MAX_BYTES = 8 * 1024 * 1024;
        async function readBoundedText(response, maxBytes) {
          const declared = Number(response.headers.get("content-length"));
          if (Number.isFinite(declared) && declared > maxBytes) {
            throw new Error("hosted event archive exceeded the byte limit");
          }
          if (!response.body) return "";
          const reader = response.body.getReader();
          const chunks = [];
          let total = 0;
          try {
            while (true) {
              const next = await reader.read();
              if (next.done) break;
              total += next.value.byteLength;
              if (total > maxBytes) {
                await reader.cancel();
                throw new Error("hosted event archive exceeded the byte limit");
              }
              chunks.push(next.value);
            }
          } catch (error) {
            if (error instanceof Error && error.message === "hosted event archive exceeded the byte limit") throw error;
            throw new Error("hosted event archive body read failed");
          }
          const bytes = new Uint8Array(total);
          let offset = 0;
          for (const chunk of chunks) {
            bytes.set(chunk, offset);
            offset += chunk.byteLength;
          }
          return new TextDecoder().decode(bytes);
        }

        function canonical(value) {
          if (Array.isArray(value)) return value.map(canonical);
          if (value && typeof value === "object") {
            return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
          }
          return value;
        }

        function lifecycleCounts(events, runId) {
          const current = events.filter((event) => event.runId === runId);
          return {
            runStarted: current.filter((event) => event.type === "RUN_STARTED").length,
            terminal: current.filter((event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR").length,
            runFinished: current.filter((event) => event.type === "RUN_FINISHED").length,
            runError: current.filter((event) => event.type === "RUN_ERROR").length
          };
        }

        const baseUrl = process.env.AEX_API_URL;
        const apiKey = process.env.AEX_API_KEY;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;
        const runtimeKind = process.env.RUNTIME_KIND;

        const client = new Aex({ baseUrl, apiKey });
        const session = await client.sessions.create({
          provider: "deepseek",
          model,
          outputMode: "stream",
          idempotencyKey: "user-test-event-stream-" + Date.now(),
          apiKeys: { deepseek: deepseekKey },
          runtime: { kind: runtimeKind }
        });
        if (session.record.runtime?.kind !== runtimeKind) {
          throw new Error("runtime identity mismatch: requested=" + runtimeKind + " observed=" + String(session.record.runtime?.kind));
        }
        process.stderr.write(JSON.stringify({ eventStreamSessionId: session.id }) + "\\n");
        const turn = session.messages.send(
          ${JSON.stringify(`Reply with exactly these words: ${probe} alpha beta gamma delta epsilon zeta eta theta iota kappa lambda.`)},
          { idleTimeoutMs: 120 * 1000 }
        );
        const streamedEvents = [];
        for await (const ev of turn) streamedEvents.push(ev);
        const result = await turn.finished();
        const sessionId = result.sessionId;
        process.stderr.write(JSON.stringify({ eventStreamRunId: result.run.runId }) + "\\n");
        const streamed = streamedEvents.map((event) => event.type);
        const streamedCustomNames = streamedEvents
          .filter((event) => event.type === "CUSTOM" && typeof event.data?.name === "string")
          .map((event) => event.data.name);
        const liveDeltas = streamedEvents.filter(
          (event) => event.type === "TEXT_MESSAGE_CONTENT" && event.data?.delta === true
        );
        const terminalIndex = streamedEvents.findIndex(
          (event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR"
        );
        const coalescedBeforeTerminal = streamedEvents.filter(
          (event, index) =>
            index < terminalIndex &&
            event.type === "TEXT_MESSAGE_CONTENT" &&
            event.data?.delta !== true
        );
        const liveSequences = liveDeltas.map((event) => event.liveSequence);
        const liveSequenceOrdered = liveSequences.every(
          (value, index) => Number.isSafeInteger(value) && (index === 0 || value > liveSequences[index - 1])
        );

        // 2. Snapshot the same run. RUN_FINISHED means the list endpoint is
        //    already consistent; no fallback or retry is valid here.
        const run = {
          status: result.status,
          runtime: "managed",
          provider: "deepseek"
        };
        const allSnapshotEvents = await session.events.list();
        const snapshot = allSnapshotEvents.filter((event) => event.runId === result.run.runId);

        // 3. Mint and fetch the server-side archive. The signed URL remains local
        //    to this block and is never included in output or thrown errors.
        const archiveLink = await session.events.archiveLink({ expiresIn: "15m" });
        let archiveResponse;
        try {
          archiveResponse = await fetch(archiveLink.url, { signal: AbortSignal.timeout(30_000) });
        } catch {
          throw new Error("hosted event archive fetch failed before response");
        }
        if (!archiveResponse.ok) {
          throw new Error("hosted event archive fetch failed with status " + archiveResponse.status);
        }
        const archiveEvents = (await readBoundedText(archiveResponse, ARCHIVE_MAX_BYTES))
          .split("\\n")
          .filter((line) => line.length > 0)
          .map((line) => JSON.parse(line));
        if (archiveEvents.length === 0) throw new Error("event archive was empty after RUN_FINISHED");
        const archiveRunEvents = archiveEvents.filter((event) => event.runId === result.run.runId);
        // Compare the complete public histories, not a hand-picked field subset or
        // only the current run. Recursive key sorting removes JSON object-order noise
        // while retaining every enumerable durable envelope field.
        const archiveMatchesSnapshot = JSON.stringify(canonical(archiveEvents)) ===
          JSON.stringify(canonical(allSnapshotEvents));
        const lifecycle = {
          streamed: lifecycleCounts(streamedEvents, result.run.runId),
          snapshot: lifecycleCounts(snapshot, result.run.runId),
          archive: lifecycleCounts(archiveRunEvents, result.run.runId)
        };

        // A passing parity verdict requires the hosted session to be cleaned,
        // not merely the caller's temporary install directory.
        await session.delete();

        const serialized = JSON.stringify({ run, streamedEvents, snapshot, archiveEvents });
        const snapshotCustomNames = snapshot
          .filter((e) => e.type === "CUSTOM" && e.data && typeof e.data.name === "string")
          .map((e) => e.data.name);
        process.stdout.write(JSON.stringify({
          sessionId,
          runId: result.run.runId,
          runStatus: run.status,
          requestedRuntimeKind: runtimeKind,
          observedRuntimeKind: session.record.runtime?.kind ?? null,
          streamedCount: streamed.length,
          streamedTypes: [...new Set(streamed)],
          streamedCustomNames: [...new Set(streamedCustomNames)],
          snapshotTypes: [...new Set(snapshot.map((e) => e.type))],
          snapshotCustomNames: [...new Set(snapshotCustomNames)],
          snapshotCount: allSnapshotEvents.length,
          archiveEventCount: archiveEvents.length,
          archiveTypes: [...new Set(archiveEvents.map((event) => event.type))],
          archiveMatchesSnapshot,
          cleanupPassed: true,
          lifecycle,
          leakedKey: [deepseekKey, apiKey].some((secret) => serialized.includes(secret)),
          terminalOutcome: snapshot.find((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR")?.data?.outcome ?? null,
          liveDeltaCount: liveDeltas.length,
          liveSequenceOrdered,
          liveDeltasNonReplayable: liveDeltas.every((event) => event.replayable === false),
          liveDeltasHaveNoSequence: liveDeltas.every((event) => !("sequence" in event)),
          coalescedBeforeTerminalCount: coalescedBeforeTerminal.length,
          resultCoalescedCount: result.events.filter((event) => event.type === "TEXT_MESSAGE_CONTENT" && event.data?.delta !== true).length,
          resultDeltaCount: result.events.filter((event) => event.type === "TEXT_MESSAGE_CONTENT" && event.data?.delta === true).length
        }));
        // The result is complete; terminate the child without waiting on any
        // coordinator socket close bookkeeping.
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-event-stream-runner.mjs");
      writeFileSync(scriptPath, script);

      const passEnv: Record<string, string> = {
        AEX_API_URL: apiUrl,
        AEX_API_KEY: apiKey,
        DEEPSEEK_KEY: deepseekKey,
        MODEL: model,
        RUNTIME_KIND: runtimeKind
      };
      const pathKey = process.platform === "win32" ? "Path" : "PATH";
      if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
      for (const k of ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData", "HOME", "TMPDIR", "LANG", "LC_ALL"]) {
        if (process.env[k]) passEnv[k] = process.env[k]!;
      }

      const child = await runCommand(getBunCommand(), [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 3 * 60 * 1000,
        env: passEnv
      });
      if (child.exitCode !== 0) {
        throw new Error(`live runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
      }
      const result = JSON.parse(child.stdout.trim()) as StreamResult;
      const failureContext = {
        sessionId: result.sessionId,
        runId: result.runId,
        streamedCount: result.streamedCount,
        snapshotCount: result.snapshotCount,
        archiveEventCount: result.archiveEventCount,
        archiveMatchesSnapshot: result.archiveMatchesSnapshot,
        lifecycle: result.lifecycle,
        streamedTypes: result.streamedTypes,
        snapshotTypes: result.snapshotTypes,
        archiveTypes: result.archiveTypes
      };

      try {
        expect(result.runStatus).toBe("succeeded");
        expect(result.requestedRuntimeKind).toBe(runtimeKind);
        expect(result.observedRuntimeKind).toBe(runtimeKind);
        expect(result.cleanupPassed).toBe(true);
        // Live WS delivered the unified envelope.
        expect(result.streamedCount).toBeGreaterThan(0);
        expect(result.streamedTypes).toContain("TEXT_MESSAGE_CONTENT");
        expect(result.streamedTypes).toContain("RUN_FINISHED");
        expect(result.liveDeltaCount).toBeGreaterThanOrEqual(2);
        expect(result.liveSequenceOrdered).toBe(true);
        expect(result.liveDeltasNonReplayable).toBe(true);
        expect(result.liveDeltasHaveNoSequence).toBe(true);
        expect(result.coalescedBeforeTerminalCount).toBe(1);
        expect(result.resultCoalescedCount).toBe(1);
        expect(result.resultDeltaCount).toBe(0);
        // Snapshot and the SDK archive agree.
        expect(result.snapshotTypes).toContain("TEXT_MESSAGE_CONTENT");
        expect(result.snapshotTypes).toContain("RUN_FINISHED");
        expect(result.archiveTypes).toContain("TEXT_MESSAGE_CONTENT");
        expect(result.archiveTypes).toContain("RUN_FINISHED");
        expect(result.terminalOutcome).toBe("succeeded");
        expect(result.leakedKey).toBe(false);
        expect(result.archiveEventCount).toBe(result.snapshotCount);
        expect(result.archiveMatchesSnapshot).toBe(true);
        for (const counts of Object.values(result.lifecycle)) {
          expect(counts.runStarted).toBe(1);
          expect(counts.terminal).toBe(1);
          expect(counts.runFinished).toBe(1);
          expect(counts.runError).toBe(0);
        }
      } catch (error) {
        console.error(`event-stream assertion context: ${JSON.stringify(failureContext)}`);
        throw error;
      }
    },
    4 * 60 * 1000
  );
});

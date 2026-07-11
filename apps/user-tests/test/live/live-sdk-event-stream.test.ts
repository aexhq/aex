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
 *   4. DOWNLOAD the durable event archive: mint a ticket and read the
 *      coordinator manifest (rolling object storage chunks + counts), proving the events
 *      are durably archived and downloadable after the session.
 *
 * Asserts the expected unified-envelope events exist (AG-UI vocabulary):
 * RUN_STARTED, ≥1 TEXT_MESSAGE_CONTENT, RUN_FINISHED — over both the live WS
 * and the snapshot — and that the archive manifest records them.
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

interface StreamResult {
  readonly runStatus: string;
  readonly streamedCount: number;
  readonly streamedTypes: readonly string[];
  readonly streamedCustomNames: readonly string[];
  readonly snapshotTypes: readonly string[];
  readonly snapshotCustomNames: readonly string[];
  readonly snapshotCount: number;
  readonly manifestEventCount: number;
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

describe("live api.aex.dev — event coordinator: listen (WS) + snapshot + download archive", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "streams the unified envelope live, snapshots it, and downloads the durable archive manifest",
    async () => {
      const probe = "evt-stream-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { Aex } from "@aexhq/sdk";

        const baseUrl = process.env.AEX_API_URL;
        const apiKey = process.env.AEX_API_KEY;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;

        const client = new Aex({ baseUrl, apiKey });
        const session = await client.sessions.create({
          provider: "deepseek",
          model,
          outputMode: "stream",
          idempotencyKey: "user-test-event-stream-" + Date.now(),
          apiKeys: { deepseek: deepseekKey }
        });
        const turn = session.messages.send(
          ${JSON.stringify(`Reply with exactly these words: ${probe} alpha beta gamma delta epsilon zeta eta theta iota kappa lambda.`)},
          { idleTimeoutMs: 120 * 1000 }
        );
        const streamedEvents = [];
        for await (const ev of turn) streamedEvents.push(ev);
        const result = await turn.finished();
        const sessionId = result.sessionId;
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

        // \`fetch\` with a hard per-request deadline: a half-open socket to the
        // Hosted API must not ride undici's ~300s default past the outer kill, or
        // the runner is SIGKILLed with no diagnostics. Every HTTP tail call is
        // bounded so the runner always reaches the final stdout.write.
        async function fetchBounded(url, init, ms) {
          const a = new AbortController();
          const t = setTimeout(() => a.abort(), ms);
          try {
            return await fetch(url, { ...(init || {}), signal: a.signal });
          } finally {
            clearTimeout(t);
          }
        }

        function buildCoordinatorManifestUrl(wsUrl, ticket) {
          const url = new URL(wsUrl);
          if (!url.pathname.endsWith("/subscribe")) {
            throw new Error("coordinator wsUrl path is not a subscribe endpoint: " + url.pathname);
          }
          url.protocol = url.protocol === "wss:" ? "https:" : "http:";
          url.pathname = url.pathname.replace(/\\/subscribe$/, "/manifest");
          url.searchParams.set("ticket", ticket);
          return url;
        }

        function withoutTicket(url) {
          const redacted = new URL(url.toString());
          redacted.searchParams.delete("ticket");
          return redacted.toString();
        }

        // 2. Snapshot the same run. RUN_FINISHED means the list endpoint is
        //    already consistent; no fallback or retry is valid here.
        const run = {
          status: result.status,
          runtime: "managed",
          provider: "deepseek"
        };
        const allSnapshotEvents = await session.events.list();
        const snapshot = allSnapshotEvents.filter((event) => event.runId === result.run.runId);

        // 3. The archive is part of the same consistency boundary.
        const tRes = await fetchBounded(baseUrl + "/api/sessions/" + sessionId + "/events/ticket", {
          method: "POST",
          headers: { authorization: "Bearer " + apiKey }
        }, 8000);
        if (!tRes.ok) throw new Error("event ticket failed with " + tRes.status);
        const grant = await tRes.json();
        const manifestUrl = buildCoordinatorManifestUrl(grant.wsUrl, grant.ticket);
        const redactedUrl = withoutTicket(manifestUrl);
        const mRes = await fetchBounded(manifestUrl.toString(), undefined, 8000);
        if (!mRes.ok) throw new Error("event manifest " + redactedUrl + " failed with " + mRes.status);
        const manifest = await mRes.json();
        if (!manifest || (manifest.eventCount ?? 0) <= 0) {
          throw new Error("event manifest was empty after RUN_FINISHED");
        }

        const serialized = JSON.stringify({ run, snapshot, manifest });
        const snapshotCustomNames = snapshot
          .filter((e) => e.type === "CUSTOM" && e.data && typeof e.data.name === "string")
          .map((e) => e.data.name);
        process.stdout.write(JSON.stringify({
          runStatus: run.status,
          streamedCount: streamed.length,
          streamedTypes: [...new Set(streamed)],
          streamedCustomNames: [...new Set(streamedCustomNames)],
          snapshotTypes: [...new Set(snapshot.map((e) => e.type))],
          snapshotCustomNames: [...new Set(snapshotCustomNames)],
          snapshotCount: allSnapshotEvents.length,
          manifestEventCount: manifest ? (manifest.eventCount ?? -1) : -1,
          leakedKey: serialized.includes(deepseekKey),
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
        MODEL: model
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

      expect(result.runStatus).toBe("succeeded");
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
      // Snapshot agrees.
      expect(result.snapshotTypes).toContain("TEXT_MESSAGE_CONTENT");
      expect(result.snapshotTypes).toContain("RUN_FINISHED");
      expect(result.terminalOutcome).toBe("succeeded");
      expect(result.leakedKey).toBe(false);
      // The O(1) durable manifest count matches the canonical list projection.
      expect(result.manifestEventCount).toBe(result.snapshotCount);
    },
    4 * 60 * 1000
  );
});

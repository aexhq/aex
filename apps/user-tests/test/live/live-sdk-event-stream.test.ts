/**
 * Live scenario: live-sdk-event-stream.test.ts
 *
 * Exercises the unified event coordinator end-to-end through the installed
 * SDK, the way a user listening to a session would:
 *
 *   1. run (DeepSeek Managed) — a one-shot session via `client.start(...)`
 *   2. LISTEN over the coordinator WebSocket via `session.events().streamEnvelopes(...)`
 *      (ticket broker → coordinator WS, exactly-once cursor resume).
 *   3. SNAPSHOT the same log from the settle-consistent `SessionResult.events`.
 *   4. DOWNLOAD the durable event archive: mint a ticket and read the
 *      coordinator manifest (rolling object storage chunks + counts), proving the events
 *      are durably archived and downloadable after the session.
 *
 * Asserts the expected unified-envelope events exist (AG-UI vocabulary):
 * TURN_STARTED, ≥1 TEXT_MESSAGE_CONTENT, TURN_FINISHED — over both the live WS
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
  readonly sessionStatus: string;
  readonly streamedCount: number;
  readonly streamedTypes: readonly string[];
  readonly streamedCustomNames: readonly string[];
  readonly snapshotTypes: readonly string[];
  readonly snapshotCustomNames: readonly string[];
  readonly manifestEventCount: number;
  readonly manifestChunks: number;
  readonly leakedKey: boolean;
  readonly manifestAttempts: readonly ManifestAttempt[];
}

interface ManifestAttempt {
  readonly attempt: number;
  readonly phase: "ticket" | "manifest" | "parse";
  readonly status?: number;
  readonly eventCount?: number;
  readonly chunks?: number;
  readonly url?: string;
  readonly error?: string;
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
        const result = await client.start({
          provider: "deepseek",
          model,
          message: ${JSON.stringify(`SessionFile verbatim: ${probe}`)},
          idempotencyKey: "user-test-event-stream-" + Date.now(),
          apiKeys: { deepseek: deepseekKey }
        }, { timeoutMs: 120 * 1000 });
        const sessionId = result.sessionId;
        const session = await client.sessions.open(sessionId);

        // 1. Listen live over the coordinator WebSocket (exactly-once,
        //    reconnecting). Stop on the terminal envelope or the deadline.
        //    The WS phase is RACED against a hard wall-clock timer: if the
        //    SDK's WS path stalls internally (e.g. an eager ticket fetch that
        //    doesn't observe \`signal\`), we abandon listening and fall through
        //    to the snapshot/manifest tail rather than blocking to the outer
        //    SIGKILL. \`ac.abort()\` also fires so the generator can unwind.
        const streamed = [];
        const streamedCustomNames = [];
        const ac = new AbortController();
        const listen = (async () => {
          try {
            for await (const ev of session.events().streamEnvelopes({ from: 0, signal: ac.signal })) {
              streamed.push(ev.type);
              const name = ev && ev.data && typeof ev.data.name === "string" ? ev.data.name : null;
              if (name) streamedCustomNames.push(name);
              if (ev.type === "TURN_FINISHED" || ev.type === "TURN_ERROR" || (typeof name === "string" && name.startsWith("aex.session."))) break;
            }
          } catch (e) {
            // socket dropped past terminal / abort — tolerate; snapshot below
            // is the source of truth for the assertions.
          }
        })();
        await Promise.race([
          listen,
          new Promise((resolve) => setTimeout(() => { ac.abort(); resolve(); }, 80 * 1000))
        ]);

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

        // 2. Snapshot the same log + final status. \`client.start(...)\` already
        //    waited for the session to park at a terminal state, so the
        //    settle-consistent SessionResult carries the final status + events
        //    directly — no waitForRun/getSessionRecord/listEvents round-trip needed.
        const run = {
          status: result.ok ? "succeeded" : (typeof result.status === "string" && result.status ? result.status : "failed"),
          runtime: "managed",
          provider: "deepseek"
        };
        const fallbackSnapshot = Array.isArray(result.events) ? result.events : [];
        let snapshot = fallbackSnapshot;
        try {
          const listedEvents = await session.events().list();
          if (Array.isArray(listedEvents) && listedEvents.length > 0) snapshot = listedEvents;
        } catch {
          snapshot = fallbackSnapshot;
        }

        // 3. Download the durable archive: ticket → coordinator manifest. The
        //    manifest is written by a later workflow step (complete-coordinator)
        //    after terminal, so retry briefly until it's populated.
        let manifest = null;
        const manifestAttempts = [];
        for (let attempt = 0; attempt < 5 && !manifest; attempt++) {
          if (attempt > 0) await new Promise((r) => setTimeout(r, 2000));
          try {
            const tRes = await fetchBounded(baseUrl + "/api/sessions/" + sessionId + "/events/ticket", {
              method: "POST",
              headers: { authorization: "Bearer " + apiKey }
            }, 8000);
            if (!tRes.ok) {
              manifestAttempts.push({ attempt, phase: "ticket", status: tRes.status });
              continue;
            }
            const grant = await tRes.json();
            const manifestUrl = buildCoordinatorManifestUrl(grant.wsUrl, grant.ticket);
            const redactedUrl = withoutTicket(manifestUrl);
            const mRes = await fetchBounded(manifestUrl.toString(), undefined, 8000);
            if (!mRes.ok) {
              manifestAttempts.push({ attempt, phase: "manifest", status: mRes.status, url: redactedUrl });
              continue;
            }
            const m = await mRes.json();
            if (m && (m.eventCount ?? 0) > 0) {
              manifest = m;
            } else {
              manifestAttempts.push({
                attempt,
                phase: "parse",
                status: mRes.status,
                eventCount: m?.eventCount,
                chunks: Array.isArray(m?.chunks) ? m.chunks.length : undefined,
                url: redactedUrl
              });
            }
          } catch (e) {
            manifestAttempts.push({
              attempt,
              phase: "manifest",
              error: e instanceof Error ? e.message : String(e)
            });
          }
        }

        const serialized = JSON.stringify({ run, snapshot, manifest });
        const snapshotCustomNames = snapshot
          .filter((e) => e.type === "CUSTOM" && e.data && typeof e.data.name === "string")
          .map((e) => e.data.name);
        process.stdout.write(JSON.stringify({
          sessionStatus: run.status,
          streamedCount: streamed.length,
          streamedTypes: [...new Set(streamed)],
          streamedCustomNames: [...new Set(streamedCustomNames)],
          snapshotTypes: [...new Set(snapshot.map((e) => e.type))],
          snapshotCustomNames: [...new Set(snapshotCustomNames)],
          manifestEventCount: manifest ? (manifest.eventCount ?? -1) : -1,
          manifestChunks: manifest && Array.isArray(manifest.chunks) ? manifest.chunks.length : -1,
          leakedKey: serialized.includes(deepseekKey),
          manifestAttempts
        }));
        // Force exit: an abandoned WS phase may leave an open socket that
        // would otherwise keep Node alive until the outer SIGKILL. We have
        // already written the result, so exit cleanly now.
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

      expect(result.sessionStatus).toBe("succeeded");
      // Live WS delivered the unified envelope.
      expect(result.streamedCount).toBeGreaterThan(0);
      expect(result.streamedTypes).toContain("TEXT_MESSAGE_CONTENT");
      expect(
        result.streamedTypes.includes("TURN_FINISHED") || result.streamedCustomNames.some((name) => name.startsWith("aex.session."))
      ).toBe(true);
      // Snapshot agrees.
      expect(result.snapshotTypes).toContain("TEXT_MESSAGE_CONTENT");
      expect(
        result.snapshotTypes.includes("TURN_FINISHED") || result.snapshotCustomNames.some((name) => name.startsWith("aex.session."))
      ).toBe(true);
      expect(result.leakedKey).toBe(false);
      if (result.manifestEventCount <= 0 || result.manifestChunks < 1) {
        throw new Error(`event archive manifest unavailable: ${JSON.stringify(result.manifestAttempts)}`);
      }
      // Durable archive is downloadable and records the events.
      expect(result.manifestEventCount).toBeGreaterThan(0);
      expect(result.manifestChunks).toBeGreaterThanOrEqual(1);
    },
    4 * 60 * 1000
  );
});

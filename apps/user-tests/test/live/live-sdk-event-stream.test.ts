/**
 * Live scenario: live-sdk-event-stream.test.ts
 *
 * Exercises the unified event coordinator end-to-end through the installed
 * SDK, the way a user listening to a run would:
 *
 *   1. submitRun (DeepSeek Managed)
 *   2. LISTEN live over the coordinator WebSocket via `client.streamEnvelopes(runId)`
 *      (ticket broker → coordinator WS, exactly-once cursor resume).
 *   3. SNAPSHOT the same log via `client.listEvents()`.
 *   4. DOWNLOAD the durable event archive: mint a ticket and read the
 *      coordinator manifest (rolling R2 chunks + counts), proving the events
 *      are durably archived and downloadable after the run.
 *
 * Asserts the expected unified-envelope events exist (AG-UI vocabulary):
 * RUN_STARTED, ≥1 TEXT_MESSAGE_CONTENT, RUN_FINISHED — over both the live WS
 * and the snapshot — and that the archive manifest records them.
 *
 * Required env:
 *   ANTPATH_API_URL              live hosted API URL (local or prod)
 *   ANTPATH_API_TOKEN             workspace API token
 *   DEEPSEEK_API_KEY    customer DeepSeek API key
 *   ANTPATH_USER_TEST_TARBALL | ANTPATH_USER_TEST_VERSION
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (event-stream): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("ANTPATH_API_URL");
const apiToken = requireEnv("ANTPATH_API_TOKEN");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const model = process.env["ANTPATH_USER_TEST_DEEPSEEK_MODEL"] ?? "deepseek-chat";

interface StreamResult {
  readonly runStatus: string;
  readonly streamedCount: number;
  readonly streamedTypes: readonly string[];
  readonly snapshotTypes: readonly string[];
  readonly manifestEventCount: number;
  readonly manifestChunks: number;
  readonly leakedKey: boolean;
}

describe("live api.antpath.ai — event coordinator: listen (WS) + snapshot + download archive", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "streams the unified envelope live, snapshots it, and downloads the durable archive manifest",
    async () => {
      const probe = "evt-stream-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { AntpathClient } from "antpath";

        const baseUrl = process.env.ANTPATH_API_URL;
        const apiToken = process.env.ANTPATH_API_TOKEN;
        const deepseekKey = process.env.DEEPSEEK_KEY;
        const model = process.env.MODEL;

        const client = new AntpathClient({ baseUrl, apiToken });
        const runId = await client.submitRun({
          provider: "deepseek",
          model,
          prompt: ${JSON.stringify(`Output verbatim: ${probe}`)},
          idempotencyKey: "user-test-event-stream-" + Date.now(),
          secrets: { deepseek: { apiKey: deepseekKey } }
        });

        // 1. Listen live over the coordinator WebSocket (exactly-once,
        //    reconnecting). Stop on the terminal envelope or the deadline.
        //    The WS phase is RACED against a hard wall-clock timer: if the
        //    SDK's WS path stalls internally (e.g. an eager ticket fetch that
        //    doesn't observe \`signal\`), we abandon listening and fall through
        //    to the snapshot/manifest tail rather than blocking to the outer
        //    SIGKILL. \`ac.abort()\` also fires so the generator can unwind.
        const streamed = [];
        const ac = new AbortController();
        const listen = (async () => {
          try {
            for await (const ev of client.streamEnvelopes(runId, { from: 0, signal: ac.signal })) {
              streamed.push(ev.type);
              if (ev.type === "RUN_FINISHED" || ev.type === "RUN_ERROR") break;
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

        // 2. Snapshot the same log + final status. Postgres \`mark-terminal\`
        //    lands AFTER the terminal WS broadcast, so poll for terminal
        //    status before reading (avoids a pre-terminal status race).
        let run;
        try {
          run = await client.waitForRun(runId, { timeoutMs: 40 * 1000, intervalMs: 2000 });
        } catch (e) {
          run = await client.getRun(runId);
        }
        const snapshot = await client.listEvents(runId);

        // 3. Download the durable archive: ticket → coordinator manifest. The
        //    manifest is written by a later Inngest step (complete-coordinator)
        //    after terminal, so retry briefly until it's populated.
        let manifest = null;
        for (let attempt = 0; attempt < 5 && !manifest; attempt++) {
          if (attempt > 0) await new Promise((r) => setTimeout(r, 2000));
          try {
            const tRes = await fetchBounded(baseUrl + "/api/runs/" + runId + "/events/ticket", {
              method: "POST",
              headers: { authorization: "Bearer " + apiToken }
            }, 8000);
            if (!tRes.ok) continue;
            const grant = await tRes.json();
            const manifestUrl = grant.wsUrl.replace(/^ws/, "http").replace(/\\/subscribe$/, "/manifest");
            const mRes = await fetchBounded(manifestUrl + "?ticket=" + encodeURIComponent(grant.ticket), undefined, 8000);
            if (!mRes.ok) continue;
            const m = await mRes.json();
            if (m && (m.eventCount ?? 0) > 0) manifest = m;
          } catch (e) { /* manifest best-effort — retry */ }
        }

        const serialized = JSON.stringify({ run, snapshot, manifest });
        process.stdout.write(JSON.stringify({
          runStatus: run.status,
          streamedCount: streamed.length,
          streamedTypes: [...new Set(streamed)],
          snapshotTypes: [...new Set(snapshot.map((e) => e.type))],
          manifestEventCount: manifest ? (manifest.eventCount ?? -1) : -1,
          manifestChunks: manifest && Array.isArray(manifest.chunks) ? manifest.chunks.length : -1,
          leakedKey: serialized.includes(deepseekKey)
        }));
        // Force exit: an abandoned WS phase may leave an open socket that
        // would otherwise keep Node alive until the outer SIGKILL. We have
        // already written the result, so exit cleanly now.
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-event-stream-runner.mjs");
      writeFileSync(scriptPath, script);

      const passEnv: Record<string, string> = {
        ANTPATH_API_URL: apiUrl,
        ANTPATH_API_TOKEN: apiToken,
        DEEPSEEK_KEY: deepseekKey,
        MODEL: model
      };
      const pathKey = process.platform === "win32" ? "Path" : "PATH";
      if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
      for (const k of ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData", "HOME", "TMPDIR", "LANG", "LC_ALL"]) {
        if (process.env[k]) passEnv[k] = process.env[k]!;
      }

      const child = await runCommand(process.execPath, [scriptPath], {
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
      expect(result.streamedTypes).toContain("RUN_STARTED");
      expect(result.streamedTypes).toContain("TEXT_MESSAGE_CONTENT");
      expect(result.streamedTypes).toContain("RUN_FINISHED");
      // Snapshot agrees.
      expect(result.snapshotTypes).toContain("RUN_STARTED");
      expect(result.snapshotTypes).toContain("TEXT_MESSAGE_CONTENT");
      expect(result.snapshotTypes).toContain("RUN_FINISHED");
      // Durable archive is downloadable and records the events.
      expect(result.manifestEventCount).toBeGreaterThan(0);
      expect(result.manifestChunks).toBeGreaterThanOrEqual(1);
      expect(result.leakedKey).toBe(false);
    },
    4 * 60 * 1000
  );
});

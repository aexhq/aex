/**
 * Live scenario: live-sdk-logs.test.ts
 *
 * Asserts the UNIFIED stream's `log` channel is visible end-to-end: a real run
 * produces verbose log lines from the platform's services, and a consumer can
 * read them back on the SAME per-run stream as the typed events (one RunLog DO;
 * see the event/log ordering contract).
 *
 *   1. submitRun (Goose Managed) → real run, real services emit logs.
 *   2. wait for terminal.
 *   3. mint a coordinator connection ticket via the hosted API broker, then read
 *      the RAW coordinator snapshot OPTING IN to logs (`/snapshot?channel=log`).
 *      The read path now DEFAULTS to the typed `event` channel only, so logs are
 *      reachable only via the explicit opt-in.
 *   4. read the events endpoint with the logs opt-in (`/events?channel=log`) and
 *      assert it surfaces the unified-stream discriminators (`channel`/`source`/
 *      `sourceSeq`/`emittedAt`) for a log record — proving the read path carries
 *      them through and a consumer can split/filter the one stream by channel.
 *   5. read the DEFAULT events (`client.listEvents()` → `/events`, no channel)
 *      and assert it contains NO `channel:"log"` record but DOES carry the typed
 *      events — proving the default read is events-only (the channel split).
 *
 * Asserts, for each service that CURRENTLY produces logs:
 *   - at least one `channel:"log"` record is present (via the opt-in);
 *   - each log record is well-formed: a known `level`, a non-empty `message`,
 *     a numeric `sourceSeq`, a numeric `emittedAt`, and `channel:"log"`;
 *   - per-source `sourceSeq` is monotonic (the DO's hard per-source guarantee);
 *   - the logs opt-in surfaces channel/source/sourceSeq/emittedAt for a log record;
 *   - the DEFAULT events read excludes log-channel records.
 *
 * Producing services today: the hosted API
 * (source:"worker", run-logger.ts) and the Workflow engine (source:"workflow",
 * the WorkflowTrace). Runner / goose / proxy log sources are a PENDING
 * follow-up — they do not emit on the log channel yet, so they are NOT asserted.
 * To add one later, extend EXPECTED_LOG_SOURCES by a single entry.
 *
 * Required env (mirrors the other live-sdk-* tests; the whole suite skips
 * cleanly when these are absent — they run in CI against the deployed plane):
 *   ANTPATH_LIVE_API_BASE              live hosted API URL (local or prod)
 *   ANTPATH_LIVE_API_TOKEN             workspace API token
 *   ANTPATH_USER_TEST_ANTHROPIC_KEY    customer Anthropic API key
 *   ANTPATH_USER_TEST_TARBALL | ANTPATH_USER_TEST_VERSION
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { installAntpath, runCommand, type InstallResult } from "../_fixtures/install.js";

// Services that emit on the `log` channel TODAY. ADD-ONE-LINE extension point:
// when runner/goose/proxy start emitting logs, append the source here and the
// per-source assertions below pick it up unchanged.
const EXPECTED_LOG_SOURCES = ["worker", "workflow"] as const;
const VALID_LEVELS = new Set(["info", "warn", "error"]);

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (logs): required env ${name} is missing.`);
  }
  return value;
}

const liveApiBase = requireEnv("ANTPATH_LIVE_API_BASE");
const apiToken = requireEnv("ANTPATH_LIVE_API_TOKEN");
const anthropicKey = requireEnv("ANTPATH_USER_TEST_ANTHROPIC_KEY");
const model = process.env["ANTPATH_USER_TEST_ANTHROPIC_MODEL"] ?? "claude-haiku-4-5";

interface LogRecord {
  readonly source: string;
  readonly channel: string;
  readonly level: string;
  readonly message: string;
  readonly sourceSeq: number | null;
  readonly emittedAt: number | null;
}

interface LogsResult {
  readonly runStatus: string;
  readonly snapshotCount: number;
  // Every `channel:"log"` record, projected to the fields under test (read via
  // the raw snapshot's `?channel=log` opt-in).
  readonly logRecords: readonly LogRecord[];
  // The SAME projection, but sourced from the events endpoint's `?channel=log`
  // opt-in — proving the read path carries the discriminators through.
  readonly listEventsLogRecords: readonly LogRecord[];
  // The DEFAULT events read (`client.listEvents()`, no channel): the count of
  // log-channel records (MUST be 0 — the default is events-only) and the count
  // of typed events (MUST be > 0 — the default still carries the event stream).
  readonly defaultLogCount: number;
  readonly defaultEventCount: number;
  readonly leakedKey: boolean;
}

describe("live api.antpath.ai — unified stream: logs from platform services are visible", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAntpath();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  it(
    "submits a goose run, waits for terminal, and reads worker + workflow logs off the unified stream",
    async () => {
      const probe = "logs-" + Math.random().toString(36).slice(2, 8);
      const script = `
        import { AntpathClient } from "antpath";

        const baseUrl = process.env.ANTPATH_API_BASE;
        const apiToken = process.env.ANTPATH_API_TOKEN;
        const anthropicKey = process.env.ANTHROPIC_KEY;
        const model = process.env.MODEL;

        const client = new AntpathClient({ baseUrl, apiToken });
        const runId = await client.submitRun({
          provider: "anthropic",
          runtime: "managed",
          model,
          prompt: ${JSON.stringify(`Output verbatim: ${probe}`)},
          idempotencyKey: "user-test-logs-" + Date.now(),
          secrets: { anthropic: { apiKey: anthropicKey } }
        });

        // Wait for terminal — the Workflow trace flushes on exit, so the
        // workflow logs are only guaranteed present after the run settles.
        let run = null;
        const deadline = Date.now() + 8 * 60 * 1000;
        while (Date.now() < deadline) {
          run = await client.getRun(runId);
          if (["succeeded","failed","cancelled","timed_out"].includes(run.status)) break;
          await new Promise((r) => setTimeout(r, 3_000));
        }

        // Bounded fetch — never ride undici's ~300s default past the outer kill.
        async function fetchBounded(url, init, ms) {
          const a = new AbortController();
          const t = setTimeout(() => a.abort(), ms);
          try { return await fetch(url, { ...(init || {}), signal: a.signal }); }
          finally { clearTimeout(t); }
        }

        // Read the RAW coordinator snapshot via the ticket broker, OPTING IN to
        // the log channel (?channel=log) — the read now defaults to events-only.
        // Mint a ticket, derive the snapshot URL from the broker's wsUrl, and
        // read the log records. Retry: the workflow trace's final flush lands
        // just after terminal.
        let snapshot = [];
        for (let attempt = 0; attempt < 6; attempt++) {
          if (attempt > 0) await new Promise((r) => setTimeout(r, 2500));
          try {
            const tRes = await fetchBounded(baseUrl + "/api/runs/" + runId + "/events/ticket", {
              method: "POST",
              headers: { authorization: "Bearer " + apiToken }
            }, 8000);
            if (!tRes.ok) continue;
            const grant = await tRes.json();
            const snapUrl = grant.wsUrl.replace(/^ws/, "http").replace(/\\/subscribe$/, "/snapshot");
            const sRes = await fetchBounded(snapUrl + "?from=0&channel=log&ticket=" + encodeURIComponent(grant.ticket), undefined, 10000);
            if (!sRes.ok) continue;
            const body = await sRes.json();
            const evs = Array.isArray(body.events) ? body.events : [];
            const logs = evs.filter((e) => e.channel === "log");
            // Got logs from all expected services? stop. Otherwise retry — a
            // flush may still be in flight.
            const sources = new Set(logs.map((e) => e.source));
            snapshot = evs;
            if (${JSON.stringify([...EXPECTED_LOG_SOURCES])}.every((s) => sources.has(s))) break;
          } catch (e) { /* best-effort — retry */ }
        }

        const toLogRecord = (e) => ({
          source: e.source,
          channel: e.channel,
          level: e.data && typeof e.data.level === "string" ? e.data.level : null,
          message: e.data && typeof e.data.message === "string" ? e.data.message : (typeof e.message === "string" ? e.message : null),
          sourceSeq: typeof e.sourceSeq === "number" ? e.sourceSeq : null,
          emittedAt: typeof e.emittedAt === "number" ? e.emittedAt : null
        });

        const logRecords = snapshot.filter((e) => e.channel === "log").map(toLogRecord);

        // The events endpoint's logs opt-in (?channel=log) carries the same
        // unified-stream discriminators the raw snapshot does. It reads the
        // durable archive (R2) once present, so retry briefly.
        let listEventsLogRecords = [];
        for (let attempt = 0; attempt < 6; attempt++) {
          if (attempt > 0) await new Promise((r) => setTimeout(r, 2500));
          try {
            const lRes = await fetchBounded(baseUrl + "/api/runs/" + runId + "/events?channel=log", {
              headers: { authorization: "Bearer " + apiToken }
            }, 10000);
            if (!lRes.ok) continue;
            const lBody = await lRes.json();
            const events = Array.isArray(lBody.events) ? lBody.events : [];
            const logs = events.filter((e) => e.channel === "log").map(toLogRecord);
            if (logs.length > 0) { listEventsLogRecords = logs; break; }
            listEventsLogRecords = logs;
          } catch (e) { /* best-effort — retry */ }
        }

        // The channel split: the DEFAULT events read (no channel) is events-only.
        // It must carry the typed events and contain NO log-channel record.
        let defaultLogCount = 0;
        let defaultEventCount = 0;
        try {
          const defaultEvents = await client.listEvents(runId);
          defaultLogCount = defaultEvents.filter((e) => e.channel === "log").length;
          defaultEventCount = defaultEvents.length;
        } catch (e) { /* leave at 0 — the assertion below fails loudly */ }

        const serialized = JSON.stringify(snapshot);
        process.stdout.write(JSON.stringify({
          runStatus: run ? run.status : "(none)",
          snapshotCount: snapshot.length,
          logRecords,
          listEventsLogRecords,
          defaultLogCount,
          defaultEventCount,
          leakedKey: serialized.includes(anthropicKey)
        }));
        process.exit(0);
      `;
      const scriptPath = join(install.installDir, "live-logs-runner.mjs");
      writeFileSync(scriptPath, script);

      const passEnv: Record<string, string> = {
        ANTPATH_API_BASE: liveApiBase,
        ANTPATH_API_TOKEN: apiToken,
        ANTHROPIC_KEY: anthropicKey,
        MODEL: model
      };
      const pathKey = process.platform === "win32" ? "Path" : "PATH";
      if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
      for (const k of ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData", "HOME", "TMPDIR", "LANG", "LC_ALL"]) {
        if (process.env[k]) passEnv[k] = process.env[k]!;
      }

      const child = await runCommand(process.execPath, [scriptPath], {
        cwd: install.installDir,
        timeoutMs: 10 * 60 * 1000,
        env: passEnv
      });
      if (child.exitCode !== 0) {
        throw new Error(`live runner exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
      }
      const result = JSON.parse(child.stdout.trim()) as LogsResult;

      expect(result.runStatus).toBe("succeeded");
      expect(result.snapshotCount).toBeGreaterThan(0);
      expect(result.leakedKey).toBe(false);

      // Every log record on the unified stream is well-formed.
      for (const rec of result.logRecords) {
        expect(rec.channel).toBe("log");
        expect(VALID_LEVELS.has(rec.level)).toBe(true);
        expect(typeof rec.message).toBe("string");
        expect(rec.message.length).toBeGreaterThan(0);
        expect(typeof rec.sourceSeq).toBe("number");
        expect(typeof rec.emittedAt).toBe("number");
      }

      // Each currently-producing service is present, and its per-source
      // `sourceSeq` is monotonic (the DO's hard per-source guarantee —
      // records of one source are never reordered relative to their sourceSeq).
      for (const source of EXPECTED_LOG_SOURCES) {
        const forSource = result.logRecords.filter((r) => r.source === source);
        expect(forSource.length, `expected >=1 log record from source=${source}`).toBeGreaterThan(0);
        // CF Workflows REPLAY the handler on each resume, and the WorkflowTrace
        // deliberately re-logs its memoized history — accepted as harmless (see
        // event/log ordering contract). So the unified snapshot can
        // carry replay-duplicated records that re-send the SAME (source,sourceSeq).
        // Collapse those by sourceSeq (keep first), THEN assert the order invariant
        // the DO actually guarantees: a single source's records appear in ascending
        // sourceSeq (the snapshot is in global-seq arrival order; never reordered).
        const seen = new Set<number>();
        const seqs = forSource
          .map((r) => r.sourceSeq as number)
          .filter((s) => (seen.has(s) ? false : (seen.add(s), true)));
        const monotonic = seqs.every((s, i) => i === 0 || s > seqs[i - 1]!);
        expect(monotonic, `source=${source} sourceSeq not monotonic after replay-dedup: ${seqs.join(",")}`).toBe(true);
      }

      // The logs opt-in (/events?channel=log) surfaces the unified-stream
      // discriminators (channel/source/sourceSeq/emittedAt) for a log record, so
      // a consumer that opts in can split/filter the one stream by channel.
      expect(
        result.listEventsLogRecords.length,
        "expected /events?channel=log to surface >=1 channel:\"log\" record"
      ).toBeGreaterThan(0);
      for (const rec of result.listEventsLogRecords) {
        expect(rec.channel).toBe("log");
        expect(typeof rec.source).toBe("string");
        expect(VALID_LEVELS.has(rec.level)).toBe(true);
        expect(typeof rec.sourceSeq).toBe("number");
        expect(typeof rec.emittedAt).toBe("number");
      }

      // The channel split: the DEFAULT events read is events-only. It carries
      // the typed events (RUN_STARTED … terminal) but NO log-channel record, so
      // existing event consumers (SDK listEvents/streamEvents, test-contracts
      // expectEventStream, the dashboard, E2E) are never polluted by logs.
      expect(
        result.defaultEventCount,
        "expected the default listEvents() to carry the typed event stream"
      ).toBeGreaterThan(0);
      expect(
        result.defaultLogCount,
        "expected the default listEvents() to contain NO channel:\"log\" records"
      ).toBe(0);
    },
    11 * 60 * 1000
  );
});

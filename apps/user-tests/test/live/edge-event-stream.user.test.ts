/**
 * EDGE-CASE SWEEP — SDK event-stream surface (customer perspective).
 *
 * Surface under test:
 *   - `session.send(...)` live async-iteration over the coordinator WebSocket.
 *   - `session.events().streamEnvelopes({ from, signal, settleConsistent })`
 *     (live AexEvent WS, exactly-once cursor resume).
 *   - `session.events().stream({ intervalMs, signal })` (RunEvent HTTP polling).
 *   - Reconnect / replay-from-seq (the key reliability property): forced
 *     mid-turn socket drops must resume with NO duplicates and NO lost events.
 *   - `idleTimeoutMs` / `pingIntervalMs` keep-alive on a live turn.
 *   - AbortSignal mid-run: clean stop, no unhandled rejection.
 *   - Stream a run that already reached terminal (replay then end, no hang).
 *
 * All cases drive the INSTALLED @aexhq/sdk in a child bun process, exactly like
 * the sibling live tests. One live run is created in `beforeAll` and REUSED by
 * every read-side case (replay/polling/abort-on-replay); only the reconnect,
 * keep-alive, and abort-mid-live cases each cost one extra live run. Model is
 * claude-haiku-4-5 with tiny prompts.
 *
 * Required env: AEX_API_URL, AEX_API_TOKEN, ANTHROPIC_API_KEY,
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-event-stream): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const anthropicKey = requireEnv("ANTHROPIC_API_KEY");
const model = process.env["AEX_USER_TEST_ANTHROPIC_MODEL"]?.trim() || "claude-haiku-4-5";

/**
 * Shared script preamble: build the client + declare helpers every case uses.
 *   - analyze(seqs): duplicate / monotonic report over a sequence array.
 *   - makeFactory({dropAfterFrames,maxDrops}): a chaos/counting WebSocket factory
 *     that wraps the real global WebSocket, counts connects, and can force a
 *     socket close after N delivered frames to exercise reconnect/resume.
 *   - emit(obj): print result JSON (+ any captured unhandled rejection) & exit.
 * No `${` appears here so it is safe to embed verbatim.
 */
const PREAMBLE = `
import { Aex } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
const ANTHROPIC_KEY = process.env.ANTHROPIC_KEY;
const MODEL = process.env.MODEL;
const RUN_ID = process.env.RUN_ID;

let __unhandled = null;
process.on("unhandledRejection", (e) => { __unhandled = (e && e.stack) ? String(e.stack) : String(e); });
process.on("uncaughtException", (e) => { __unhandled = (e && e.stack) ? String(e.stack) : String(e); });

function analyze(seqs) {
  const seen = new Set();
  const dups = [];
  let monotonic = true;
  let gaps = 0;
  for (let i = 0; i < seqs.length; i++) {
    const s = seqs[i];
    if (seen.has(s)) dups.push(s);
    seen.add(s);
    if (i > 0) {
      if (s <= seqs[i - 1]) monotonic = false;
      if (s !== seqs[i - 1] + 1) gaps++;
    }
  }
  return {
    count: seqs.length,
    unique: seen.size,
    dups: dups.slice(0, 20),
    dupCount: dups.length,
    monotonic,
    nonContiguousSteps: gaps,
    min: seqs.length ? seqs[0] : null,
    max: seqs.length ? seqs[seqs.length - 1] : null
  };
}

function typeCounts(events) {
  const m = {};
  for (const e of events) m[e.type] = (m[e.type] || 0) + 1;
  return m;
}
function customNamesOf(events) {
  const s = new Set();
  for (const e of events) {
    if (e.type === "CUSTOM" && e.data && typeof e.data.name === "string") s.add(e.data.name);
  }
  return [...s];
}

// Wraps the real WebSocket. Counts connects; optionally force-closes the socket
// after 'dropAfterFrames' delivered frames, up to 'maxDrops' times total (across
// all reconnections), so we can prove the SDK resumes exactly-once from cursor.
function makeFactory(opts) {
  const dropAfterFrames = (opts && opts.dropAfterFrames) || 0;
  const maxDrops = (opts && opts.maxDrops) || 0;
  let connects = 0;
  let drops = 0;
  const factory = (url) => {
    connects++;
    const real = new WebSocket(url);
    let frames = 0;
    return {
      close(code, reason) { try { real.close(code, reason); } catch (e) {} },
      send(data) { try { real.send(data); } catch (e) {} },
      addEventListener(type, listener) {
        if (type === "message") {
          real.addEventListener("message", (ev) => {
            listener(ev);
            frames++;
            if (maxDrops > 0 && drops < maxDrops && frames >= dropAfterFrames) {
              drops++;
              try { real.close(4001, "chaos-drop"); } catch (e) {}
            }
          });
        } else {
          real.addEventListener(type, (ev) => listener(ev));
        }
      }
    };
  };
  factory.stats = () => ({ connects, drops });
  return factory;
}

async function emit(obj) {
  // Give any pending unhandled rejection a tick to surface before we report.
  await new Promise((r) => setTimeout(r, 250));
  process.stdout.write(JSON.stringify({ ...obj, unhandled: __unhandled }));
  process.exit(0);
}
`;

interface Analyze {
  readonly count: number;
  readonly unique: number;
  readonly dups: readonly number[];
  readonly dupCount: number;
  readonly monotonic: boolean;
  readonly nonContiguousSteps: number;
  readonly min: number | null;
  readonly max: number | null;
}

let install: InstallResult;

async function spawnScript<T>(
  scriptName: string,
  body: string,
  opts: { readonly extraEnv?: Record<string, string>; readonly timeoutMs?: number } = {}
): Promise<T> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${PREAMBLE}\n${body}\n`);

  const passEnv: Record<string, string> = {
    AEX_API_URL: apiUrl,
    AEX_API_TOKEN: apiToken,
    ANTHROPIC_KEY: anthropicKey,
    MODEL: model,
    ...(opts.extraEnv ?? {})
  };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) passEnv[pathKey] = process.env[pathKey]!;
  for (const k of [
    "SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA",
    "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData", "HOME", "TMPDIR", "LANG", "LC_ALL"
  ]) {
    if (process.env[k]) passEnv[k] = process.env[k]!;
  }

  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs: opts.timeoutMs ?? 4 * 60 * 1000,
    env: passEnv
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `child ${scriptName} exited non-zero (${child.exitCode}):\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  const out = child.stdout.trim();
  try {
    return JSON.parse(out) as T;
  } catch {
    throw new Error(`child ${scriptName} did not print JSON:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`);
  }
}

// ---------------------------------------------------------------------------
// Baseline live run — also case A (live async-iteration ordering). Created once,
// reused by every read-side case.
// ---------------------------------------------------------------------------
interface BaseResult {
  readonly runId: string;
  readonly seqs: readonly number[];
  readonly analyze: Analyze;
  readonly typeCounts: Record<string, number>;
  readonly customNames: readonly string[];
  readonly textDenseLen: number;
  readonly streamErrors: readonly unknown[];
  readonly unhandled: string | null;
}
let base: BaseResult;

describe("edge — SDK event stream (streamEnvelopes / stream / reconnect / keep-alive / abort)", () => {
  beforeAll(async () => {
    install = await installAex();
    base = await spawnScript<BaseResult>(
      "edge-evtstream-base.mjs",
      `
      const session = await client.sessions.create({
        provider: "anthropic",
        model: MODEL,
        outputMode: "stream",
        idempotencyKey: ${JSON.stringify("edge-evt-base-")} + Date.now(),
        apiKeys: { anthropic: ANTHROPIC_KEY }
      });
      const events = [];
      const seqs = [];
      let text = "";
      for await (const ev of session.send("Write three short sentences about the ocean. Keep each under 12 words.")) {
        events.push(ev);
        if (typeof ev.sequence === "number") seqs.push(ev.sequence);
        if (ev.type === "TEXT_MESSAGE_CONTENT" && ev.data && typeof ev.data.text === "string") text += ev.data.text;
      }
      const streamErrors = events
        .filter((e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.stream_error")
        .map((e) => e.data && e.data.value ? e.data.value : e.data);
      await emit({
        runId: session.id,
        seqs,
        analyze: analyze(seqs),
        typeCounts: typeCounts(events),
        customNames: customNamesOf(events),
        textDenseLen: text.replace(/\\s+/g, "").length,
        streamErrors
      });
      `,
      { timeoutMs: 4 * 60 * 1000 }
    );
  }, 5 * 60 * 1000);

  afterAll(() => {
    install?.cleanup();
  });

  it("case A — live send() async-iteration: ordered content chunks + terminal + monotonic seq", () => {
    expect(base.unhandled).toBeNull();
    expect(base.runId).toBeTruthy();
    // Assistant text arrived. NOTE (finding): with outputMode:"stream" the managed
    // Anthropic path delivers the whole multi-sentence reply as ONE
    // TEXT_MESSAGE_CONTENT event (no per-token deltas) — see report. We assert >=1
    // (content present) and record the actual count for the report.
    expect(base.typeCounts["TEXT_MESSAGE_CONTENT"] ?? 0).toBeGreaterThanOrEqual(1);
    expect(base.textDenseLen).toBeGreaterThan(0);
    // Session-turn terminal is CUSTOM aex.session.idle (there is NO RUN_FINISHED).
    expect(base.customNames).toContain("aex.session.idle");
    // Strict ordering: monotonic increasing, no duplicates (sequences are sparse,
    // so contiguity is NOT expected — only no-dupe + monotonic).
    expect(base.analyze.monotonic).toBe(true);
    expect(base.analyze.dupCount).toBe(0);
    // Any stream errors would surface as CUSTOM aex.stream_error events (none expected on a clean run).
    expect(base.streamErrors.length).toBe(0);
  });

  it("case B — streamEnvelopes({from:0}) replays a finished run in order and terminates on session-idle; polling stream() agrees", async () => {
    const r = await spawnScript<{
      readonly envTypes: Record<string, number>;
      readonly envCustomNames: readonly string[];
      readonly envAnalyze: Analyze;
      readonly endedNaturally: boolean;
      readonly rfPresent: boolean;
      readonly settleEndedNaturally: boolean;
      readonly settleHasBarrier: boolean;
      readonly pollTypes: Record<string, number>;
      readonly pollCount: number;
      readonly pollErr: string | null;
      readonly unhandled: string | null;
    }>(
      "edge-evtstream-replay0.mjs",
      `
      const session = await client.sessions.open(RUN_ID);
      // 1. streamEnvelopes({from:0}) on a FINISHED session run. Session turns emit
      //    CUSTOM aex.session.idle rather than RUN_FINISHED; the SDK treats that
      //    custom event as terminal so replay ends naturally.
      const envEvents = [];
      const envSeqs = [];
      const ac = new AbortController();
      const start = Date.now();
      const guard = setTimeout(() => ac.abort(), 30000);
      for await (const ev of session.events().streamEnvelopes({ from: 0, signal: ac.signal })) {
        envEvents.push(ev);
        envSeqs.push(ev.sequence);
        // safety: if a run-level terminal ever appears, stop too.
        if (ev.type === "RUN_FINISHED" || ev.type === "RUN_ERROR") break;
      }
      clearTimeout(guard);
      const endedNaturally = !ac.signal.aborted;
      const rfPresent = envEvents.some((e) => e.type === "RUN_FINISHED" || e.type === "RUN_ERROR");

      // 1b. settleConsistent:true is documented to end on the aex.run.settled
      //     barrier. Does that barrier arrive? Guard 15s.
      let settleEndedNaturally = false;
      let settleHasBarrier = false;
      {
        const ac2 = new AbortController();
        const g2 = setTimeout(() => ac2.abort(), 15000);
        try {
          for await (const ev of session.events().streamEnvelopes({ from: 0, signal: ac2.signal, settleConsistent: true })) {
            if (ev.type === "CUSTOM" && ev.data && ev.data.name === "aex.run.settled") settleHasBarrier = true;
          }
        } catch (e) {}
        settleEndedNaturally = !ac2.signal.aborted;
        clearTimeout(g2);
      }

      // 2. Polling RunEvent stream() over the same finished run — one pass, then it
      //    returns because the session is parked. Consistency: same core types.
      let pollTypes = {};
      let pollCount = 0;
      let pollErr = null;
      try {
        const ac3 = new AbortController();
        const guard3 = setTimeout(() => ac3.abort(), 30000);
        const seen = {};
        for await (const ev of session.events().stream({ intervalMs: 800, signal: ac3.signal })) {
          seen[ev.type] = (seen[ev.type] || 0) + 1;
          pollCount++;
          if (pollCount > 1000) break;
        }
        clearTimeout(guard3);
        pollTypes = seen;
      } catch (e) {
        pollErr = String(e);
      }

      await emit({
        envTypes: typeCounts(envEvents),
        envCustomNames: customNamesOf(envEvents),
        envAnalyze: analyze(envSeqs),
        endedNaturally,
        rfPresent,
        settleEndedNaturally,
        settleHasBarrier,
        pollTypes,
        pollCount,
        pollErr
      });
      `,
      { extraEnv: { RUN_ID: base.runId }, timeoutMs: 4 * 60 * 1000 }
    );

    expect(r.unhandled).toBeNull();
    // Correct behavior: the recorded run replays in order, no dupes, and delivers
    // BOTH the assistant text and the session-idle terminal.
    expect(r.envTypes["TEXT_MESSAGE_CONTENT"] ?? 0).toBeGreaterThan(0);
    expect(r.envCustomNames).toContain("aex.session.idle");
    expect(r.envAnalyze.monotonic).toBe(true);
    expect(r.envAnalyze.dupCount).toBe(0);

    // The default session-envelope stream terminates on CUSTOM aex.session.idle,
    // even though session runs still do not emit run-level RUN_FINISHED/RUN_ERROR.
    expect(r.rfPresent).toBe(false);
    expect(
      r.endedNaturally,
      `streamEnvelopes({from:0}) replay did not end naturally (30s guard aborted): ${JSON.stringify({
        envTypes: r.envTypes,
        envCustomNames: r.envCustomNames,
        envAnalyze: r.envAnalyze,
        rfPresent: r.rfPresent,
        pollTypes: r.pollTypes,
        pollCount: r.pollCount,
        pollErr: r.pollErr
      })}`
    ).toBe(true);
    // settleConsistent waits for the post-mirror aex.run.settled barrier on
    // run-finished paths. Managed session turns do not emit that barrier; the
    // stream ends at the session-park terminal because the record is already
    // terminal by then.
    expect(r.settleHasBarrier).toBe(false);
    expect(r.settleEndedNaturally).toBe(true);

    // Polling stream() (RunEvent path) self-terminates on the parked session
    // (unlike streamEnvelopes) and agrees on the presence of assistant text.
    expect(r.pollErr).toBeNull();
    expect(r.pollTypes["TEXT_MESSAGE_CONTENT"] ?? 0).toBeGreaterThan(0);
  });

  it("case C — replay-from-seq: streamEnvelopes({from:midSeq}) yields EXACTLY the tail of the full stream (no gap/dupe)", async () => {
    const r = await spawnScript<{
      readonly fullCount: number;
      readonly midSeq: number;
      readonly midMatchesTail: boolean;
      readonly midAnalyze: Analyze;
      readonly midFirstSeq: number | null;
      readonly beyondFrom: number;
      readonly beyondCount: number;
      readonly beyondEndedNaturally: boolean;
      readonly beyondMs: number;
      readonly unhandled: string | null;
    }>(
      "edge-evtstream-fromseq.mjs",
      `
      const session = await client.sessions.open(RUN_ID);
      async function drain(opts, guardMs, stopOnTerminal) {
        const seqs = [];
        const ac = new AbortController();
        const g = setTimeout(() => ac.abort(), guardMs);
        try {
          for await (const ev of session.events().streamEnvelopes({ ...opts, signal: ac.signal })) {
            seqs.push(ev.sequence);
            const nm = ev.type === "CUSTOM" && ev.data && typeof ev.data.name === "string" ? ev.data.name : "";
            // Session runs have no RUN_FINISHED; the true terminal is aex.session.idle.
            if (stopOnTerminal && (ev.type === "RUN_FINISHED" || ev.type === "RUN_ERROR" || nm === "aex.session.idle")) break;
          }
        } finally {
          clearTimeout(g);
        }
        return { seqs, aborted: ac.signal.aborted };
      }

      // Full replay to learn the sequence space + pick a mid cursor.
      const full = (await drain({ from: 0 }, 40000, true)).seqs;
      const midSeq = full[Math.floor(full.length / 2)];
      const expectedTail = full.filter((s) => s >= midSeq);

      // Resume from the mid cursor — exactly-once resume: must equal the full tail.
      const mid = (await drain({ from: midSeq }, 40000, true)).seqs;
      const midMatchesTail = JSON.stringify(mid) === JSON.stringify(expectedTail);

      // Edge: subscribe from a cursor BEYOND the finished run's terminal. There is
      // no future event to deliver — does it end cleanly or hang until the idle
      // watchdog / reconnect loop? Hard-guarded so the test itself never hangs.
      const beyondFrom = full[full.length - 1] + 1000;
      const start = Date.now();
      const beyond = await drain({ from: beyondFrom }, 12000, false);
      const beyondMs = Date.now() - start;

      await emit({
        fullCount: full.length,
        midSeq,
        midMatchesTail,
        midAnalyze: analyze(mid),
        midFirstSeq: mid.length ? mid[0] : null,
        beyondFrom,
        beyondCount: beyond.seqs.length,
        beyondEndedNaturally: !beyond.aborted,
        beyondMs
      });
      `,
      { extraEnv: { RUN_ID: base.runId }, timeoutMs: 4 * 60 * 1000 }
    );

    expect(r.unhandled).toBeNull();
    expect(r.fullCount).toBeGreaterThan(2);
    // The key reliability property: resume-from-cursor is exactly the tail.
    expect(r.midMatchesTail).toBe(true);
    expect(r.midFirstSeq).toBeGreaterThanOrEqual(r.midSeq);
    expect(r.midAnalyze.monotonic).toBe(true);
    expect(r.midAnalyze.dupCount).toBe(0);
    // NOTE: 'beyond' fields are asserted softly — see report. A finished-run
    // subscribe past the terminal has no event to deliver; we only require it not
    // to deliver phantom events. Whether it self-terminates is captured for the
    // report (design edge), not hard-asserted.
    expect(r.beyondCount).toBe(0);
  });

  it("case D — abort DURING replay via AbortSignal: clean stop, no unhandled rejection", async () => {
    const r = await spawnScript<{
      readonly collected: number;
      readonly threw: string | null;
      readonly aborted: boolean;
      readonly unhandled: string | null;
    }>(
      "edge-evtstream-abort-replay.mjs",
      `
      const session = await client.sessions.open(RUN_ID);
      const ac = new AbortController();
      const collected = [];
      let threw = null;
      try {
        for await (const ev of session.events().streamEnvelopes({ from: 0, signal: ac.signal })) {
          collected.push(ev.sequence);
          if (collected.length >= 2) { ac.abort(); break; }
        }
      } catch (e) {
        threw = String(e);
      }
      await emit({ collected: collected.length, threw, aborted: ac.signal.aborted });
      `,
      { extraEnv: { RUN_ID: base.runId }, timeoutMs: 2 * 60 * 1000 }
    );

    expect(r.unhandled).toBeNull();
    expect(r.threw).toBeNull();
    expect(r.aborted).toBe(true);
    expect(r.collected).toBeGreaterThanOrEqual(1);
  });

  it("case E — reconnect/replay: forced mid-turn socket drops resume exactly-once (no dupes, no lost events)", async () => {
    const r = await spawnScript<{
      readonly runId: string;
      readonly ok: boolean;
      readonly status: string;
      readonly analyze: Analyze;
      readonly typeCounts: Record<string, number>;
      readonly customNames: readonly string[];
      readonly stats: { readonly connects: number; readonly drops: number };
      readonly replayCount: number;
      readonly missingCount: number;
      readonly missingSample: readonly number[];
      readonly unhandled: string | null;
    }>(
      "edge-evtstream-chaos.mjs",
      `
      // Force socket drops mid-turn (after each delivered frame, up to 2 times).
      // The SDK must re-mint a ticket and resume from cursor+1 with no gap/dupe.
      // dropAfterFrames:1 keeps it robust even for sparse, few-event turns.
      const factory = makeFactory({ dropAfterFrames: 1, maxDrops: 2 });
      const result = await client.run({
        provider: "anthropic",
        model: MODEL,
        outputMode: "stream",
        idempotencyKey: ${JSON.stringify("edge-evt-chaos-")} + Date.now(),
        apiKeys: { anthropic: ANTHROPIC_KEY },
        message: "Write four short sentences about mountains. Keep each under 12 words."
      }, { timeoutMs: 3 * 60 * 1000, webSocketFactory: factory });

      const events = Array.isArray(result.events) ? result.events : [];
      const seqs = events.map((e) => e.sequence).filter((s) => typeof s === "number");
      const stats = factory.stats();

      // Clean replay of the SAME finished run: every seq a clean replay delivers
      // MUST also be in the chaotic stream (proves the reconnects lost nothing).
      let replaySeqs = [];
      try {
        const session = await client.sessions.open(result.runId);
        const ac = new AbortController();
        const g = setTimeout(() => ac.abort(), 40000);
        for await (const ev of session.events().streamEnvelopes({ from: 0, signal: ac.signal })) {
          replaySeqs.push(ev.sequence);
          const nm = ev.type === "CUSTOM" && ev.data && typeof ev.data.name === "string" ? ev.data.name : "";
          // A session run has no RUN_FINISHED; stop on the aex.session.idle terminal.
          if (ev.type === "RUN_FINISHED" || ev.type === "RUN_ERROR" || nm === "aex.session.idle") break;
        }
        clearTimeout(g);
      } catch (e) {}

      // The chaos stream comes from send(), which starts at the TURN cursor, while
      // the clean replay starts at 0 (it includes pre-turn events like
      // RUN_STARTED@0). Compare only within the chaos-covered range: every clean
      // seq >= the chaos stream's min must be present in the chaos set (no loss).
      const chaosSet = new Set(seqs);
      const chaosMin = seqs.length ? Math.min(...seqs) : 0;
      const missing = replaySeqs.filter((s) => s >= chaosMin && !chaosSet.has(s));

      await emit({
        runId: result.runId,
        ok: result.ok,
        status: result.status,
        analyze: analyze(seqs),
        typeCounts: typeCounts(events),
        customNames: customNamesOf(events),
        stats,
        chaosMin,
        replayCount: replaySeqs.length,
        missingCount: missing.length,
        missingSample: missing.slice(0, 10)
      });
      `,
      { timeoutMs: 4 * 60 * 1000 }
    );

    expect(r.unhandled).toBeNull();
    // Terminal reached despite the forced drops.
    expect(r.ok).toBe(true);
    // Reconnect actually fired (>1 connect; at least one forced drop taken).
    expect(r.stats.connects).toBeGreaterThan(1);
    expect(r.stats.drops).toBeGreaterThanOrEqual(1);
    // Exactly-once across reconnects: no duplicate sequences, strictly ordered.
    expect(r.analyze.dupCount).toBe(0);
    expect(r.analyze.monotonic).toBe(true);
    // No gap relative to a clean replay: chaotic stream delivered every event.
    expect(r.missingCount).toBe(0);
    // Content + terminal survived the drops.
    expect(r.typeCounts["TEXT_MESSAGE_CONTENT"] ?? 0).toBeGreaterThan(0);
    expect(r.customNames).toContain("aex.session.idle");
  });

  it("case F — keep-alive: short idleTimeoutMs with pings holds a live turn open (exactly-once, terminal reached)", async () => {
    const r = await spawnScript<{
      readonly ok: boolean;
      readonly status: string;
      readonly analyze: Analyze;
      readonly connects: number;
      readonly typeCounts: Record<string, number>;
      readonly customNames: readonly string[];
      readonly unhandled: string | null;
    }>(
      "edge-evtstream-keepalive.mjs",
      `
      // Aggressive watchdog: 800ms idle window, 250ms ping cadence. A pong from the
      // coordinator keeps a legitimately-quiet moment alive; if pings work, connects
      // stays 1 (no false disconnect). Either way the exactly-once contract holds.
      const factory = makeFactory({});
      const result = await client.run({
        provider: "anthropic",
        model: MODEL,
        outputMode: "stream",
        idempotencyKey: ${JSON.stringify("edge-evt-keepalive-")} + Date.now(),
        apiKeys: { anthropic: ANTHROPIC_KEY },
        message: "Write five short sentences about forests. Keep each under 14 words."
      }, { timeoutMs: 3 * 60 * 1000, webSocketFactory: factory, idleTimeoutMs: 800, pingIntervalMs: 250 });

      const events = Array.isArray(result.events) ? result.events : [];
      const seqs = events.map((e) => e.sequence).filter((s) => typeof s === "number");
      await emit({
        ok: result.ok,
        status: result.status,
        analyze: analyze(seqs),
        connects: factory.stats().connects,
        typeCounts: typeCounts(events),
        customNames: customNamesOf(events)
      });
      `,
      { timeoutMs: 4 * 60 * 1000 }
    );

    expect(r.unhandled).toBeNull();
    expect(r.ok).toBe(true);
    // Contract holds regardless of reconnects: no dupes, strictly ordered, terminal reached.
    expect(r.analyze.dupCount).toBe(0);
    expect(r.analyze.monotonic).toBe(true);
    expect(r.typeCounts["TEXT_MESSAGE_CONTENT"] ?? 0).toBeGreaterThan(0);
    expect(r.customNames).toContain("aex.session.idle");
    // connects is reported: 1 => pings prevented a false disconnect; >1 => a
    // reconnect happened but recovered exactly-once. At least one connect.
    expect(r.connects).toBeGreaterThanOrEqual(1);
  });

  it("case G — abort mid-LIVE-run via AbortSignal: clean stop, no unhandled rejection", async () => {
    const r = await spawnScript<{
      readonly collected: number;
      readonly threw: string | null;
      readonly aborted: boolean;
      readonly bg: string;
      readonly unhandled: string | null;
    }>(
      "edge-evtstream-abort-live.mjs",
      `
      const session = await client.sessions.create({
        provider: "anthropic",
        model: MODEL,
        outputMode: "stream",
        idempotencyKey: ${JSON.stringify("edge-evt-abortlive-")} + Date.now(),
        apiKeys: { anthropic: ANTHROPIC_KEY }
      });
      // Kick the turn live in the background (its own WS); we abort a SEPARATE
      // streamEnvelopes subscription with a signal while the run is producing.
      const bg = session
        .send("Write six short sentences about deserts. Keep each under 14 words.")
        .done()
        .then(() => "done")
        .catch((e) => "error:" + String(e));
      await new Promise((r) => setTimeout(r, 600));

      const ac = new AbortController();
      const collected = [];
      let threw = null;
      try {
        for await (const ev of session.events().streamEnvelopes({ from: 0, signal: ac.signal })) {
          collected.push(ev.sequence);
          if (collected.length >= 2) { ac.abort(); break; }
        }
      } catch (e) {
        threw = String(e);
      }
      // Let the background turn settle so the process exits clean.
      const bgres = await Promise.race([
        bg,
        new Promise((r) => setTimeout(() => r("timeout"), 100000))
      ]);
      await emit({ collected: collected.length, threw, aborted: ac.signal.aborted, bg: String(bgres).slice(0, 40) });
      `,
      { timeoutMs: 4 * 60 * 1000 }
    );

    expect(r.unhandled).toBeNull();
    expect(r.threw).toBeNull();
    expect(r.aborted).toBe(true);
    expect(r.collected).toBeGreaterThanOrEqual(1);
  });
});

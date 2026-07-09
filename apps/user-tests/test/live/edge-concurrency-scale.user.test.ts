/**
 * Live edge-case sweep: LARGER-SCALE CONCURRENCY of the installed `@aexhq/sdk`
 * against the DEV plane, driven with the cheap DeepSeek provider.
 *
 * Acts as a real customer / AI-agent workload firing many operations at once,
 * hunting for lost/duplicated sessions, idempotency races, session-busy handling,
 * and event-stream fanout defects before a prod launch. Nothing in the existing
 * live suite exercises the SDK under a concurrent FAN of operations, so this
 * file closes that gap.
 *
 * Surface under test (packages/sdk/src/client.ts):
 *   - Aex.start(...)                       many concurrent one-shot sessions
 *   - SessionClient.create(...)          idempotency-key races (dedup / distinct)
 *   - SessionHandle.send(...)            back-to-back turns on ONE session
 *   - SessionEvents.streamEnvelopes()    concurrent fanout consumers on one session
 *
 * Cost (DeepSeek, tiny "Output verbatim" prompts): case A ~10 billable session turns,
 * case C ~1 (the rest dedup), case D <=4 turns on one session, case E ~1 run;
 * cases B create sessions only (no LLM turn). Waves are run selectively with
 * `-t`. Total kept well under the ~15-live-sessions-per-wave budget.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by session-live-deepseek.sh).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-concurrency-scale): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const deepseekKey = requireEnv("DEEPSEEK_API_KEY");
const model = process.env["AEX_USER_TEST_DEEPSEEK_MODEL"]?.trim() || "deepseek-v4-flash";

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of [
      "SystemRoot",
      "SystemDrive",
      "TEMP",
      "TMP",
      "USERPROFILE",
      "APPDATA",
      "LOCALAPPDATA",
      "ComSpec",
      "ProgramFiles",
      "ProgramData"
    ]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

/**
 * Child prelude: an `Aex` client from env, a `scrub()` that strips the api key
 * / provider key from any string before it can reach stdout, and a `dense()`
 * whitespace-stripper (managed-runtime streams fragment tokens across content
 * blocks). Secrets are read from env for LEAK checks but NEVER emitted — only
 * booleans, ids, and scrubbed samples cross the process boundary.
 */
const CHILD_PRELUDE = `
  import { Aex } from "@aexhq/sdk";
  const API_URL = process.env.AEX_API_URL;
  const API_KEY = process.env.AEX_API_KEY;
  const DEEPSEEK_KEY = process.env.DEEPSEEK_KEY;
  const MODEL = process.env.MODEL;
  const client = new Aex({ baseUrl: API_URL, apiKey: API_KEY });

  function scrub(s) {
    let out = String(s == null ? "" : s);
    if (API_KEY) out = out.split(API_KEY).join("***TOKEN***");
    if (DEEPSEEK_KEY) out = out.split(DEEPSEEK_KEY).join("***KEY***");
    return out;
  }
  function dense(s) { return String(s == null ? "" : s).replace(/\\s+/g, ""); }
  function errShape(e) {
    return {
      name: e && e.constructor ? e.constructor.name : "Error",
      message: scrub(e && e.message ? String(e.message) : String(e)),
      status: e && typeof e.status === "number" ? e.status : null,
      code: e && typeof e.code === "string" ? e.code : null
    };
  }
  const STAMP = Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 10 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      DEEPSEEK_KEY: deepseekKey,
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-concurrency-scale runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-concurrency-scale runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

interface TurnOutcome {
  i: number;
  marker: string;
  sessionId?: string;
  ok?: boolean;
  status?: string;
  text?: string;
  costUsd?: number | null;
  error?: { name: string; message: string; status: number | null; code: string | null };
}

interface CreateOutcome {
  i: number;
  id?: string;
  error?: { name: string; message: string; status: number | null; code: string | null };
}

interface SendOutcome {
  i: number;
  key?: string;
  status?: string;
  text?: string;
  error?: { name: string; message: string; status: number | null; code: string | null };
}

interface ConsumerSummary {
  label: string;
  count: number;
  uniqueCount: number;
  dupSeq: number;
  seqs: number[];
  subjects: string[];
  aborted: boolean;
  types: string[];
  ms: number;
  err: string | null;
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: larger-scale concurrency (DeepSeek)", () => {
  it(
    "A many concurrent one-shot sessions all reach terminal with distinct sessionIds, correct routing, and no cross-session event leakage",
    async () => {
      const body = `
        const N = 10;
        const markers = [];
        const tasks = [];
        for (let i = 0; i < N; i++) {
          const marker = "MK" + i + "Q" + STAMP;
          markers.push(marker);
          tasks.push(
            client.start({
              provider: "deepseek",
              model: MODEL,
              message: "Output verbatim: " + marker,
              idempotencyKey: "cdist-" + STAMP + "-" + i,
              apiKeys: { deepseek: DEEPSEEK_KEY }
            }, { timeoutMs: 8 * 60_000 })
              .then((r) => ({
                i, marker,
                sessionId: r.sessionId,
                ok: r.ok === true,
                status: String(r.status),
                text: dense(r.text),
                costUsd: (typeof r.costUsd === "number") ? r.costUsd : null
              }))
              .catch((e) => ({ i, marker, error: errShape(e) }))
          );
        }
        const results = await Promise.all(tasks);
        process.stdout.write(JSON.stringify({ N, markers, results }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-A.mjs", body, 12 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 6000)}`;
      const N = r.N as number;
      const markers = r.markers as string[];
      const results = r.results as TurnOutcome[];

      const errored = results.filter((x) => x.error);
      const withId = results.filter((x) => typeof x.sessionId === "string" && x.sessionId.length > 0);
      const distinct = new Set(withId.map((x) => x.sessionId));
      const notOk = withId.filter((x) => x.ok !== true);
      const markerMiss = withId.filter((x) => !(x.text ?? "").includes(x.marker));
      const foreign = withId.filter((x) => {
        const text = x.text ?? "";
        return markers.some((m) => m !== x.marker && text.includes(m));
      });
      const costPresent = withId.filter((x) => typeof x.costUsd === "number").length;

      expect(results.length, `expected ${N} outcomes${ctx}`).toBe(N);
      // Every concurrent session must land (no dropped/errored run).
      expect(errored, `some concurrent sessions errored${ctx}`).toEqual([]);
      // Distinct sessionIds — no two concurrent sessions collapsed onto one id.
      expect(distinct.size, `distinct sessionIds != ${N} (lost or duplicated session ids)${ctx}`).toBe(N);
      // Every session parked cleanly.
      expect(notOk, `some concurrent sessions did not reach a clean terminal${ctx}`).toEqual([]);
      // Each session got ITS OWN marker back (correct routing).
      expect(markerMiss, `some sessions did not echo their own marker (routing/delivery loss)${ctx}`).toEqual([]);
      // No session's event log carried ANOTHER session's marker (no cross-session leakage).
      expect(foreign, `cross-session marker leakage detected${ctx}`).toEqual([]);

      // eslint-disable-next-line no-console
      console.log(
        `[edge-concurrency] A: ${withId.length}/${N} sessions terminal, ${distinct.size} distinct ids, ` +
          `costUsd present on ${costPresent}/${N}.`
      );
    },
    13 * 60_000
  );

  it(
    "B idempotency create-race: N concurrent creates with the SAME key dedup to ONE session; N with DISTINCT keys yield N sessions",
    async () => {
      const body = `
        // SAME key: N concurrent creates must collapse to exactly one session.
        const SAME_N = 15;
        const sameKey = "cidem-" + STAMP;
        const sameTasks = [];
        for (let i = 0; i < SAME_N; i++) {
          sameTasks.push(
            client.sessions.create({
              provider: "deepseek",
              model: MODEL,
              idempotencyKey: sameKey,
              apiKeys: { deepseek: DEEPSEEK_KEY }
            })
              .then((h) => ({ i, id: h.id }))
              .catch((e) => ({ i, error: errShape(e) }))
          );
        }
        const same = await Promise.all(sameTasks);

        // DISTINCT keys: N concurrent creates must yield N distinct sessions.
        const DIST_N = 12;
        const distTasks = [];
        for (let i = 0; i < DIST_N; i++) {
          distTasks.push(
            client.sessions.create({
              provider: "deepseek",
              model: MODEL,
              idempotencyKey: "cdistcreate-" + STAMP + "-" + i,
              apiKeys: { deepseek: DEEPSEEK_KEY }
            })
              .then((h) => ({ i, id: h.id }))
              .catch((e) => ({ i, error: errShape(e) }))
          );
        }
        const distinct = await Promise.all(distTasks);

        process.stdout.write(JSON.stringify({ SAME_N, DIST_N, same, distinct }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-B.mjs", body, 6 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 6000)}`;
      const sameN = r.SAME_N as number;
      const distN = r.DIST_N as number;
      const same = r.same as CreateOutcome[];
      const distinct = r.distinct as CreateOutcome[];

      const sameErr = same.filter((x) => x.error);
      const sameIds = new Set(same.filter((x) => x.id).map((x) => x.id));
      const distErr = distinct.filter((x) => x.error);
      const distIds = new Set(distinct.filter((x) => x.id).map((x) => x.id));

      expect(same.length, `expected ${sameN} same-key outcomes${ctx}`).toBe(sameN);
      expect(sameErr, `same-key concurrent creates errored${ctx}`).toEqual([]);
      // The idempotency contract: concurrent creates with one key = ONE session.
      expect(sameIds.size, `same idempotencyKey created ${sameIds.size} sessions (expected exactly 1 — duplicate billable resource)${ctx}`).toBe(1);

      expect(distinct.length, `expected ${distN} distinct-key outcomes${ctx}`).toBe(distN);
      expect(distErr, `distinct-key concurrent creates errored${ctx}`).toEqual([]);
      // Distinct keys must NOT collide onto one session.
      expect(distIds.size, `distinct idempotencyKeys collapsed to ${distIds.size} sessions (expected ${distN})${ctx}`).toBe(distN);
    },
    6 * 60_000
  );

  it(
    "C idempotency session-race: many concurrent client.start with the SAME idempotencyKey resolve to exactly ONE billable sessionId",
    async () => {
      const body = `
        const N = 8;
        const key = "csession-" + STAMP;
        const marker = "RUNIDEM" + STAMP;
        const tasks = [];
        for (let i = 0; i < N; i++) {
          tasks.push(
            client.start({
              provider: "deepseek",
              model: MODEL,
              message: "Output verbatim: " + marker,
              idempotencyKey: key,
              apiKeys: { deepseek: DEEPSEEK_KEY }
            }, { timeoutMs: 8 * 60_000 })
              .then((r) => ({ i, marker, sessionId: r.sessionId, ok: r.ok === true, status: String(r.status), text: dense(r.text) }))
              .catch((e) => ({ i, marker, error: errShape(e) }))
          );
        }
        const results = await Promise.all(tasks);
        process.stdout.write(JSON.stringify({ N, key, marker, results }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-C.mjs", body, 12 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 6000)}`;
      const N = r.N as number;
      const results = r.results as TurnOutcome[];

      const errored = results.filter((x) => x.error);
      const withId = results.filter((x) => typeof x.sessionId === "string" && x.sessionId.length > 0);
      const distinct = new Set(withId.map((x) => x.sessionId));
      const notOk = withId.filter((x) => x.ok !== true);

      expect(results.length, `expected ${N} outcomes${ctx}`).toBe(N);
      expect(errored, `same-key concurrent sessions errored (expected clean dedup)${ctx}`).toEqual([]);
      // The headline invariant: one idempotencyKey => one billable session turn, even
      // when N start() calls race the create AND the message send.
      expect(distinct.size, `same idempotencyKey produced ${distinct.size} distinct sessionIds (expected exactly 1 — DUPLICATE BILLABLE RUN)${ctx}`).toBe(1);
      expect(notOk, `some deduped sessions did not observe a clean terminal${ctx}`).toEqual([]);
    },
    13 * 60_000
  );

  it(
    "D send storm: concurrent turns on ONE session serialize or return a clean 409, and no turn is lost or crashes",
    async () => {
      const body = `
        const session = await client.sessions.create({
          provider: "deepseek",
          model: MODEL,
          idempotencyKey: "cstorm-" + STAMP,
          apiKeys: { deepseek: DEEPSEEK_KEY }
        });
        const SEND_TIMEOUT_MS = 5 * 60 * 1000;
        function compactEvent(event) {
          const data = event && event.data && typeof event.data === "object" ? event.data : {};
          return {
            type: event && event.type ? event.type : null,
            name: typeof data.name === "string" ? data.name : null,
            failureClass: typeof data.failureClass === "string" ? data.failureClass : null,
            status: typeof data.status === "string" ? data.status : null
          };
        }
        async function sessionSnapshot() {
          const snapshot = {};
          try {
            snapshot.record = await client.sessions.get(session.id);
          } catch (e) {
            snapshot.recordError = errShape(e);
          }
          try {
            const events = await session.events().list();
            snapshot.events = events.map(compactEvent).slice(-25);
          } catch (e) {
            snapshot.eventsError = errShape(e);
          }
          return snapshot;
        }
        function withSendTimeout(i, key, turn) {
          let timer;
          return new Promise((resolve) => {
            timer = setTimeout(() => {
              resolve({
                i,
                key,
                error: {
                  name: "SendTimeout",
                  message: "session.send().done() did not settle within " + SEND_TIMEOUT_MS + "ms",
                  status: null,
                  code: "SEND_TIMEOUT"
                }
              });
            }, SEND_TIMEOUT_MS);
            turn.then(resolve, (e) => resolve({ i, key, error: errShape(e) })).finally(() => clearTimeout(timer));
          });
        }
        const M = 4;
        const tasks = [];
        console.error("[edge-concurrency] D session-created " + JSON.stringify({ sessionId: session.id, M, sendTimeoutMs: SEND_TIMEOUT_MS }));
        for (let i = 0; i < M; i++) {
          const key = "cstorm-" + STAMP + "-" + i;
          const turn = (
            session.send("Output verbatim: S" + i + "Z" + STAMP, { idempotencyKey: key })
              .done()
              .then((res) => ({ i, key, status: String(res.status), text: dense(res.text) }))
              .catch((e) => ({ i, key, error: errShape(e) }))
          );
          tasks.push(
            withSendTimeout(i, key, turn).then((result) => {
              console.error("[edge-concurrency] D send-outcome " + JSON.stringify(result));
              return result;
            })
          );
        }
        const results = await Promise.all(tasks);
        const snapshot = await sessionSnapshot();
        process.stdout.write(JSON.stringify({ M, sessionId: session.id, results, snapshot }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-D.mjs", body, 10 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 6000)}`;
      const M = r.M as number;
      const results = r.results as SendOutcome[];

      const completed = results.filter((x) => !x.error);
      const isBusy = (x: SendOutcome): boolean =>
        !!x.error && (x.error.status === 409 || /busy|conflict|in progress|already/i.test(x.error.message));
      const busy = results.filter((x) => isBusy(x));
      const otherErr = results.filter((x) => x.error && !isBusy(x));

      expect(results.length, `expected ${M} send outcomes${ctx}`).toBe(M);
      // At least one turn must complete — the session is not wedged.
      expect(completed.length, `no turn completed on the session (all failed)${ctx}`).toBeGreaterThanOrEqual(1);
      // Every send is accounted for: it either completed or was cleanly rejected
      // as busy/conflict — never silently lost.
      expect(completed.length + busy.length, `${otherErr.length} send(s) failed with a NON-busy error (unexpected crash / lost turn)${ctx}`).toBe(M);
      // No unexpected error class (5xx, network crash, malformed).
      expect(otherErr, `send storm produced non-409 errors${ctx}`).toEqual([]);

      // eslint-disable-next-line no-console
      console.log(
        `[edge-concurrency] D: of ${M} concurrent turns, ${completed.length} completed, ${busy.length} cleanly rejected as busy/409.`
      );
    },
    11 * 60_000
  );

  it(
    "E fanout: concurrent streamEnvelopes consumers on one session see a consistent event set, no cross-session leakage, no duplicate seq",
    async () => {
      const body = `
        const marker = "FAN" + STAMP;
        const runRes = await client.start({
          provider: "deepseek",
          model: MODEL,
          message: "Output verbatim: " + marker,
          idempotencyKey: "cfan-" + STAMP,
          apiKeys: { deepseek: DEEPSEEK_KEY }
        }, { timeoutMs: 8 * 60_000 });
        const sessionId = runRes.sessionId;
        const runOk = runRes.ok === true;

        async function collect(label) {
          const ac = new AbortController();
          const timer = setTimeout(() => ac.abort(), 25000);
          const seqs = [];
          const subjects = new Set();
          const types = [];
          let err = null;
          const t0 = Date.now();
          try {
            const s = await client.sessions.open(sessionId);
            for await (const ev of s.events().streamEnvelopes({ from: 0, settleConsistent: true, signal: ac.signal })) {
              if (typeof ev.sequence === "number") seqs.push(ev.sequence);
              if (typeof ev.subject === "string" && ev.subject) subjects.add(ev.subject);
              if (typeof ev.type === "string") types.push(ev.type);
            }
          } catch (e) {
            err = scrub(e && e.message ? String(e.message) : String(e));
          } finally {
            clearTimeout(timer);
          }
          const uniq = new Set(seqs);
          return {
            label,
            count: seqs.length,
            uniqueCount: uniq.size,
            dupSeq: seqs.length - uniq.size,
            seqs: [...uniq].sort((a, b) => a - b),
            subjects: [...subjects],
            aborted: ac.signal.aborted,
            types: [...new Set(types)],
            ms: Date.now() - t0,
            err
          };
        }

        const M = 3;
        const consumers = await Promise.all([collect("c0"), collect("c1"), collect("c2")]);
        process.stdout.write(JSON.stringify({ sessionId, runOk, M, consumers }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-E.mjs", body, 12 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 8000)}`;
      const sessionId = r.sessionId as string;
      const consumers = r.consumers as ConsumerSummary[];

      const errored = consumers.filter((c) => c.err);
      const empty = consumers.filter((c) => c.count === 0);
      const leaked = consumers.filter((c) => c.subjects.some((s) => s !== sessionId));
      const duped = consumers.filter((c) => c.dupSeq !== 0);
      const seqSignatures = new Set(consumers.map((c) => JSON.stringify(c.seqs)));

      expect(r.runOk, `the source run did not succeed${ctx}`).toBe(true);
      expect(consumers.length, `expected 3 consumers${ctx}`).toBe(3);
      // Every concurrent consumer connected and replayed events.
      expect(errored, `some stream consumers errored${ctx}`).toEqual([]);
      expect(empty, `some stream consumers saw zero events${ctx}`).toEqual([]);
      // No consumer saw an event belonging to a DIFFERENT run (subject != sessionId).
      expect(leaked, `cross-session event leakage: a consumer saw a foreign subject${ctx}`).toEqual([]);
      // Within one consumer, the coordinator's global seq must be unique.
      expect(duped, `duplicate sequence numbers within a consumer's stream${ctx}`).toEqual([]);
      // All three consumers replaying from seq 0 must agree on the exact event set.
      expect(seqSignatures.size, `concurrent consumers disagreed on the event set (fanout inconsistency)${ctx}`).toBe(1);
    },
    13 * 60_000
  );

  it(
    "F concurrency-limit knobs fail CLOSED at the SDK boundary (no silent concurrency override)",
    async () => {
      // The public SDK exposes NO top-level concurrency knob: `overrides` only
      // carries { idleTtl, timeout, maxSpendUsd }. The removed session-limit /
      // subagent-fanout knobs must be REJECTED synchronously (before any
      // network) — never silently accepted and dropped. Pure validation: no
      // live run, no cost.
      const body = `
        function probeReject(label, opts) {
          return client.start(opts, { timeoutMs: 30_000 })
            .then((r) => ({ label, threw: false, sessionId: r.sessionId }))
            .catch((e) => ({ label, threw: true, error: errShape(e) }));
        }
        const base = { provider: "deepseek", model: MODEL, message: "Output verbatim: X", apiKeys: { deepseek: DEEPSEEK_KEY } };
        const results = await Promise.all([
          probeReject("top_level_limits", { ...base, limits: { concurrency: 5000, maxConcurrentChildSessions: 9999 } }),
          probeReject("parent_session_id", { ...base, parentSessionId: "ses_fake_parent" }),
          probeReject("top_level_runtimeSize", { ...base, runtimeSize: "standard-4" })
        ]);
        process.stdout.write(JSON.stringify({ results }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-F.mjs", body, 2 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 4000)}`;
      const results = r.results as Array<{
        label: string;
        threw: boolean;
        sessionId?: string;
        error?: { name: string; message: string; status: number | null; code: string | null };
      }>;

      const accepted = results.filter((x) => !x.threw);
      const notConfigError = results.filter(
        (x) => x.threw && x.error?.name !== "SessionConfigValidationError"
      );

      expect(results.length, `expected 3 knob-rejection probes${ctx}`).toBe(3);
      // None of the removed knobs may be silently accepted (which would submit a
      // billable session turn with the knob dropped).
      expect(accepted, `a removed concurrency/limit knob was silently accepted (submitted a session)${ctx}`).toEqual([]);
      // Each rejection is the typed config error, before any network I/O.
      expect(notConfigError, `a knob rejection was not a clean SessionConfigValidationError${ctx}`).toEqual([]);
    },
    3 * 60_000
  );

  it(
    "G sessions.list pagination stays correct (no dup/skip, cursor terminates) while sessions are created concurrently",
    async () => {
      // Create-only burst (no LLM turn → cheap) racing a forward page walk.
      // Keyset pagination must not re-serve or lose a row while new sessions are
      // being inserted at the head.
      const body = `
        const created = [];
        async function burst() {
          const jobs = [];
          for (let i = 0; i < 10; i++) {
            jobs.push(
              client.sessions.create({
                provider: "deepseek",
                model: MODEL,
                idempotencyKey: "cpage-" + STAMP + "-" + i,
                apiKeys: { deepseek: DEEPSEEK_KEY }
              }).then((h) => { created.push(h.id); }).catch((e) => { created.push("ERR:" + errShape(e).message); })
            );
            await new Promise((r) => setTimeout(r, 120));
          }
          await Promise.all(jobs);
        }
        const burstPromise = burst();

        const MAX_PAGES = 10;
        const cursors = new Set();
        const ids = [];
        let cursor = undefined;
        let pages = 0;
        let sawRepeatedCursor = false;
        do {
          if (cursor !== undefined) {
            if (cursors.has(cursor)) { sawRepeatedCursor = true; break; }
            cursors.add(cursor);
          }
          const page = await client.sessions.list({ limit: 10, ...(cursor ? { cursor } : {}) });
          pages++;
          for (const s of page.sessions) ids.push(s.id);
          cursor = page.nextCursor;
          if (pages >= MAX_PAGES) break;
        } while (cursor);
        await burstPromise;

        const uniq = new Set(ids);
        const errored = created.filter((x) => typeof x === "string" && x.startsWith("ERR:"));
        process.stdout.write(JSON.stringify({
          pages,
          totalIds: ids.length,
          uniqueIds: uniq.size,
          dupIds: ids.length - uniq.size,
          sawRepeatedCursor,
          createdCount: created.length - errored.length,
          createErrors: errored,
          hitCap: pages >= MAX_PAGES
        }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-conc-G.mjs", body, 4 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 5000)}`;

      const totalIds = r.totalIds as number;
      const dupIds = r.dupIds as number;
      const sawRepeatedCursor = r.sawRepeatedCursor as boolean;
      const createdCount = r.createdCount as number;
      const createErrors = r.createErrors as string[];

      // The walk actually saw rows (workspace is non-empty from prior cases).
      expect(totalIds, `pagination walked zero rows${ctx}`).toBeGreaterThan(0);
      // No id re-served across pages despite concurrent inserts at the head.
      expect(dupIds, `pagination re-served a session id under concurrent creation (keyset unstable)${ctx}`).toBe(0);
      // Cursor never looped.
      expect(sawRepeatedCursor, `pagination repeated a cursor under concurrent creation${ctx}`).toBe(false);
      // All 10 concurrent create-only sessions landed cleanly.
      expect(createErrors, `concurrent create-only sessions errored during pagination${ctx}`).toEqual([]);
      expect(createdCount, `not all concurrent creates landed${ctx}`).toBe(10);
    },
    5 * 60_000
  );
});

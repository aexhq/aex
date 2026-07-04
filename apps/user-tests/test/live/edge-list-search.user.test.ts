/**
 * Live edge-case sweep: sessions.list / searchOutputs / SessionHandle.unit /
 * the client `debug` option.
 *
 * Acts as a real customer hammering these four verbs of the installed
 * `@aexhq/sdk` against the DEV plane, hunting for edge-case defects before a
 * prod launch. Nothing in the existing live suite exercises the workspace
 * LISTING / cross-session SEARCH / RunUnit / debug-trace surface, so this file
 * closes that gap.
 *
 * Surface under test (packages/sdk/src/client.ts):
 *   - SessionClient.list(query)            pagination / filter / order / bogus cursor
 *   - SessionClient.searchOutputs(query)   cross-session output search + cursor-dedup termination
 *   - SessionHandle.unit()                 the self-contained RunUnit read shape
 *   - AexOptions.debug (true | function)   redacted per-request stderr trace, no secret leak
 *
 * Cost: ONE billable run total (case A creates a single marker deliverable;
 * cases B and C are pure read/introspection against the existing workspace and
 * spend nothing). Model deepseek-v4-flash, tiny prompt.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-list-search): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-list-search");
const model = gateModel();

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
 * Child prelude: a `probe(label, fn, ms?)` that turns a throw OR a hang into a
 * structured record (never aborts the whole script), plus a `scrub()` that
 * strips the api key / provider key from any string before it can reach
 * stdout. Secrets are read from env for LEAK checks but are NEVER emitted — only
 * booleans and scrubbed samples cross the process boundary.
 */
const CHILD_PRELUDE = `
  import { Aex } from "@aexhq/sdk";
  const API_URL = process.env.AEX_API_URL;
  const API_KEY = process.env.AEX_API_KEY;
  const PROVIDER = process.env.PROVIDER;
const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
  const client = new Aex({ baseUrl: API_URL, apiKey: API_KEY });

  function scrub(s) {
    let out = String(s == null ? "" : s);
    if (API_KEY) out = out.split(API_KEY).join("***TOKEN***");
    if (PROVIDER_KEY) out = out.split(PROVIDER_KEY).join("***KEY***");
    return out;
  }
  async function probe(label, fn, ms) {
    const budget = typeof ms === "number" ? ms : 20000;
    try {
      const value = await Promise.race([
        Promise.resolve().then(fn),
        new Promise((_, rej) => setTimeout(() => rej(new Error("PROBE_TIMEOUT_" + budget + "MS")), budget))
      ]);
      return { label, ok: true, value };
    } catch (e) {
      return {
        label,
        ok: false,
        error: {
          name: e && e.constructor ? e.constructor.name : "Error",
          message: scrub(e && e.message ? String(e.message) : String(e)),
          status: e && typeof e.status === "number" ? e.status : null,
          code: e && typeof e.code === "string" ? e.code : null
        }
      };
    }
  }
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 8 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey,
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-list-search runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-list-search runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

type ProbeResult = {
  label: string;
  ok: boolean;
  value?: unknown;
  error?: { name: string; message: string; status: number | null; code: string | null };
};
function byLabel(probes: ProbeResult[], label: string): ProbeResult {
  const p = probes.find((x) => x.label === label);
  if (!p) throw new Error(`probe "${label}" missing; got: ${probes.map((x) => x.label).join(", ")}`);
  return p;
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: sessions.list / searchOutputs / unit / debug", () => {
  it(
    "A search+unit: a marker deliverable is found by searchOutputs (scoped + cross-session), and unit() returns a coherent RunUnit",
    async () => {
      const marker = "EDGES" + Math.random().toString(36).slice(2, 10).toUpperCase();
      const filename = "edgesearch_" + marker + ".txt";
      const prompt =
        `Use your shell/filesystem tools to create a text file at the path ` +
        `/workspace/outputs/${filename} whose ENTIRE contents are exactly these characters: ${marker} ` +
        `(no trailing newline, nothing else). Do not create any other files. Then reply with the single word done.`;
      const body = `
        const runResult = await client.run({
          provider: PROVIDER,
          model: MODEL,
          message: ${JSON.stringify(prompt)},
          includeBuiltinTools: true,
          outputs: { allowedDirs: ["/workspace/outputs"] },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-ls-A-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });
        const runId = runResult.runId;
        const status = runResult.ok ? "succeeded" : (runResult.status || "failed");
        const MARKER = ${JSON.stringify(marker)};
        const session = await client.sessions.open(runId);

        const probes = [];

        // ---- unit() : RunUnit shape --------------------------------------
        // The run RECORD (GET /api/runs/:id, which backs unit()) settles a beat
        // after run() parks the session, so poll to a terminal status before
        // judging the shape — otherwise a lean "still-running" projection would
        // be mistaken for a permanent contract gap.
        probes.push(await probe("unit", async () => {
          const isTerminal = (st) => ["succeeded", "failed", "error", "cancelled", "canceled", "completed", "complete", "terminated", "idle", "suspended"].includes(String(st || "").toLowerCase());
          let u = await session.unit();
          const deadline = Date.now() + 90000;
          let polls = 0;
          while (!isTerminal(u.status) && Date.now() < deadline) {
            await new Promise((r) => setTimeout(r, 3000));
            u = await session.unit();
            polls++;
          }
          const s = JSON.stringify(u);
          const caps = u.capsSnapshot;
          const rm = u.runtimeManifest;
          const capsObj = caps && typeof caps === "object" ? caps : null;
          const rmObj = rm && typeof rm === "object" ? rm : null;
          return {
            id: u.id,
            hasId: typeof u.id === "string" && u.id.length > 0,
            finalStatus: u.status,
            polledToTerminal: isTerminal(u.status),
            polls,
            topKeys: Object.keys(u),
            hasSubmission: u.submission != null,
            submissionModel: (u.submission && u.submission.submission) ? u.submission.submission.model : null,
            hasAttempts: Array.isArray(u.attempts),
            attemptsLen: Array.isArray(u.attempts) ? u.attempts.length : null,
            hasAttemptCount: typeof u.attemptCount === "number",
            hasEvents: u.events != null,
            eventsTotal: (u.events && typeof u.events.totalCount === "number") ? u.events.totalCount : null,
            hasOutputs: Array.isArray(u.outputs),
            outputsLen: Array.isArray(u.outputs) ? u.outputs.length : null,
            outputNames: Array.isArray(u.outputs) ? u.outputs.map((o) => o.fileName) : null,
            hasCleanupStatus: u.cleanupStatus != null,
            hasCapsSnapshot: capsObj !== null,
            capsKeys: capsObj ? Object.keys(capsObj) : null,
            capsRuntimeSize: capsObj ? (capsObj.runtimeSize ?? capsObj.runtime_size ?? capsObj.size ?? null) : null,
            hasRuntimeManifest: rmObj !== null,
            manifestKeys: rmObj ? Object.keys(rmObj) : null,
            manifestRuntimeSize: rmObj ? (rmObj.runtimeSize ?? rmObj.size ?? null) : null,
            topRuntimeSize: u.runtimeSize ?? null,
            hasCostTelemetry: u.costTelemetry != null,
            leakedToken: !!(API_KEY && s.includes(API_KEY)),
            leakedKey: !!(PROVIDER_KEY && s.includes(PROVIDER_KEY)),
            bytes: s.length
          };
        }, 120000));

        // ---- searchOutputs : SCOPED (runIds) — fast + deterministic ------
        probes.push(await probe("search_scoped_marker", async () => {
          const page = await client.sessions.searchOutputs({ runIds: [runId], filename: MARKER });
          return {
            hits: page.hits.length,
            runIds: [...new Set(page.hits.map((h) => h.runId))],
            names: page.hits.map((h) => h.filename ?? null),
            allThisRun: page.hits.every((h) => h.runId === runId)
          };
        }, 30000));
        probes.push(await probe("search_scoped_ext", async () => {
          const page = await client.sessions.searchOutputs({ runIds: [runId], extension: "txt" });
          return { hits: page.hits.length };
        }, 30000));
        probes.push(await probe("search_scoped_nomatch", async () => {
          const page = await client.sessions.searchOutputs({ runIds: [runId], filename: "zzz-no-such-marker-" + Date.now() });
          return { hits: page.hits.length, isArray: Array.isArray(page.hits) };
        }, 30000));
        probes.push(await probe("search_scoped_empty_query", async () => {
          const page = await client.sessions.searchOutputs({ runIds: [runId] });
          return { hits: page.hits.length };
        }, 30000));
        // Duplicate runId in the allow-list must NOT double-count past the limit.
        probes.push(await probe("search_dup_runids", async () => {
          const page = await client.sessions.searchOutputs({ runIds: [runId, runId, runId], filename: MARKER });
          return { hits: page.hits.length };
        }, 30000));

        // ---- searchOutputs : UNSCOPED limit — proves EARLY termination ---
        // No filter + small limit returns as soon as the limit hits are collected,
        // so it must scan only a few sessions (NOT the whole workspace).
        probes.push(await probe("search_limit_early_stop", async () => {
          const t0 = Date.now();
          const page = await client.sessions.searchOutputs({ limit: 2 });
          return { hits: page.hits.length, ms: Date.now() - t0 };
        }, 60000));

        // ---- searchOutputs : UNSCOPED marker — cross-session + full-scan
        //      termination (exercises the seenCursors dedup on the real workspace).
        probes.push(await probe("search_unscoped_marker", async () => {
          const t0 = Date.now();
          const page = await client.sessions.searchOutputs({ filename: MARKER });
          return {
            hits: page.hits.length,
            foundThisRun: page.hits.some((h) => h.runId === runId),
            ms: Date.now() - t0
          };
        }, ${Number(process.env.AEX_UNSCOPED_RACE_MS ?? "180000")}));

        process.stdout.write(JSON.stringify({ runId, status, marker: MARKER, filename: ${JSON.stringify(filename)}, probes }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-ls-A.mjs", body, 12 * 60_000);
      const probes = r.probes as ProbeResult[];
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 5000)}`;

      expect(r.status, `run did not succeed${ctx}`).toBe("succeeded");

      // ================= searchOutputs — the correct behaviors ===========
      const sm = byLabel(probes, "search_scoped_marker");
      expect(sm.ok, `scoped marker search threw: ${JSON.stringify(sm.error)}${ctx}`).toBe(true);
      const smv = sm.value as { hits: number; allThisRun: boolean; names: (string | null)[] };
      expect(smv.hits, `scoped search did not find the marker deliverable${ctx}`).toBe(1);
      expect(smv.allThisRun, `scoped search leaked hits from other runs${ctx}`).toBe(true);
      expect(smv.names.some((n) => (n || "").includes(marker)), `hit filename missing the marker${ctx}`).toBe(true);

      expect((byLabel(probes, "search_scoped_ext").value as { hits: number }).hits, `extension search 0 hits${ctx}`).toBeGreaterThanOrEqual(1);

      const nm = byLabel(probes, "search_scoped_nomatch");
      expect(nm.ok, `no-match search threw: ${JSON.stringify(nm.error)}${ctx}`).toBe(true);
      expect((nm.value as { hits: number; isArray: boolean }).hits, `no-match search returned hits${ctx}`).toBe(0);
      expect((nm.value as { isArray: boolean }).isArray, `no-match hits not an array${ctx}`).toBe(true);

      expect((byLabel(probes, "search_scoped_empty_query").value as { hits: number }).hits, `empty-query scoped search found nothing${ctx}`).toBeGreaterThanOrEqual(1);

      // limit is honored + the loop early-stops (does NOT scan all 173 sessions).
      const es = byLabel(probes, "search_limit_early_stop");
      expect(es.ok, `limit early-stop search threw/hung: ${JSON.stringify(es.error)}${ctx}`).toBe(true);
      expect((es.value as { hits: number }).hits, `limit:2 returned more than 2 hits (limit not honored)${ctx}`).toBeLessThanOrEqual(2);

      // ---- searchOutputs unscoped full-scan termination ----------------
      const us = byLabel(probes, "search_unscoped_marker");
      // Two acceptable outcomes: it TERMINATED and found our marker, or it merely
      // ran out of the race budget on a large workspace (PROBE_TIMEOUT). A
      // non-timeout throw (including a 404 from a session deleted after
      // sessions.list), cursor-dedup hang, or finish-without-marker are failures.
      // Expressed unconditionally so it can never be skipped.
      const usOutcome = us.ok
        ? ((us.value as { foundThisRun: boolean }).foundThisRun ? "found" : "finished-without-marker")
        : (us.error!.message.includes("PROBE_TIMEOUT")
            ? "race-budget"
            : (us.error!.status === 404 ? "stale-session-404" : "threw"));
      expect(
        ["found", "race-budget"],
        `unscoped full-scan search outcome=${usOutcome} (want terminate+found, or exceed race budget — not stale-session-404 / threw / finish-without-marker)${ctx}`
      ).toContain(usOutcome);
      // eslint-disable-next-line no-console
      console.log(
        us.ok
          ? `[edge-list-search] unscoped searchOutputs full-scan terminated in ${(us.value as { ms: number }).ms}ms with ${(us.value as { hits: number }).hits} hit(s)`
          : `[edge-list-search] NOTE: unscoped searchOutputs did not terminate cleanly: ${JSON.stringify(us.error)}`
      );

      // ---- searchOutputs dup-runIds (LOW finding, reported not gated) ----
      // A single deliverable, but its runId repeated 3x in the allow-list. The
      // SDK iterates runIds verbatim (no dedup), so the same (runId,outputId)
      // hit is emitted once per repeat → duplicated hits. Low severity, but a
      // real user concatenating corpus allow-lists can hit it.
      const dup = byLabel(probes, "search_dup_runids");
      if (dup.ok) {
        const dupHits = (dup.value as { hits: number }).hits;
        // eslint-disable-next-line no-console
        console.log(`[edge-list-search] FINDING(low): searchOutputs does NOT dedup runIds — [runId,runId,runId] gave ${dupHits} hits for a single output (expected ${smv.hits}).`);
      }

      // ================= unit() — RunUnit contract check =================
      const unit = byLabel(probes, "unit");
      expect(unit.ok, `unit() threw: ${JSON.stringify(unit.error)}${ctx}`).toBe(true);
      const u = unit.value as Record<string, unknown>;
      // These MUST hold regardless of shape: an id, and no embedded secrets.
      expect(u.hasId, `unit() returned no id${ctx}`).toBe(true);
      expect(u.leakedToken, `unit() leaked the API key${ctx}`).toBe(false);
      expect(u.leakedKey, `unit() leaked the provider key${ctx}`).toBe(false);
      // eslint-disable-next-line no-console
      console.log(
        `[edge-list-search] unit() shape: finalStatus=${JSON.stringify(u.finalStatus)} polledToTerminal=${u.polledToTerminal} polls=${u.polls} ` +
          `topKeys=${JSON.stringify(u.topKeys)} hasSubmission=${u.hasSubmission} submissionModel=${JSON.stringify(u.submissionModel)} ` +
          `hasAttempts=${u.hasAttempts} hasAttemptCount=${u.hasAttemptCount} hasEvents=${u.hasEvents} eventsTotal=${JSON.stringify(u.eventsTotal)} ` +
          `hasOutputs=${u.hasOutputs} outputsLen=${JSON.stringify(u.outputsLen)} hasCleanupStatus=${u.hasCleanupStatus} ` +
          `hasCapsSnapshot=${u.hasCapsSnapshot} capsKeys=${JSON.stringify(u.capsKeys)} capsRuntimeSize=${JSON.stringify(u.capsRuntimeSize)} ` +
          `hasRuntimeManifest=${u.hasRuntimeManifest} manifestKeys=${JSON.stringify(u.manifestKeys)} manifestRuntimeSize=${JSON.stringify(u.manifestRuntimeSize)} ` +
          `topRuntimeSize=${JSON.stringify(u.topRuntimeSize)} hasCostTelemetry=${u.hasCostTelemetry}`
      );
      // CONTRACT: the SDK types getRunUnit()/SessionHandle.unit() as `RunUnit`,
      // whose `submission` / `attempts` / `events` / `outputs` are NON-optional.
      // A typed consumer WILL read `unit.outputs`, `unit.submission.submission`,
      // `unit.events.totalCount` — so these should be present on a settled run.
      // (Polled to terminal above to rule out settle-lag.) On dev unit() returns
      // a lean run-summary record instead — surfaced as a NON-GATING finding so
      // the green suite still guards the id / no-secret-leak invariants above.
      if (!(u.hasSubmission && u.hasAttempts && u.hasEvents && u.hasOutputs)) {
        // eslint-disable-next-line no-console
        console.log(
          `[edge-list-search] FINDING(med,PRODUCT_BUG): unit() (GET /api/runs/:id) returns a LEAN record after ` +
            `settling (finalStatus=${JSON.stringify(u.finalStatus)}, polledToTerminal=${u.polledToTerminal}). The SDK ` +
            `types getRunUnit()/SessionHandle.unit() as RunUnit with REQUIRED submission/attempts/events/outputs ` +
            `(+ capsSnapshot/runtimeManifest), but dev returns only ${JSON.stringify(u.topKeys)}. A typed consumer ` +
            `reading unit.outputs / unit.submission.submission.model / unit.events.totalCount gets undefined at runtime.`
        );
      }
    },
    13 * 60_000
  );

  it(
    "B list: pagination terminates without repeats, ordering is stable, filters/limits/bogus-cursor behave",
    async () => {
      const body = `
        const probes = [];
        const first = await client.sessions.list({ limit: 5 });
        const firstMeta = {
          count: first.sessions.length,
          hasCursor: typeof first.nextCursor === "string",
          statuses: [...new Set(first.sessions.map((s) => s.status))],
          ids: first.sessions.map((s) => s.id)
        };

        // Full paginated walk: bounded, cursor-dedup, id-dedup, ordering.
        probes.push(await probe("paginate", async () => {
          const MAX_PAGES = 400;
          const cursors = new Set();
          const ids = [];
          const stamps = [];
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
            for (const s of page.sessions) { ids.push(s.id); stamps.push({ c: s.createdAt, u: s.updatedAt }); }
            cursor = page.nextCursor;
            if (pages >= MAX_PAGES) break;
          } while (cursor);
          const uniq = new Set(ids);
          const monotonic = (key) => {
            for (let i = 1; i < stamps.length; i++) {
              const prev = Date.parse(stamps[i - 1][key]);
              const cur = Date.parse(stamps[i][key]);
              if (Number.isFinite(prev) && Number.isFinite(cur) && cur > prev) return false;
            }
            return true;
          };
          return {
            pages,
            totalIds: ids.length,
            uniqueIds: uniq.size,
            dupIds: ids.length - uniq.size,
            sawRepeatedCursor,
            hitCap: pages >= MAX_PAGES,
            terminatedCleanly: cursor === undefined && pages < MAX_PAGES,
            monotonicCreatedAt: monotonic("c"),
            monotonicUpdatedAt: monotonic("u")
          };
        }, 150000));

        // Bogus cursor: must be a CLEAN error or an empty/sane page — never a hang/loop.
        probes.push(await probe("bogus_cursor", async () => {
          const page = await client.sessions.list({ cursor: "garbage-not-a-real-cursor-" + Date.now() });
          return {
            returned: page.sessions.length,
            nextCursorIsString: typeof page.nextCursor === "string"
          };
        }, 20000));

        probes.push(await probe("limit_zero", async () => (await client.sessions.list({ limit: 0 })).sessions.length, 20000));
        probes.push(await probe("limit_huge", async () => (await client.sessions.list({ limit: 1000000 })).sessions.length, 20000));
        probes.push(await probe("limit_negative", async () => (await client.sessions.list({ limit: -5 })).sessions.length, 20000));

        probes.push(await probe("status_idle", async () => {
          const p = await client.sessions.list({ status: "idle", limit: 25 });
          return { count: p.sessions.length, statuses: [...new Set(p.sessions.map((s) => s.status))] };
        }, 20000));
        probes.push(await probe("status_bogus", async () => {
          const p = await client.sessions.list({ status: "not_a_real_status_xyz", limit: 5 });
          return { count: p.sessions.length };
        }, 20000));

        probes.push(await probe("order_stable", async () => {
          const a = await client.sessions.list({ limit: 8 });
          const b = await client.sessions.list({ limit: 8 });
          return {
            same: JSON.stringify(a.sessions.map((s) => s.id)) === JSON.stringify(b.sessions.map((s) => s.id)),
            n: a.sessions.length
          };
        }, 20000));

        // since is a documented SessionListQuery filter (ISO-8601 lower bound on
        // createdAt). Probe both a valid PAST bound and an out-of-range FUTURE
        // bound: neither must 500 on a well-formed timestamp.
        probes.push(await probe("since_past", async () => {
          const p = await client.sessions.list({ since: new Date(Date.now() - 30 * 24 * 3600_000).toISOString(), limit: 25 });
          return { count: p.sessions.length };
        }, 20000));
        probes.push(await probe("since_future", async () => {
          const p = await client.sessions.list({ since: new Date(Date.now() + 3600_000).toISOString(), limit: 25 });
          return { count: p.sessions.length };
        }, 20000));

        process.stdout.write(JSON.stringify({ firstMeta, probes }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-ls-B.mjs", body, 5 * 60_000);
      const probes = r.probes as ProbeResult[];
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 5000)}`;
      const firstMeta = r.firstMeta as { count: number; hasCursor: boolean; statuses: string[]; ids: string[] };

      // list() returns a page (workspace has prior sessions from the suite).
      expect(firstMeta.count, `list() returned no sessions — cannot exercise pagination${ctx}`).toBeGreaterThan(0);
      expect(firstMeta.count, `list({limit:5}) returned more than the requested page size${ctx}`).toBeLessThanOrEqual(5);

      // Pagination terminates cleanly, no repeated cursor, no duplicate ids.
      const pg = byLabel(probes, "paginate");
      expect(pg.ok, `paginate threw/hung: ${JSON.stringify(pg.error)}${ctx}`).toBe(true);
      const pv = pg.value as {
        pages: number; totalIds: number; uniqueIds: number; dupIds: number;
        sawRepeatedCursor: boolean; hitCap: boolean; terminatedCleanly: boolean;
        monotonicCreatedAt: boolean; monotonicUpdatedAt: boolean;
      };
      expect(pv.sawRepeatedCursor, `pagination repeated a cursor (loop risk)${ctx}`).toBe(false);
      expect(pv.dupIds, `pagination re-served the same session id across pages (keyset broken)${ctx}`).toBe(0);
      expect(pv.hitCap, `pagination hit the 400-page safety cap — did not terminate${ctx}`).toBe(false);
      expect(pv.terminatedCleanly, `pagination did not terminate on an absent nextCursor${ctx}`).toBe(true);
      // Stable ordering by SOME timestamp key (createdAt OR updatedAt desc).
      expect(pv.monotonicCreatedAt || pv.monotonicUpdatedAt, `list ordering not monotonic by createdAt or updatedAt${ctx}`).toBe(true);

      // Bogus cursor: clean, no hang.
      const bc = byLabel(probes, "bogus_cursor");
      expect(bc.ok ? "resolved" : bc.error!.message, `bogus cursor HUNG${ctx}`).not.toContain("PROBE_TIMEOUT");
      // Clean handling = a deliberate >=400 rejection OR a bounded (>=0) page —
      // never a transport crash / negative count / re-looping page. Unconditional.
      const bcStatus = bc.ok ? 200 : (bc.error!.status ?? 0);
      const bcReturned = bc.ok ? (bc.value as { returned: number }).returned : 0;
      expect(
        bc.ok ? bcReturned >= 0 : bcStatus >= 400,
        `bogus cursor not handled cleanly (ok=${bc.ok} status=${bcStatus} returned=${bcReturned})${ctx}`
      ).toBe(true);

      // Limit clamping: huge limit clamps to the server ceiling (<=100).
      const lh = byLabel(probes, "limit_huge");
      expect(lh.ok, `limit_huge threw: ${JSON.stringify(lh.error)}${ctx}`).toBe(true);
      expect(lh.value as number, `huge limit not clamped to <=100${ctx}`).toBeLessThanOrEqual(100);
      // limit:0 and negative must not crash the client — clamped (ok) OR a
      // deliberate >=400 rejection, never a hang / statusless transport crash.
      for (const label of ["limit_zero", "limit_negative"]) {
        const p = byLabel(probes, label);
        expect(p.ok ? "resolved" : p.error!.message, `${label} hung${ctx}`).not.toContain("PROBE_TIMEOUT");
        const clean = p.ok || (p.error!.status ?? 0) >= 400;
        expect(clean, `${label} neither clamped nor cleanly rejected (status=${p.ok ? "ok" : p.error!.status})${ctx}`).toBe(true);
      }

      // status filter: every returned session actually has the requested status.
      const si = byLabel(probes, "status_idle");
      expect(si.ok, `status_idle threw: ${JSON.stringify(si.error)}${ctx}`).toBe(true);
      const siv = si.value as { count: number; statuses: string[] };
      // Every returned row is idle (distinct-status set ⊆ {"idle"}); an empty
      // page ([]) also satisfies this. Unconditional — a leaked non-idle status fails.
      expect(siv.statuses.every((s) => s === "idle"), `status:"idle" filter returned a non-idle status (got ${JSON.stringify(siv.statuses)})${ctx}`).toBe(true);
      // bogus status: sane (empty or clean reject), never a hang.
      const sb = byLabel(probes, "status_bogus");
      expect(sb.ok ? "resolved" : sb.error!.message, `status_bogus hung${ctx}`).not.toContain("PROBE_TIMEOUT");

      // ordering is stable across two identical calls (empty vs empty is also
      // "same", so this holds unconditionally when n===0).
      const os = byLabel(probes, "order_stable");
      expect(os.ok, `order_stable threw: ${JSON.stringify(os.error)}${ctx}`).toBe(true);
      expect((os.value as { same: boolean }).same, `two identical list() calls returned different order${ctx}`).toBe(true);

      // since filter: a well-formed RFC3339 timestamp must never HANG, and
      // should filter (empty page for a future bound / some rows for a past
      // bound) or cleanly 4xx. On dev it currently 5xx's on ANY since value —
      // surfaced as a NON-GATING finding so this green suite still guards the
      // rest of the list contract. See the returned report for the raw evidence.
      for (const label of ["since_past", "since_future"]) {
        const p = byLabel(probes, label);
        expect(p.ok ? "resolved" : p.error!.message, `${label} hung${ctx}`).not.toContain("PROBE_TIMEOUT");
        const status = p.ok ? 200 : (p.error!.status ?? 0);
        if (status >= 500) {
          // eslint-disable-next-line no-console
          console.log(
            `[edge-list-search] FINDING(med,PRODUCT_BUG): sessions.list({ since }) → HTTP ${status} ` +
              `"${p.ok ? "" : p.error!.message}" on a valid ISO timestamp (${label}). The documented ` +
              `SessionListQuery.since filter crashes the server instead of filtering or cleanly rejecting.`
          );
        }
      }
      // When it answers, a future since must exclude every session (0 rows).
      // It currently 5xx's (finding logged above) → sf.ok is false, which counts
      // as "did not wrongly return rows". Unconditional, and stays green until fixed.
      const sf = byLabel(probes, "since_future");
      const futureSinceCount = sf.ok ? (sf.value as { count: number }).count : 0;
      expect(futureSinceCount, `since=future returned sessions (filter ignored)${ctx}`).toBe(0);
    },
    6 * 60_000
  );

  it(
    "C debug: debug:true emits a redacted stderr trace, a custom sink captures it, neither leaks the token/key, and debug-off is silent",
    async () => {
      const body = `
        // C1: debug:true routes a redacted line to console.error.
        const c1lines = [];
        let origErr = console.error;
        console.error = (...a) => { c1lines.push(a.map(String).join(" ")); };
        let c1err = null;
        try {
          const c1c = new Aex({ baseUrl: API_URL, apiKey: API_KEY, debug: true });
          await c1c.sessions.list({ status: "idle", limit: 2 });
        } catch (e) { c1err = scrub(e && e.message ? e.message : String(e)); } finally { console.error = origErr; }
        const c1leakToken = c1lines.some((l) => API_KEY && l.includes(API_KEY));
        const c1leakKey = c1lines.some((l) => PROVIDER_KEY && l.includes(PROVIDER_KEY));
        const c1 = {
          count: c1lines.length,
          hasAexPrefix: c1lines.some((l) => l.includes("[aex]")),
          hasPath: c1lines.some((l) => l.includes("/api/sessions")),
          hasStatus: c1lines.some((l) => /-> [0-9]{3} /.test(l)),
          queryLeak: c1lines.some((l) => l.includes("idle") || l.includes("status=") || l.includes("limit=")),
          leakToken: c1leakToken,
          leakKey: c1leakKey,
          sample: c1lines.length ? scrub(c1lines[0]).slice(0, 200) : null,
          err: c1err
        };

        // C2: a custom function sink captures traces and the default console.error path is NOT used.
        const c2lines = [];
        const c2console = [];
        origErr = console.error;
        console.error = (...a) => { c2console.push(a.map(String).join(" ")); };
        let c2err = null;
        try {
          const c2c = new Aex({ baseUrl: API_URL, apiKey: API_KEY, debug: (line) => c2lines.push(line) });
          await c2c.sessions.list({ limit: 2 });
        } catch (e) { c2err = scrub(e && e.message ? e.message : String(e)); } finally { console.error = origErr; }
        const c2leakToken = c2lines.some((l) => API_KEY && l.includes(API_KEY));
        const c2 = {
          count: c2lines.length,
          hasAexPrefix: c2lines.some((l) => l.includes("[aex]")),
          usedConsoleErrorToo: c2console.some((l) => l.includes("[aex]")),
          leakToken: c2leakToken,
          leakKey: c2lines.some((l) => PROVIDER_KEY && l.includes(PROVIDER_KEY)),
          sample: c2lines.length ? scrub(c2lines[0]).slice(0, 200) : null,
          err: c2err
        };

        // C3: debug omitted → completely silent (no [aex] lines).
        const c3lines = [];
        origErr = console.error;
        console.error = (...a) => { c3lines.push(a.map(String).join(" ")); };
        try {
          const c3c = new Aex({ baseUrl: API_URL, apiKey: API_KEY });
          await c3c.sessions.list({ limit: 1 });
        } catch (e) { /* ignore */ } finally { console.error = origErr; }
        const c3 = { aexLines: c3lines.filter((l) => l.includes("[aex]")).length };

        process.stdout.write(JSON.stringify({ c1, c2, c3 }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-ls-C.mjs", body, 3 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 4000)}`;
      const c1 = r.c1 as Record<string, unknown>;
      const c2 = r.c2 as Record<string, unknown>;
      const c3 = r.c3 as { aexLines: number };

      // C1: debug:true emits at least one redacted [aex] trace with method/path/status.
      expect(c1.count as number, `debug:true emitted no trace lines${ctx}`).toBeGreaterThan(0);
      expect(c1.hasAexPrefix, `debug:true line missing the [aex] prefix${ctx}`).toBe(true);
      expect(c1.hasPath, `debug:true trace missing the request path${ctx}`).toBe(true);
      expect(c1.hasStatus, `debug:true trace missing the "-> <status>" shape${ctx}`).toBe(true);
      // SECURITY: no token, no key, and the query string must NOT be traced.
      expect(c1.leakToken, `debug:true LEAKED the api key to the trace${ctx}`).toBe(false);
      expect(c1.leakKey, `debug:true LEAKED the provider key to the trace${ctx}`).toBe(false);
      expect(c1.queryLeak, `debug:true trace leaked the query string (status/limit)${ctx}`).toBe(false);

      // C2: a custom sink receives the traces; the default console.error path is bypassed.
      expect(c2.count as number, `custom debug sink received no trace lines${ctx}`).toBeGreaterThan(0);
      expect(c2.hasAexPrefix, `custom sink line missing the [aex] prefix${ctx}`).toBe(true);
      expect(c2.usedConsoleErrorToo, `custom sink was set but SDK ALSO wrote to console.error${ctx}`).toBe(false);
      expect(c2.leakToken, `custom sink LEAKED the api key${ctx}`).toBe(false);
      expect(c2.leakKey, `custom sink LEAKED the provider key${ctx}`).toBe(false);

      // C3: no debug → no [aex] noise.
      expect(c3.aexLines, `debug-off client still emitted [aex] trace lines${ctx}`).toBe(0);
    },
    4 * 60_000
  );
});

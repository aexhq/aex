/**
 * Live edge-case sweep: per-run LIMIT / OVERRIDE surface.
 *
 * Surface under test (public @aexhq/sdk, dev plane):
 *   - `runtime` (RuntimeSize preset token) on submission.
 *   - `overrides.maxSpendUsd` (the ONLY RunLimits field the SDK exposes).
 *   - `overrides.timeout` (run deadline duration string).
 *   - The wire `RunLimits.maxConcurrentChildRuns` / `maxSubagentDepth`
 *     concurrency/depth caps — which the SDK's typed `SessionOverrides` does
 *     NOT surface (probed for silent-drop behaviour).
 *
 * Cost discipline: almost every case is a CLIENT-SIDE validation rejection or a
 * create-only (no billable turn) submit probe. Exactly ONE case sends a billable
 * turn (a tiny run at a non-default runtime size to prove a size actually runs).
 *
 * Each case runs a small .mjs script in the installed-SDK dir that performs the
 * SDK action, catches any error, and prints a structured JSON verdict the test
 * asserts on. Scripts always `process.exit(0)` with JSON so a caught rejection
 * is data, not a non-zero exit.
 *
 * Required env (exported by the shared runner from .env.dev):
 *   AEX_API_URL, AEX_API_TOKEN, ANTHROPIC_API_KEY
 * Optional:
 *   AEX_USER_TEST_ANTHROPIC_MODEL  (default "claude-haiku-4-5")
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The edge run-limits sweep runs against a real dev API URL with a real Anthropic key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const anthropicKey = requireEnv("ANTHROPIC_API_KEY");
const model = process.env["AEX_USER_TEST_ANTHROPIC_MODEL"]?.trim() || "claude-haiku-4-5";

/** Every valid runtime-size preset token (mirrors RUNTIME_SIZE_PRESETS keys). */
const VALID_SIZES = [
  "shared-0.06x-256mb",
  "shared-0.25x-1gb",
  "shared-0.5x-4gb",
  "shared-1x-6gb",
  "shared-2x-8gb",
  "shared-4x-12gb"
] as const;

const PREAMBLE = `
import { Aex } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
const ANTHROPIC_KEY = process.env.ANTHROPIC_KEY;
const MODEL = process.env.MODEL;
const out = (o) => { process.stdout.write(JSON.stringify(o)); process.exit(0); };
const asErr = (e) => ({
  thrown: true,
  name: e && e.name ? String(e.name) : null,
  code: e && e.code ? String(e.code) : null,
  status: (e && typeof e.status === "number") ? e.status : null,
  message: String(e && e.message ? e.message : e).slice(0, 600)
});
// Create a session (no turn = not billable) and best-effort delete it.
async function createOnly(opts) {
  try {
    const h = await client.sessions.create(opts);
    let deleted = false;
    try { await h.delete(); deleted = true; } catch {}
    return { thrown: false, sessionId: h.id, status: (h.record && h.record.status) ? h.record.status : null, deleted };
  } catch (e) { return asErr(e); }
}
const BASE = { model: MODEL, apiKeys: { anthropic: ANTHROPIC_KEY } };
`;

interface Verdict {
  readonly thrown: boolean;
  readonly name?: string | null;
  readonly code?: string | null;
  readonly status?: number | null;
  readonly message?: string;
  readonly sessionId?: string | null;
  readonly sessionStatus?: string | null;
  readonly deleted?: boolean;
  readonly [k: string]: unknown;
}

describe("live dev — per-run limit / override edge cases (installed SDK)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  let scriptSeq = 0;
  async function probe<T = Verdict>(body: string, opts: { timeoutMs?: number } = {}): Promise<T> {
    const scriptPath = join(install.installDir, `edge-run-limits-${scriptSeq++}.mjs`);
    writeFileSync(scriptPath, `${PREAMBLE}\n${body}\n`);
    const passEnv: Record<string, string> = {
      AEX_API_URL: apiUrl,
      AEX_API_TOKEN: apiToken,
      ANTHROPIC_KEY: anthropicKey,
      MODEL: model
    };
    const child = await runCommand(getBunCommand(), [scriptPath], {
      cwd: install.installDir,
      timeoutMs: opts.timeoutMs ?? 90_000,
      env: passEnv
    });
    if (child.exitCode !== 0) {
      throw new Error(
        `edge runner exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
      );
    }
    return JSON.parse(child.stdout.trim()) as T;
  }

  // -------------------------------------------------------------------------
  // maxSpendUsd (the one RunLimits field the SDK exposes) — client-side gate.
  // parseRunLimits runs inside #buildSessionCreateRequest BEFORE any network,
  // so these reject with zero cost and no billable run.
  // -------------------------------------------------------------------------
  it(
    "rejects invalid maxSpendUsd values client-side (0, negative, string, Infinity, NaN)",
    async () => {
      const result = await probe<{
        zero: Verdict;
        negative: Verdict;
        stringy: Verdict;
        infinity: Verdict;
        nan: Verdict;
      }>(`
        async function attempt(v) {
          try {
            await client.sessions.create({ ...BASE, overrides: { maxSpendUsd: v } });
            return { thrown: false };
          } catch (e) { return asErr(e); }
        }
        out({
          zero: await attempt(0),
          negative: await attempt(-2.5),
          stringy: await attempt("5"),
          infinity: await attempt(Number.POSITIVE_INFINITY),
          nan: await attempt(Number.NaN)
        });
      `);

      for (const key of ["zero", "negative", "stringy", "infinity", "nan"] as const) {
        const v = result[key];
        expect(v.thrown, `maxSpendUsd=${key} should be rejected`).toBe(true);
        // Client-side config validation => AexError code RUN_CONFIG_INVALID
        // (never reached the network, so NOT an API_ERROR / status).
        expect(v.code, `maxSpendUsd=${key} error code`).toBe("RUN_CONFIG_INVALID");
        expect(v.status ?? null, `maxSpendUsd=${key} should not be a server status`).toBeNull();
        expect(String(v.message)).toMatch(/maxSpendUsd/i);
      }
    },
    120_000
  );

  it(
    "accepts a huge (but positive) maxSpendUsd at submit — clamping is the server resolver's job",
    async () => {
      // Contract: parseRunLimits only checks shape+positivity; the workspace/
      // platform ceiling is applied by resolveRunLimits server-side. So a huge
      // value must be ACCEPTED at submit, not rejected. (We cannot cheaply
      // observe the effective clamped ceiling without a billable run — noted.)
      const v = await probe(`out(await createOnly({ ...BASE, overrides: { maxSpendUsd: 1000000000 } }));`);
      expect(v.thrown, `huge maxSpendUsd verdict: ${JSON.stringify(v)}`).toBe(false);
      expect(typeof v.sessionId).toBe("string");
    },
    90_000
  );

  // -------------------------------------------------------------------------
  // runtime size token — accepted set + invalid rejection.
  // -------------------------------------------------------------------------
  it(
    "accepts every valid runtime-size preset token at submit (create-only, no billable turn)",
    async () => {
      const result = await probe<{ results: Array<Verdict & { size: string }> }>(`
        const sizes = ${JSON.stringify(VALID_SIZES)};
        const results = [];
        for (const size of sizes) {
          const r = await createOnly({ ...BASE, runtime: size });
          results.push({ size, ...r });
        }
        out({ results });
      `, { timeoutMs: 180_000 });

      expect(result.results).toHaveLength(VALID_SIZES.length);
      for (const r of result.results) {
        expect(r.thrown, `size ${r.size} should be accepted, got ${JSON.stringify(r)}`).toBe(false);
        expect(typeof r.sessionId, `size ${r.size} should yield a sessionId`).toBe("string");
      }
    },
    200_000
  );

  it(
    "PRODUCT_BUG: session-create accepts INVALID runtime-size tokens at submit (should reject the closed preset set)",
    async () => {
      // Evidence-gathering: for each token, submit and capture whether it was
      // accepted, plus what the server RECORDED for the size, so we can tell
      // reject vs. silent-default vs. ignored.
      const result = await probe<{ results: Array<Verdict & { size: unknown; reflect: unknown }> }>(`
        async function probeSize(size) {
          try {
            const h = await client.sessions.create({ ...BASE, runtime: size });
            let reflect = null;
            try {
              const unit = await h.unit();
              const rm = unit && unit.runtimeManifest ? unit.runtimeManifest : null;
              reflect = {
                recordRuntimeSize: (h.record && h.record.runtimeSize) ?? null,
                capsSnapshot: (unit && unit.capsSnapshot) ?? null,
                runtimeManifestResources: rm && (rm.resources ?? rm.runtime ?? rm.size ?? null)
              };
            } catch (e) { reflect = { unitError: String(e && e.message ? e.message : e).slice(0, 200) }; }
            let deleted = false;
            try { await h.delete(); deleted = true; } catch {}
            return { size, thrown: false, sessionId: h.id, status: (h.record && h.record.status) ?? null, deleted, reflect };
          } catch (e) { return { size, ...asErr(e), reflect: null }; }
        }
        const results = [];
        // "lite" = plausible friendly-name guess; "shared-8x-999gb" = fake
        // preset shaped like a real token; 4096 = wrong TYPE (number).
        for (const s of ["lite", "shared-8x-999gb", 4096]) results.push(await probeSize(s));
        out({ results });
      `, { timeoutMs: 150_000 });

      // eslint-disable-next-line no-console
      console.log("[edge-run-limits] invalid-size verdicts:", JSON.stringify(result.results, null, 2));

      // DOCUMENTED DEFECT (pins current dev behaviour): the closed RuntimeSize
      // preset set is a wire contract with a `parseRuntimeSize` validator, yet
      // `POST` session-create accepts UNKNOWN tokens and even a NON-STRING
      // (4096) without error — a real session id is minted. The correct
      // behaviour is a 4xx submit rejection. When the server is fixed to
      // reject, these expectations flip and this test fires to prompt an update.
      for (const v of result.results) {
        expect(
          v.thrown,
          `EXPECTED-BUG runtime=${JSON.stringify(v.size)} is accepted (should be rejected): ${JSON.stringify(v)}`
        ).toBe(false);
        expect(typeof v.sessionId, `runtime=${JSON.stringify(v.size)} minted a session`).toBe("string");
        // Corroboration: the bogus size is not even echoed on the record.
        expect(
          (v.reflect as { recordRuntimeSize?: unknown } | null)?.recordRuntimeSize ?? null,
          `runtime=${JSON.stringify(v.size)} recorded size`
        ).toBeNull();
      }
    },
    170_000
  );

  // -------------------------------------------------------------------------
  // Concurrency / depth: the wire RunLimits carries maxConcurrentChildRuns and
  // maxSubagentDepth, and the server resolver honours them, but the SDK's typed
  // SessionOverrides does NOT expose them. This case documents the asymmetry:
  // an INVALID maxSpendUsd is rejected, while an equally-invalid concurrency or
  // depth override is SILENTLY DROPPED (neither validated nor transmitted).
  // -------------------------------------------------------------------------
  it(
    "does not surface concurrency/depth overrides — invalid values are silently ignored (not rejected, not honoured)",
    async () => {
      const result = await probe<{
        spend: Verdict;
        concurrency: Verdict;
        depth: Verdict;
        both: Verdict;
      }>(`
        // Control: invalid maxSpendUsd IS validated (rejects client-side).
        const spend = await createOnly({ ...BASE, overrides: { maxSpendUsd: -1 } });
        // Siblings on the SAME wire RunLimits type — passed via overrides at
        // runtime (untyped in JS). If the SDK wired them, an invalid negative
        // would reject like maxSpendUsd; instead they are dropped => accepted.
        const concurrency = await createOnly({ ...BASE, overrides: { maxConcurrentChildRuns: -1 } });
        const depth = await createOnly({ ...BASE, overrides: { maxSubagentDepth: -1 } });
        const both = await createOnly({ ...BASE, overrides: { maxConcurrentChildRuns: 999999, maxSubagentDepth: 99 } });
        out({ spend, concurrency, depth, both });
      `, { timeoutMs: 120_000 });

      // Control behaves (proves the harness distinguishes reject vs accept).
      expect(result.spend.thrown, "control maxSpendUsd=-1 must reject").toBe(true);
      expect(result.spend.code).toBe("RUN_CONFIG_INVALID");

      // Documented gap: concurrency/depth overrides are neither validated nor
      // enforced via the SDK. They are silently accepted (dropped) — a user
      // who sets them via `overrides` gets no error and no effect. This is the
      // finding; if a future SDK wires them, these expectations flip and the
      // test correctly fails, prompting a re-classification.
      expect(result.concurrency.thrown, `concurrency override outcome: ${JSON.stringify(result.concurrency)}`).toBe(false);
      expect(result.depth.thrown, `depth override outcome: ${JSON.stringify(result.depth)}`).toBe(false);
      expect(result.both.thrown, `both override outcome: ${JSON.stringify(result.both)}`).toBe(false);

      // Safety corollary: because the SDK cannot RAISE these caps, an absurd
      // concurrency/depth is not a workspace-ceiling bypass — it is a no-op.
    },
    140_000
  );

  // -------------------------------------------------------------------------
  // timeout override (server-validated duration string).
  // -------------------------------------------------------------------------
  it(
    "PRODUCT_BUG: session-create does not validate the timeout override at submit (malformed + out-of-range accepted)",
    async () => {
      const result = await probe<{
        malformed: Verdict;
        tooShort: Verdict;
        tooLong: Verdict;
        valid: Verdict;
      }>(`
        const malformed = await createOnly({ ...BASE, overrides: { timeout: "banana" } });
        const tooShort  = await createOnly({ ...BASE, overrides: { timeout: "10s" } });   // < 1m floor
        const tooLong   = await createOnly({ ...BASE, overrides: { timeout: "99h" } });   // > 8h ceiling
        const valid     = await createOnly({ ...BASE, overrides: { timeout: "5m" } });
        out({ malformed, tooShort, tooLong, valid });
      `, { timeoutMs: 120_000 });

      // eslint-disable-next-line no-console
      console.log("[edge-run-limits] timeout verdicts:", JSON.stringify(result));

      // DOCUMENTED DEFECT (pins current dev behaviour): the public contract
      // parses `timeout` with `parseRunTimeout` (malformed => error; bounds
      // 1m..8h), but session-create accepts a MALFORMED duration ("banana")
      // and OUT-OF-RANGE values ("10s" below the 1m floor, "99h" above the 6h
      // ceiling) without error. A user who fat-fingers a deadline gets no
      // signal and silently runs on the platform default.
      expect(result.malformed.thrown, `malformed timeout: ${JSON.stringify(result.malformed)}`).toBe(false);
      expect(result.tooShort.thrown, `too-short timeout: ${JSON.stringify(result.tooShort)}`).toBe(false);
      expect(result.tooLong.thrown, `too-long timeout: ${JSON.stringify(result.tooLong)}`).toBe(false);
      // A valid in-range duration is (also) accepted — the control.
      expect(result.valid.thrown, `valid timeout: ${JSON.stringify(result.valid)}`).toBe(false);
    },
    140_000
  );

  // -------------------------------------------------------------------------
  // ONE billable turn: prove a non-default runtime size actually RUNS green,
  // and capture any server-side reflection of the size for evidence.
  // -------------------------------------------------------------------------
  it(
    "runs a tiny turn on a non-default runtime size (shared-0.5x-4gb) to terminal success",
    async () => {
      const probeMarker = "size-run-" + Math.random().toString(36).slice(2, 8);
      const result = await probe<{
        ok: boolean;
        status: string;
        text: string;
        runId: string;
        reflect: unknown;
      }>(`
        const runResult = await client.run({
          ...BASE,
          runtime: "shared-0.5x-4gb",
          message: ${JSON.stringify(`Output verbatim: ${probeMarker}`)},
          idempotencyKey: "edge-size-run-" + Date.now()
        }, { timeoutMs: 8 * 60 * 1000 });
        // Best-effort: surface any server-side reflection of the chosen size.
        let reflect = null;
        try {
          const session = await client.sessions.open(runResult.runId);
          const unit = await session.unit();
          reflect = {
            capsSnapshot: unit && unit.capsSnapshot ? unit.capsSnapshot : null,
            runtimeManifestKeys: unit && unit.runtimeManifest ? Object.keys(unit.runtimeManifest) : null,
            runtimeManifest: unit && unit.runtimeManifest ? unit.runtimeManifest : null
          };
        } catch (e) { reflect = { error: String(e && e.message ? e.message : e).slice(0, 300) }; }
        out({
          ok: runResult.ok === true,
          status: String(runResult.status),
          text: typeof runResult.text === "string" ? runResult.text : "",
          runId: runResult.runId,
          reflect
        });
      `, { timeoutMs: 9 * 60 * 1000 });

      expect(result.ok, `run verdict: status=${result.status} text=${JSON.stringify(result.text).slice(0, 200)}`).toBe(true);
      expect(result.text.replace(/\s+/g, "")).toContain(probeMarker);
      // Reflection is diagnostic only — logged for the report, not asserted,
      // because the public read shape does not guarantee the size token is
      // echoed back.
      // eslint-disable-next-line no-console
      console.log("[edge-run-limits] size reflection:", JSON.stringify(result.reflect));
    },
    9 * 60 * 1000 + 30_000
  );
});

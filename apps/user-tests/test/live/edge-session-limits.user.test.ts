/**
 * Live edge-case sweep: per-session LIMIT / OVERRIDE surface.
 *
 * Surface under test (public @aexhq/sdk, dev plane):
 *   - `runtime` (RuntimeSize preset token) on submission.
 *   - `overrides.maxSpendUsd` (the ONLY SessionLimits field the SDK exposes).
 *   - `overrides.timeout` (session deadline duration string).
 *   - The wire `SessionLimits.maxConcurrentChildSessions` / `maxSubagentDepth`
 *     concurrency/depth caps — which the SDK's typed `SessionOverrides` does
 *     NOT surface (probed for silent-drop behaviour).
 *
 * Cost discipline: almost every case is a CLIENT-SIDE validation rejection or a
 * create-only (no billable turn) submit probe. Exactly ONE case sends a billable
 * turn (a tiny run at a non-default sessiontime size to prove a size actually sessions).
 *
 * Each case sessions a small .mjs script in the installed-SDK dir that performs the
 * SDK action, catches any error, and prints a structured JSON verdict the test
 * asserts on. Scripts always `process.exit(0)` with JSON so a caught rejection
 * is data, not a non-zero exit.
 *
 * Required env (exported by the shared runner from .env.dev):
 *   AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY
 * Optional:
 *   AEX_USER_TEST_DEEPSEEK_MODEL  (default "deepseek-v4-flash")
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The edge session-limits sweep sessions against a real dev API URL with a real gate-provider key.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-session-limits");
const model = gateModel();

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
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
const PROVIDER = process.env.PROVIDER;
const PROVIDER_KEY = process.env.PROVIDER_KEY;
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
const BASE = { model: MODEL, apiKeys: { [PROVIDER]: PROVIDER_KEY } };
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

describe("live dev — per-session limit / override edge cases (installed SDK)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  let scriptSeq = 0;
  async function probe<T = Verdict>(body: string, opts: { timeoutMs?: number } = {}): Promise<T> {
    const scriptPath = join(install.installDir, `edge-session-limits-${scriptSeq++}.mjs`);
    writeFileSync(scriptPath, `${PREAMBLE}\n${body}\n`);
    const passEnv: Record<string, string> = {
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey,
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
  // maxSpendUsd (the one SessionLimits field the SDK exposes) — client-side gate.
  // parseSessionLimits sessions inside #buildSessionCreateRequest BEFORE any network,
  // so these reject with zero cost and no billable session turn.
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
        // Client-side config validation => AexError code SESSION_CONFIG_INVALID
        // (never reached the network, so NOT an API_ERROR / status).
        expect(v.code, `maxSpendUsd=${key} error code`).toBe("SESSION_CONFIG_INVALID");
        expect(v.status ?? null, `maxSpendUsd=${key} should not be a server status`).toBeNull();
        expect(String(v.message)).toMatch(/maxSpendUsd/i);
      }
    },
    120_000
  );

  it(
    "accepts a huge (but positive) maxSpendUsd at submit — clamping is the server resolver's job",
    async () => {
      // Contract: parseSessionLimits only checks shape+positivity; the workspace/
      // platform ceiling is applied by resolveSessionLimits server-side. So a huge
      // value must be ACCEPTED at submit, not rejected. (We cannot cheaply
      // observe the effective clamped ceiling without a billable session turn — noted.)
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
    "rejects invalid runtime-size tokens client-side with SessionConfigValidationError",
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
      console.log("[edge-session-limits] invalid-size verdicts:", JSON.stringify(result.results, null, 2));

      // The public RuntimeSize preset set is closed. The SDK now validates this
      // before issuing HTTP, so bad tokens and wrong types must fail without
      // minting a billable session.
      for (const v of result.results) {
        expect(
          v.thrown,
          `runtime=${JSON.stringify(v.size)} should be rejected before submit: ${JSON.stringify(v)}`
        ).toBe(true);
        expect(v.name, `runtime=${JSON.stringify(v.size)} error name: ${JSON.stringify(v)}`).toBe("SessionConfigValidationError");
        expect(v.code, `runtime=${JSON.stringify(v.size)} error code: ${JSON.stringify(v)}`).toBe("SESSION_CONFIG_INVALID");
        expect(v.status ?? null, `runtime=${JSON.stringify(v.size)} error status: ${JSON.stringify(v)}`).toBeNull();
        expect(String(v.message), `runtime=${JSON.stringify(v.size)} error message: ${JSON.stringify(v)}`).toMatch(/runtimeSize must be one of/i);
        expect(v.reflect, `runtime=${JSON.stringify(v.size)} should not reach unit reflection`).toBeNull();
        expect((v as { sessionId?: unknown }).sessionId, `runtime=${JSON.stringify(v.size)} minted a session`).toBeUndefined();
      }
    },
    170_000
  );

  // -------------------------------------------------------------------------
  // Concurrency / depth: the wire SessionLimits carries maxConcurrentChildSessions and
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
        // Siblings on the SAME wire SessionLimits type — passed via overrides at
        // runtime (untyped in JS). If the SDK wired them, an invalid negative
        // would reject like maxSpendUsd; instead they are dropped => accepted.
        const concurrency = await createOnly({ ...BASE, overrides: { maxConcurrentChildSessions: -1 } });
        const depth = await createOnly({ ...BASE, overrides: { maxSubagentDepth: -1 } });
        const both = await createOnly({ ...BASE, overrides: { maxConcurrentChildSessions: 999999, maxSubagentDepth: 99 } });
        out({ spend, concurrency, depth, both });
      `, { timeoutMs: 120_000 });

      // Control behaves (proves the harness distinguishes reject vs accept).
      expect(result.spend.thrown, "control maxSpendUsd=-1 must reject").toBe(true);
      expect(result.spend.code).toBe("SESSION_CONFIG_INVALID");

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
    "rejects malformed and out-of-range timeout overrides client-side; accepts valid timeout",
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
      console.log("[edge-session-limits] timeout verdicts:", JSON.stringify(result));

      // The public timeout contract is validated before HTTP: malformed values
      // and values outside the 1m..8h bounds must fail without creating a
      // session. A valid in-range duration is the control.
      expect(result.malformed.thrown, `malformed timeout: ${JSON.stringify(result.malformed)}`).toBe(true);
      expect(result.malformed.name, `malformed timeout: ${JSON.stringify(result.malformed)}`).toBe("SessionConfigValidationError");
      expect(result.malformed.code, `malformed timeout: ${JSON.stringify(result.malformed)}`).toBe("SESSION_CONFIG_INVALID");
      expect(result.malformed.status ?? null, `malformed timeout: ${JSON.stringify(result.malformed)}`).toBeNull();
      expect(String(result.malformed.message), `malformed timeout: ${JSON.stringify(result.malformed)}`).toMatch(/invalid duration/i);

      expect(result.tooShort.thrown, `too-short timeout: ${JSON.stringify(result.tooShort)}`).toBe(true);
      expect(result.tooShort.name, `too-short timeout: ${JSON.stringify(result.tooShort)}`).toBe("SessionConfigValidationError");
      expect(result.tooShort.code, `too-short timeout: ${JSON.stringify(result.tooShort)}`).toBe("SESSION_CONFIG_INVALID");
      expect(result.tooShort.status ?? null, `too-short timeout: ${JSON.stringify(result.tooShort)}`).toBeNull();
      expect(String(result.tooShort.message), `too-short timeout: ${JSON.stringify(result.tooShort)}`).toMatch(/at least 60000ms/i);

      expect(result.tooLong.thrown, `too-long timeout: ${JSON.stringify(result.tooLong)}`).toBe(true);
      expect(result.tooLong.name, `too-long timeout: ${JSON.stringify(result.tooLong)}`).toBe("SessionConfigValidationError");
      expect(result.tooLong.code, `too-long timeout: ${JSON.stringify(result.tooLong)}`).toBe("SESSION_CONFIG_INVALID");
      expect(result.tooLong.status ?? null, `too-long timeout: ${JSON.stringify(result.tooLong)}`).toBeNull();
      expect(String(result.tooLong.message), `too-long timeout: ${JSON.stringify(result.tooLong)}`).toMatch(/at most 28800000ms/i);

      // A valid in-range duration is (also) accepted — the control.
      expect(result.valid.thrown, `valid timeout: ${JSON.stringify(result.valid)}`).toBe(false);
    },
    140_000
  );

  // -------------------------------------------------------------------------
  // ONE billable turn: prove a non-default sessiontime size actually SESSIONS green,
  // and capture any server-side reflection of the size for evidence.
  // -------------------------------------------------------------------------
  it(
    "sessions a tiny turn on a non-default sessiontime size (shared-0.5x-4gb) to terminal success",
    async () => {
      const probeMarker = "size-session-" + Math.random().toString(36).slice(2, 8);
      const result = await probe<{
        ok: boolean;
        status: string;
        text: string;
        sessionId: string;
        reflect: unknown;
      }>(`
        const sessionResult = await client.start({
          ...BASE,
          runtime: "shared-0.5x-4gb",
          message: ${JSON.stringify(`SessionFile verbatim: ${probeMarker}`)},
          idempotencyKey: "edge-size-session-" + Date.now()
        }, { timeoutMs: 8 * 60 * 1000 });
        // Best-effort: surface any server-side reflection of the chosen size.
        let reflect = null;
        try {
          const session = await client.sessions.open(sessionResult.sessionId);
          const unit = await session.unit();
          reflect = {
            capsSnapshot: unit && unit.capsSnapshot ? unit.capsSnapshot : null,
            runtimeManifestKeys: unit && unit.runtimeManifest ? Object.keys(unit.runtimeManifest) : null,
            runtimeManifest: unit && unit.runtimeManifest ? unit.runtimeManifest : null
          };
        } catch (e) { reflect = { error: String(e && e.message ? e.message : e).slice(0, 300) }; }
        out({
          ok: sessionResult.ok === true,
          status: String(sessionResult.status),
          text: typeof sessionResult.text === "string" ? sessionResult.text : "",
          sessionId: sessionResult.sessionId,
          reflect
        });
      `, { timeoutMs: 9 * 60 * 1000 });

      expect(result.ok, `run verdict: status=${result.status} text=${JSON.stringify(result.text).slice(0, 200)}`).toBe(true);
      expect(result.text.replace(/\s+/g, "")).toContain(probeMarker);
      // Reflection is diagnostic only — logged for the report, not asserted,
      // because the public read shape does not guarantee the size token is
      // echoed back.
      // eslint-disable-next-line no-console
      console.log("[edge-session-limits] size reflection:", JSON.stringify(result.reflect));
    },
    9 * 60 * 1000 + 30_000
  );
});

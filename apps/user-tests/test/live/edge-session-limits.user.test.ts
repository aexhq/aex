/**
 * Live edge-case sweep: per-session LIMIT / OVERRIDE surface.
 *
 * Surface under test (public @aexhq/sdk, dev plane):
 *   - `runtime` (RuntimeSize preset token) on submission.
 *   - `overrides.maxSpendUsd` (the ONLY SessionLimits field the SDK exposes).
 *   - `overrides.timeout` (session deadline duration string).
 *   - Unsupported `maxConcurrentChildSessions` / `maxSubagentDepth` override
 *     keys, which the SDK rejects instead of silently dropping.
 *
 * Cost discipline: almost every case is a CLIENT-SIDE validation rejection or a
 * create-only (no billable turn) submit probe. Exactly ONE case sends a billable
 * turn (a tiny run at a non-default runtime size to prove a size actually works).
 *
 * Each case runs a small .mjs script in the installed-SDK dir that performs the
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
      `user-tests live: required env ${name} is missing. The edge session-limits sweep runs against a real dev API URL with a real gate-provider key.`
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
const asErr = (e) => {
  const details = e && e.details && typeof e.details === "object" && !Array.isArray(e.details)
    ? e.details
    : null;
  return {
    thrown: true,
    name: e && e.name ? String(e.name) : null,
    code: e && e.code ? String(e.code) : null,
    status: (e && typeof e.status === "number") ? e.status : null,
    hasMessage: !!(e && e.message),
    detailsField: details && typeof details.field === "string" ? details.field : null
  };
};
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
  readonly hasMessage?: boolean;
  readonly detailsField?: string | null;
  readonly sessionId?: string | null;
  readonly sessionStatus?: string | null;
  readonly deleted?: boolean;
  readonly [k: string]: unknown;
}

function expectConfigError(verdict: Verdict, field: string, label: string): void {
  expect(verdict.thrown, `${label} should reject`).toBe(true);
  expect(verdict.name, `${label} error name`).toBe("SessionConfigValidationError");
  expect(verdict.code, `${label} error code`).toBe("SESSION_CONFIG_INVALID");
  expect(verdict.status ?? null, `${label} should not carry an HTTP status`).toBeNull();
  expect(verdict.hasMessage, `${label} should retain human guidance`).toBe(true);
  expect(verdict.detailsField, `${label} stable field`).toBe(field);
  expect(verdict.sessionId, `${label} minted a session`).toBeUndefined();
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
  // parseSessionLimits runs inside #buildSessionCreateRequest BEFORE any network,
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
        expectConfigError(v, "overrides.maxSpendUsd", `maxSpendUsd=${key}`);
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
            const h = await client.sessions.create({ ...BASE, runtime: { size } });
            const rm = h.record.runtimeManifest ?? null;
            const reflect = {
              recordRuntime: h.record.runtime?.size ?? null,
              runtimeManifestResources: rm && (rm.resources ?? rm.runtime ?? rm.size ?? null)
            };
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
        expectConfigError(v, "runtime", `runtime=${JSON.stringify(v.size)}`);
        expect(v.reflect, `runtime=${JSON.stringify(v.size)} should not reach unit reflection`).toBeNull();
      }
    },
    170_000
  );

  // -------------------------------------------------------------------------
  // Concurrency / depth are not public SessionOverrides fields. JavaScript callers
  // still receive a typed, field-addressable error instead of silent omission.
  // -------------------------------------------------------------------------
  it(
    "rejects unsupported concurrency/depth override keys before submission",
    async () => {
      const result = await probe<{
        spend: Verdict;
        concurrency: Verdict;
        depth: Verdict;
      }>(`
        // Control: an invalid supported override also rejects client-side.
        const spend = await createOnly({ ...BASE, overrides: { maxSpendUsd: -1 } });
        // These keys are deliberately absent from SessionOverrides. Probe from
        // untyped JavaScript to prove they are rejected rather than ignored.
        const concurrency = await createOnly({ ...BASE, overrides: { maxConcurrentChildSessions: -1 } });
        const depth = await createOnly({ ...BASE, overrides: { maxSubagentDepth: -1 } });
        out({ spend, concurrency, depth });
      `, { timeoutMs: 120_000 });

      expectConfigError(result.spend, "overrides.maxSpendUsd", "control maxSpendUsd=-1");
      expectConfigError(
        result.concurrency,
        "overrides.maxConcurrentChildSessions",
        "unsupported concurrency override"
      );
      expectConfigError(result.depth, "overrides.maxSubagentDepth", "unsupported depth override");
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
      expectConfigError(result.malformed, "overrides.timeout", "malformed timeout");
      expectConfigError(result.tooShort, "overrides.timeout", "too-short timeout");
      expectConfigError(result.tooLong, "overrides.timeout", "too-long timeout");

      // A valid in-range duration is (also) accepted — the control.
      expect(result.valid.thrown, `valid timeout: ${JSON.stringify(result.valid)}`).toBe(false);
    },
    140_000
  );

  // -------------------------------------------------------------------------
  // ONE billable turn: prove a non-default runtime size actually runs successfully,
  // and capture any server-side reflection of the size for evidence.
  // -------------------------------------------------------------------------
  it(
    "runs a tiny turn on a non-default runtime size (shared-0.5x-4gb) to terminal success",
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
          runtime: { size: "shared-0.5x-4gb" },
          message: ${JSON.stringify(`SessionFile verbatim: ${probeMarker}`)},
          idempotencyKey: "edge-size-session-" + Date.now()
        }, { timeoutMs: 8 * 60 * 1000 });
        const session = await client.sessions.open(sessionResult.sessionId);
        const reflect = {
          runtime: session.record.runtime?.size ?? null,
          runtimeManifest: session.record.runtimeManifest ?? null
        };
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
      expect((result.reflect as { runtime?: unknown }).runtime).toBe("shared-0.5x-4gb");
      // eslint-disable-next-line no-console
      console.log("[edge-session-limits] size run evidence:", JSON.stringify({
        sessionId: result.sessionId,
        status: result.status,
        reflect: result.reflect
      }));
    },
    9 * 60 * 1000 + 30_000
  );
});

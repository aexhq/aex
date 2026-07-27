/**
 * Live edge-case sweep: per-session LIMIT / OVERRIDE surface.
 *
 * Surface under test (public @aexhq/sdk, dev plane) — only what a PLANE can
 * answer, i.e. which values the SERVER accepts at submit:
 *   - every `runtime` RuntimeSize preset token is accepted (create-only).
 *   - a huge-but-positive `overrides.maxSpendUsd` and an in-range
 *     `overrides.timeout` are accepted; clamping is the server resolver's job.
 *   - a tiny turn actually runs at a non-default runtime size.
 *
 * The mirror-image REJECTIONS (bad maxSpendUsd, bad size token, unsupported
 * `maxConcurrentChildSessions`/`maxSubagentDepth`, malformed/out-of-range
 * timeout) are a client-side gate that never issues an HTTP request, so they
 * moved to `test/offline/session-limits-validation.test.ts` on 2026-07-27 rather
 * than paying a live runtime-matrix job to prove the network was not used.
 *
 * Cost discipline: two create-only (no billable turn) submit probes plus exactly
 * ONE billable turn (a tiny run at a non-default runtime size).
 *
 * Each case runs a small .mjs script in the installed-SDK dir that performs the
 * SDK action, catches any error, and prints a structured JSON verdict the test
 * asserts on. Scripts always `process.exit(0)` with JSON so a caught rejection
 * is data, not a non-zero exit.
 *
 * Required env (exported by the shared runner from .env.dev):
 *   AEX_API_URL, AEX_API_KEY
 * Optional:
 *   AEX_USER_TEST_DEEPSEEK_MODEL  (default "deepseek/deepseek-v4-flash")
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { gateModel } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(
      `user-tests live: required env ${name} is missing. The edge session-limits sweep runs against a real hosted API.`
    );
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const model = gateModel();

/** Every valid runtime-size preset token (mirrors RUNTIME_SIZE_PRESETS keys). */
const VALID_SIZES = [
  "0.25cpu-1gb",
  "0.5cpu-4gb",
  "1cpu-6gb",
  "2cpu-8gb",
  "4cpu-12gb"
] as const;

const PREAMBLE = `
import { Aex } from "@aexhq/sdk";
const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
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
const BASE = { model: MODEL };
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
  // What only a PLANE can answer: which override values the server ACCEPTS at
  // submit. The mirror-image rejections are a client-side gate that never
  // reaches HTTP, so they live in test/offline/session-limits-validation.test.ts.
  // -------------------------------------------------------------------------
  it(
    "accepts a huge maxSpendUsd and an in-range timeout at submit — clamping is the server resolver's job",
    async () => {
      // Contract: parseSessionLimits only checks shape+positivity; the workspace/
      // platform ceiling is applied by resolveSessionLimits server-side. So a huge
      // value must be ACCEPTED at submit, not rejected. (We cannot cheaply
      // observe the effective clamped ceiling without a billable session turn — noted.)
      const result = await probe<{ spend: Verdict; timeout: Verdict }>(`
        const spend = await createOnly({ ...BASE, overrides: { maxSpendUsd: 1000000000 } });
        const timeout = await createOnly({ ...BASE, overrides: { timeout: "5m" } });
        out({ spend, timeout });
      `, { timeoutMs: 120_000 });

      expect(result.spend.thrown, `huge maxSpendUsd verdict: ${JSON.stringify(result.spend)}`).toBe(false);
      expect(typeof result.spend.sessionId).toBe("string");
      expect(result.timeout.thrown, `in-range timeout verdict: ${JSON.stringify(result.timeout)}`).toBe(false);
      expect(typeof result.timeout.sessionId).toBe("string");
    },
    140_000
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
          const r = await createOnly({ ...BASE, runtime: { size } });
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


  // -------------------------------------------------------------------------
  // ONE billable turn: prove a non-default runtime size actually runs successfully,
  // and capture any server-side reflection of the size for evidence.
  // -------------------------------------------------------------------------
  it(
    "runs a tiny turn on a non-default runtime size (0.5cpu-4gb) to terminal success",
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
          runtime: { size: "0.5cpu-4gb" },
          message: ${JSON.stringify(`Reply with exactly the following token and nothing else, character for character: ${probeMarker}`)},
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
      expect((result.reflect as { runtime?: unknown }).runtime).toBe("0.5cpu-4gb");
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

/**
 * Live edge-case sweep: openrouter managed runs must fail honestly (or work).
 *
 * REGRESSION PROBE — fixed after the 2026-07-04 dev sweep. Managed OpenRouter
 * runs used to crash BEFORE the first LLM call and burn the whole recovery
 * budget:
 *
 *   - The api Lambda freezes the CANONICAL model id into the boot
 *     sessionConfig (`model: sub.model`, infra/lambdas/src/api.ts:3144)
 *     without translating to the provider-NATIVE id
 *     (`resolveProviderModelId`, e.g. gpt-4o-mini → "openai/gpt-4o-mini").
 *   - The in-container brain's model registry is keyed on the NATIVE id
 *     (packages/agent-session/src/models.ts MODEL_REGISTRY), so
 *     `registryFor` throws `no MODEL_REGISTRY entry for
 *     openrouter/gpt-4o-mini` (packages/container-runtime/src/brain.ts:1228),
 *     the worker exits 1, and the epoch-bump recovery loop relaunches the
 *     container SIX times (~7 minutes) for a deterministic failure.
 *   - The customer-facing terminal is the generic "run worker exited before
 *     producing a terminal result (exitCode=1)" with failureClass
 *     "recoveries_exhausted" — the real cause never surfaces.
 *   - Secondary drift: MODEL_REGISTRY has no "openai/gpt-4o" entry at all,
 *     though the public contracts advertise gpt-4o with openrouter as its
 *     ONLY provider route.
 *
 * The probe uses a syntactically plausible FAKE key: the crash happens
 * before any provider auth, and after the platform fix the same probe run
 * should surface an HONEST provider auth failure (failureClass
 * "provider-permanent" naming HTTP 401/auth), never "recoveries_exhausted".
 * Repro evidence: run_3b972773ba204cb75519a35199f64397 /
 * run_3bb525927902852f9e501462740b93b5 (dev, 2026-07-04).
 *
 * Cost: no billable LLM turns today (the run dies pre-LLM), but several
 * container boots and ~7 minutes of wall clock while the defect stands.
 *
 * Required env: AEX_API_URL, AEX_API_KEY + AEX_USER_TEST_TARBALL/VERSION
 * (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-openrouter-managed): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");

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

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs: number
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(
    scriptPath,
    `
    import { Aex, isTerminalSessionStatus } from "@aexhq/sdk";
    const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiKey: process.env.AEX_API_KEY });
    ${body}
    `
  );
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({ AEX_API_URL: apiUrl, AEX_API_KEY: apiKey })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-openrouter-managed runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-openrouter-managed runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: openrouter managed run fails honestly", () => {
  it(
    "an openrouter run with a bad key surfaces a provider auth failure, not a recovery crash-loop",
    async () => {
      const body = `
        const out = { sessionId: null, status: null, failureClass: null, errorMessage: null, error: null, elapsedMs: null, pollTrace: [] };
        let sid = null;
        const t0 = Date.now();
        try {
          const session = await client.openSession({
            model: "gpt-4o-mini",
            provider: "openrouter",
            apiKeys: { openrouter: "sk-or-v1-" + "0".repeat(64) },
          });
          sid = session.id;
          out.sessionId = sid;
          try {
            await session.send("Reply with exactly OK.").done();
          } catch {}
          // Poll the record to a terminal/parked state (the crash-loop today
          // takes ~7 minutes; an honest provider-permanent failure takes ~1-2).
          const deadline = Date.now() + 9 * 60_000;
          while (Date.now() < deadline) {
            const rec = await client.sessions.get(sid);
            out.pollTrace.push({
              t: Date.now() - t0,
              status: rec.status,
              lastTurnOutcome: rec.lastTurnOutcome ?? null,
              failureClass: rec.failureClass ?? null,
              hasErrorMessage: Boolean(rec.errorMessage)
            });
            if (out.pollTrace.length > 12) out.pollTrace.shift();
            if (isTerminalSessionStatus(rec.status)) {
              out.status = rec.status;
              out.lastTurnOutcome = rec.lastTurnOutcome ?? null;
              out.failureClass = rec.failureClass ?? null;
              out.errorMessage = (rec.errorMessage ?? "").slice(0, 300);
              break;
            }
            await new Promise((r) => setTimeout(r, 10_000));
          }
          out.elapsedMs = Date.now() - t0;
        } catch (e) {
          out.error = String(e).slice(0, 500);
        } finally {
          if (sid) { try { await client.sessions.delete(sid); } catch {} }
        }
        console.log(JSON.stringify(out));
      `;
      const out = await runChild(install, "openrouter-badkey-probe.mjs", body, 12 * 60_000);
      const dump = JSON.stringify(out).slice(0, 1200);
      expect(out.error, `OpenRouter probe threw before terminal diagnostics: ${dump}`).toBeNull();
      expect(out.status, `OpenRouter bad-key probe did not terminalize as failed: ${dump}`).toBe("failed");
      expect(out.failureClass, `OpenRouter bad-key probe did not expose provider auth failure: ${dump}`).toBe(
        "provider-permanent"
      );
      expect(String(out.errorMessage), `OpenRouter bad-key error did not name auth/401: ${dump}`).toMatch(/401|auth/i);
    },
    14 * 60_000
  );
});

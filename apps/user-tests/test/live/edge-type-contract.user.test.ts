/**
 * Live edge-case sweep: public type-contract gaps found by the iter-8
 * SDK-shape audit (2026-07-04).
 *
 * DEFECT PROBES — red on the dev plane until the platform closes them:
 *
 *   1. Token usage is unavailable on EVERY public surface. `RunResult.usage`
 *      / `Run.usage` / `Session.usage` document aggregate token counts "when
 *      the deployment exposes it", but the managed plane emits no `aex.usage`
 *      events, never populates record `usage`, and the served
 *      `costTelemetry` (GET /api/runs/:id) carries no `providerUsage` block —
 *      a customer cannot see input/output token counts for any run.
 *   2. Prompt size is unbounded at session create: a multi-MiB prompt is
 *      admitted (201) with no server-side cap (same family as the unbounded
 *      agentsMd finding — the only ceiling is the API gateway body limit).
 *
 * Billing: probe 1 runs ONE tiny billable turn (~$0.0004); probe 2 creates a
 * born-empty idle session and deletes it (zero billable).
 *
 * Required env: AEX_API_URL, AEX_API_TOKEN, DEEPSEEK_API_KEY, +
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
    throw new Error(`user-tests live (edge-type-contract): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiToken = requireEnv("AEX_API_TOKEN");
const providerKey = requireGateKey("edge-type-contract");
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

const CHILD_PRELUDE = `
  import { Aex } from "@aexhq/sdk";
  const client = new Aex({ baseUrl: process.env.AEX_API_URL, apiToken: process.env.AEX_API_TOKEN });
  const PROVIDER = process.env.PROVIDER;
  const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;
  const raw = async (method, path, body) => {
    const res = await fetch(process.env.AEX_API_URL + path, {
      method,
      headers: { authorization: "Bearer " + process.env.AEX_API_TOKEN, "content-type": "application/json" },
      body: body ? JSON.stringify(body) : undefined
    });
    let parsed = null;
    try { parsed = await res.json(); } catch {}
    return { status: res.status, body: parsed };
  };
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 5 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_TOKEN: apiToken,
      PROVIDER: GATE_PROVIDER,
      PROVIDER_KEY: providerKey,
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-type-contract runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-type-contract runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: public type-contract gaps", () => {
  it(
    "token usage is exposed on at least one public surface for a settled run",
    async () => {
      const body = `
        const out = { runId: null, resultUsage: null, sessionUsage: null, providerUsage: null, usageEvents: 0 };
        const result = await client.run(
          {
            provider: PROVIDER,
            model: MODEL,
            includeBuiltinTools: false,
            apiKeys: { [PROVIDER]: PROVIDER_KEY },
            message: "Reply with exactly: USAGE-PROBE-OK"
          },
          { timeoutMs: 240000, settleConsistent: true }
        );
        out.runId = result.runId;
        out.resultUsage = result.usage ?? null;
        out.usageEvents = result.events.filter(
          (e) => e.type === "CUSTOM" && e.data && e.data.name === "aex.usage"
        ).length;
        const record = await client.sessions.get(result.runId);
        out.sessionUsage = record.usage ?? null;
        const unit = await raw("GET", "/api/runs/" + result.runId);
        out.providerUsage = unit.body && unit.body.costTelemetry && unit.body.costTelemetry.providerUsage
          ? unit.body.costTelemetry.providerUsage
          : null;
        await raw("DELETE", "/api/sessions/" + result.runId);
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "type-contract-usage.mjs", body, 6 * 60_000);
      // DEFECT (dev): all three are null/0 — token counts are documented on
      // RunResult.usage / Run.usage / costTelemetry.providerUsage but no
      // public surface ever carries them.
      const anyUsage =
        result.resultUsage !== null ||
        result.sessionUsage !== null ||
        result.providerUsage !== null ||
        (result.usageEvents as number) > 0;
      expect(anyUsage).toBe(true);
    },
    8 * 60_000
  );

  it(
    "a multi-MiB prompt is rejected at session create",
    async () => {
      const body = `
        const out = { status: null, error: null, admittedId: null };
        const r = await raw("POST", "/api/sessions", {
          provider: PROVIDER,
          submission: { model: MODEL, includeBuiltinTools: false, prompt: ["x".repeat(2 * 1024 * 1024)] },
          secrets: { apiKeys: { [PROVIDER]: "sk-probe-fake-key" } }
        });
        out.status = r.status;
        out.error = r.body && typeof r.body.error === "string" ? r.body.error : null;
        const admitted = r.body && (r.body.session?.id ?? r.body.id ?? r.body.runId);
        if (admitted) {
          out.admittedId = admitted;
          await raw("DELETE", "/api/sessions/" + admitted);
        }
        console.log(JSON.stringify(out));
      `;
      const result = await runChild(install, "type-contract-prompt-cap.mjs", body);
      // DEFECT (dev): 201 — no server-side prompt-size cap; a 2 MiB (and a
      // live-verified 5 MiB) prompt is admitted at create.
      expect([400, 413]).toContain(result.status);
    },
    5 * 60_000
  );
});

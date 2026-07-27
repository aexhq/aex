/**
 * Offline coverage of the per-session LIMIT / OVERRIDE client-side gate, through
 * a clean installed `@aexhq/sdk` (blackbox, child process, cwd = install tempdir).
 *
 * `parseSessionLimits` runs inside `#buildSessionCreateRequest` BEFORE any
 * network call, so every rejection below is a client-side contract. These cases
 * were the majority of `test/live/edge-session-limits.user.test.ts` — its own
 * header said "almost every case is a CLIENT-SIDE validation rejection" — yet
 * they paid a live runtime-matrix job. Split out 2026-07-27.
 *
 * The injected fetch makes the claim testable rather than assumed: it records
 * every call, and each case asserts the SDK issued ZERO requests. The live file
 * keeps only what a plane can answer: which values the SERVER accepts at submit.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

interface Verdict {
  readonly thrown: boolean;
  readonly name: string | null;
  readonly code: string | null;
  readonly status: number | null;
  readonly hasMessage: boolean;
  readonly detailsField: string | null;
  readonly httpCalls: number;
}

const SCRIPT = String.raw`
import { Aex } from "@aexhq/sdk";

const calls = [];
const client = new Aex({
  baseUrl: "https://example.invalid",
  apiKey: "aex_offline_session_limits",
  fetch: async (input) => {
    calls.push(String(input));
    return Response.json({ error: "the offline limits gate must never reach the network" }, { status: 500 });
  }
});

const BASE = { model: "deepseek/deepseek-v4-flash" };

async function attempt(options) {
  const before = calls.length;
  try {
    await client.sessions.create({ ...BASE, ...options });
    return { thrown: false, name: null, code: null, status: null, hasMessage: false, detailsField: null, httpCalls: calls.length - before };
  } catch (err) {
    const details = err && err.details && typeof err.details === "object" && !Array.isArray(err.details) ? err.details : null;
    return {
      thrown: true,
      name: err && err.name ? String(err.name) : null,
      code: err && err.code ? String(err.code) : null,
      status: err && typeof err.status === "number" ? err.status : null,
      hasMessage: !!(err && err.message),
      detailsField: details && typeof details.field === "string" ? details.field : null,
      httpCalls: calls.length - before
    };
  }
}

process.stdout.write(JSON.stringify({
  maxSpendUsd: {
    zero: await attempt({ overrides: { maxSpendUsd: 0 } }),
    negative: await attempt({ overrides: { maxSpendUsd: -2.5 } }),
    stringy: await attempt({ overrides: { maxSpendUsd: "5" } }),
    infinity: await attempt({ overrides: { maxSpendUsd: Number.POSITIVE_INFINITY } }),
    nan: await attempt({ overrides: { maxSpendUsd: Number.NaN } })
  },
  runtimeSize: {
    // "lite" = a plausible friendly-name guess; "shared-8x-999gb" = a fake preset
    // shaped like a real token; 4096 = the wrong TYPE.
    friendlyName: await attempt({ runtime: { size: "lite" } }),
    fakePreset: await attempt({ runtime: { size: "shared-8x-999gb" } }),
    wrongType: await attempt({ runtime: { size: 4096 } })
  },
  unsupportedOverrides: {
    concurrency: await attempt({ overrides: { maxConcurrentChildSessions: -1 } }),
    depth: await attempt({ overrides: { maxSubagentDepth: -1 } })
  },
  timeout: {
    malformed: await attempt({ overrides: { timeout: "banana" } }),
    tooShort: await attempt({ overrides: { timeout: "10s" } }),
    tooLong: await attempt({ overrides: { timeout: "99h" } })
  },
  totalHttpCalls: calls.length
}));
`;

describe("installed SDK per-session limit/override client-side gate", () => {
  let install: InstallResult;
  let result: {
    maxSpendUsd: Record<string, Verdict>;
    runtimeSize: Record<string, Verdict>;
    unsupportedOverrides: Record<string, Verdict>;
    timeout: Record<string, Verdict>;
    totalHttpCalls: number;
  };

  beforeAll(async () => {
    install = await installAex();
    const path = join(install.installDir, "session-limits-validation.mjs");
    writeFileSync(path, SCRIPT);
    const child = await runCommand(getBunCommand(), [path], {
      cwd: install.installDir,
      timeoutMs: 120_000
    });
    if (child.exitCode !== 0) {
      throw new Error(`session-limits-validation.mjs exited ${child.exitCode}\n${child.stderr}`);
    }
    result = JSON.parse(child.stdout.trim());
  }, 300_000);

  afterAll(() => install?.cleanup());

  function expectConfigError(verdict: Verdict, field: string, label: string): void {
    expect(verdict.thrown, `${label} should reject`).toBe(true);
    expect(verdict.name, `${label} error name`).toBe("SessionConfigValidationError");
    expect(verdict.code, `${label} error code`).toBe("SESSION_CONFIG_INVALID");
    expect(verdict.status, `${label} should not carry an HTTP status`).toBeNull();
    expect(verdict.hasMessage, `${label} should retain human guidance`).toBe(true);
    expect(verdict.detailsField, `${label} stable field`).toBe(field);
    // The whole point of a client-side gate: no request, so no billable session.
    expect(verdict.httpCalls, `${label} reached the network`).toBe(0);
  }

  it("rejects invalid maxSpendUsd values (0, negative, string, Infinity, NaN) before any HTTP call", () => {
    for (const key of ["zero", "negative", "stringy", "infinity", "nan"]) {
      expectConfigError(result.maxSpendUsd[key]!, "overrides.maxSpendUsd", `maxSpendUsd=${key}`);
    }
  });

  it("rejects invalid runtime-size tokens with SessionConfigValidationError", () => {
    // The public RuntimeSize preset set is closed, so a bad token or a wrong type
    // must fail without minting a session — never silently default to the
    // smallest box, which is the defect this case was written for.
    for (const key of ["friendlyName", "fakePreset", "wrongType"]) {
      expectConfigError(result.runtimeSize[key]!, "runtime.size", `runtime.size=${key}`);
    }
  });

  it("rejects unsupported concurrency/depth override keys instead of silently dropping them", () => {
    expectConfigError(
      result.unsupportedOverrides["concurrency"]!,
      "overrides.maxConcurrentChildSessions",
      "unsupported concurrency override"
    );
    expectConfigError(
      result.unsupportedOverrides["depth"]!,
      "overrides.maxSubagentDepth",
      "unsupported depth override"
    );
  });

  it("rejects malformed and out-of-range (1m..8h) timeout overrides", () => {
    expectConfigError(result.timeout["malformed"]!, "overrides.timeout", "malformed timeout");
    expectConfigError(result.timeout["tooShort"]!, "overrides.timeout", "too-short timeout");
    expectConfigError(result.timeout["tooLong"]!, "overrides.timeout", "too-long timeout");
  });

  it("issues no HTTP request at all across the whole gate", () => {
    expect(result.totalHttpCalls).toBe(0);
  });
});

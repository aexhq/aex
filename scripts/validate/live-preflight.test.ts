import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));
const preflightUrl = pathToFileURL(resolve(repoRoot, "scripts/cicd/preflight-live-user-tests.mjs")).href;

const baseEnv = {
  AEX_API_URL: "https://dev-api.aex.dev",
  AEX_API_KEY: "aex_secret_token",
  DEEPSEEK_API_KEY: "deepseek_secret",
  LIVE_USER_TEST_MIN_MAX_CONCURRENT_RUNS: "50",
  LIVE_USER_TEST_PREFLIGHT_RETRY_BASE_MS: "1"
};

interface ChildResult {
  readonly ok: boolean;
  readonly message?: string;
  readonly result?: {
    readonly status: number;
    readonly maxConcurrentRuns: number;
    readonly attempt: number;
  };
  readonly calls: number;
  readonly sleeps: number[];
  readonly logs: string;
  readonly out: string;
}

function runScenario(scenario: string): ChildResult {
  const code = `
    const mod = await import(${JSON.stringify(preflightUrl)});
    const scenario = ${JSON.stringify(scenario)};
    const baseEnv = ${JSON.stringify(baseEnv)};
    const sleeps = [];
    const logs = [];
    const out = [];
    let calls = 0;
    const response = (status, body, headers = {}) => new Response(JSON.stringify(body), { status, headers });
    const fetchImpl = async () => {
      calls += 1;
      if (scenario === "retry503") {
        return calls === 1
          ? response(503, { error: "db_resuming" }, { "apigw-requestid": "req-1", "retry-after": "3" })
          : response(200, { limits: { maxConcurrentRuns: 50 } }, { "x-amzn-requestid": "req-2" });
      }
      if (scenario === "auth401") return response(401, { error: "unauthorized" });
      if (scenario === "lowLimit") return response(200, { limits: { maxConcurrentRuns: 49 } });
      return response(200, { limits: { maxConcurrentRuns: 50 } });
    };
    try {
      const env = {
        ...baseEnv,
        ...(scenario === "missingEnv" ? { AEX_API_KEY: undefined, DEEPSEEK_API_KEY: undefined } : {}),
        ...(scenario === "blankEnv" ? { AEX_API_KEY: "   ", DEEPSEEK_API_KEY: "" } : {}),
        ...(scenario === "privateUrl" ? { AEX_API_URL: "https://127.0.0.1:8787" } : {}),
        ...(scenario === "allowPrivateUrl" ? { AEX_API_URL: "https://127.0.0.1:8787", LIVE_USER_TEST_ALLOW_PRIVATE_API_URL: "true" } : {}),
        ...(scenario === "nonHttps" ? { AEX_API_URL: "http://dev-api.aex.dev" } : {}),
        ...(scenario === "wrongHost" ? { AEX_EXPECTED_API_HOST: "api.aex.dev" } : {}),
        ...(scenario === "capCeiling" ? { LIVE_USER_TEST_MAX_MAX_CONCURRENT_RUNS: "20" } : {})
      };
      const result = await mod.checkLiveUserTestsPreflight({
        env,
        fetchImpl,
        sleepFn: async (ms) => { sleeps.push(ms); },
        err: { write: (s) => logs.push(s) },
        out: { write: (s) => out.push(s) }
      });
      process.stdout.write(JSON.stringify({ ok: true, result, calls, sleeps, logs: logs.join(""), out: out.join("") }));
    } catch (error) {
      process.stdout.write(JSON.stringify({
        ok: false,
        message: error instanceof Error ? error.message : String(error),
        calls,
        sleeps,
        logs: logs.join(""),
        out: out.join("")
      }));
    }
  `;
  return JSON.parse(
    execFileSync(process.execPath, ["--input-type=module", "--eval", code], {
      cwd: repoRoot,
      encoding: "utf8"
    })
  ) as ChildResult;
}

describe("live user-test preflight", () => {
  it("retries transient HTTP whoami failures without logging secrets", () => {
    const result = runScenario("retry503");

    expect(result.ok).toBe(true);
    expect(result.result).toMatchObject({ status: 200, maxConcurrentRuns: 50, attempt: 2 });
    expect(result.calls).toBe(2);
    expect(result.sleeps).toEqual([3000]);
    expect(result.logs).toContain("transient HTTP 503");
    expect(result.logs).toContain("requestId=req-1");
    expect(`${result.logs}${result.out}`).not.toContain(baseEnv.AEX_API_KEY);
    expect(`${result.logs}${result.out}`).not.toContain(baseEnv.DEEPSEEK_API_KEY);
  });

  it("does not retry deterministic auth failures", () => {
    const result = runScenario("auth401");

    expect(result.ok).toBe(false);
    expect(result.message).toMatch(/status=401/);
    expect(result.calls).toBe(1);
  });

  it("fails when required environment values are missing", () => {
    const result = runScenario("missingEnv");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("AEX_API_KEY, DEEPSEEK_API_KEY");
    expect(result.calls).toBe(0);
  });

  it("treats blank required environment values as missing", () => {
    const result = runScenario("blankEnv");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("AEX_API_KEY, DEEPSEEK_API_KEY");
    expect(result.calls).toBe(0);
  });

  it("requires the workspace concurrency floor", () => {
    const result = runScenario("lowLimit");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("maxConcurrentRuns=49");
  });

  it("rejects private live endpoints unless explicitly allowed", () => {
    const result = runScenario("privateUrl");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("is not a public live endpoint");
    expect(result.calls).toBe(0);
  });

  it("can explicitly allow private endpoints for non-live local verification", () => {
    const result = runScenario("allowPrivateUrl");

    expect(result.ok).toBe(true);
    expect(result.result).toMatchObject({ status: 200, maxConcurrentRuns: 50 });
    expect(result.calls).toBe(1);
  });

  it("rejects non-HTTPS live endpoints before network I/O", () => {
    const result = runScenario("nonHttps");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("must use https");
    expect(result.calls).toBe(0);
  });

  it("requires the expected API host when configured", () => {
    const result = runScenario("wrongHost");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("does not match expected host=api.aex.dev");
    expect(result.calls).toBe(0);
  });

  it("can enforce a concurrency ceiling for feature/admission gates", () => {
    const result = runScenario("capCeiling");

    expect(result.ok).toBe(false);
    expect(result.message).toContain("above allowed maximum 20");
  });
});

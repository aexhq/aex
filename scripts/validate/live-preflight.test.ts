import { execFileSync, spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "bun:test";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const preflightUrl = pathToFileURL(resolve(repoRoot, "scripts/cicd/preflight-live-user-tests.mjs")).href;
const baseEnv = {
  AEX_API_URL: "https://eu-west-1.dev-api.aex.dev",
  AEX_API_KEY: "aex_secret_token",
  LIVE_USER_TEST_PREFLIGHT_RETRY_BASE_MS: "1"
};

interface ChildResult {
  readonly ok: boolean;
  readonly message?: string;
  readonly result?: { readonly status: number; readonly attempt: number };
  readonly calls: number;
  readonly sleeps: number[];
  readonly urls: string[];
  readonly logs: string;
  readonly out: string;
}

function runScenario(scenario: string): ChildResult {
  const source = `
    const mod = await import(${JSON.stringify(preflightUrl)});
    const scenario = ${JSON.stringify(scenario)};
    const baseEnv = ${JSON.stringify(baseEnv)};
    const sleeps = [];
    const urls = [];
    const logs = [];
    const out = [];
    let calls = 0;
    const response = (status, body, headers = {}) =>
      new Response(JSON.stringify(body), { status, headers });
    const fetchImpl = async (url) => {
      urls.push(String(url));
      calls += 1;
      if (scenario === "retry503") {
        return calls === 1
          ? response(503, { error: "resuming" }, { "apigw-requestid": "req-1", "retry-after": "3" })
          : response(200, { items: [] }, { "x-amzn-requestid": "req-2" });
      }
      if (scenario === "connectionRefused" && calls === 1) {
        const error = new Error("Unable to connect. Is the computer able to access the url?");
        Object.assign(error, { code: "ConnectionRefused" });
        throw error;
      }
      if (scenario === "auth401") return response(401, { error: "unauthorized" });
      if (scenario === "malformed") return response(200, {});
      return response(200, { items: [] });
    };
    const env = {
      ...baseEnv,
      ...(scenario === "missingEnv" ? { AEX_API_KEY: undefined } : {}),
      ...(scenario === "blankEnv" ? { AEX_API_KEY: "   " } : {}),
      ...(scenario === "privateUrl" ? { AEX_API_URL: "https://127.0.0.1:8787" } : {}),
      ...(scenario === "allowPrivateUrl"
        ? { AEX_API_URL: "https://127.0.0.1:8787", LIVE_USER_TEST_ALLOW_PRIVATE_API_URL: "true" }
        : {}),
      ...(scenario === "nonHttps" ? { AEX_API_URL: "http://dev-api.aex.dev" } : {}),
      ...(scenario === "wrongHost" ? { AEX_EXPECTED_API_HOST: "api.aex.dev" } : {})
    };
    try {
      const result = await mod.checkLiveUserTestsPreflight({
        env,
        fetchImpl,
        sleepFn: async (ms) => sleeps.push(ms),
        err: { write: (text) => logs.push(text) },
        out: { write: (text) => out.push(text) }
      });
      process.stdout.write(JSON.stringify({
        ok: true, result, calls, sleeps, urls, logs: logs.join(""), out: out.join("")
      }));
    } catch (error) {
      process.stdout.write(JSON.stringify({
        ok: false,
        message: error instanceof Error ? error.message : String(error),
        calls, urls,
        sleeps,
        logs: logs.join(""),
        out: out.join("")
      }));
    }
  `;
  return JSON.parse(execFileSync(process.execPath, ["--input-type=module", "--eval", source], {
    cwd: repoRoot,
    encoding: "utf8"
  })) as ChildResult;
}

describe("live user-test preflight", () => {
  it("proves authenticated access through the canonical registry list route", () => {
    const result = runScenario("retry503");

    expect(result.ok).toBe(true);
    expect(result.result).toMatchObject({ status: 200, attempt: 2 });
    expect(result.calls).toBe(2);
    expect(result.sleeps).toEqual([3_000]);
    expect(result.urls).toEqual([
      "https://eu-west-1.dev-api.aex.dev/api/files?limit=1",
      "https://eu-west-1.dev-api.aex.dev/api/files?limit=1",
    ]);
    expect(result.logs).toContain("/api/files transient HTTP 503");
    expect(result.out).toContain("/api/files preflight passed");
    expect(`${result.logs}${result.out}`).not.toContain(baseEnv.AEX_API_KEY);
  });

  it("fails fast for deterministic authentication and response-shape errors", () => {
    const unauthorized = runScenario("auth401");
    expect(unauthorized.ok).toBe(false);
    expect(unauthorized.message).toContain("status=401");
    expect(unauthorized.calls).toBe(1);

    const malformed = runScenario("malformed");
    expect(malformed.ok).toBe(false);
    expect(malformed.message).toContain("items array");
    expect(malformed.calls).toBe(1);
  });

  it("validates required environment and public HTTPS origin before network I/O", () => {
    for (const [scenario, message] of [
      ["missingEnv", "AEX_API_KEY"],
      ["blankEnv", "AEX_API_KEY"],
      ["privateUrl", "not a public live endpoint"],
      ["nonHttps", "must use https"],
      ["wrongHost", "does not match expected host"]
    ] as const) {
      const result = runScenario(scenario);
      expect(result.ok).toBe(false);
      expect(result.message).toContain(message);
      expect(result.calls).toBe(0);
    }
  });

  it("allows an explicitly authorized private endpoint for local verification", () => {
    const result = runScenario("allowPrivateUrl");
    expect(result.ok).toBe(true);
    expect(result.calls).toBe(1);
  });

  it("retries Bun's named connection failure before declaring the live endpoint unavailable", () => {
    const result = runScenario("connectionRefused");

    expect(result.ok).toBe(true);
    expect(result.calls).toBe(2);
    expect(result.sleeps).toEqual([1]);
    expect(result.logs).toContain("code=ConnectionRefused");
  });

  it("executes its fail-closed CLI when invoked through the relative workflow path", () => {
    const result = spawnSync(process.execPath, ["scripts/cicd/preflight-live-user-tests.mjs"], {
      cwd: repoRoot,
      env: {},
      encoding: "utf8",
    });

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("AEX_API_URL, AEX_API_KEY");
  });
});

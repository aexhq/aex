import { pathToFileURL } from "node:url";
import { describe, expect, it, vi } from "vitest";

const mod = await import(pathToFileURL(`${process.cwd()}/scripts/cicd/preflight-live-user-tests.mjs`).href);

const baseEnv = {
  AEX_API_URL: "https://dev-api.aex.dev",
  AEX_API_KEY: "aex_secret_token",
  DEEPSEEK_API_KEY: "deepseek_secret",
  LIVE_USER_TEST_MIN_MAX_CONCURRENT_RUNS: "50",
  LIVE_USER_TEST_PREFLIGHT_RETRY_BASE_MS: "1"
};

function response(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), { status, headers });
}

describe("live user-test preflight", () => {
  it("retries transient HTTP whoami failures without logging secrets", async () => {
    const logs: string[] = [];
    const out: string[] = [];
    const sleepFn = vi.fn(async () => {});
    const fetchImpl = vi
      .fn()
      .mockResolvedValueOnce(
        response(503, { error: "db_resuming" }, { "apigw-requestid": "req-1", "retry-after": "3" })
      )
      .mockResolvedValueOnce(response(200, { limits: { maxConcurrentRuns: 50 } }, { "x-amzn-requestid": "req-2" }));

    const result = await mod.checkLiveUserTestsPreflight({
      env: baseEnv,
      fetchImpl,
      sleepFn,
      err: { write: (s: string) => logs.push(s) },
      out: { write: (s: string) => out.push(s) }
    });

    expect(result).toMatchObject({ status: 200, maxConcurrentRuns: 50, attempt: 2 });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(sleepFn).toHaveBeenCalledWith(3000);
    expect(logs.join("")).toContain("transient HTTP 503");
    expect(logs.join("")).toContain("requestId=req-1");
    expect(`${logs.join("")}${out.join("")}`).not.toContain(baseEnv.AEX_API_KEY);
    expect(`${logs.join("")}${out.join("")}`).not.toContain(baseEnv.DEEPSEEK_API_KEY);
  });

  it("does not retry deterministic auth failures", async () => {
    const fetchImpl = vi.fn().mockResolvedValue(response(401, { error: "unauthorized" }));

    await expect(
      mod.checkLiveUserTestsPreflight({
        env: baseEnv,
        fetchImpl,
        sleepFn: async () => {},
        err: { write: () => {} },
        out: { write: () => {} }
      })
    ).rejects.toThrow(/status=401/);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
  });

  it("fails when required environment values are missing", async () => {
    await expect(
      mod.checkLiveUserTestsPreflight({
        env: { AEX_API_URL: "https://dev-api.aex.dev" },
        fetchImpl: async () => response(200, {}),
        sleepFn: async () => {},
        err: { write: () => {} },
        out: { write: () => {} }
      })
    ).rejects.toThrow("AEX_API_KEY, DEEPSEEK_API_KEY");
  });

  it("requires the workspace concurrency floor", async () => {
    await expect(
      mod.checkLiveUserTestsPreflight({
        env: baseEnv,
        fetchImpl: async () => response(200, { limits: { maxConcurrentRuns: 49 } }),
        sleepFn: async () => {},
        err: { write: () => {} },
        out: { write: () => {} }
      })
    ).rejects.toThrow("maxConcurrentRuns=49");
  });
});

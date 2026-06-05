/**
 * Regression coverage for the terminal-status set used by `waitForRun`.
 *
 * The SDK's terminal check is backed by the shared `TERMINAL_RUN_STATUSES`
 * set, which includes `timed_out`. A prior hardcoded local set omitted it,
 * so a run that resolved to `timed_out` would poll forever. These tests
 * prove `waitForRun` returns promptly for `timed_out` (and that the shared
 * set still recognizes the other terminal statuses).
 */
import { describe, expect, it } from "vitest";
import { TERMINAL_RUN_STATUSES } from "@aexhq/contracts";
import { AgentExecutor } from "../../src/index.js";

function makeGetRunFetch(status: string): { fetch: typeof fetch; calls: number } {
  const state = { calls: 0 };
  const fakeFetch: typeof fetch = async (input) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (/\/runs\/run-abc$/.test(url)) {
      state.calls++;
      return new Response(JSON.stringify({ id: "run-abc", status }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
    }
    throw new Error(`No fake responder for ${url}`);
  };
  return {
    fetch: fakeFetch,
    get calls() {
      return state.calls;
    }
  };
}

describe("AgentExecutor.waitForRun — terminal statuses", () => {
  it("returns immediately for a timed_out run instead of hanging", async () => {
    const f = makeGetRunFetch("timed_out");
    const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: f.fetch });
    const run = await client.waitForRun("run-abc", { intervalMs: 1, timeoutMs: 1_000 });
    expect(run.status).toBe("timed_out");
    // A single GET is enough; no polling loop means no sleep happened.
    expect(f.calls).toBe(1);
  });

  it("treats every shared TERMINAL_RUN_STATUSES value as terminal", async () => {
    for (const status of TERMINAL_RUN_STATUSES) {
      const f = makeGetRunFetch(status);
      const client = new AgentExecutor({ apiToken: "tk", baseUrl: "https://dash.test", fetch: f.fetch });
      const run = await client.waitForRun("run-abc", { intervalMs: 1, timeoutMs: 1_000 });
      expect(run.status).toBe(status);
      expect(f.calls).toBe(1);
    }
  });
});

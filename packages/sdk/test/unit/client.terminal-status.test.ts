/**
 * Regression coverage for the terminal-status set used by `SessionHandle.wait`.
 *
 * The SDK's parked check is backed by the shared `TERMINAL_SESSION_CONTROL_STATUSES`
 * set, which includes `timed_out`. A prior hardcoded local set omitted it,
 * so a session that resolved to `timed_out` would poll forever. These tests
 * prove `session.wait()` returns promptly for `timed_out` (and that the shared
 * set still recognizes the other terminal statuses).
 */
import { describe, expect, it } from "vitest";
import { TERMINAL_SESSION_CONTROL_STATUSES } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

function makeSessionFetch(status: string): { fetch: typeof fetch; calls: number } {
  const state = { calls: 0 };
  const fakeFetch: typeof fetch = async (input) => {
    const url =
      typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    if (/\/sessions\/session-abc$/.test(url)) {
      state.calls++;
      return new Response(JSON.stringify({ id: "session-abc", status }), {
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

describe("SessionHandle.wait — terminal statuses", () => {
  it("returns immediately for deleted and expired sessions", async () => {
    for (const status of ["deleted", "expired"]) {
      const f = makeSessionFetch(status);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f.fetch });
      const session = await client.openSession("session-abc");
      const before = f.calls;
      const run = await session.wait({ intervalMs: 1, timeoutMs: 1_000 });
      expect(run.status).toBe(status);
      expect(f.calls - before).toBe(1);
    }
  });

  it("returns immediately for a timed_out session instead of hanging", async () => {
    const f = makeSessionFetch("timed_out");
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f.fetch });
    const session = await client.openSession("session-abc");
    const before = f.calls; // the openSession rehydrate read
    const run = await session.wait({ intervalMs: 1, timeoutMs: 1_000 });
    expect(run.status).toBe("timed_out");
    // A single status read is enough; no polling loop means no sleep happened.
    expect(f.calls - before).toBe(1);
  });

  it("treats every shared TERMINAL_SESSION_CONTROL_STATUSES value as terminal", async () => {
    for (const status of TERMINAL_SESSION_CONTROL_STATUSES) {
      const f = makeSessionFetch(status);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f.fetch });
      const session = await client.openSession("session-abc");
      const before = f.calls;
      const run = await session.wait({ intervalMs: 1, timeoutMs: 1_000 });
      expect(run.status).toBe(status);
      expect(f.calls - before).toBe(1);
    }
  });
});

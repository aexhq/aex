import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/http.js";
import { operations } from "../src/internal.js";

function clientFor(session: unknown): HttpClient {
  return new HttpClient({
    apiKey: "tok",
    baseUrl: "https://api.test",
    fetch: async () => new Response(JSON.stringify({ session }), {
      status: 200,
      headers: { "content-type": "application/json" }
    })
  });
}

const FAILED = {
  id: "ses_fault",
  status: "error",
  acceptsMessages: true,
  lastRun: {
    sessionId: "ses_fault",
    turnSeq: 1,
    runId: "run_1",
    phase: "error",
    outcome: "failed"
  }
};

describe("Session providerFault normalization", () => {
  it("returns the strict canonical detail only when lastRun failed", async () => {
    const session = await operations.getSession(clientFor({
      ...FAILED,
      providerFault: { provider: "anthropic", kind: "overloaded", status: 529 }
    }), "ses_fault");
    expect(session.providerFault).toEqual({ provider: "anthropic", kind: "overloaded", status: 529 });
  });

  it.each([
    { kind: "rate_limit", status: "429" },
    { kind: "rate_limit", statusCode: 429 },
    { kind: "Rate_Limit" }
  ])("fails closed on malformed present detail: %j", async (providerFault) => {
    await expect(operations.getSession(clientFor({ ...FAILED, providerFault }), "ses_fault"))
      .rejects.toThrow(/invalid providerFault/);
  });

  it.each([
    ["missing", undefined],
    ["running", { ...FAILED.lastRun, phase: "running", outcome: undefined }],
    ["succeeded", { ...FAILED.lastRun, phase: "finished", outcome: "succeeded" }],
    ["cancelled", { ...FAILED.lastRun, phase: "finished", outcome: "cancelled" }]
  ] as const)("rejects provider detail when lastRun is %s", async (_label, lastRun) => {
    const { lastRun: _failedLastRun, ...withoutLastRun } = FAILED;
    await expect(operations.getSession(clientFor({
      ...withoutLastRun,
      ...(lastRun !== undefined ? { lastRun } : {}),
      providerFault: { kind: "rate_limit" }
    }), "ses_fault")).rejects.toThrow(/failed lastRun/);
  });

  it("keeps a field-absent session without lastRun compatible", async () => {
    const { lastRun: _failedLastRun, ...withoutLastRun } = FAILED;
    const session = await operations.getSession(clientFor(withoutLastRun), "ses_fault");
    expect(session.providerFault).toBeUndefined();
    expect(session.lastRun).toBeUndefined();
  });
});

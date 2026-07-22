import { describe, expect, expectTypeOf, it } from "vitest";
import {
  HttpClient,
  SESSION_RUN_PHASES,
  SESSION_TERMINAL_OUTCOMES,
  type SessionRunPhase
} from "../src/index.js";
import { operations } from "../src/internal.js";

const SESSION = { id: "ses_1", status: "running", acceptsMessages: false };

function clientFor(body: unknown): HttpClient {
  return new HttpClient({
    apiKey: "test",
    baseUrl: "https://api.test",
    fetch: async () => new Response(JSON.stringify(body), {
      status: 202,
      headers: { "content-type": "application/json" }
    })
  });
}

async function send(run: Record<string, unknown>) {
  return operations.sendSessionMessage(clientFor({ session: SESSION, run }), "ses_1", {
    input: "continue"
  });
}

function run(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    sessionId: "ses_1",
    runId: "ses_1:turn:2",
    turnSeq: 2,
    phase: "running",
    ...overrides
  };
}

describe("public session run taxonomy ownership", () => {
  it("exports the ordered phase owner and derives SessionRunPhase from it", () => {
    expect(SESSION_RUN_PHASES).toEqual(["queued", "starting", "running", "finished", "error"]);
    expectTypeOf<(typeof SESSION_RUN_PHASES)[number]>().toEqualTypeOf<SessionRunPhase>();
  });

  it.each(SESSION_RUN_PHASES)("normalizes the owner phase %s", async (phase) => {
    await expect(send(run({ phase }))).resolves.toMatchObject({ run: { phase } });
  });

  it.each(SESSION_TERMINAL_OUTCOMES)("normalizes the owner outcome %s", async (outcome) => {
    await expect(send(run({ phase: "finished", outcome }))).resolves.toMatchObject({
      run: { phase: "finished", outcome }
    });
  });

  it.each(["finalizing", "booting", ""])("keeps non-public phase %j fail-closed", async (phase) => {
    await expect(send(run({ phase }))).rejects.toThrow(/run\.phase is invalid/);
  });

  it.each(["preempted", "", null, 1])("keeps unknown/malformed outcome %j fail-closed", async (outcome) => {
    await expect(send(run({ phase: "finished", outcome }))).rejects.toThrow(/run\.outcome is invalid/);
  });
});

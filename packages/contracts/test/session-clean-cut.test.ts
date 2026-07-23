import { describe, expect, it } from "bun:test";
import { HttpClient, type SessionRunPhase } from "../src/index.js";
import { operations } from "../src/internal.js";

function clientReturning(body: unknown): HttpClient {
  return new HttpClient({
    apiKey: "test",
    baseUrl: "https://api.test",
    fetch: async () => new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" }
    })
  });
}

const run = {
  sessionId: "session-1",
  turnSeq: 1,
  runId: "run-1",
  phase: "running",
  eventCursor: 4
};

const session = {
  id: "session-1",
  status: "running",
  acceptsMessages: false
};

describe("canonical session clean cut", () => {
  it("has no public finalizing phase", () => {
    // @ts-expect-error finalization is internal; callers observe RUN_FINISHED/RUN_ERROR.
    const phase: SessionRunPhase = "finalizing";
    expect<string>(phase).toBe("finalizing");
  });

  it.each([
    { ...run, phase: "finalizing" },
    { ...run, executionEndedAt: "2026-07-11T00:00:00.000Z" }
  ])("rejects internal run-finalization fields on session reads", async (currentRun) => {
    await expect(operations.getSession(clientReturning({
      session: { ...session, currentRun }
    }), session.id)).rejects.toThrow();
  });

  it("tolerates additive run metadata while projecting the known run contract", async () => {
    const result = await operations.getSession(clientReturning({
      session: { ...session, currentRun: { ...run, serverTraceId: "trace-1" } }
    }), session.id);

    // expect<unknown>: the golden literal widens phase to string; the deep
    // equality is the assertion.
    expect<unknown>(result.currentRun).toEqual(run);
    expect(result.currentRun).not.toHaveProperty("serverTraceId");
  });

  it("rejects malformed known run fields", async () => {
    await expect(operations.getSession(clientReturning({
      session: { ...session, currentRun: { ...run, turnSeq: 0, serverTraceId: "trace-1" } }
    }), session.id)).rejects.toThrow(/turnSeq/);
  });

  it("exports only canonical operation names", () => {
    const surface = operations as unknown as Record<string, unknown>;
    for (const name of [
      "getSessionRecord",
      "listSessionRecords",
      "listSessionRecordEvents",
      "getCoordinatorTicket",
      "createSessionFileLink",
      "getSecretValue"
    ]) {
      expect(surface[name], `${name} must not survive as a compatibility alias`).toBeUndefined();
    }
    expect(surface.getSession).toBeTypeOf("function");
    expect(surface.listSessions).toBeTypeOf("function");
    expect(surface.listSessionEvents).toBeTypeOf("function");
    expect(surface.getSessionCoordinatorTicket).toBeTypeOf("function");
    expect(surface.sessionFileLink).toBeTypeOf("function");
  });
});

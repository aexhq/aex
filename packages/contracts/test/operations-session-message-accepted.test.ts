import { describe, expect, it } from "vitest";
import { HttpClient } from "../src/index.js";
import { operations } from "../src/internal.js";

const SESSION = { id: "ses_1", status: "running", acceptsMessages: false };
const RUN = {
  sessionId: "ses_1",
  runId: "ses_1:turn:2",
  turnSeq: 2,
  phase: "starting",
  eventCursor: 42
};

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

async function send(body: unknown) {
  return operations.sendSessionMessage(clientFor(body), "ses_1", { input: "continue" });
}

describe("session message accepted wire envelope", () => {
  it("accepts the canonical session/run envelope", async () => {
    await expect(send({ session: SESSION, run: RUN, eventCursor: 42 })).resolves.toEqual({
      session: SESSION,
      run: RUN,
      eventCursor: 42
    });
  });

  it("rejects the removed turn envelope", async () => {
    await expect(send({ session: SESSION, turn: RUN, eventCursor: 42 }))
      .rejects.toThrow(/removed turn field/);
  });

  it("preserves additive top-level metadata without weakening canonical fields", async () => {
    const accepted = await send({ session: SESSION, run: RUN, eventCursor: 42, traceId: "trace_1" });
    expect(accepted).toMatchObject({ session: SESSION, run: RUN, eventCursor: 42 });
    expect((accepted as unknown as Record<string, unknown>).traceId).toBe("trace_1");
  });

  it("requires a run object", async () => {
    await expect(send({ session: SESSION })).rejects.toThrow(/must contain a run object/);
  });

  it.each([
    [{ ...RUN, sessionId: "ses_other" }, /run\.sessionId does not match session\.id/],
    [{ ...RUN, runId: "" }, /run\.runId must be a non-empty string/],
    [{ ...RUN, turnSeq: 0 }, /run\.turnSeq must be a positive safe integer/],
    [{ ...RUN, turnSeq: 1.5 }, /run\.turnSeq must be a positive safe integer/],
    [{ ...RUN, phase: "booting" }, /run\.phase is invalid/]
  ] as const)("rejects an invalid run identity or phase", async (run, message) => {
    await expect(send({ session: SESSION, run, eventCursor: 42 })).rejects.toThrow(message);
  });

  it("requires the response session id to match the requested path", async () => {
    await expect(send({
      session: { ...SESSION, id: "ses_other" },
      run: { ...RUN, sessionId: "ses_other" },
      eventCursor: 42
    })).rejects.toThrow(/session\.id does not match the requested session/);
  });

  it("rejects inconsistent event cursors", async () => {
    await expect(send({ session: SESSION, run: RUN, eventCursor: 43 }))
      .rejects.toThrow(/eventCursor does not match run\.eventCursor/);
  });
});

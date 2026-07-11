/**
 * SessionRunStream single-send invariant.
 *
 * The documented turn pattern is:
 *
 *   const turn = session.messages.send("…");
 *   for await (const event of turn) { … }
 *   await turn.finished();
 *
 * Every consumer of the stream — iteration, `done()`, or both — must share
 * ONE underlying send generator. A fresh generator per consumer re-POSTs the
 * message: a second billable turn when the session has parked, or a 409
 * `session_busy` while the first turn is still in flight (observed live).
 */
import { describe, expect, it } from "vitest";
import type { AexEventView } from "@aexhq/contracts";
import { SessionRunStream } from "../../src/client.js";

type Result = { readonly status: string; readonly text: string };

function makeStream(events: readonly string[], result: Result, counter: { sessions: number }, failAt?: number) {
  return new SessionRunStream(async function* () {
    counter.sessions += 1;
    for (const [i, type] of events.entries()) {
      if (failAt !== undefined && i === failAt) {
        throw new Error("boom");
      }
      yield { type } as unknown as AexEventView;
    }
    return result as never;
  });
}

describe("SessionRunStream", () => {
  it("iterate-then-finished runs the send exactly once and returns the result", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B", "C"], { status: "idle", text: "hi" }, counter);
    const seen: string[] = [];
    for await (const event of stream) {
      seen.push((event as { type: string }).type);
    }
    const result = await stream.finished();
    expect(seen).toEqual(["A", "B", "C"]);
    expect(result).toEqual({ status: "idle", text: "hi" });
    expect(counter.sessions).toBe(1);
  });

  it("done() alone still drains and resolves", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A"], { status: "idle", text: "solo" }, counter);
    const result = await stream.finished();
    expect(result.text).toBe("solo");
    expect(counter.sessions).toBe(1);
  });

  it("done() twice returns the same memoized result without retrying", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A"], { status: "idle", text: "memo" }, counter);
    const first = await stream.finished();
    const second = await stream.finished();
    expect(second).toBe(first);
    expect(counter.sessions).toBe(1);
  });

  it("lets finished() and iteration consume concurrently without stealing events", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B", "C", "D"], { status: "idle", text: "complete" }, counter);
    const done = stream.finished();
    const seen: string[] = [];

    for await (const event of stream) {
      seen.push((event as { type: string }).type);
    }

    await expect(done).resolves.toEqual({ status: "idle", text: "complete" });
    expect(seen).toEqual(["A", "B", "C", "D"]);
    expect(counter.sessions).toBe(1);
  });

  it("breaking out of iteration does not close the turn; done() drains the rest", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B", "C"], { status: "idle", text: "after-break" }, counter);
    for await (const event of stream) {
      void event;
      break; // consumer bails after the first event
    }
    const result = await stream.finished();
    expect(result.text).toBe("after-break");
    expect(counter.sessions).toBe(1);
  });

  it("a second iteration after completion yields nothing instead of re-sending", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B"], { status: "idle", text: "once" }, counter);
    const first: string[] = [];
    for await (const event of stream) {
      first.push((event as { type: string }).type);
    }
    const second: string[] = [];
    for await (const event of stream) {
      second.push((event as { type: string }).type);
    }
    expect(first).toEqual(["A", "B"]);
    expect(second).toEqual([]);
    expect(counter.sessions).toBe(1);
  });

  it("a send failure propagates to the iterator and is replayed by done()", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B"], { status: "idle", text: "n/a" }, counter, 1);
    const seen: string[] = [];
    await expect(async () => {
      for await (const event of stream) {
        seen.push((event as { type: string }).type);
      }
    }).rejects.toThrow("boom");
    await expect(stream.finished()).rejects.toThrow("boom");
    expect(seen).toEqual(["A"]);
    expect(counter.sessions).toBe(1);
  });
});

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
import { describe, expect, it } from "bun:test";
import { asAexEventView } from "@aexhq/contracts";
import { SessionRunStream } from "../../src/client.js";
import type { SessionRunResult } from "../../src/index.js";
import { makeAexEvent, makeRunResult } from "../helpers/builders.js";

function makeStream(
  events: readonly string[],
  result: SessionRunResult,
  counter: { sessions: number },
  failAt?: number
) {
  return new SessionRunStream(async function* () {
    counter.sessions += 1;
    for (const [i, type] of events.entries()) {
      if (failAt !== undefined && i === failAt) {
        throw new Error("boom");
      }
      yield asAexEventView(makeAexEvent(type, { sequence: i + 1 }));
    }
    return result;
  });
}

describe("SessionRunStream", () => {
  it("iterate-then-finished runs the send exactly once and returns the result", async () => {
    const counter = { sessions: 0 };
    const sentinel = makeRunResult({ text: "hi" });
    const stream = makeStream(["A", "B", "C"], sentinel, counter);
    const seen: string[] = [];
    for await (const event of stream) {
      seen.push(event.type);
    }
    const result = await stream.finished();
    expect(seen).toEqual(["A", "B", "C"]);
    expect(result).toBe(sentinel);
    expect(counter.sessions).toBe(1);
  });

  it("done() alone still drains and resolves", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A"], makeRunResult({ text: "solo" }), counter);
    const result = await stream.finished();
    expect(result.text).toBe("solo");
    expect(counter.sessions).toBe(1);
  });

  it("done() twice returns the same memoized result without retrying", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A"], makeRunResult({ text: "memo" }), counter);
    const first = await stream.finished();
    const second = await stream.finished();
    expect(second).toBe(first);
    expect(counter.sessions).toBe(1);
  });

  it("lets finished() and iteration consume concurrently without stealing events", async () => {
    const counter = { sessions: 0 };
    const sentinel = makeRunResult({ text: "complete" });
    const stream = makeStream(["A", "B", "C", "D"], sentinel, counter);
    const done = stream.finished();
    const seen: string[] = [];

    for await (const event of stream) {
      seen.push(event.type);
    }

    await expect(done).resolves.toBe(sentinel);
    expect(seen).toEqual(["A", "B", "C", "D"]);
    expect(counter.sessions).toBe(1);
  });

  it("breaking out of iteration does not close the turn; done() drains the rest", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B", "C"], makeRunResult({ text: "after-break" }), counter);
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
    const stream = makeStream(["A", "B"], makeRunResult({ text: "once" }), counter);
    const first: string[] = [];
    for await (const event of stream) {
      first.push(event.type);
    }
    const second: string[] = [];
    for await (const event of stream) {
      second.push(event.type);
    }
    expect(first).toEqual(["A", "B"]);
    expect(second).toEqual([]);
    expect(counter.sessions).toBe(1);
  });

  it("a send failure propagates to the iterator and is replayed by done()", async () => {
    const counter = { sessions: 0 };
    const stream = makeStream(["A", "B"], makeRunResult({ text: "n/a" }), counter, 1);
    const seen: string[] = [];
    const iterate = (async () => {
      for await (const event of stream) {
        seen.push(event.type);
      }
    })();
    await expect(iterate).rejects.toThrow("boom");
    await expect(stream.finished()).rejects.toThrow("boom");
    expect(seen).toEqual(["A"]);
    expect(counter.sessions).toBe(1);
  });
});

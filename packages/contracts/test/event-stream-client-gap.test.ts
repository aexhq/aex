import { describe, expect, it } from "bun:test";
import { streamCoordinatorEvents, type AexEvent, type AexStreamEvent } from "../src/index.js";
import { FakeWebSocket } from "../src/testing.js";

/**
 * The client half of the coordinator's in-hand delivery contract.
 *
 * The coordinator links every durable frame to the event it continues from
 * (`prevSequence`). Without this check a client that missed a range would snap
 * its cursor over the hole and never ask for it back, turning a recoverable gap
 * into permanent data loss — which is why this file is the deploy gate for the
 * server-side in-hand fan-out, not an optimisation of it.
 */

const evt = (sequence: number, type: AexEvent["type"] = "TEXT_MESSAGE_CONTENT"): AexEvent => ({
  specversion: "1.0",
  id: `r:${sequence}`,
  source: "agent",
  type,
  subject: "r",
  threadId: "r",
  runId: "run_r",
  time: new Date(sequence).toISOString(),
  sequence,
  data: {}
});

function durableSequence(event: AexStreamEvent): number {
  if (event.replayable === false || typeof event.sequence !== "number") {
    throw new Error("expected a durable event");
  }
  return event.sequence;
}

// Drain the macro/microtask queues so the generator advances to its next await.
const flush = async (n = 4): Promise<void> => {
  for (let i = 0; i < n; i++) await new Promise<void>((r) => setTimeout(r, 0));
};

describe("streamCoordinatorEvents — declared-predecessor gap detection", () => {
  /** A durable frame carrying the server's `prevSequence` link, exactly as the wire sends it. */
  const linked = (sequence: number, prevSequence: number, type: AexEvent["type"] = "TEXT_MESSAGE_CONTENT") => ({
    ...evt(sequence, type),
    prevSequence
  });
  const pulls = (ws: FakeWebSocket): number[] =>
    ws.sent
      .map((frame) => {
        try {
          return JSON.parse(frame) as { action?: unknown; from?: unknown };
        } catch {
          return {};
        }
      })
      .filter((body) => body.action === "replay")
      .map((body) => body.from as number);

  it("asks to replay from its own cursor on open", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 7,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const consume = (async () => {
      for await (const event of gen) void event;
    })();

    await flush();
    ws!.open();
    expect(pulls(ws!)).toEqual([7]);

    ws!.message(linked(7, 6, "RUN_FINISHED"));
    await consume;
  });

  it("refuses a frame whose declared predecessor is beyond the cursor and pulls the missing range", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    ws!.open();
    ws!.message(linked(0, -1));
    await flush();
    // Jumps the stream: 1024 continues from 3, which this client has never seen.
    ws!.message(linked(1024, 3));
    await flush();

    expect(received).toEqual([0]);
    // The gapped frame did NOT advance the cursor, and a pull for (0, …] is owed.
    expect(pulls(ws!)).toEqual([0, 1]);

    // The pull is answered with the contiguous range, and the stream continues.
    ws!.message(linked(1, 0));
    ws!.message(linked(3, 1));
    ws!.message(linked(1024, 3, "RUN_FINISHED"));
    await consume;
    expect(received).toEqual([0, 1, 3, 1024]);
  });

  it("does not end the stream on a terminal it cannot chain", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    ws!.open();
    ws!.message(linked(9, 8, "RUN_FINISHED"));
    await flush();

    expect(received).toEqual([]);
    expect(ws!.closed).toBe(false);

    ws!.message(linked(0, -1));
    ws!.message(linked(8, 0));
    ws!.message(linked(9, 8, "RUN_FINISHED"));
    await consume;
    expect(received).toEqual([0, 8, 9]);
  });

  it("accepts a contiguous burst that arrives faster than the consumer drains", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    ws!.open();
    // All four land in one turn, before a single yield advances the cursor. Each
    // links to the one before it, so none of them is a gap.
    ws!.message(linked(0, -1));
    ws!.message(linked(1, 0));
    ws!.message(linked(2, 1));
    ws!.message(linked(3, 2, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([0, 1, 2, 3]);
    expect(pulls(ws!)).toEqual([0]); // the open pull only — no gap was observed
  });

  it("accepts an unlinked frame, so a producer that makes no claim still streams", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    ws!.open();
    ws!.message(evt(5)); // no prevSequence at all
    ws!.message(evt(9, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([5, 9]);
    expect(pulls(ws!)).toEqual([0]);
  });

  it("pulls once per cursor position, and re-arms once the cursor moves", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    ws!.open();
    ws!.message(linked(0, -1));
    await flush();
    ws!.message(linked(50, 40));
    ws!.message(linked(51, 50));
    ws!.message(linked(52, 51));
    await flush();

    // Three unchainable frames, one outstanding pull.
    expect(pulls(ws!)).toEqual([0, 1]);

    ws!.message(linked(40, 0)); // fills the hole → cursor moves
    await flush();
    ws!.message(linked(90, 80)); // a fresh gap at the new position pulls again
    await flush();
    expect(pulls(ws!)).toEqual([0, 1, 41]);

    ws!.message(linked(80, 40));
    ws!.message(linked(90, 80, "RUN_FINISHED"));
    await consume;
    expect(received).toEqual([0, 40, 80, 90]);
  });

  it("resumes from the cursor it actually holds after a gap and a drop", async () => {
    const sockets: FakeWebSocket[] = [];
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => {
        const socket = new FakeWebSocket(url);
        sockets.push(socket);
        return socket;
      },
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 0
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    sockets[0]!.open();
    sockets[0]!.message(linked(0, -1));
    sockets[0]!.message(linked(7, 5)); // refused
    await flush();
    sockets[0]!.close();
    await flush(8);

    // Resume from 1 — the refused frame must not have moved the cursor to 8.
    expect(sockets[1]!.url).toBe("wss://co/sessions/r/subscribe?ticket=tkt&from=1");
    sockets[1]!.message(linked(5, 0));
    sockets[1]!.message(linked(7, 5, "RUN_FINISHED"));
    await consume;
    expect(received).toEqual([0, 5, 7]);
  });
});


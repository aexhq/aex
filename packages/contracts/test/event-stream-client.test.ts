import { describe, expect, it, mock, spyOn } from "bun:test";
import {
  filterStream,
  isFromSource,
  mapStream,
  streamCoordinatorEvents,
  toAGUI,
  type AexEvent,
  type AexLiveEvent,
  type AexStreamEvent
} from "../src/index.js";
import { createFakeTimers, FakeWebSocket } from "../src/testing.js";

const evt = (sequence: number, type: AexEvent["type"] = "TEXT_MESSAGE_CONTENT", source: AexEvent["source"] = "agent"): AexEvent => ({
  specversion: "1.0",
  id: `r:${sequence}`,
  source,
  type,
  subject: "r",
  threadId: "r",
  runId: "run_r",
  time: new Date(sequence).toISOString(),
  sequence,
  data: {}
});

const liveEvt = (liveSequence: number, id = `r:live:${liveSequence}`): AexLiveEvent => ({
  specversion: "1.0",
  id,
  source: "agent",
  type: "TEXT_MESSAGE_CONTENT",
  subject: "r",
  threadId: "r",
  runId: "run_r",
  time: new Date(liveSequence).toISOString(),
  replayable: false,
  liveSequence,
  receivedAt: liveSequence,
  ephemeral: true,
  data: { text: `delta-${liveSequence}`, delta: true }
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

describe("streamCoordinatorEvents — live fanout", () => {
  it("yields events in order and stops on a terminal event", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url))
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    expect(ws!.url).toBe("wss://co/sessions/r/subscribe?ticket=tkt&from=0");
    ws!.message(evt(0));
    ws!.message(evt(1));
    ws!.message(evt(2, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([0, 1, 2]);
    expect(ws!.closed).toBe(true);
  });

  it("orders durable frames across interleaved live slots before advancing the cursor", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url))
    });
    const received: string[] = [];
    const consume = (async () => {
      for await (const event of gen) {
        received.push(event.replayable === false ? `live:${event.liveSequence}` : `durable:${event.sequence}`);
      }
    })();

    await flush();
    ws!.message(evt(10));
    ws!.message(liveEvt(0));
    ws!.message(evt(2));
    ws!.message(evt(11, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual(["durable:2", "live:0", "durable:10", "durable:11"]);
  });

  it("bounds live-id dedup while retaining the most recent reconnect window", async () => {
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
    const received: string[] = [];
    const consume = (async () => {
      for await (const event of gen) {
        if (event.replayable === false) received.push(event.id);
      }
    })();

    await flush();
    for (let index = 0; index < 4_100; index += 1) ws!.message(liveEvt(index, `live-${index}`));
    ws!.message(liveEvt(0, "live-0"));
    ws!.message(liveEvt(4_099, "live-4099"));
    ws!.message(evt(0, "RUN_FINISHED"));
    await consume;

    expect(received).toHaveLength(4_101);
    expect(received.at(-1)).toBe("live-0");
    expect(received.filter((id) => id === "live-4099")).toHaveLength(1);
  });

  it("preserves existing WebSocket URL query parameters", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe?region=us-west",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url))
    });
    const consume = (async () => {
      for await (const event of gen) {
        void event;
        break;
      }
    })();

    await flush();
    expect(ws!.url).toBe("wss://co/sessions/r/subscribe?region=us-west&ticket=tkt&from=0");
    ws!.message(evt(0, "RUN_FINISHED"));
    await consume;
  });

  it("sends a post-open replay request so catch-up does not depend only on the connection stream", async () => {
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
    const consume = (async () => {
      for await (const event of gen) {
        void event;
      }
    })();

    await flush();
    ws!.open();
    expect(ws!.sent).toEqual([JSON.stringify({ action: "replay" })]);
    ws!.message(evt(0, "RUN_FINISHED"));
    await consume;
  });

  it("closes the WebSocket when the caller breaks the iterator early", async () => {
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
      for await (const e of gen) {
        received.push(durableSequence(e));
        if (received.length === 2) break; // early exit mid-stream, no terminal
      }
    })();

    await flush();
    ws!.message(evt(0));
    ws!.message(evt(1));
    ws!.message(evt(2)); // buffered but never consumed — the break wins first
    await consume;

    expect(received).toEqual([0, 1]);
    expect(ws!.closed).toBe(true);
  });
});

describe("streamCoordinatorEvents — reconnect resumes exactly once", () => {
  it("removes the reconnect-delay abort listener after the timer wins", async () => {
    const clock = createFakeTimers();
    const sockets: FakeWebSocket[] = [];
    const controller = new AbortController();
    const add = spyOn(controller.signal, "addEventListener");
    const remove = spyOn(controller.signal, "removeEventListener");
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 10,
      signal: controller.signal,
      fetchTicket: async () => "tkt",
      timers: clock,
      webSocketFactory: (url) => {
        const socket = new FakeWebSocket(url);
        sockets.push(socket);
        return socket;
      }
    });
    const consume = (async () => {
      for await (const event of gen) void event;
    })();

    await clock.advanceAsync(0);
    sockets[0]!.close();
    await clock.advanceAsync(20);
    sockets[1]!.message(evt(0, "RUN_FINISHED"));
    await clock.advanceAsync(0);
    await consume;

    const abortAdds = add.mock.calls.filter(([type]) => type === "abort").length;
    const abortRemoves = remove.mock.calls.filter(([type]) => type === "abort").length;
    expect(abortRemoves).toBe(abortAdds);
  });

  it("reconnects from lastSeq+1 with no gap and no duplicate", async () => {
    const sockets: FakeWebSocket[] = [];
    const fetchTicket = mock(async () => "tkt");
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 0,
      fetchTicket,
      webSocketFactory: (url) => {
        const w = new FakeWebSocket(url);
        sockets.push(w);
        return w;
      }
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    sockets[0]!.message(evt(0));
    sockets[0]!.message(evt(1));
    await flush();
    sockets[0]!.close(); // transport drop after seq 1
    await flush(8); // allow backoff + reconnect

    expect(sockets).toHaveLength(2);
    // Resume strictly after the last seen sequence, with a freshly-minted ticket.
    expect(sockets[1]!.url).toBe("wss://co/sessions/r/subscribe?ticket=tkt&from=2");
    expect(fetchTicket).toHaveBeenCalledTimes(2);
    sockets[1]!.message(evt(2));
    sockets[1]!.message(evt(3, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([0, 1, 2, 3]);
  });

  it("drops a duplicate replay at or below the cursor after reconnect", async () => {
    const sockets: FakeWebSocket[] = [];
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => {
        const w = new FakeWebSocket(url);
        sockets.push(w);
        return w;
      }
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await flush();
    sockets[0]!.message(evt(0));
    sockets[0]!.message(evt(1));
    await flush();
    sockets[0]!.close();
    await flush(8);
    // Coordinator re-sends seq 1 (already delivered) plus fresh ones — the
    // client must drop seq <= cursor.
    sockets[1]!.message(evt(1));
    sockets[1]!.message(evt(2, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([0, 1, 2]);
  });

  it("yields and deduplicates live-only frames without advancing the durable reconnect cursor", async () => {
    const sockets: FakeWebSocket[] = [];
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 7,
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
    const received: AexStreamEvent[] = [];
    const consume = (async () => {
      for await (const event of gen) received.push(event);
    })();

    await flush();
    sockets[0]!.message(evt(7));
    sockets[0]!.message(liveEvt(0));
    sockets[0]!.message(liveEvt(0)); // same stable id: suppress on this connection
    sockets[0]!.message(liveEvt(1));
    await flush();
    sockets[0]!.close();
    await flush(8);

    expect(sockets[1]!.url).toBe("wss://co/sessions/r/subscribe?ticket=tkt&from=8");
    sockets[1]!.message(liveEvt(1)); // suppress across reconnect too
    sockets[1]!.message(evt(8, "RUN_FINISHED"));
    await consume;

    expect(received.map((event) => event.id)).toEqual(["r:7", "r:live:0", "r:live:1", "r:8"]);
    const live = received.filter((event): event is AexLiveEvent => event.replayable === false);
    expect(live.map((event) => event.liveSequence)).toEqual([0, 1]);
    expect(live.every((event) => !("sequence" in event))).toBe(true);
  });
});

describe("streamCoordinatorEvents — half-open watchdog", () => {
  it("treats a silently stalled socket as dead and reconnects from the cursor", async () => {
    const clock = createFakeTimers();
    const sockets: FakeWebSocket[] = [];
    const fetchTicket = mock(async () => "tkt");
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 10,
      idleTimeoutMs: 1000,
      pingIntervalMs: 0, // isolate the watchdog from the ping cadence
      fetchTicket,
      timers: clock,
      webSocketFactory: (url) => {
        const w = new FakeWebSocket(url);
        sockets.push(w);
        return w;
      }
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await clock.advanceAsync(0); // settle fetchTicket + connect
    expect(sockets).toHaveLength(1);
    sockets[0]!.message(evt(0));
    await clock.advanceAsync(0); // deliver seq 0 + re-arm the watchdog

    // No frame at all for the whole window → presumed half-open → reconnect.
    await clock.advanceAsync(1000); // idle watchdog fires, schedules backoff
    await clock.advanceAsync(20); // backoff(10) + fresh ticket + reconnect

    expect(sockets).toHaveLength(2);
    expect(sockets[0]!.closed).toBe(true);
    // Resume strictly after the last delivered sequence, with a fresh ticket.
    expect(sockets[1]!.url).toBe("wss://co/sessions/r/subscribe?ticket=tkt&from=1");
    expect(fetchTicket).toHaveBeenCalledTimes(2);

    sockets[1]!.message(evt(1, "RUN_FINISHED"));
    await clock.advanceAsync(0);
    await consume;
    expect(received).toEqual([0, 1]);
    expect(clock.pendingTimerCount()).toBe(0); // a finished stream leaves no timers armed
  });

  it("pings on open and a pong keeps a quiet run from reconnecting", async () => {
    const clock = createFakeTimers();
    const sockets: FakeWebSocket[] = [];
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 0,
      idleTimeoutMs: 1000,
      pingIntervalMs: 300,
      fetchTicket: async () => "tkt",
      timers: clock,
      webSocketFactory: (url) => {
        const w = new FakeWebSocket(url);
        sockets.push(w);
        return w;
      }
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await clock.advanceAsync(0);
    sockets[0]!.open(); // begin the ping cadence

    // 2.4s of zero events, but each ping is answered with a pong → no reconnect.
    for (let i = 0; i < 8; i++) {
      await clock.advanceAsync(300);
      sockets[0]!.pong();
    }

    expect(sockets).toHaveLength(1); // never tripped the watchdog
    expect(sockets[0]!.sent.filter((s) => s === "aex:ping").length).toBeGreaterThanOrEqual(7);

    sockets[0]!.message(evt(0, "RUN_FINISHED"));
    await clock.advanceAsync(0);
    await consume;
    expect(received).toEqual([0]);
    expect(clock.pendingTimerCount()).toBe(0); // a finished stream leaves no timers armed
  });
});

describe("streamCoordinatorEvents — event-quiet recheck", () => {
  it("forces a reconnect when pongs flow but no event frame arrives (dead fan-out self-heal)", async () => {
    // The half-open watchdog counts ANY frame — including pongs — as liveness. But a
    // pong only proves the SOCKET is alive, not the delivery pipeline behind it: a
    // reaped/wedged server-side subscription keeps answering pings while never
    // delivering another event, and the client hangs forever one frame short of the
    // terminal. The quiet recheck reconnects on an event-frame gap; the replay-on-
    // connect path then reads the events table directly and recovers the terminal.
    const clock = createFakeTimers();
    const sockets: FakeWebSocket[] = [];
    const fetchTicket = mock(async () => "tkt");
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 10,
      idleTimeoutMs: 1000,
      pingIntervalMs: 300,
      eventQuietRecheckMs: 2000,
      fetchTicket,
      timers: clock,
      webSocketFactory: (url) => {
        const w = new FakeWebSocket(url);
        sockets.push(w);
        return w;
      }
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await clock.advanceAsync(0);
    sockets[0]!.open();
    sockets[0]!.message(evt(0));
    await clock.advanceAsync(0);

    // Pongs keep the idle watchdog fed for the whole window — no event frames.
    for (let i = 0; i < 7; i++) {
      await clock.advanceAsync(300);
      sockets[0]!.pong();
    }
    // 2100ms of event silence has passed → the quiet recheck must have fired.
    await clock.advanceAsync(20); // backoff(10) + fresh ticket + reconnect

    expect(sockets).toHaveLength(2);
    expect(sockets[0]!.closed).toBe(true);
    // Resume strictly after the last delivered sequence, with a fresh ticket.
    expect(sockets[1]!.url).toBe("wss://co/sessions/r/subscribe?ticket=tkt&from=1");
    expect(fetchTicket).toHaveBeenCalledTimes(2);

    sockets[1]!.open();
    sockets[1]!.message(evt(1, "RUN_FINISHED"));
    await clock.advanceAsync(0);
    await consume;
    expect(received).toEqual([0, 1]);
  });

  it("an event frame re-arms the quiet recheck — a steadily-streaming run never recycles", async () => {
    const clock = createFakeTimers();
    const sockets: FakeWebSocket[] = [];
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/sessions/r/subscribe",
      from: 0,
      reconnectDelayMs: 0,
      idleTimeoutMs: 0,
      pingIntervalMs: 0,
      eventQuietRecheckMs: 1000,
      fetchTicket: async () => "tkt",
      timers: clock,
      webSocketFactory: (url) => {
        const w = new FakeWebSocket(url);
        sockets.push(w);
        return w;
      }
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(durableSequence(e));
    })();

    await clock.advanceAsync(0);
    sockets[0]!.open();
    // Events every 600ms — each re-arms the 1000ms recheck; no reconnect.
    for (let seq = 0; seq < 4; seq++) {
      sockets[0]!.message(evt(seq));
      await clock.advanceAsync(600);
    }
    expect(sockets).toHaveLength(1);

    sockets[0]!.message(evt(4, "RUN_FINISHED"));
    await clock.advanceAsync(0);
    await consume;
    expect(received).toEqual([0, 1, 2, 3, 4]);
  });
});

describe("client-side filter + projection", () => {
  async function* arr(items: AexEvent[]): AsyncGenerator<AexEvent> {
    for (const i of items) yield i;
  }

  it("filterStream narrows by a guard predicate", async () => {
    const events = [evt(0, "TEXT_MESSAGE_CONTENT", "agent"), evt(1, "CUSTOM", "aex"), evt(2, "TOOL_CALL_START", "agent")];
    const out: number[] = [];
    for await (const e of filterStream(arr(events), (e) => isFromSource(e, "agent"))) out.push(e.sequence);
    expect(out).toEqual([0, 2]);
  });

  it("mapStream projects to strict AG-UI", async () => {
    const events = [
      { ...evt(0, "TEXT_MESSAGE_CONTENT"), data: { text: "hello" } },
      { ...evt(1, "RUN_FINISHED"), data: { outcome: "succeeded" } }
    ];
    const out: string[] = [];
    for await (const a of mapStream(arr(events), toAGUI)) out.push(a.type);
    expect(out).toEqual(["TEXT_MESSAGE_CONTENT", "RUN_FINISHED"]);
  });
});

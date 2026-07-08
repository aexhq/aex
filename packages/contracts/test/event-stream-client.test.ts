import { describe, expect, it, vi } from "vitest";
import {
  filterStream,
  isFromSource,
  isRunSettled,
  mapStream,
  streamCoordinatorEvents,
  toAGUI,
  type AexEvent,
  type WebSocketLike
} from "../src/index.js";

const evt = (sequence: number, type: AexEvent["type"] = "TEXT_MESSAGE_CONTENT", source: AexEvent["source"] = "agent"): AexEvent => ({
  specversion: "1.0",
  id: `r:${sequence}`,
  source,
  type,
  subject: "r",
  time: new Date(sequence).toISOString(),
  sequence,
  data: {}
});

const sessionIdle = (sequence: number): AexEvent => ({
  ...evt(sequence, "CUSTOM", "runtime"),
  data: { name: "aex.session.idle", value: { state: "idle", reason: "complete" } }
});

class FakeWebSocket implements WebSocketLike {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};
  readonly sent: string[] = [];
  closed = false;
  constructor(url: string) {
    this.url = url;
  }
  addEventListener(type: "open" | "message" | "close" | "error", cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }
  send(data: string): void {
    this.sent.push(data);
  }
  close(): void {
    this.closed = true;
    this.#emit("close", {});
  }
  open(): void {
    this.#emit("open", {});
  }
  message(event: AexEvent): void {
    this.#emit("message", { data: JSON.stringify(event) });
  }
  /** A keep-alive pong (or any non-event frame): proves liveness, carries no sequence. */
  pong(data = "aex:pong"): void {
    this.#emit("message", { data });
  }
  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}

// Drain the macro/microtask queues so the generator advances to its next await.
const flush = async (n = 4): Promise<void> => {
  for (let i = 0; i < n; i++) await new Promise<void>((r) => setTimeout(r, 0));
};

describe("streamCoordinatorEvents — live fanout", () => {
  it("yields events in order and stops on a terminal event", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url))
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(e.sequence);
    })();

    await flush();
    expect(ws!.url).toBe("wss://co/runs/r/subscribe?ticket=tkt&from=0");
    ws!.message(evt(0));
    ws!.message(evt(1));
    ws!.message(evt(2, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([0, 1, 2]);
    expect(ws!.closed).toBe(true);
  });

  it("stops on a managed-runtime session-park terminal (aex.session.idle) — F19 no-hang", async () => {
    // A managed one-shot run PARKS (CUSTOM aex.session.idle) instead of emitting
    // RUN_FINISHED. The default terminal predicate must treat that as terminal,
    // else streamEnvelopes() over a finished managed run hangs on the watchdog.
    const idle = sessionIdle(2);
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      webSocketFactory: (url) => (ws = new FakeWebSocket(url))
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(e.sequence);
    })();

    await flush();
    ws!.message(evt(0));
    ws!.message(evt(1));
    ws!.message(idle);
    await consume;

    expect(received).toEqual([0, 1, 2]);
    expect(ws!.closed).toBe(true);
  });

  it("drains replay backfill before yielding a gapped terminal event", async () => {
    vi.useFakeTimers();
    try {
      let ws: FakeWebSocket | undefined;
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        fetchTicket: async () => "tkt",
        webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
        idleTimeoutMs: 0,
        pingIntervalMs: 0,
        eventQuietRecheckMs: 0,
        terminalDrainGraceMs: 1000
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0);
      ws!.message(sessionIdle(3072));
      await vi.advanceTimersByTimeAsync(0);
      expect(received).toEqual([]);

      ws!.message(evt(0));
      ws!.message(evt(1024));
      await vi.advanceTimersByTimeAsync(0);
      expect(received).toEqual([0, 1024]);

      await vi.advanceTimersByTimeAsync(1000);
      await consume;

      expect(received).toEqual([0, 1024, 3072]);
      expect(ws!.closed).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("holds a gapped terminal even when an earlier replay frame is already buffered", async () => {
    vi.useFakeTimers();
    try {
      let ws: FakeWebSocket | undefined;
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        fetchTicket: async () => "tkt",
        webSocketFactory: (url) => (ws = new FakeWebSocket(url)),
        idleTimeoutMs: 0,
        pingIntervalMs: 0,
        eventQuietRecheckMs: 0,
        terminalDrainGraceMs: 1000
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0);
      ws!.message(evt(0));
      ws!.message(sessionIdle(3072));
      await vi.advanceTimersByTimeAsync(0);
      expect(received).toEqual([0]);

      ws!.message(evt(1024));
      await vi.advanceTimersByTimeAsync(0);
      expect(received).toEqual([0, 1024]);

      await vi.advanceTimersByTimeAsync(1000);
      await consume;

      expect(received).toEqual([0, 1024, 3072]);
      expect(ws!.closed).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not drain a terminal once buffered predecessors make it contiguous", async () => {
    vi.useFakeTimers();
    try {
      const sockets: FakeWebSocket[] = [];
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        fetchTicket: async () => "tkt",
        webSocketFactory: (url) => {
          const ws = new FakeWebSocket(url);
          sockets.push(ws);
          return ws;
        },
        idleTimeoutMs: 0,
        pingIntervalMs: 0,
        eventQuietRecheckMs: 0,
        terminalDrainGraceMs: 1000
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0);
      sockets[0]!.message(evt(0));
      sockets[0]!.message(sessionIdle(1));
      await vi.advanceTimersByTimeAsync(0);
      await consume;

      expect(received).toEqual([0, 1]);
      expect(sockets).toHaveLength(1);
      expect(sockets[0]!.closed).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("preserves existing WebSocket URL query parameters", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe?region=us-west",
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
    expect(ws!.url).toBe("wss://co/runs/r/subscribe?region=us-west&ticket=tkt&from=0");
    ws!.message(evt(0, "RUN_FINISHED"));
    await consume;
  });

  it("sends a post-open replay request so catch-up does not depend only on the connection stream", async () => {
    let ws: FakeWebSocket | undefined;
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe",
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
      wsUrl: "wss://co/runs/r/subscribe",
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
        received.push(e.sequence);
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

describe("streamCoordinatorEvents — settle-consistent terminal predicate", () => {
  it("keeps reading past RUN_FINISHED until the aex.run.settled barrier", async () => {
    let ws: FakeWebSocket | undefined;
    const settled: AexEvent = {
      ...evt(4, "CUSTOM", "aex"),
      data: { name: "aex.run.settled", value: { runId: "r", outcome: "succeeded" } }
    };
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe",
      from: 0,
      fetchTicket: async () => "tkt",
      isTerminal: isRunSettled,
      webSocketFactory: (url) => (ws = new FakeWebSocket(url))
    });
    const received: number[] = [];
    const consume = (async () => {
      for await (const e of gen) received.push(e.sequence);
    })();

    await flush();
    ws!.message(evt(0));
    // The AG-UI terminal must NOT end a settle-consistent stream...
    ws!.message(evt(1, "RUN_FINISHED"));
    // ...nor an interleaved lifecycle fact (CUSTOM without the settled name)...
    ws!.message(evt(2, "CUSTOM", "aex"));
    // ...only the post-mirror barrier ends it.
    ws!.message(settled);
    await consume;

    expect(received).toEqual([0, 1, 2, 4]);
    expect(ws!.closed).toBe(true);
  });
});

describe("streamCoordinatorEvents — reconnect resumes exactly once", () => {
  it("reconnects from lastSeq+1 with no gap and no duplicate", async () => {
    const sockets: FakeWebSocket[] = [];
    const fetchTicket = vi.fn(async () => "tkt");
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe",
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
      for await (const e of gen) received.push(e.sequence);
    })();

    await flush();
    sockets[0]!.message(evt(0));
    sockets[0]!.message(evt(1));
    await flush();
    sockets[0]!.close(); // transport drop after seq 1
    await flush(8); // allow backoff + reconnect

    expect(sockets).toHaveLength(2);
    // Resume strictly after the last seen sequence, with a freshly-minted ticket.
    expect(sockets[1]!.url).toBe("wss://co/runs/r/subscribe?ticket=tkt&from=2");
    expect(fetchTicket).toHaveBeenCalledTimes(2);
    sockets[1]!.message(evt(2));
    sockets[1]!.message(evt(3, "RUN_FINISHED"));
    await consume;

    expect(received).toEqual([0, 1, 2, 3]);
  });

  it("drops a duplicate replay at or below the cursor after reconnect", async () => {
    const sockets: FakeWebSocket[] = [];
    const gen = streamCoordinatorEvents({
      wsUrl: "wss://co/runs/r/subscribe",
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
      for await (const e of gen) received.push(e.sequence);
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
});

describe("streamCoordinatorEvents — half-open watchdog", () => {
  it("treats a silently stalled socket as dead and reconnects from the cursor", async () => {
    vi.useFakeTimers();
    try {
      const sockets: FakeWebSocket[] = [];
      const fetchTicket = vi.fn(async () => "tkt");
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        reconnectDelayMs: 10,
        idleTimeoutMs: 1000,
        pingIntervalMs: 0, // isolate the watchdog from the ping cadence
        fetchTicket,
        webSocketFactory: (url) => {
          const w = new FakeWebSocket(url);
          sockets.push(w);
          return w;
        }
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0); // settle fetchTicket + connect
      expect(sockets).toHaveLength(1);
      sockets[0]!.message(evt(0));
      await vi.advanceTimersByTimeAsync(0); // deliver seq 0 + re-arm the watchdog

      // No frame at all for the whole window → presumed half-open → reconnect.
      await vi.advanceTimersByTimeAsync(1000); // idle watchdog fires, schedules backoff
      await vi.advanceTimersByTimeAsync(20); // backoff(10) + fresh ticket + reconnect

      expect(sockets).toHaveLength(2);
      expect(sockets[0]!.closed).toBe(true);
      // Resume strictly after the last delivered sequence, with a fresh ticket.
      expect(sockets[1]!.url).toBe("wss://co/runs/r/subscribe?ticket=tkt&from=1");
      expect(fetchTicket).toHaveBeenCalledTimes(2);

      sockets[1]!.message(evt(1, "RUN_FINISHED"));
      await vi.advanceTimersByTimeAsync(0);
      await consume;
      expect(received).toEqual([0, 1]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("pings on open and a pong keeps a quiet run from reconnecting", async () => {
    vi.useFakeTimers();
    try {
      const sockets: FakeWebSocket[] = [];
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        reconnectDelayMs: 0,
        idleTimeoutMs: 1000,
        pingIntervalMs: 300,
        fetchTicket: async () => "tkt",
        webSocketFactory: (url) => {
          const w = new FakeWebSocket(url);
          sockets.push(w);
          return w;
        }
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0);
      sockets[0]!.open(); // begin the ping cadence

      // 2.4s of zero events, but each ping is answered with a pong → no reconnect.
      for (let i = 0; i < 8; i++) {
        await vi.advanceTimersByTimeAsync(300);
        sockets[0]!.pong();
      }

      expect(sockets).toHaveLength(1); // never tripped the watchdog
      expect(sockets[0]!.sent.filter((s) => s === "aex:ping").length).toBeGreaterThanOrEqual(7);

      sockets[0]!.message(evt(0, "RUN_FINISHED"));
      await vi.advanceTimersByTimeAsync(0);
      await consume;
      expect(received).toEqual([0]);
    } finally {
      vi.useRealTimers();
    }
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
    vi.useFakeTimers();
    try {
      const sockets: FakeWebSocket[] = [];
      const fetchTicket = vi.fn(async () => "tkt");
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        reconnectDelayMs: 10,
        idleTimeoutMs: 1000,
        pingIntervalMs: 300,
        eventQuietRecheckMs: 2000,
        fetchTicket,
        webSocketFactory: (url) => {
          const w = new FakeWebSocket(url);
          sockets.push(w);
          return w;
        }
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0);
      sockets[0]!.open();
      sockets[0]!.message(evt(0));
      await vi.advanceTimersByTimeAsync(0);

      // Pongs keep the idle watchdog fed for the whole window — no event frames.
      for (let i = 0; i < 7; i++) {
        await vi.advanceTimersByTimeAsync(300);
        sockets[0]!.pong();
      }
      // 2100ms of event silence has passed → the quiet recheck must have fired.
      await vi.advanceTimersByTimeAsync(20); // backoff(10) + fresh ticket + reconnect

      expect(sockets).toHaveLength(2);
      expect(sockets[0]!.closed).toBe(true);
      // Resume strictly after the last delivered sequence, with a fresh ticket.
      expect(sockets[1]!.url).toBe("wss://co/runs/r/subscribe?ticket=tkt&from=1");
      expect(fetchTicket).toHaveBeenCalledTimes(2);

      sockets[1]!.open();
      sockets[1]!.message(evt(1, "RUN_FINISHED"));
      await vi.advanceTimersByTimeAsync(0);
      await consume;
      expect(received).toEqual([0, 1]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("an event frame re-arms the quiet recheck — a steadily-streaming run never recycles", async () => {
    vi.useFakeTimers();
    try {
      const sockets: FakeWebSocket[] = [];
      const gen = streamCoordinatorEvents({
        wsUrl: "wss://co/runs/r/subscribe",
        from: 0,
        reconnectDelayMs: 0,
        idleTimeoutMs: 0,
        pingIntervalMs: 0,
        eventQuietRecheckMs: 1000,
        fetchTicket: async () => "tkt",
        webSocketFactory: (url) => {
          const w = new FakeWebSocket(url);
          sockets.push(w);
          return w;
        }
      });
      const received: number[] = [];
      const consume = (async () => {
        for await (const e of gen) received.push(e.sequence);
      })();

      await vi.advanceTimersByTimeAsync(0);
      sockets[0]!.open();
      // Events every 600ms — each re-arms the 1000ms recheck; no reconnect.
      for (let seq = 0; seq < 4; seq++) {
        sockets[0]!.message(evt(seq));
        await vi.advanceTimersByTimeAsync(600);
      }
      expect(sockets).toHaveLength(1);

      sockets[0]!.message(evt(4, "RUN_FINISHED"));
      await vi.advanceTimersByTimeAsync(0);
      await consume;
      expect(received).toEqual([0, 1, 2, 3, 4]);
    } finally {
      vi.useRealTimers();
    }
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
    const events = [evt(0, "TEXT_MESSAGE_CONTENT"), evt(1, "RUN_FINISHED")];
    const out: string[] = [];
    for await (const a of mapStream(arr(events), toAGUI)) out.push(a.type);
    expect(out).toEqual(["TEXT_MESSAGE_CONTENT", "RUN_FINISHED"]);
  });
});

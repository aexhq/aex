import { describe, expect, it } from "vitest";
import {
  filterStream,
  isFromSource,
  mapStream,
  streamCoordinatorEvents,
  toAGUI,
  type AntpathEvent,
  type WebSocketLike
} from "../src/index.js";

const evt = (sequence: number, type: AntpathEvent["type"] = "TEXT_MESSAGE_CONTENT", source: AntpathEvent["source"] = "agent"): AntpathEvent => ({
  specversion: "1.0",
  id: `r:${sequence}`,
  source,
  type,
  subject: "r",
  time: new Date(sequence).toISOString(),
  sequence,
  data: {}
});

class FakeWebSocket implements WebSocketLike {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};
  closed = false;
  constructor(url: string) {
    this.url = url;
  }
  addEventListener(type: "open" | "message" | "close" | "error", cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }
  close(): void {
    this.closed = true;
    this.#emit("close", {});
  }
  message(event: AntpathEvent): void {
    this.#emit("message", { data: JSON.stringify(event) });
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
});

describe("streamCoordinatorEvents — reconnect resumes exactly once", () => {
  it("reconnects from lastSeq+1 with no gap and no duplicate", async () => {
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
    sockets[0]!.close(); // transport drop after seq 1
    await flush(8); // allow backoff + reconnect

    expect(sockets).toHaveLength(2);
    // Resume strictly after the last seen sequence.
    expect(sockets[1]!.url).toContain("from=2");
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

describe("client-side filter + projection", () => {
  async function* arr(items: AntpathEvent[]): AsyncGenerator<AntpathEvent> {
    for (const i of items) yield i;
  }

  it("filterStream narrows by a guard predicate", async () => {
    const events = [evt(0, "TEXT_MESSAGE_CONTENT", "agent"), evt(1, "CUSTOM", "antpath"), evt(2, "TOOL_CALL_START", "agent")];
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

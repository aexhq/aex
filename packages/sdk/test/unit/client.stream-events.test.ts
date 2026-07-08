/**
 * SDK-level coverage for `SessionHandle.streamEvents` (the loose `RunEvent`
 * snapshot poll loop). It polls the coordinator-backed `/events` endpoint,
 * dedupes by event id, and stops once the session parks (or on an abort). The
 * low-latency live envelope stream is covered separately (streamEnvelopes →
 * coordinator WS, shared event-stream-client tests).
 */
import { describe, expect, it } from "vitest";
import { Aex } from "../../src/index.js";
import type { AexEvent, JsonValue } from "@aexhq/contracts";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

function makeFetch(plan: ReadonlyArray<{ match: RegExp; respond: () => Response }>): {
  fetch: typeof fetch;
  calls: string[];
} {
  const calls: string[] = [];
  const fakeFetch: typeof fetch = async (input) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    calls.push(url);
    for (const entry of plan) {
      if (entry.match.test(url)) return entry.respond();
    }
    throw new Error(`No fake responder for ${url}`);
  };
  return { fetch: fakeFetch, calls };
}

function evt(sequence: number, type: AexEvent["type"], data: Record<string, JsonValue> = {}): AexEvent {
  return {
    specversion: "1.0",
    id: `run-abc:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: "run-abc",
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

class FakeWebSocket {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};

  constructor(url: string) {
    this.url = url;
  }

  addEventListener(type: "open" | "message" | "close" | "error", cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }

  close(): void {
    this.#emit("close", {});
  }

  message(event: AexEvent): void {
    this.#emit("message", { data: JSON.stringify(event) });
  }

  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}

const flush = async (n = 4): Promise<void> => {
  for (let i = 0; i < n; i++) await new Promise<void>((resolve) => setTimeout(resolve, 0));
};

const waitFor = async (predicate: () => boolean, timeoutMs = 1500): Promise<void> => {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error(`Timed out waiting ${timeoutMs}ms for predicate`);
    await new Promise<void>((resolve) => setTimeout(resolve, 10));
  }
};

describe("SessionHandle.streamEvents — polling the coordinator-backed /events", () => {
  it("yields events, dedupes by id across polls, and stops when the session parks", async () => {
    let listCount = 0;
    let getCount = 0;
    const { fetch: f, calls } = makeFetch([
      {
        match: /\/events$/,
        respond: () => {
          listCount++;
          const byCall: Record<number, ReadonlyArray<{ id: string; type: string }>> = {
            1: [{ id: "e1", type: "TEXT_MESSAGE_CONTENT" }],
            2: [
              { id: "e1", type: "TEXT_MESSAGE_CONTENT" },
              { id: "e2", type: "TEXT_MESSAGE_CONTENT" }
            ]
          };
          return jsonResponse({ events: byCall[listCount] ?? [] });
        }
      },
      {
        match: /\/sessions\/run-abc$/,
        respond: () => {
          getCount++;
          // getCount 1 = openSession rehydrate; the poll loop reads status on
          // 2 (running) and 3 (succeeded → parked).
          return jsonResponse({ id: "run-abc", status: getCount >= 3 ? "succeeded" : "running" });
        }
      }
    ]);

    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
    const session = await client.openSession("run-abc");
    const events: string[] = [];
    for await (const ev of session.events().stream({ intervalMs: 1 })) {
      events.push(ev.id);
    }
    expect(events).toEqual(["e1", "e2"]);
    // No SSE endpoint is ever touched.
    expect(calls.some((u) => u.endsWith("/events/stream"))).toBe(false);
  });

  it("stops promptly when the signal aborts", async () => {
    const { fetch: f, calls } = makeFetch([
      { match: /\/events$/, respond: () => jsonResponse({ events: [] }) },
      { match: /\/sessions\/run-abc$/, respond: () => jsonResponse({ id: "run-abc", status: "running" }) }
    ]);
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
    const session = await client.openSession("run-abc");
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 5);
    const events: string[] = [];
    for await (const ev of session.events().stream({ signal: controller.signal, intervalMs: 1 })) {
      events.push(ev.id);
    }
    expect(events).toEqual([]);
    // The loop was provably live (polling started) before the abort stopped it.
    expect(calls.length).toBeGreaterThan(0);
  });
});

describe("SessionEvents.streamEnvelopes — coordinator WebSocket terminal handling", () => {
  it("default stream ends naturally on a clean managed-session terminal", async () => {
    const sockets: FakeWebSocket[] = [];
    const originalWebSocket = globalThis.WebSocket;
    const fakeConstructor = class extends FakeWebSocket {
      constructor(url: string) {
        super(url);
        sockets.push(this);
      }
    };
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = fakeConstructor;

    try {
      const { fetch: f } = makeFetch([
        { match: /\/sessions\/run-abc\/events\/ticket$/, respond: () => jsonResponse({ wsUrl: "wss://events.test/run-abc", ticket: "ticket" }) },
        { match: /\/sessions\/run-abc$/, respond: () => jsonResponse({ id: "run-abc", status: "succeeded" }) }
      ]);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
      const session = await client.openSession("run-abc");
      const iterator = session.events().streamEnvelopes({ from: 0 })[Symbol.asyncIterator]();

      const first = iterator.next();
      await flush();
      sockets[0]!.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "hello", messageId: "m1" }));
      await expect(first).resolves.toMatchObject({ done: false, value: { type: "TEXT_MESSAGE_CONTENT" } });

      const second = iterator.next();
      await flush();
      sockets[0]!.message(evt(2, "CUSTOM", { name: "aex.session.succeeded", value: { turnSeq: 1, reason: "completed" } }));
      await expect(second).resolves.toMatchObject({ done: false, value: { type: "CUSTOM" } });

      await expect(iterator.next()).resolves.toMatchObject({ done: true });
    } finally {
      (globalThis as unknown as { WebSocket: unknown }).WebSocket = originalWebSocket;
    }
  });

  it("forwards replay self-heal timing options to the coordinator stream", async () => {
    const sockets: FakeWebSocket[] = [];
    const originalWebSocket = globalThis.WebSocket;
    const fakeConstructor = class extends FakeWebSocket {
      constructor(url: string) {
        super(url);
        sockets.push(this);
      }
    };
    (globalThis as unknown as { WebSocket: unknown }).WebSocket = fakeConstructor;

    try {
      const { fetch: f } = makeFetch([
        { match: /\/sessions\/run-abc\/events\/ticket$/, respond: () => jsonResponse({ wsUrl: "wss://events.test/run-abc", ticket: "ticket" }) },
        { match: /\/sessions\/run-abc$/, respond: () => jsonResponse({ id: "run-abc", status: "succeeded" }) }
      ]);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
      const session = await client.openSession("run-abc");
      const controller = new AbortController();
      const consume = (async () => {
        for await (const event of session.events().streamEnvelopes({
          from: 0,
          signal: controller.signal,
          idleTimeoutMs: 0,
          pingIntervalMs: 0,
          eventQuietRecheckMs: 20,
          terminalDrainGraceMs: 20
        })) {
          void event;
          // This test only needs the reconnect side effect.
        }
      })();

      await flush();
      expect(sockets).toHaveLength(1);
      await waitFor(() => sockets.length >= 2);
      controller.abort();
      await consume;

      expect(new URL(sockets[0]!.url).searchParams.get("from")).toBe("0");
      expect(new URL(sockets[1]!.url).searchParams.get("from")).toBe("0");
    } finally {
      (globalThis as unknown as { WebSocket: unknown }).WebSocket = originalWebSocket;
    }
  });
});

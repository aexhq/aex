/**
 * SDK-level coverage for `SessionHandle.streamEvents` (the loose `TurnEvent`
 * snapshot poll loop). It polls the coordinator-backed `/events` endpoint,
 * dedupes by event id, and stops at a RUN terminal (or on an abort). The
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
    id: `session-abc:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: "session-abc",
    threadId: "session-abc",
    runId: "run-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

function childEvt(sequence: number, type: AexEvent["type"], data: Record<string, JsonValue> = {}): AexEvent {
  return {
    ...evt(sequence, type, data),
    id: `child-abc:${sequence}`,
    subject: "child-abc",
    threadId: "child-abc",
    runId: sequence < 10 ? "child-old" : "child-current"
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
  it("yields events, dedupes by id across polls, and stops at RUN_FINISHED", async () => {
    let listCount = 0;
    const { fetch: f, calls } = makeFetch([
      {
        match: /\/events$/,
        respond: () => {
          listCount++;
          const byCall: Record<number, readonly AexEvent[]> = {
            1: [evt(1, "TEXT_MESSAGE_CONTENT", { text: "one", messageId: "m1" })],
            2: [
              evt(1, "TEXT_MESSAGE_CONTENT", { text: "one", messageId: "m1" }),
              evt(2, "TEXT_MESSAGE_CONTENT", { text: "two", messageId: "m1" }),
              evt(3, "RUN_FINISHED", { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } })
            ]
          };
          return jsonResponse({ events: byCall[listCount] ?? [] });
        }
      },
      {
        match: /\/sessions\/session-abc$/,
        respond: () => {
          return jsonResponse({ session: { id: "session-abc", status: "running", acceptsMessages: false } });
        }
      }
    ]);

    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
    const session = await client.sessions.open("session-abc");
    const events: string[] = [];
    for await (const ev of session.events.stream({ intervalMs: 1 })) {
      events.push(ev.id);
    }
    expect(events).toEqual(["session-abc:1", "session-abc:2", "session-abc:3"]);
    // No SSE endpoint is ever touched.
    expect(calls.some((u) => u.endsWith("/events/stream"))).toBe(false);
  });

  it("stops promptly when the signal aborts", async () => {
    const { fetch: f, calls } = makeFetch([
      { match: /\/events$/, respond: () => jsonResponse({ events: [] }) },
      { match: /\/sessions\/session-abc$/, respond: () => jsonResponse({ session: { id: "session-abc", status: "running", acceptsMessages: false } }) }
    ]);
    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
    const session = await client.sessions.open("session-abc");
    const controller = new AbortController();
    setTimeout(() => controller.abort(), 5);
    const events: string[] = [];
    for await (const ev of session.events.stream({ signal: controller.signal, intervalMs: 1 })) {
      events.push(ev.id);
    }
    expect(events).toEqual([]);
    // The loop was provably live (polling started) before the abort stopped it.
    expect(calls.length).toBeGreaterThan(0);
  });

  it("applies from before yield and terminal detection", async () => {
    let listCount = 0;
    const oldTerminal = evt(4, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-old" }
    });
    const { fetch: f } = makeFetch([
      {
        match: /\/events$/,
        respond: () => {
          listCount += 1;
          return jsonResponse({
            events: listCount === 1
              ? [evt(3, "TEXT_MESSAGE_CONTENT", { text: "old", messageId: "old" }), oldTerminal]
              : [
                  evt(3, "TEXT_MESSAGE_CONTENT", { text: "old", messageId: "old" }),
                  oldTerminal,
                  evt(10, "TEXT_MESSAGE_CONTENT", { text: "current", messageId: "current" }),
                  evt(11, "RUN_FINISHED", {
                    outcome: "succeeded",
                    costUsd: 0,
                    providerUsage: [],
                    checkpoint: { checkpointId: "cp-current" }
                  })
                ]
          });
        }
      },
      {
        match: /\/sessions\/session-abc$/,
        respond: () => jsonResponse({ session: { id: "session-abc", status: "running", acceptsMessages: false } })
      }
    ]);

    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
    const session = await client.sessions.open("session-abc");
    const sequences: number[] = [];
    for await (const event of session.events.stream({ from: 10, intervalMs: 1 })) {
      sequences.push(event.sequence);
    }

    expect(listCount).toBe(2);
    expect(sequences).toEqual([10, 11]);
  });

  it("does not let an older run terminal end a current-run stream", async () => {
    let listCount = 0;
    const oldTerminal = { ...evt(4, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-old" }
    }), runId: "run-old" };
    const currentText = { ...evt(10, "TEXT_MESSAGE_CONTENT", { text: "current", messageId: "current" }), runId: "run-current" };
    const currentTerminal = { ...evt(11, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-current" }
    }), runId: "run-current" };
    const { fetch: f } = makeFetch([
      {
        match: /\/events$/,
        respond: () => jsonResponse({ events: ++listCount === 1 ? [oldTerminal] : [oldTerminal, currentText, currentTerminal] })
      },
      {
        match: /\/sessions\/session-abc$/,
        respond: () => jsonResponse({
          session: {
            id: "session-abc",
            status: "running",
            acceptsMessages: false,
            currentRun: { sessionId: "session-abc", runId: "run-current", turnSeq: 2, phase: "running" }
          }
        })
      }
    ]);

    const session = await new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f }).sessions.open("session-abc");
    const runIds: string[] = [];
    for await (const event of session.events.stream({ intervalMs: 1 })) runIds.push(event.runId);

    expect(listCount).toBe(2);
    expect(runIds).toEqual(["run-old", "run-current", "run-current"]);
  });

  it("returns after one snapshot when an idle run terminal is before from", async () => {
    let listCount = 0;
    const terminal = evt(4, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-last" }
    });
    const { fetch: f } = makeFetch([
      { match: /\/events$/, respond: () => { listCount += 1; return jsonResponse({ events: [terminal] }); } },
      {
        match: /\/sessions\/session-abc$/,
        respond: () => jsonResponse({
          session: {
            id: "session-abc",
            status: "idle",
            acceptsMessages: true,
            lastRun: { sessionId: "session-abc", runId: "run-1", turnSeq: 1, phase: "finished", outcome: "succeeded" }
          }
        })
      }
    ]);
    const session = await new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f }).sessions.open("session-abc");
    const events: AexEvent[] = [];
    for await (const event of session.events.stream({ from: 5, intervalMs: 1 })) events.push(event);
    expect(events).toEqual([]);
    expect(listCount).toBe(1);
  });

  it("applies the same from boundary to read-only child polling", async () => {
    let listCount = 0;
    const oldTerminal = childEvt(4, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-old" }
    });
    const { fetch: f } = makeFetch([
      {
        match: /\/sessions\/session-abc\/children$/,
        respond: () => jsonResponse({
          children: [{
            id: "child-abc",
            parentSessionId: "session-abc",
            status: "running",
            createdAt: "2026-07-11T00:00:00.000Z",
            updatedAt: "2026-07-11T00:01:00.000Z"
          }]
        })
      },
      {
        match: /\/sessions\/child-abc\/events$/,
        respond: () => {
          listCount += 1;
          return jsonResponse({
            events: listCount === 1
              ? [childEvt(3, "TEXT_MESSAGE_CONTENT", { text: "old", messageId: "old" }), oldTerminal]
              : [
                  childEvt(3, "TEXT_MESSAGE_CONTENT", { text: "old", messageId: "old" }),
                  oldTerminal,
                  childEvt(10, "TEXT_MESSAGE_CONTENT", { text: "current", messageId: "current" }),
                  childEvt(11, "RUN_ERROR", {
                    outcome: "failed",
                    failureClass: "provider_permanent",
                    failureMessage: "current failed",
                    costUsd: 0,
                    providerUsage: []
                  })
                ]
          });
        }
      },
      {
        match: /\/sessions\/session-abc$/,
        respond: () => jsonResponse({ session: { id: "session-abc", status: "running", acceptsMessages: false } })
      }
    ]);

    const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
    const parent = await client.sessions.open("session-abc");
    const child = (await parent.children())[0]!;
    const sequences: number[] = [];
    for await (const event of child.events.stream({ from: 10, intervalMs: 1 })) {
      sequences.push(event.sequence);
    }

    expect(listCount).toBe(2);
    expect(sequences).toEqual([10, 11]);
  });

  it("does not let a child's last-run terminal end its currently progressing stream", async () => {
    let listCount = 0;
    const oldTerminal = { ...childEvt(4, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-old" }
    }), runId: "child-old" };
    const currentTerminal = { ...childEvt(11, "RUN_FINISHED", {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-current" }
    }), runId: "child-current" };
    const { fetch: f } = makeFetch([
      {
        match: /\/sessions\/session-abc\/children$/,
        respond: () => jsonResponse({
          children: [{
            id: "child-abc",
            parentSessionId: "session-abc",
            status: "running",
            createdAt: "2026-07-11T00:00:00.000Z",
            updatedAt: "2026-07-11T00:01:00.000Z",
            lastRun: { sessionId: "child-abc", runId: "child-old", turnSeq: 1, phase: "finished", outcome: "succeeded" }
          }]
        })
      },
      {
        match: /\/sessions\/child-abc\/events$/,
        respond: () => jsonResponse({ events: ++listCount === 1 ? [oldTerminal] : [oldTerminal, currentTerminal] })
      },
      {
        match: /\/sessions\/session-abc$/,
        respond: () => jsonResponse({ session: { id: "session-abc", status: "running", acceptsMessages: false } })
      }
    ]);
    const parent = await new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f }).sessions.open("session-abc");
    const child = (await parent.children())[0]!;
    const runIds: string[] = [];
    for await (const event of child.events.stream({ intervalMs: 1 })) runIds.push(event.runId);
    expect(listCount).toBe(2);
    expect(runIds).toEqual(["child-old", "child-current"]);
  });

  it.each([-1, 1.5, Number.NaN, Number.POSITIVE_INFINITY, Number.MAX_SAFE_INTEGER + 1])(
    "rejects an invalid polling cursor %s",
    async (from) => {
      const { fetch: f, calls } = makeFetch([
        { match: /\/events$/, respond: () => jsonResponse({ events: [] }) },
        {
          match: /\/sessions\/session-abc$/,
          respond: () => jsonResponse({ session: { id: "session-abc", status: "running", acceptsMessages: false } })
        }
      ]);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
      const session = await client.sessions.open("session-abc");
      const iterator = session.events.stream({ from })[Symbol.asyncIterator]();

      await expect(iterator.next()).rejects.toThrow(/from must be a non-negative safe integer/);
      expect(calls.filter((url) => url.endsWith("/events"))).toEqual([]);
    }
  );
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
        { match: /\/sessions\/session-abc\/events\/ticket$/, respond: () => jsonResponse({ wsUrl: "wss://events.test/session-abc", ticket: "ticket" }) },
        { match: /\/sessions\/session-abc$/, respond: () => jsonResponse({ session: { id: "session-abc", status: "idle", acceptsMessages: true } }) }
      ]);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
      const session = await client.sessions.open("session-abc");
      const iterator = session.events.streamEnvelopes({ from: 0 })[Symbol.asyncIterator]();

      const first = iterator.next();
      await flush();
      sockets[0]!.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "hello", messageId: "m1" }));
      await expect(first).resolves.toMatchObject({ done: false, value: { type: "TEXT_MESSAGE_CONTENT" } });

      const second = iterator.next();
      await flush();
      sockets[0]!.message(evt(2, "RUN_FINISHED", { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } }));
      await expect(second).resolves.toMatchObject({ done: false, value: { type: "RUN_FINISHED" } });

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
        { match: /\/sessions\/session-abc\/events\/ticket$/, respond: () => jsonResponse({ wsUrl: "wss://events.test/session-abc", ticket: "ticket" }) },
        { match: /\/sessions\/session-abc$/, respond: () => jsonResponse({ session: { id: "session-abc", status: "idle", acceptsMessages: true } }) }
      ]);
      const client = new Aex({ apiKey: "tk", baseUrl: "https://dash.test", fetch: f });
      const session = await client.sessions.open("session-abc");
      const controller = new AbortController();
      const consume = (async () => {
        for await (const event of session.events.streamEnvelopes({
          from: 0,
          signal: controller.signal,
          idleTimeoutMs: 0,
          pingIntervalMs: 0,
          eventQuietRecheckMs: 20
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

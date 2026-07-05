/**
 * Follow-up `send()` after a turn parks idle must not intermittently 409.
 *
 * A turn stream ends on the idle park EVENT, but the session RECORD can lag at
 * `running` for a short window before the platform commits it — so an immediate
 * follow-up `send()` POST is rejected 409 `session_busy` even though, from the
 * caller's view, the previous turn already parked. The SDK reconciles this by
 * polling the record until it leaves `running`, then retrying the SAME idempotent
 * POST. A session that stays genuinely busy past the bound surfaces a clear,
 * bounded error instead of hanging or masking a real in-flight turn.
 */
import { describe, expect, it, vi } from "vitest";
import { AexApiError, type AexEvent, type WebSocketLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";

interface HarnessState {
  sessionStatus: string;
  turnSeq: number;
  /** How many of the next `messages` POSTs answer 409 `session_busy`. */
  busyRemaining: number;
  /** The `status` the 409 body reports as the session's CURRENT status. */
  busyStatus: string;
  /** Realistic settle: a `getSession` read catches the record up from running. */
  flipRunningToIdleOnRead: boolean;
}

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function idleEvent(sequence: number): AexEvent {
  return {
    specversion: "1.0",
    id: `sess_1:${sequence}`,
    source: "runtime",
    type: "CUSTOM",
    subject: "sess_1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { name: "aex.session.idle", value: { sessionId: "sess_1" } }
  };
}

function textEvent(sequence: number): AexEvent {
  return {
    specversion: "1.0",
    id: `sess_1:${sequence}`,
    source: "agent",
    type: "TEXT_MESSAGE_CONTENT",
    subject: "sess_1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { text: "hello" }
  };
}

/** A WebSocket that auto-parks the turn: opens, streams one text event, idles. */
class AutoIdleWebSocket implements WebSocketLike {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};

  constructor(url: string) {
    this.url = url;
    queueMicrotask(() => this.#emit("open", {}));
    setTimeout(() => {
      this.#emit("message", { data: JSON.stringify(textEvent(4096)) });
      this.#emit("message", { data: JSON.stringify(idleEvent(4097)) });
    }, 0);
  }

  addEventListener(type: "open" | "message" | "close" | "error", cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }

  close(): void {
    this.#emit("close", {});
  }

  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}

function makeHarness(overrides: Partial<HarnessState> = {}): {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
  readonly sockets: AutoIdleWebSocket[];
  readonly webSocketFactory: (url: string) => AutoIdleWebSocket;
  readonly state: HarnessState;
} {
  const state: HarnessState = {
    sessionStatus: "idle",
    turnSeq: 0,
    busyRemaining: 0,
    busyStatus: "running",
    flipRunningToIdleOnRead: true,
    ...overrides
  };
  const calls: CapturedRequest[] = [];
  const sockets: AutoIdleWebSocket[] = [];

  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    calls.push({ url, method });

    if (url.endsWith("/api/sessions") && method === "POST") {
      return json({ session: { id: "sess_1", status: "idle", turnSeq: 0 } }, 201);
    }
    if (url.endsWith("/api/sessions/sess_1/messages")) {
      if (state.busyRemaining > 0) {
        state.busyRemaining -= 1;
        return json({ error: "session_busy", status: state.busyStatus }, 409);
      }
      state.turnSeq += 1;
      state.sessionStatus = "running";
      return json({
        session: { id: "sess_1", status: "running", turnSeq: state.turnSeq },
        turn: { sessionId: "sess_1", turnSeq: state.turnSeq },
        eventCursor: 4096
      });
    }
    if (url.endsWith("/api/sessions/sess_1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/sess_1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.endsWith("/api/sessions/sess_1/outputs")) {
      return json({ outputs: [] });
    }
    if (url.endsWith("/api/sessions/sess_1")) {
      if (state.flipRunningToIdleOnRead && state.sessionStatus === "running") state.sessionStatus = "idle";
      return json({ session: { id: "sess_1", status: state.sessionStatus, turnSeq: state.turnSeq } });
    }
    return json({});
  };

  const client = new Aex({ apiKey: "tkn", baseUrl: "https://api.example.test", fetch });
  const webSocketFactory = (url: string): AutoIdleWebSocket => {
    const ws = new AutoIdleWebSocket(url);
    sockets.push(ws);
    return ws;
  };
  return { client, calls, sockets, webSocketFactory, state };
}

const messagePosts = (calls: readonly CapturedRequest[]): number =>
  calls.filter((c) => c.method === "POST" && c.url.endsWith("/api/sessions/sess_1/messages")).length;

const sessionReads = (calls: readonly CapturedRequest[]): number =>
  calls.filter((c) => c.method === "GET" && c.url.endsWith("/api/sessions/sess_1")).length;

describe("session idle -> send reconcile", () => {
  it("transparently recovers when a follow-up send 409s session_busy from the previous turn's settle lag", async () => {
    const { client, calls, state, webSocketFactory } = makeHarness();
    const session = await client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });

    // First turn parks idle.
    const first = await session.send("hello", { webSocketFactory }).done();
    expect(first.status).toBe("idle");

    // The server is still catching up from that park: the very next send's POST
    // is rejected once with `session_busy (status: running)`.
    state.busyRemaining = 1;
    const readsBefore = sessionReads(calls);

    const second = await session.send("again", { webSocketFactory }).done();

    expect(second.status).toBe("idle");
    // The 2nd turn's POST was attempted twice: the 409, then the reconciled retry.
    expect(messagePosts(calls)).toBe(3); // turn 1 (1) + turn 2 (409 + retry = 2)
    // The retry only fired after we read the record back to a non-running state.
    expect(sessionReads(calls)).toBeGreaterThan(readsBefore);
  });

  it("surfaces a clear, bounded error when the session stays genuinely busy", async () => {
    const { client, state } = makeHarness({
      busyRemaining: Number.POSITIVE_INFINITY,
      flipRunningToIdleOnRead: false,
      sessionStatus: "running"
    });
    const session = await client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });

    vi.useFakeTimers();
    try {
      const pending = session.send("busy").done();
      const assertion = expect(pending).rejects.toMatchObject({
        code: "RUN_STATE_ERROR",
        message: expect.stringMatching(/still running .* after the previous turn parked/)
      });
      // Advance past the reconcile deadline (30s) so the bounded loop gives up.
      await vi.advanceTimersByTimeAsync(31_000);
      await assertion;
    } finally {
      vi.useRealTimers();
    }
    // Never fell into the streaming path: it stayed a POST-time reconcile.
    expect(state.busyRemaining).toBe(Number.POSITIVE_INFINITY);
  });

  it("passes a non-running session_busy straight through (a real, non-transient rejection)", async () => {
    const { client, calls } = makeHarness({ busyRemaining: 1, busyStatus: "suspended" });
    const session = await client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });

    const err = await session
      .send("to a suspending session")
      .done()
      .then(() => undefined)
      .catch((e: unknown) => e);

    expect(err).toBeInstanceOf(AexApiError);
    expect((err as AexApiError).status).toBe(409);
    expect((err as AexApiError).message).toContain("session_busy");
    // Surfaced immediately: exactly one POST, no reconcile polling.
    expect(messagePosts(calls)).toBe(1);
    expect(sessionReads(calls)).toBe(0);
  });
});

import { describe, expect, it } from "vitest";
import { Aex, type SessionResult } from "../../src/index.js";
import type { AexEvent, JsonValue, WebSocketLike } from "@aexhq/contracts";

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function evt(sequence: number, type: AexEvent["type"], data: Record<string, JsonValue> = {}): AexEvent {
  return {
    specversion: "1.0",
    id: `session-1:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: "session-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

class FakeWebSocket implements WebSocketLike {
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

/** A settled session record (parked + settle-stamped costUsd) that GET returns. */
function settledSession(overrides: Record<string, unknown>): Record<string, unknown> {
  return { id: "session-1", turnSeq: 1, costUsd: 0, ...overrides };
}

function runClient(session: Record<string, unknown>): {
  readonly client: Aex;
  readonly urls: string[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
} {
  const urls: string[] = [];
  const sockets: FakeWebSocket[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString();
    urls.push(`${method} ${url}`);
    if (url.endsWith("/api/sessions/session-1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/session-1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.endsWith("/api/sessions/session-1/outputs")) {
      return json({ outputs: [{ id: "o1", filename: "report.txt" }] });
    }
    if (url.endsWith("/api/sessions/session-1/messages")) {
      return json({
        session: { id: "session-1", status: "running", turnSeq: 1 },
        turn: { sessionId: "session-1", turnSeq: 1 },
        eventCursor: 1024
      });
    }
    if (url.endsWith("/api/sessions/session-1")) {
      return json({ session });
    }
    if (url.endsWith("/api/sessions")) {
      return json({ session: { id: "session-1", status: "idle", turnSeq: 0 } });
    }
    return json({});
  };
  const factory = (url: string): FakeWebSocket => {
    const ws = new FakeWebSocket(url);
    sockets.push(ws);
    return ws;
  };
  return { client: new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch }), urls, sockets, webSocketFactory: factory };
}

/** The CUSTOM `aex.session.<name>` terminal event ending the turn. */
function terminal(name: string): AexEvent {
  return evt(1026, "CUSTOM", { name, value: { turnSeq: 1 } });
}

async function collectRun(
  session: Record<string, unknown>,
  terminalEvent: AexEvent = terminal("aex.session.idle")
): Promise<{ readonly result: SessionResult; readonly urls: readonly string[] }> {
  const { client, urls, sockets, webSocketFactory } = runClient(session);
  const promise = client.start(
    {
      model: "claude-haiku-4-5",
      message: "say hello world",
      apiKeys: { anthropic: "sk-ant" }
    },
    { webSocketFactory }
  );

  await flush();
  sockets[0]!.message(evt(1024, "TEXT_MESSAGE_CONTENT", { text: "hello ", messageId: "m1" }));
  sockets[0]!.message(evt(1025, "TEXT_MESSAGE_CONTENT", { text: "world", messageId: "m1" }));
  sockets[0]!.message(terminalEvent);

  return { result: await promise, urls };
}

describe("Aex.start -> unified settled SessionResult", () => {
  it("a clean one-shot reports the SUCCEEDED outcome with cost + usage always present", async () => {
    const { result, urls } = await collectRun(
      settledSession({
        status: "idle",
        costUsd: 0.0123,
        costTelemetry: { providerUsage: [{ inputTokens: 10, outputTokens: 5, totalTokens: 15 }] }
      })
    );

    expect(result.sessionId).toBe("session-1");
    expect(result.sessionId).toBe("session-1");
    expect(result.ok).toBe(true);
    // The RESULT status is the OUTCOME (never a bare `idle`)...
    expect(result.status).toBe("succeeded");
    // ...while the session RECORD keeps its resumable lifecycle status.
    expect(result.session?.status).toBe("idle");
    expect(result.text).toBe("hello world");
    expect(result.events.map((e) => e.type)).toEqual(["TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_CONTENT", "CUSTOM"]);
    // cost + usage are NON-optional and always present at a settled read.
    expect(result.costUsd).toBe(0.0123);
    expect(result.usage).toEqual({ inputTokens: 10, outputTokens: 5, totalTokens: 15 });
    expect(result.error).toBeUndefined();
    expect(urls.some((url) => url.includes("/api/sessions"))).toBe(false);
  });

  it("derives usage from costTelemetry.providerUsage (retiring the session.usage path)", async () => {
    // A record carrying only a legacy top-level `usage` (no costTelemetry) yields
    // an empty usage — the SDK reads token counts from provider usage only.
    const { result } = await collectRun(
      settledSession({ status: "idle", costUsd: 0.0004, usage: { inputTokens: 99 } })
    );
    expect(result.usage).toEqual({});
  });

  it("a $0 / turnBilled===0 settle resolves as settled with costUsd:0 (no hang, not undefined)", async () => {
    const { result } = await collectRun(settledSession({ status: "idle", costUsd: 0 }));
    expect(result.ok).toBe(true);
    expect(result.costUsd).toBe(0);
    expect(result.usage).toEqual({});
  });

  it("a CANCELLED turn reports outcome cancelled + ok:false", async () => {
    const { result } = await collectRun(
      settledSession({ status: "cancelled", costUsd: 0.01, lastTurnOutcome: "cancelled" }),
      terminal("aex.session.cancelled")
    );
    expect(result.status).toBe("cancelled");
    expect(result.ok).toBe(false);
  });

  it("a wall-clock TIMEOUT reports outcome timed_out + ok:false", async () => {
    const { result } = await collectRun(
      settledSession({ status: "timed_out", costUsd: 0.01, lastTurnOutcome: "timed_out" }),
      terminal("aex.session.timed_out")
    );
    expect(result.status).toBe("timed_out");
    expect(result.ok).toBe(false);
  });

  it("populates error from the terminal TURN_ERROR event (not just the lagged record)", async () => {
    // The record has NO errorMessage yet; the TURN_ERROR event carries the
    // immediate authoritative failure text.
    const { result } = await collectRun(
      settledSession({ status: "failed", costUsd: 0, failureClass: "provider-permanent" }),
      evt(1026, "TURN_ERROR", { failureMessage: "invalid provider api key" })
    );
    expect(result.ok).toBe(false);
    expect(result.status).toBe("failed");
    expect(result.error).toBe("invalid provider api key");
    expect(result.session.failureClass).toBe("provider-permanent");
  });

  it("waits for the settled record so cost/usage survive the park-event → settle race", async () => {
    const sockets: FakeWebSocket[] = [];
    let sessionReads = 0;
    const fetch: typeof globalThis.fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      void init;
      if (url.endsWith("/api/sessions/session-1/events/ticket")) {
        return json({ wsUrl: "wss://events.example.test/sessions/session-1", ticket: "ticket", expiresAtMs: 1 });
      }
      if (url.endsWith("/api/sessions/session-1/outputs")) return json({ outputs: [] });
      if (url.endsWith("/api/sessions/session-1/messages")) {
        return json({
          session: { id: "session-1", status: "running", turnSeq: 1 },
          turn: { sessionId: "session-1", turnSeq: 1 },
          eventCursor: 1024
        });
      }
      if (url.endsWith("/api/sessions/session-1")) {
        sessionReads += 1;
        // Read 1 (post-stream): settle hasn't landed — running, no settle stamp.
        // Read 2+: settled with costUsd + provider usage.
        return json({
          session:
            sessionReads < 2
              ? { id: "session-1", status: "running", turnSeq: 1 }
              : {
                  id: "session-1",
                  status: "idle",
                  turnSeq: 1,
                  costUsd: 0.0042,
                  costTelemetry: { providerUsage: [{ inputTokens: 3, outputTokens: 2, totalTokens: 5 }] }
                }
        });
      }
      if (url.endsWith("/api/sessions")) return json({ session: { id: "session-1", status: "idle", turnSeq: 0 } });
      return json({});
    };
    const factory = (url: string): FakeWebSocket => {
      const ws = new FakeWebSocket(url);
      sockets.push(ws);
      return ws;
    };
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const promise = client.start(
      { model: "claude-haiku-4-5", message: "p", apiKeys: { anthropic: "sk-ant" } },
      { webSocketFactory: factory }
    );

    await flush();
    sockets[0]!.message(evt(1024, "TEXT_MESSAGE_CONTENT", { text: "hi", messageId: "m1" }));
    sockets[0]!.message(terminal("aex.session.idle"));

    const result = await promise;
    expect(result.ok).toBe(true);
    expect(result.status).toBe("succeeded");
    expect(result.costUsd).toBe(0.0042);
    expect(result.usage).toEqual({ inputTokens: 3, outputTokens: 2, totalTokens: 5 });
    expect(result.session?.status).toBe("idle");
    expect(sessionReads).toBeGreaterThanOrEqual(2);
  });

  it("await:'park' returns at the park event without waiting for settle", async () => {
    const sockets: FakeWebSocket[] = [];
    let sessionReads = 0;
    const fetch: typeof globalThis.fetch = async (input) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      if (url.endsWith("/api/sessions/session-1/events/ticket")) {
        return json({ wsUrl: "wss://events.example.test/sessions/session-1", ticket: "ticket", expiresAtMs: 1 });
      }
      if (url.endsWith("/api/sessions/session-1/outputs")) return json({ outputs: [] });
      if (url.endsWith("/api/sessions/session-1/messages")) {
        return json({ session: { id: "session-1", status: "running", turnSeq: 1 }, turn: { sessionId: "session-1", turnSeq: 1 }, eventCursor: 1024 });
      }
      if (url.endsWith("/api/sessions/session-1")) {
        sessionReads += 1;
        return json({ session: { id: "session-1", status: "running", turnSeq: 1 } });
      }
      if (url.endsWith("/api/sessions")) return json({ session: { id: "session-1", status: "idle", turnSeq: 0 } });
      return json({});
    };
    const factory = (url: string): FakeWebSocket => {
      const ws = new FakeWebSocket(url);
      sockets.push(ws);
      return ws;
    };
    const client = new Aex({ apiKey: "tkn", baseUrl: "https://x", fetch });
    const promise = client.start(
      { model: "claude-haiku-4-5", message: "p", apiKeys: { anthropic: "sk-ant" } },
      { webSocketFactory: factory, await: "park" }
    );
    await flush();
    sockets[0]!.message(terminal("aex.session.idle"));
    const result = await promise;
    // Only the single post-stream read — no settle poll loop.
    expect(sessionReads).toBe(1);
    // The outcome still reads from the carried event.
    expect(result.status).toBe("succeeded");
  });

  it("throws when throwOnFailure is set and the turn did not park cleanly", async () => {
    const { client, sockets, webSocketFactory } = runClient(settledSession({ status: "failed", costUsd: 0 }));
    const promise = client.start(
      {
        model: "claude-haiku-4-5",
        message: "p",
        apiKeys: { anthropic: "sk-ant" }
      },
      { throwOnFailure: true, webSocketFactory }
    );

    await flush();
    sockets[0]!.message(evt(1026, "TURN_ERROR", { failureMessage: "boom" }));

    await expect(promise).rejects.toThrow(/session session-1 ended failed: boom/);
  });
});

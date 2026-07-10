import { describe, expect, it } from "vitest";
import { Aex, type SessionStartResult } from "../../src/index.js";
import type { AexEvent, WebSocketLike } from "@aexhq/contracts";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

function event(sequence: number, patch: Partial<AexEvent> = {}): AexEvent {
  return {
    specversion: "1.0",
    id: `sess_1:${sequence}`,
    source: "agent",
    type: "TEXT_MESSAGE_CONTENT",
    subject: "sess_1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { text: "hello" },
    ...patch
  };
}

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

  message(evt: AexEvent): void {
    this.#emit("message", { data: JSON.stringify(evt) });
  }

  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}

const flush = async (n = 4): Promise<void> => {
  for (let i = 0; i < n; i++) await new Promise<void>((resolve) => setTimeout(resolve, 0));
};

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json" }
  });
}

function makeClient(options: { readonly getSessionStatus?: string } = {}): {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
} {
  const calls: CapturedRequest[] = [];
  const sockets: FakeWebSocket[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const headers: Record<string, string> = {};
    if (init?.headers instanceof Headers) {
      for (const [k, v] of init.headers.entries()) headers[k] = v;
    } else if (Array.isArray(init?.headers)) {
      for (const [k, v] of init.headers) headers[k] = v;
    } else if (init?.headers) {
      Object.assign(headers, init.headers);
    }
    let body: unknown = init?.body;
    if (typeof body === "string") body = JSON.parse(body) as unknown;
    calls.push({ url, method: (init?.method ?? "GET").toString(), headers, body });

    if (url.endsWith("/api/sessions")) {
      return json({ session: { id: "sess_1", status: "idle", turnSeq: 0 } });
    }
    if (url.endsWith("/api/sessions/sess_1/messages")) {
      return json({
        session: { id: "sess_1", status: "running", turnSeq: 1 },
        turn: { sessionId: "sess_1", turnSeq: 1 },
        eventCursor: 4096
      });
    }
    if (url.endsWith("/api/sessions/sess_1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/sess_1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.endsWith("/api/sessions/sess_1/files")) {
      return json({ files: [{ id: "out_1", filename: "answer.txt" }] });
    }
    if (url.endsWith("/api/sessions/sess_1")) {
      // Settle-stamped (costUsd present) so the default await-settle resolves on
      // the first read; a `running` override stays unsettled to exercise the lag.
      return json({ session: { id: "sess_1", status: options.getSessionStatus ?? "idle", turnSeq: 1, costUsd: 0 } });
    }
    return json({});
  };
  const client = new Aex({ apiKey: "tkn", baseUrl: "https://api.example.test", fetch });
  const factory = (url: string): FakeWebSocket => {
    const ws = new FakeWebSocket(url);
    sockets.push(ws);
    return ws;
  };
  return { client, calls, sockets, webSocketFactory: factory };
}

describe("Aex sessions", () => {
  it("sessions.start creates a session, sends one message, and stops on a session idle event", async () => {
    const { client, calls, sockets, webSocketFactory } = makeClient();
    const promise = client.sessions.start({
      model: "claude-haiku-4-5",
      message: "say hello",
      apiKeys: { anthropic: "sk-ant" },
      stream: { webSocketFactory }
    });

    await flush();
    expect(sockets).toHaveLength(1);
    expect(sockets[0]!.url).toBe("wss://events.example.test/sessions/sess_1?ticket=ticket&from=4096");
    sockets[0]!.message(event(4096));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "CUSTOM",
      data: { name: "aex.session.idle", value: { sessionId: "sess_1", turnSeq: 1 } }
    }));

    const result: SessionStartResult = await promise;
    expect(result.sessionId).toBe("sess_1");
    // The RESULT status is the terminal OUTCOME (a clean park ⇒ succeeded); the
    // resumable lifecycle `idle` stays on the session record.
    expect(result.status).toBe("succeeded");
    expect(result.session.status).toBe("idle");
    expect(result.text).toBe("hello");
    expect(result.events.map((evt) => evt.sequence)).toEqual([4096, 4097]);
    expect(result.files).toEqual([{ id: "out_1", filename: "answer.txt" }]);
    expect(calls.map((call) => `${call.method} ${call.url}`)).toContain(
      "POST https://api.example.test/api/sessions/sess_1/messages"
    );
    const create = calls.find((call) => call.method === "POST" && call.url.endsWith("/api/sessions"));
    expect((create!.body as Record<string, unknown>).retention).toEqual({ idleTtl: "3m" });
    expect(calls.map((call) => `${call.method} ${call.url}`)).toContain(
      "GET https://api.example.test/api/sessions/sess_1/files"
    );
  });

  it("patches a stale (running) post-stream record from the terminal event (await:'park')", async () => {
    const { client, sockets, webSocketFactory } = makeClient({ getSessionStatus: "running" });
    const promise = client.sessions.start({
      model: "claude-haiku-4-5",
      message: "say hello",
      apiKeys: { anthropic: "sk-ant" },
      // 'park' skips the settle-poll (the record here never settles) so the
      // terminal-event → record patch is what we exercise.
      stream: { webSocketFactory, await: "park" }
    });

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "CUSTOM",
      data: { name: "aex.session.idle", value: { sessionId: "sess_1", turnSeq: 1 } }
    }));

    const result = await promise;
    // Outcome status is succeeded; the stale `running` record is patched to the
    // carried `idle` park.
    expect(result.status).toBe("succeeded");
    expect(result.session.status).toBe("idle");
  });

  it("session.send can be consumed as an async event stream", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const session = await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" }
    });
    const seen: number[] = [];
    const consume = (async () => {
      for await (const evt of session.send("continue", { webSocketFactory })) {
        seen.push(evt.sequence);
      }
    })();

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "CUSTOM",
      data: { name: "aex.session.idle", value: { turnSeq: 1 } }
    }));
    await consume;

    expect(seen).toEqual([4096, 4097]);
  });

  it("yields events carrying the is*() type-guard methods, and narrows in the branch", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const session = await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" }
    });
    const texts: string[] = [];
    const toolNames: string[] = [];
    let sawSettled = false;
    const consume = (async () => {
      for await (const evt of session.send("continue", { webSocketFactory })) {
        if (evt.isTextMessage()) {
          // `evt.data.text` is narrowed to `string` — no cast needed.
          texts.push(evt.data.text);
        } else if (evt.isToolCallStart()) {
          toolNames.push(evt.data.name);
        }
        if (evt.isSessionSettled()) sawSettled = true;
        // The lifecycle discriminants are mutually exclusive on a text event.
        // eslint-disable-next-line aex/no-conditional-expect -- per-event-type check over a stream the harness guarantees yields text events; asserts discriminant mutual-exclusivity for each text event.
        if (evt.isTextMessage()) {
          expect(evt.isToolCallStart()).toBe(false);
          expect(evt.isCustom()).toBe(false);
          expect(evt.isEventChannel()).toBe(true);
        }
      }
    })();

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(
      event(4097, {
        source: "agent",
        type: "TOOL_CALL_START",
        data: { id: "tc1", name: "read_file", arguments: { path: "/x" } }
      })
    );
    sockets[0]!.message(
      event(4098, {
        source: "runtime",
        type: "CUSTOM",
        data: { name: "aex.session.idle", value: { turnSeq: 1 } }
      })
    );
    await consume;

    expect(texts).toEqual(["hello"]);
    expect(toolNames).toEqual(["read_file"]);
    expect(sawSettled).toBe(true);
  });

  it("collected result.events carry the is*() methods too", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const promise = client.sessions.start({
      model: "claude-haiku-4-5",
      message: "say hello",
      apiKeys: { anthropic: "sk-ant" },
      stream: { webSocketFactory }
    });

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(
      event(4097, {
        source: "runtime",
        type: "CUSTOM",
        data: { name: "aex.session.idle", value: { turnSeq: 1 } }
      })
    );

    const result = await promise;
    expect(result.events[0]!.isTextMessage()).toBe(true);
    expect(result.events[1]!.isCustom()).toBe(true);
    expect(result.events[1]!.isSessionSettled()).toBe(true);
  });

  it("openSession rehydrates an existing session handle", async () => {
    const { client, calls } = makeClient();
    const session = await client.openSession("sess_1");

    expect(session.id).toBe("sess_1");
    expect(calls.map((call) => `${call.method} ${call.url}`)).toContain(
      "GET https://api.example.test/api/sessions/sess_1"
    );
  });

  it("sessions.delete deletes an id-addressed session", async () => {
    const { client, calls } = makeClient();

    await client.sessions.delete("sess_1", { idempotencyKey: "delete-key-1" });

    const req = calls.find((call) => call.method === "DELETE" && call.url.endsWith("/api/sessions/sess_1"));
    expect(req).toBeTruthy();
    expect(req!.headers["Idempotency-Key"] ?? req!.headers["idempotency-key"]).toBe("delete-key-1");
  });

  it("openSession lets callers override the idle-to-suspend TTL", async () => {
    const { client, calls } = makeClient();
    await client.openSession({
      model: "claude-haiku-4-5",
      overrides: { idleTtl: "10m" },
      apiKeys: { anthropic: "sk-ant" }
    });

    const create = calls.find((call) => call.method === "POST" && call.url.endsWith("/api/sessions"));
    expect((create!.body as Record<string, unknown>).retention).toEqual({
      idleTtl: "10m"
    });
  });

  it("rejects the old idleSuspendAfter override", async () => {
    const { client } = makeClient();
    await expect(
      client.openSession({
        model: "claude-haiku-4-5",
        overrides: { idleSuspendAfter: "10m" } as never,
        apiKeys: { anthropic: "sk-ant" }
      })
    ).rejects.toThrow(/overrides\.idleSuspendAfter is not a supported option; use overrides\.idleTtl/);
  });

  it("rejects legacy one-shot prompt input before any HTTP request", async () => {
    const { client, calls } = makeClient();

    await expect(
      client.start({
        model: "claude-haiku-4-5",
        prompt: "legacy one-shot input",
        apiKeys: { anthropic: "sk-ant" }
      } as never)
    ).rejects.toThrow(/Aex\.start: prompt is not a supported option; use message/);
    await expect(
      client.sessions.start({
        model: "claude-haiku-4-5",
        prompt: "legacy one-shot input",
        apiKeys: { anthropic: "sk-ant" }
      } as never)
    ).rejects.toThrow(/Aex\.sessions\.start: prompt is not a supported option; use message/);
    expect(calls).toHaveLength(0);
  });

  it("rejects invalid one-shot messages before any HTTP request", async () => {
    const { client, calls } = makeClient();

    await expect(
      client.start({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-ant" }
      } as never)
    ).rejects.toThrow(/Aex\.start: message must be a non-empty string or string array/);
    await expect(
      client.sessions.start({
        model: "claude-haiku-4-5",
        message: "",
        apiKeys: { anthropic: "sk-ant" }
      })
    ).rejects.toThrow(/Aex\.sessions\.start: message must be a non-empty string/);
    await expect(
      client.sessions.start({
        model: "claude-haiku-4-5",
        message: ["ok", ""] as never,
        apiKeys: { anthropic: "sk-ant" }
      })
    ).rejects.toThrow(/Aex\.sessions\.start: message segments must be non-empty strings/);
    expect(calls).toHaveLength(0);
  });

  it("rejects invalid session.send input before sending a message request", async () => {
    const { client, calls } = makeClient();
    const session = await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" }
    });
    calls.length = 0;

    expect(() => session.send("")).toThrow(/SessionHandle\.send: input must be a non-empty string/);
    expect(() => session.send([] as never)).toThrow(/SessionHandle\.send: input must be a non-empty string or string array/);
    expect(() => session.send(["ok", ""] as never)).toThrow(/SessionHandle\.send: input segments must be non-empty strings/);
    expect(calls).toHaveLength(0);
  });

  it("rejects public AbortSignal controls on the session API", async () => {
    const { client } = makeClient();
    const signal = new AbortController().signal;
    const session = await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" }
    });

    expect(() => session.send("continue", { signal } as never)).toThrow(/signal is not a supported option/);
    await expect(
      client.start({
        model: "claude-haiku-4-5",
        message: "continue",
        apiKeys: { anthropic: "sk-ant" },
        stream: { signal } as never
      })
    ).rejects.toThrow(/signal is not a supported option/);
  });
});

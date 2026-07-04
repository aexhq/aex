/**
 * Aex-level resilience coverage: the built-in transport retry, the
 * STABLE idempotency key that keeps a retried submit from creating a duplicate
 * billable run (defect sdk-dx-3), `session.replayLast()`, and the structured
 * throttle error surfaced on a provider-throttled turn.
 *
 * Runs against the SDK source (not a packed install) with a scripted fetch + a
 * fake coordinator WebSocket, so a whole `run` / `send` turn is driven
 * deterministically without a live backend.
 */
import { describe, expect, it } from "vitest";
import { Aex, isRateLimited, AexRateLimitError, RunStateError } from "../../src/index.js";
import type { AexEvent, JsonValue, WebSocketLike } from "@aexhq/contracts";

interface RecordedCall {
  readonly method: string;
  readonly url: string;
  readonly headers: Record<string, string>;
}

function headersToObject(headers: HeadersInit | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (!headers) return out;
  if (headers instanceof Headers) {
    for (const [k, v] of headers.entries()) out[k.toLowerCase()] = v;
  } else if (Array.isArray(headers)) {
    for (const [k, v] of headers) out[String(k).toLowerCase()] = String(v);
  } else {
    for (const [k, v] of Object.entries(headers)) out[String(k).toLowerCase()] = String(v);
  }
  return out;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function idleEvent(turnSeq = 1, seq = 1024): AexEvent {
  return {
    specversion: "1.0",
    id: `run-1:${seq}`,
    source: "runtime",
    type: "CUSTOM",
    subject: "run-1",
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: { name: "aex.session.idle", value: { turnSeq } } as Record<string, JsonValue>
  };
}

function errorEvent(turnSeq = 1, seq = 1024): AexEvent {
  return {
    specversion: "1.0",
    id: `run-1:${seq}`,
    source: "runtime",
    type: "CUSTOM",
    subject: "run-1",
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: { name: "aex.session.error", value: { turnSeq } } as Record<string, JsonValue>
  };
}

function runErrorEvent(seq = 1024): AexEvent {
  return {
    specversion: "1.0",
    id: `run-1:${seq}`,
    source: "runtime",
    type: "RUN_ERROR",
    subject: "run-1",
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: {
      reason: "failed",
      failureClass: "transient-provider",
      failureMessage: "provider returned no public assistant content"
    } as Record<string, JsonValue>
  };
}

class FakeWebSocket implements WebSocketLike {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};
  constructor(url: string) {
    this.url = url;
  }
  addEventListener(type: string, cb: (ev: { data?: unknown }) => void): void {
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

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

async function waitForSocket(sockets: readonly FakeWebSocket[], count: number): Promise<void> {
  for (let i = 0; i < 200 && sockets.length < count; i++) await tick();
  if (sockets.length < count) throw new Error(`socket ${count} never opened`);
}

interface Harness {
  readonly client: Aex;
  readonly calls: RecordedCall[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
  idempotencyKeys(pathSuffix: string): string[];
}

/**
 * @param finalSession the record the post-stream `GET /api/sessions/run-1`
 *        returns (drives run ok/error/throttle branches).
 * @param createStatuses status codes the create endpoint plays IN ORDER (last
 *        repeats); e.g. `[429, 201]` to inject one throttle before success.
 */
function harness(
  finalSession: Record<string, unknown> = { id: "run-1", status: "idle", turnSeq: 1 },
  createStatuses: readonly number[] = [201]
): Harness {
  const calls: RecordedCall[] = [];
  const sockets: FakeWebSocket[] = [];
  let createCount = 0;

  const fetchImpl: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = String(init?.method ?? "GET").toUpperCase();
    calls.push({ method, url, headers: headersToObject(init?.headers) });

    if (url.endsWith("/api/sessions/run-1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/run-1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.endsWith("/api/sessions/run-1/outputs")) {
      return json({ outputs: [] });
    }
    if (url.endsWith("/api/sessions/run-1/messages")) {
      return json({
        session: { id: "run-1", status: "running", turnSeq: 1 },
        turn: { sessionId: "run-1", turnSeq: 1 },
        eventCursor: 1024
      });
    }
    if (url.endsWith("/api/sessions/run-1")) {
      return json({ session: finalSession });
    }
    if (url.endsWith("/api/sessions")) {
      const status = createStatuses[Math.min(createCount, createStatuses.length - 1)] ?? 201;
      createCount += 1;
      if (status >= 400) return json({ error: "slow down" }, status);
      return json({ session: { id: "run-1", status: "idle", turnSeq: 0 } }, status);
    }
    return json({});
  };

  const webSocketFactory = (url: string): FakeWebSocket => {
    const ws = new FakeWebSocket(url);
    sockets.push(ws);
    return ws;
  };

  const client = new Aex({
    apiKey: "tkn",
    baseUrl: "https://x",
    fetch: fetchImpl,
    // Instant backoff keeps the test fast; the retry LOGIC is exercised fully.
    retry: { initialDelayMs: 0, maxDelayMs: 0, maxAttempts: 4 }
  });

  return {
    client,
    calls,
    sockets,
    webSocketFactory,
    idempotencyKeys: (pathSuffix) =>
      calls
        .filter((c) => c.method === "POST" && c.url.endsWith(pathSuffix))
        .map((c) => c.headers["idempotency-key"] ?? "")
  };
}

describe("Aex idempotency (sdk-dx-3)", () => {
  it("run() derives the message key from the create key so a retried run never double-bills", async () => {
    const h = harness();
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hello", apiKeys: { anthropic: "sk-ant" }, idempotencyKey: "fixed-key" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    const result = await promise;
    expect(result.ok).toBe(true);

    // The create carries the caller's key; the billable turn carries the DERIVED
    // key — so re-invoking run() with the same key de-duplicates BOTH.
    expect(h.idempotencyKeys("/api/sessions")).toEqual(["fixed-key"]);
    expect(h.idempotencyKeys("/api/sessions/run-1/messages")).toEqual(["fixed-key:message"]);
  });

  it("run() without a key still ties the message key to the create key", async () => {
    const h = harness();
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hello", apiKeys: { anthropic: "sk-ant" } },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await promise;

    const createKey = h.idempotencyKeys("/api/sessions")[0]!;
    const messageKey = h.idempotencyKeys("/api/sessions/run-1/messages")[0]!;
    expect(createKey).toBeTruthy();
    expect(messageKey).toBe(`${createKey}:message`);
  });

  it("sessions.run() derives the message key the same way", async () => {
    const h = harness();
    const promise = h.client.sessions.run({
      model: "claude-haiku-4-5",
      message: "hello",
      apiKeys: { anthropic: "sk-ant" },
      idempotencyKey: "run-key",
      stream: { webSocketFactory: h.webSocketFactory }
    });
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await promise;
    expect(h.idempotencyKeys("/api/sessions")).toEqual(["run-key"]);
    expect(h.idempotencyKeys("/api/sessions/run-1/messages")).toEqual(["run-key:message"]);
  });
});

describe("Aex built-in transport retry", () => {
  it("retries a throttled create with the SAME idempotency key (no duplicate billable run)", async () => {
    const h = harness({ id: "run-1", status: "idle", turnSeq: 1 }, [429, 201]);
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hello", apiKeys: { anthropic: "sk-ant" }, idempotencyKey: "K" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    const result = await promise;
    expect(result.ok).toBe(true);

    // Two create attempts (429 then 201), both with the identical key.
    const createKeys = h.idempotencyKeys("/api/sessions");
    expect(createKeys).toEqual(["K", "K"]);
  });

  it("surfaces AexRateLimitError when a create is throttled past the attempt budget", async () => {
    const h = harness({ id: "run-1", status: "idle", turnSeq: 1 }, [429]);
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hello", apiKeys: { anthropic: "sk-ant" } },
      { webSocketFactory: h.webSocketFactory }
    );
    const err = await promise.catch((e: unknown) => e);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).status).toBe(429);
    expect((err as AexRateLimitError).attempts).toBe(4);
    // Four create attempts, one shared key, and NO WebSocket ever opened.
    expect(h.idempotencyKeys("/api/sessions")).toHaveLength(4);
    expect(h.sockets).toHaveLength(0);
  });
});

describe("SessionHandle.replayLast", () => {
  it("replays the last message reusing the same idempotency key", async () => {
    const h = harness();
    const session = await h.client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });

    const first = session.send("do the thing", { webSocketFactory: h.webSocketFactory }).done();
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await first;

    const replay = session.replayLast({ webSocketFactory: h.webSocketFactory }).done();
    await waitForSocket(h.sockets, 2);
    h.sockets[1]!.message(idleEvent());
    await replay;

    const keys = h.idempotencyKeys("/api/sessions/run-1/messages");
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBe(keys[1]);
    expect(keys[0]).toBeTruthy();
  });

  it("throws a clear error when nothing has been sent yet", async () => {
    const h = harness();
    const session = await h.client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    expect(() => session.replayLast()).toThrow(/no message has been sent/);
  });

  it("a fresh idempotency key forces a brand-new billable turn", async () => {
    const h = harness();
    const session = await h.client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    const first = session.send("go", { webSocketFactory: h.webSocketFactory }).done();
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await first;

    const replay = session.replayLast({ idempotencyKey: "override", webSocketFactory: h.webSocketFactory }).done();
    await waitForSocket(h.sockets, 2);
    h.sockets[1]!.message(idleEvent());
    await replay;

    const keys = h.idempotencyKeys("/api/sessions/run-1/messages");
    expect(keys[1]).toBe("override");
    expect(keys[0]).not.toBe("override");
  });
});

describe("Aex throttle error on a provider-throttled turn", () => {
  it("ends a session turn on RUN_ERROR even when no aex.session.error event follows", async () => {
    const h = harness({ id: "run-1", status: "error", turnSeq: 1, errorMessage: "provider returned no public assistant content" });
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hi", apiKeys: { anthropic: "sk-ant" } },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(runErrorEvent());

    const result = await Promise.race([
      promise,
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error("session turn did not end on RUN_ERROR")), 250))
    ]);

    expect(result.ok).toBe(false);
    expect(result.status).toBe("error");
    expect(result.events.map((event) => event.type)).toEqual(["RUN_ERROR"]);
  });

  it("throwOnFailure raises AexRateLimitError from a structured provider fault", async () => {
    const h = harness({
      id: "run-1",
      status: "error",
      turnSeq: 1,
      providerFault: { provider: "anthropic", kind: "overloaded", status: 529, retryAfterMs: 4000 }
    });
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hi", apiKeys: { anthropic: "sk-ant" } },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(isRateLimited(err)).toBe(true);
    const rate = err as AexRateLimitError;
    expect(rate.source).toBe("provider");
    expect(rate.status).toBe(529);
    expect(rate.retryAfterMs).toBe(4000);
    expect(rate.providerFault?.provider).toBe("anthropic");
  });

  it("throwOnFailure raises AexRateLimitError from a rate-limit error MESSAGE", async () => {
    const h = harness({ id: "run-1", status: "error", turnSeq: 1, errorMessage: "provider rate limit (429) exceeded" });
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hi", apiKeys: { anthropic: "sk-ant" } },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).source).toBe("provider");
  });

  it("a non-throttle failure still raises the plain RunStateError", async () => {
    const h = harness({ id: "run-1", status: "error", turnSeq: 1, errorMessage: "disk full" });
    const promise = h.client.run(
      { model: "claude-haiku-4-5", message: "hi", apiKeys: { anthropic: "sk-ant" } },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RunStateError);
    expect(isRateLimited(err)).toBe(false);
  });
});

/**
 * Aex-level resilience coverage: the built-in transport retry, the
 * STABLE idempotency key that keeps a retried submit from creating a duplicate
 * billable session turn (defect sdk-dx-3), `session.messages.replayLast()`, and the structured
 * throttle error surfaced on a provider-throttled turn.
 *
 * Runs against the SDK source (not a packed install) with a scripted fetch + a
 * fake coordinator WebSocket, so a whole `run` / `send` turn is driven
 * deterministically without a live backend.
 */
import { describe, expect, it } from "bun:test";
import { HTTP_RETRY_POLICY, type FetchLike } from "@aexhq/contracts";
import { Aex, isRateLimited, AexRateLimitError, SessionStateError } from "../../src/index.js";
import type { AexEvent, JsonValue } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";

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
    id: `session-1:${seq}`,
    source: "runtime",
    type: "RUN_FINISHED",
    subject: "session-1",
    threadId: "session-1",
    runId: `run-${turnSeq}`,
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: `cp-${turnSeq}` } } as Record<string, JsonValue>
  };
}

function errorEvent(turnSeq = 1, seq = 1024): AexEvent {
  return {
    specversion: "1.0",
    id: `session-1:${seq}`,
    source: "runtime",
    type: "RUN_ERROR",
    subject: "session-1",
    threadId: "session-1",
    runId: `run-${turnSeq}`,
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: { outcome: "failed", failureMessage: "run failed", costUsd: 0, providerUsage: [] } as Record<string, JsonValue>
  };
}

function runErrorEvent(seq = 1024): AexEvent {
  return {
    specversion: "1.0",
    id: `session-1:${seq}`,
    source: "runtime",
    type: "RUN_ERROR",
    subject: "session-1",
    threadId: "session-1",
    runId: "run-1",
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: {
      reason: "failed",
      outcome: "failed",
      failureClass: "transient-provider",
      failureMessage: "provider returned no public assistant content",
      costUsd: 0,
      providerUsage: []
    } as Record<string, JsonValue>
  };
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
 * @param finalSession the record the post-stream `GET /api/sessions/session-1`
 *        returns (drives run ok/error/throttle branches).
 * @param createStatuses status codes the create endpoint plays IN ORDER (last
 *        repeats); e.g. `[429, 201]` to inject one throttle before success.
 */
function harness(
  finalSession: Record<string, unknown> = { id: "session-1", status: "idle", acceptsMessages: true },
  createStatuses: readonly number[] = [201],
  debug?: (line: string) => void
): Harness {
  const calls: RecordedCall[] = [];
  const sockets: FakeWebSocket[] = [];
  let createCount = 0;

  const fetchImpl: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = String(init?.method ?? "GET").toUpperCase();
    calls.push({ method, url, headers: headersToObject(init?.headers) });

    if (url.endsWith("/api/sessions/session-1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/session-1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.includes("/api/sessions/session-1/files?checkpointId=cp-1")) {
      return json({
        revision: { checkpointId: "cp-1", runId: "run-1", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: 1024 },
        files: []
      });
    }
    if (url.endsWith("/api/sessions/session-1/messages")) {
      return json({
        session: { id: "session-1", status: "running", acceptsMessages: false },
        run: { sessionId: "session-1", turnSeq: 1, runId: "run-1", phase: "running", eventCursor: 1024 },
        eventCursor: 1024
      });
    }
    if (url.endsWith("/api/sessions/session-1")) {
      // Terminal billing is already committed, so the first read is authoritative.
      const failed = finalSession.status === "error";
      return json({ session: {
        acceptsMessages: true,
        costUsd: 0,
        costTelemetry: { providerUsage: [] },
        lastRun: {
          sessionId: "session-1",
          runId: "run-1",
          turnSeq: 1,
          phase: failed ? "error" : "finished",
          outcome: failed ? "failed" : "succeeded"
        },
        ...finalSession
      } });
    }
    if (url.endsWith("/api/sessions")) {
      const status = createStatuses[Math.min(createCount, createStatuses.length - 1)] ?? 201;
      createCount += 1;
      if (status >= 400) return json({ error: "slow down" }, status);
      return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }, status);
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
    ...(debug ? { debug } : {}),
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
  it("start() derives the message key from the SDK-minted create key so a retried run never double-bills", async () => {
    const h = harness();
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hello" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    const result = await promise;
    expect(result.ok).toBe(true);

    // The create carries the SDK-minted key; the billable turn carries the
    // DERIVED key, so the two halves of one start() de-duplicate together.
    const createKey = h.idempotencyKeys("/api/sessions")[0]!;
    expect(createKey).toMatch(/^idem_[0-9a-f]{32}$/);
    expect(h.idempotencyKeys("/api/sessions/session-1/messages")).toEqual([`${createKey}:message`]);
  });

  it("start() without a key still ties the message key to the create key", async () => {
    const h = harness();
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hello" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await promise;

    const createKey = h.idempotencyKeys("/api/sessions")[0]!;
    const messageKey = h.idempotencyKeys("/api/sessions/session-1/messages")[0]!;
    expect(createKey).toBeTruthy();
    expect(messageKey).toBe(`${createKey}:message`);
  });

  it("derives the message key the same way when the stream options are inline", async () => {
    const h = harness();
    const promise = h.client.start({
      model: "anthropic/claude-haiku-4-5",
      message: "hello",
      stream: { webSocketFactory: h.webSocketFactory }
    });
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await promise;

    const createKey = h.idempotencyKeys("/api/sessions")[0]!;
    expect(createKey).toMatch(/^idem_[0-9a-f]{32}$/);
    expect(h.idempotencyKeys("/api/sessions/session-1/messages")).toEqual([`${createKey}:message`]);
  });

  // The long-create-key DIGEST branch of `deriveMessageIdempotencyKey` is no
  // longer reachable from the SDK: a minted key is always `idem_<32 hex>`. That
  // branch is pinned at its own layer, in
  // `packages/contracts/test/operations-idempotency-headers.test.ts`.
  it("keeps the minted create key and its derived message key within the 255-character limit", async () => {
    const h = harness();
    const promise = h.client.start({
      model: "anthropic/claude-haiku-4-5",
      message: "hello",
      stream: { webSocketFactory: h.webSocketFactory }
    });
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await promise;

    const createKey = h.idempotencyKeys("/api/sessions")[0]!;
    const messageKey = h.idempotencyKeys("/api/sessions/session-1/messages")[0]!;
    expect(createKey.length).toBeLessThanOrEqual(255);
    expect(messageKey.length).toBeLessThanOrEqual(255);
  });
});

describe("Aex built-in transport retry", () => {
  it("retries a throttled create with the SAME idempotency key (no duplicate billable session turn)", async () => {
    const h = harness({ id: "session-1", status: "idle" }, [429, 201]);
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hello" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    const result = await promise;
    expect(result.ok).toBe(true);

    // Two create attempts (429 then 201) carrying the identical MINTED key.
    // Comparing the values matters more than counting them: a key re-minted per
    // attempt would still produce two calls, and would double-bill silently.
    const createKeys = h.idempotencyKeys("/api/sessions");
    expect(createKeys).toHaveLength(2);
    expect(createKeys[0]).toMatch(/^idem_[0-9a-f]{32}$/);
    expect(createKeys[0]).toBe(createKeys[1]);
  });

  it("surfaces AexRateLimitError when a create is throttled past the attempt budget", async () => {
    const h = harness({ id: "session-1", status: "idle" }, [429]);
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hello" },
      { webSocketFactory: h.webSocketFactory }
    );
    const err = await promise.catch((e: unknown) => e);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).status).toBe(429);
    // The attempt budget is the ONE shared policy's — the same object the `aex`
    // CLI hands its transport (see cli/test/retry-policy-parity.test.ts).
    expect((err as AexRateLimitError).attempts).toBe(HTTP_RETRY_POLICY.maxAttempts);
    // One create attempt per budgeted try, ONE shared key, NO WebSocket opened.
    // The key equality is asserted, not just the attempt count: counting alone
    // cannot tell a stable key from a fresh one minted per attempt.
    const createKeys = h.idempotencyKeys("/api/sessions");
    expect(createKeys).toHaveLength(HTTP_RETRY_POLICY.maxAttempts);
    expect(new Set(createKeys).size).toBe(1);
    expect(h.sockets).toHaveLength(0);
  });
});

describe("SessionHandle.replayLast", () => {
  it("replays the last message reusing the same idempotency key", async () => {
    const h = harness();
    const session = await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });

    const first = session.messages.send("do the thing", { webSocketFactory: h.webSocketFactory }).finished();
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await first;

    const replay = session.messages.replayLast({ webSocketFactory: h.webSocketFactory }).finished();
    await waitForSocket(h.sockets, 2);
    h.sockets[1]!.message(idleEvent());
    await replay;

    const keys = h.idempotencyKeys("/api/sessions/session-1/messages");
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBe(keys[1]);
    expect(keys[0]).toBeTruthy();
  });

  it("throws a clear error when nothing has been sent yet", async () => {
    const h = harness();
    const session = await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    expect(() => session.messages.replayLast()).toThrow(/no message has been sent/);
  });

  // A caller can no longer override the replay key to force a second billable
  // turn — the key is SDK-owned. `session.messages.send(...)` is how you ask for
  // a NEW turn; `replayLast()` re-presents the ORIGINAL identity, so a replay
  // that reaches a server which already recorded it is de-duplicated.
  it("rejects a caller-supplied idempotencyKey on replayLast", async () => {
    const h = harness();
    const session = await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    const first = session.messages.send("go", { webSocketFactory: h.webSocketFactory }).finished();
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await first;

    expect(() =>
      session.messages.replayLast({ idempotencyKey: "override" } as never)
    ).toThrow(/idempotency/i);
  });

  it("a fresh send mints a NEW key, so it is a brand-new billable turn", async () => {
    const h = harness();
    const session = await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    const first = session.messages.send("go", { webSocketFactory: h.webSocketFactory }).finished();
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(idleEvent());
    await first;

    const second = session.messages.send("go again", { webSocketFactory: h.webSocketFactory }).finished();
    await waitForSocket(h.sockets, 2);
    h.sockets[1]!.message(idleEvent());
    await second;

    const keys = h.idempotencyKeys("/api/sessions/session-1/messages");
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBeTruthy();
    expect(keys[1]).toBeTruthy();
    expect(keys[0]).not.toBe(keys[1]);
  });
});

describe("Aex throttle error on a provider-throttled turn", () => {
  it("ends a session turn on RUN_ERROR even when no aex.session.error event follows", async () => {
    const h = harness({ id: "session-1", status: "error", errorMessage: "provider returned no public assistant content" });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(runErrorEvent());

    const result = await Promise.race([
      promise,
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error("session turn did not end on RUN_ERROR")), 250))
    ]);

    expect(result.ok).toBe(false);
    // The bare `error` outcome is retired — a RUN_ERROR turn reads `failed`.
    expect(result.status).toBe("failed");
    expect(result.events.map((event) => event.type)).toEqual(["RUN_ERROR"]);
  });

  it("fails closed when RUN_ERROR omits per-run billing", async () => {
    const h = harness({ id: "session-1", status: "error", acceptsMessages: true });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message({
      ...runErrorEvent(),
      data: { outcome: "failed", failureMessage: "run failed" }
    });

    await expect(promise).rejects.toThrow(/missing valid per-run cost and provider usage/);
  });

  it("fails closed when a RUN terminal omits its explicit outcome", async () => {
    const h = harness({ id: "session-1", status: "error", acceptsMessages: true });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message({
      ...runErrorEvent(),
      data: { failureMessage: "run failed", costUsd: 0, providerUsage: [] }
    });

    await expect(promise).rejects.toThrow(/missing a valid explicit outcome/);
  });

  it("throwOnFailure raises AexRateLimitError from a structured provider fault", async () => {
    const h = harness({
      id: "session-1",
      status: "error",
      providerFault: { provider: "anthropic", kind: "overloaded", status: 529, retryAfterMs: 4000 }
    });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
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

  it("uses the narrow field-absent legacy adapter and emits one redacted diagnostic", async () => {
    const diagnostics: string[] = [];
    const h = harness({
      id: "session-1",
      status: "error",
      failureClass: "transient-provider",
      errorMessage: "llm provider unavailable (HTTP 429) — throttled or overloaded; request was not replayed automatically: secret detail"
    }, [201], (line) => diagnostics.push(line));
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(isRateLimited(err)).toBe(true);
    expect((err as AexRateLimitError).source).toBe("provider");
    expect(diagnostics.filter((line) => line.includes("legacy_provider_fault_fallback"))).toEqual([
      "[aex] legacy_provider_fault_fallback kind=rate_limit source=session"
    ]);
    expect(diagnostics.join("\n")).not.toContain("secret detail");
  });

  it.each([
    "tool returned code 429 while parsing a local fixture",
    "corporate limit policy rejected the request",
    "proveedor temporalmente limitado"
  ])("does not classify arbitrary failure prose as a provider throttle: %s", async (errorMessage) => {
    const h = harness({ id: "session-1", status: "error", errorMessage });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SessionStateError);
    expect(isRateLimited(err)).toBe(false);
  });

  it("does not override a present canonical non-throttle with legacy fields", async () => {
    const h = harness({
      id: "session-1",
      status: "error",
      providerFault: { kind: "provider_error", status: 429 },
      failureClass: "transient-provider",
      errorMessage: "llm provider unavailable (HTTP 429) — throttled or overloaded; request was not replayed automatically"
    });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SessionStateError);
    expect(isRateLimited(err)).toBe(false);
  });

  it("a non-throttle failure still raises the plain SessionStateError", async () => {
    const h = harness({ id: "session-1", status: "error", errorMessage: "disk full" });
    const promise = h.client.start(
      { model: "anthropic/claude-haiku-4-5", message: "hi" },
      { throwOnFailure: true, webSocketFactory: h.webSocketFactory }
    );
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(errorEvent());
    const err = await promise.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SessionStateError);
    expect(isRateLimited(err)).toBe(false);
  });
});

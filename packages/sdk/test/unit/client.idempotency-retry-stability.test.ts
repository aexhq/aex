/**
 * The SDK owns the idempotency key, so the ONE property that makes it worth
 * owning must be pinned: every automatic transport retry of a single logical
 * mutation ships the SAME `Idempotency-Key`.
 *
 * Why this file exists. The API Lambda has a 29-second timeout, so a submit can
 * succeed server-side while its response never reaches the client. The stable
 * key is what turns the automatic retry into a dedup instead of a second
 * session, a second container and a second bill. A regression that minted a
 * FRESH key per attempt would look completely correct — no error, no warning,
 * no failing type — and would silently double-bill in production.
 *
 * It is untestable by inspection and it fails silently, so it is asserted here
 * directly on the outbound headers.
 *
 * The pre-existing coverage in `client.retry.test.ts` cannot catch it:
 *   - "retries a throttled create with the SAME idempotency key" pins the keys
 *     but supplies an explicit `"K"`, and re-resolving an explicit key returns
 *     that same key — the assertion holds even if resolution moved per-attempt.
 *   - "surfaces AexRateLimitError when a create is throttled past the attempt
 *     budget" does auto-mint the key, but only asserts the attempt COUNT; it
 *     never compares the keys to one another.
 *
 * Every case below therefore mints the key internally — no caller-supplied key
 * exists on the public API — and compares the recorded headers across attempts.
 */
import { describe, expect, it } from "bun:test";
import type { AexEvent, FetchLike, JsonValue } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";
import { Aex } from "../../src/index.js";

interface RecordedCall {
  readonly method: string;
  readonly url: string;
  readonly idempotencyKey: string | undefined;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

function idempotencyKeyOf(init: RequestInit | undefined): string | undefined {
  return new Headers(init?.headers ?? {}).get("idempotency-key") ?? undefined;
}

function runFinished(seq = 1024): AexEvent {
  return {
    specversion: "1.0",
    id: `session-1:${seq}`,
    source: "runtime",
    type: "RUN_FINISHED",
    subject: "session-1",
    threadId: "session-1",
    runId: "run-1",
    time: new Date(seq).toISOString(),
    sequence: seq,
    data: {
      outcome: "succeeded",
      costUsd: 0,
      providerUsage: [],
      checkpoint: { checkpointId: "cp-1" }
    } as Record<string, JsonValue>
  };
}

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

async function waitForSocket(sockets: readonly FakeWebSocket[], count: number): Promise<void> {
  for (let i = 0; i < 200 && sockets.length < count; i++) await tick();
  if (sockets.length < count) throw new Error(`socket ${count} never opened`);
}

/**
 * A transport that fails the first `failures` attempts of the named POST path
 * with a retryable 500 and records the `Idempotency-Key` of EVERY outbound
 * request, including the ones that failed.
 */
function harness(options: {
  readonly failPathSuffix: string;
  readonly failures: number;
}): {
  readonly client: Aex;
  readonly calls: RecordedCall[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
  keysFor(pathSuffix: string): (string | undefined)[];
} {
  const calls: RecordedCall[] = [];
  const sockets: FakeWebSocket[] = [];
  let remainingFailures = options.failures;

  const fetchImpl: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = String(init?.method ?? "GET").toUpperCase();
    const path = new URL(url).pathname;
    calls.push({ method, url, idempotencyKey: idempotencyKeyOf(init) });

    if (method === "POST" && path.endsWith(options.failPathSuffix) && remainingFailures > 0) {
      remainingFailures -= 1;
      // 500 is retryable but NOT a rate-limit status, so an exhausted budget
      // would surface the plain API error rather than a throttle error.
      return json({ error: "internal" }, 500);
    }

    if (path.endsWith("/api/sessions/session-1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/session-1", ticket: "t", expiresAtMs: 1 });
    }
    if (path.endsWith("/api/sessions/session-1/files")) {
      return json({
        revision: { checkpointId: "cp-1", runId: "run-1", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: 1024 },
        files: []
      });
    }
    if (path.endsWith("/api/sessions/session-1/messages")) {
      return json({
        session: { id: "session-1", status: "running", acceptsMessages: false },
        run: { sessionId: "session-1", turnSeq: 1, runId: "run-1", phase: "running", eventCursor: 1024 },
        eventCursor: 1024
      });
    }
    if (path.endsWith("/api/sessions/session-1")) {
      return json({
        session: {
          id: "session-1",
          status: "idle",
          acceptsMessages: true,
          costUsd: 0,
          costTelemetry: { providerUsage: [] },
          lastRun: { sessionId: "session-1", runId: "run-1", turnSeq: 1, phase: "finished", outcome: "succeeded" }
        }
      });
    }
    if (path.endsWith("/api/sessions")) {
      return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }, 201);
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
    // Instant backoff keeps the test fast; the retry LOGIC is exercised in full.
    retry: { initialDelayMs: 0, maxDelayMs: 0 }
  });

  return {
    client,
    calls,
    sockets,
    webSocketFactory,
    keysFor: (pathSuffix) =>
      calls
        .filter((c) => c.method === "POST" && new URL(c.url).pathname.endsWith(pathSuffix))
        .map((c) => c.idempotencyKey)
  };
}

/** Every attempt carried one and the same non-empty key. */
function expectOneStableKey(keys: readonly (string | undefined)[], attempts: number): void {
  expect(keys).toHaveLength(attempts);
  for (const key of keys) {
    expect(typeof key).toBe("string");
    expect(key).toBeTruthy();
  }
  // The assertion that matters: ONE distinct value across every attempt.
  expect(new Set(keys).size).toBe(1);
}

describe("the SDK-minted idempotency key is STABLE across automatic retries", () => {
  it("sessions.create: 3 attempts of one logical create share one auto-minted key", async () => {
    const h = harness({ failPathSuffix: "/api/sessions", failures: 2 });

    const session = await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    expect(session.id).toBe("session-1");

    // Two 500s then the 201 — one logical create, three transport attempts.
    expectOneStableKey(h.keysFor("/api/sessions"), 3);
  });

  it("messages.send: 3 attempts of one logical send share one auto-minted key", async () => {
    const h = harness({ failPathSuffix: "/api/sessions/session-1/messages", failures: 2 });
    const session = await h.client.sessions.open("session-1");

    const finished = session.messages.send("do the thing", { webSocketFactory: h.webSocketFactory }).finished();
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(runFinished());
    await finished;

    expectOneStableKey(h.keysFor("/api/sessions/session-1/messages"), 3);
  });

  it("start(): the retried create AND the retried first message each keep one key", async () => {
    const h = harness({ failPathSuffix: "/api/sessions", failures: 2 });

    const promise = h.client.start({
      model: "anthropic/claude-haiku-4-5",
      message: "hello",
      stream: { webSocketFactory: h.webSocketFactory }
    });
    await waitForSocket(h.sockets, 1);
    h.sockets[0]!.message(runFinished());
    await promise;

    const createKeys = h.keysFor("/api/sessions");
    expectOneStableKey(createKeys, 3);

    // The first message key stays DERIVED from the (stable) create key, so a
    // replayed start() de-duplicates the billable turn as well as the create.
    const messageKeys = h.keysFor("/api/sessions/session-1/messages");
    expectOneStableKey(messageKeys, 1);
    expect(messageKeys[0]).toBe(`${createKeys[0]}:message`);
  });

  it("two SEPARATE logical creates mint DIFFERENT keys (the retry reuse is not a global constant)", async () => {
    const h = harness({ failPathSuffix: "/never", failures: 0 });

    await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });
    await h.client.sessions.create({ model: "anthropic/claude-haiku-4-5" });

    const keys = h.keysFor("/api/sessions");
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBeTruthy();
    expect(keys[1]).toBeTruthy();
    expect(keys[0]).not.toBe(keys[1]);
  });
});

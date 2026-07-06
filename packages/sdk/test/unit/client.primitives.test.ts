/**
 * WS10 new primitives (schema-decode `run<T>`, fire-and-forget `submit`, `batch`
 * rollup, HITL approval gate) + WS8 subagent `children()`.
 */
import { describe, expect, it, beforeEach, afterEach } from "vitest";
import { Aex, ChildRunHandle, SessionHandle } from "../../src/index.js";
import type { AexEvent, JsonValue, WebSocketLike } from "@aexhq/contracts";

function evt(sequence: number, type: AexEvent["type"], data: Record<string, JsonValue>): AexEvent {
  return {
    specversion: "1.0",
    id: `run-1:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: "run-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

// Events every stubbed coordinator socket emits for a turn. Tests mutate this.
let SOCKET_EVENTS: AexEvent[] = [
  evt(1024, "TEXT_MESSAGE_CONTENT", { text: "ok", messageId: "m1" }),
  evt(1025, "CUSTOM", { name: "aex.session.idle", value: { turnSeq: 1 } })
];

class AutoWS implements WebSocketLike {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};
  constructor(url: string) {
    this.url = url;
    queueMicrotask(() => this.#emit("open", {}));
    setTimeout(() => {
      for (const e of SOCKET_EVENTS) this.#emit("message", { data: JSON.stringify(e) });
    }, 0);
  }
  addEventListener(type: string, cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }
  close(): void {
    this.#emit("close", {});
  }
  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}

interface Env {
  readonly client: Aex;
  readonly bodies: Record<string, unknown>[];
  readonly headers: Array<Record<string, string>>;
  readonly urls: string[];
}

const RealWebSocket = globalThis.WebSocket;

function makeEnv(session: Record<string, unknown> = { id: "run-1", status: "idle", turnSeq: 1, costUsd: 0.001, costTelemetry: { providerUsage: [{ totalTokens: 5 }] } }): Env {
  const bodies: Record<string, unknown>[] = [];
  const headers: Array<Record<string, string>> = [];
  const urls: string[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    urls.push(`${(init?.method ?? "GET").toString()} ${url}`);
    headers.push(headersRecord(init?.headers));
    if (typeof init?.body === "string") bodies.push(JSON.parse(init.body) as Record<string, unknown>);
    const json = (b: unknown, status = 200): Response =>
      new Response(JSON.stringify(b), { status, headers: { "content-type": "application/json" } });
    if (url.endsWith("/events/ticket")) return json({ wsUrl: "wss://ev.test/run-1", ticket: "t", expiresAtMs: 1 });
    if (/\/api\/(sessions|runs)\/[^/]+\/outputs$/.test(url)) return json({ outputs: [] });
    if (url.endsWith("/api/sessions/run-1/messages")) {
      return json({ session: { id: "run-1", status: "running", turnSeq: 1 }, turn: { sessionId: "run-1", turnSeq: 1 }, eventCursor: 1024 });
    }
    if (url.endsWith("/api/sessions/run-1/request-approval")) return json({ session: { id: "run-1", status: "awaiting_approval" } });
    if (url.endsWith("/api/sessions/run-1/approve")) return json({ session: { id: "run-1", status: "running" } });
    if (url.endsWith("/api/sessions/run-1/deny")) return json({ session: { id: "run-1", status: "cancelled" } });
    if (url.endsWith("/api/runs/run-1/children")) {
      return json({ children: [{ id: "child-1", parentRunId: "run-1", status: "succeeded", depth: 1, costUsd: 0 }] });
    }
    if (url.endsWith("/api/sessions/run-1")) return json({ session });
    if (url.endsWith("/api/sessions")) return json({ session: { id: "run-1", status: "idle", turnSeq: 0 } }, 201);
    return json({});
  };
  return { client: new Aex({ apiKey: "tk", baseUrl: "https://x", fetch }), bodies, headers, urls };
}

function headersRecord(input: HeadersInit | undefined): Record<string, string> {
  if (!input) return {};
  if (input instanceof Headers) {
    return Object.fromEntries(input.entries());
  }
  if (Array.isArray(input)) {
    return Object.fromEntries(input.map(([key, value]) => [key.toLowerCase(), value]));
  }
  return Object.fromEntries(Object.entries(input).map(([key, value]) => [key.toLowerCase(), value]));
}

beforeEach(() => {
  SOCKET_EVENTS = [
    evt(1024, "TEXT_MESSAGE_CONTENT", { text: "ok", messageId: "m1" }),
    evt(1025, "CUSTOM", { name: "aex.session.idle", value: { turnSeq: 1 } })
  ];
  let opened = 0;
  (globalThis as { WebSocket: unknown }).WebSocket = class extends AutoWS {
    constructor(url: string) {
      super(url);
      opened += 1;
    }
  };
  (globalThis as { __wsOpened?: () => number }).__wsOpened = () => opened;
});
afterEach(() => {
  (globalThis as { WebSocket: unknown }).WebSocket = RealWebSocket;
});

describe("run<T> — typed schema-decode outcome (WS10)", () => {
  it("returns a decoded value from an aex.result.decoded event", async () => {
    SOCKET_EVENTS = [
      evt(1024, "TEXT_MESSAGE_CONTENT", { text: "{...}", messageId: "m1" }),
      evt(1025, "CUSTOM", { name: "aex.result.decoded", value: { value: { answer: 42 } } }),
      evt(1026, "CUSTOM", { name: "aex.session.idle", value: { turnSeq: 1 } })
    ];
    const { client } = makeEnv();
    const result = await client.run<{ answer: number }>({
      model: "claude-haiku-4-5",
      message: "give me the number",
      apiKeys: { anthropic: "sk-ant" },
      responseFormat: { kind: "json_schema", schema: { type: "object" } }
    });
    expect(result.outcome).toEqual({ kind: "decoded", value: { answer: 42 } });
  });

  it("returns a typed refusal from an aex.result.refused event", async () => {
    SOCKET_EVENTS = [
      evt(1025, "CUSTOM", { name: "aex.result.refused", value: { reason: "schema_violation", detail: "bad json" } }),
      evt(1026, "CUSTOM", { name: "aex.session.idle", value: { turnSeq: 1 } })
    ];
    const { client } = makeEnv();
    const result = await client.run({
      model: "claude-haiku-4-5",
      message: "give me the number",
      apiKeys: { anthropic: "sk-ant" },
      responseFormat: { kind: "json_schema", schema: { type: "object" } }
    });
    expect(result.outcome).toEqual({ kind: "refused", reason: "schema_violation", detail: "bad json" });
  });
});

describe("aex.submit — fire-and-forget (WS10)", () => {
  it("resolves with {runId, session} WITHOUT opening a coordinator socket", async () => {
    const { client, bodies, headers, urls } = makeEnv();
    const { runId, session } = await client.submit({
      model: "claude-haiku-4-5",
      message: "go",
      idempotencyKey: "submit-key",
      messageIdempotencyKey: "turn-key",
      apiKeys: { anthropic: "sk-ant" }
    });
    expect(runId).toBe("run-1");
    expect(session).toBeInstanceOf(SessionHandle);
    expect((globalThis as { __wsOpened?: () => number }).__wsOpened!()).toBe(0);
    expect(urls.filter((url) => url.startsWith("POST https://x/api/sessions"))).toEqual([
      "POST https://x/api/sessions",
      "POST https://x/api/sessions/run-1/messages"
    ]);
    expect("input" in bodies[0]!).toBe(false);
    expect(bodies[1]!.input).toBe("go");
    expect(headers[0]!["idempotency-key"]).toBe("submit-key");
    expect(headers[1]!["idempotency-key"]).toBe("turn-key");
  });
});

describe("sessions.get — terminal failure projection", () => {
  it("preserves failed status, outcome, failureClass, and errorMessage", async () => {
    const { client } = makeEnv({
      id: "run-1",
      status: "failed",
      turnSeq: 1,
      lastTurnOutcome: "failed",
      failureClass: "provider-permanent",
      errorMessage: "llm provider rejected the request (HTTP 401) - not retryable",
      costUsd: 0,
      usage: {}
    });

    const session = await client.sessions.get("run-1");

    expect(session.status).toBe("failed");
    expect(session.lastTurnOutcome).toBe("failed");
    expect(session.failureClass).toBe("provider-permanent");
    expect(session.errorMessage).toMatch(/401/);
  });
});

describe("aex.batch — real cost/usage rollup (WS10)", () => {
  it("sums settle-stamped per-item costs and buckets failures", async () => {
    const { client } = makeEnv();
    const result = await client.batch(
      [1, 2, 3].map(() => ({ model: "claude-haiku-4-5" as const, message: "hi", apiKeys: { anthropic: "sk-ant" } })),
      { concurrency: 2 }
    );
    expect(result.results).toHaveLength(3);
    expect(result.okCount).toBe(3);
    expect(result.failed).toHaveLength(0);
    expect(result.totalCostUsd).toBeCloseTo(0.003, 6);
    expect(result.totalUsage.totalTokens).toBe(15);
  });
});

describe("HITL approval gate (WS10)", () => {
  it("requestApproval → awaiting_approval; approve → running; deny → cancelled", async () => {
    const { client } = makeEnv();
    const session = await client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    await session.requestApproval();
    expect(session.record.status).toBe("awaiting_approval");
    await session.approve();
    expect(session.record.status).toBe("running");
    await session.deny();
    expect(session.record.status).toBe("cancelled");
  });

  it("threads a declarative approvalGate into the submission wire body", async () => {
    const { client, bodies } = makeEnv();
    await client.openSession({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" },
      approvalGate: { tools: ["delete_file"] }
    });
    const create = bodies.find((b) => b.submission);
    expect((create!.submission as Record<string, unknown>).approvalGate).toEqual({ tools: ["delete_file"] });
  });
});

describe("subagent children (WS8)", () => {
  it("session.children() returns resolvable ChildRunHandles backed by the run facade", async () => {
    const { client } = makeEnv();
    const session = await client.openSession({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    const children = await session.children();
    expect(children).toHaveLength(1);
    const child = children[0]!;
    expect(child).toBeInstanceOf(ChildRunHandle);
    expect(child.id).toBe("child-1");
    expect(child.parentRunId).toBe("run-1");
    expect(child.depth).toBe(1);
    expect(child.status).toBe("succeeded");
    // Resolves through the RUN facade (/runs/:id/outputs), not openSession.
    const outputs = await child.outputs().list();
    expect(outputs).toEqual([]);
  });
});

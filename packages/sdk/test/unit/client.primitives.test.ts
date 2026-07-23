/**
 * Schema decode, HITL approval gates, and subagent children.
 */
import { describe, expect, it, beforeEach, afterEach } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex } from "../../src/index.js";
import { ChildSessionHandle, SessionHandle } from "../../src/client.js";
import type { AexEvent, JsonValue, WebSocketLike } from "@aexhq/contracts";

function evt(sequence: number, type: AexEvent["type"], data: Record<string, JsonValue>): AexEvent {
  return {
    specversion: "1.0",
    id: `session-1:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: "session-1",
    threadId: "session-1",
    runId: "run-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

// Events every stubbed coordinator socket emits for a turn. Tests mutate this.
let SOCKET_EVENTS: AexEvent[] = [
  evt(1024, "TEXT_MESSAGE_CONTENT", { text: "ok", messageId: "m1" }),
  evt(1025, "RUN_FINISHED", { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } })
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

function makeEnv(session: Record<string, unknown> = {
  id: "session-1",
  status: "idle",
  acceptsMessages: true,
  lastRun: { sessionId: "session-1", runId: "run-1", turnSeq: 1, phase: "finished", outcome: "succeeded" },
  costUsd: 0.001,
  costTelemetry: { providerUsage: [{ totalTokens: 5 }] }
}): Env {
  const bodies: Record<string, unknown>[] = [];
  const headers: Array<Record<string, string>> = [];
  const urls: string[] = [];
  const fetch: FetchLike = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    urls.push(`${(init?.method ?? "GET").toString()} ${url}`);
    headers.push(headersRecord(init?.headers));
    if (typeof init?.body === "string") bodies.push(JSON.parse(init.body) as Record<string, unknown>);
    const json = (b: unknown, status = 200): Response =>
      new Response(JSON.stringify(b), { status, headers: { "content-type": "application/json" } });
    if (url.endsWith("/events/ticket")) return json({ wsUrl: "wss://ev.test/session-1", ticket: "t", expiresAtMs: 1 });
    if (/\/api\/sessions\/[^/]+\/files(?:\?|$)/.test(url)) return json({
      revision: { checkpointId: "cp-1", runId: "run-1", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: 1025 },
      files: []
    });
    if (url.endsWith("/api/sessions/session-1/messages")) {
      return json({
        session: { id: "session-1", status: "running", acceptsMessages: false },
        run: { sessionId: "session-1", turnSeq: 1, runId: "run-1", phase: "running", eventCursor: 1024 },
        eventCursor: 1024
      });
    }
    if (url.endsWith("/api/sessions/session-1/request-approval")) return json({ session: { id: "session-1", status: "awaiting_approval", acceptsMessages: false } });
    if (url.endsWith("/api/sessions/session-1/approve")) return json({ session: { id: "session-1", status: "running", acceptsMessages: false } });
    if (url.endsWith("/api/sessions/session-1/deny")) {
      return json({
        session: {
          id: "session-1",
          status: "idle",
          acceptsMessages: true,
          lastRun: { sessionId: "session-1", runId: "run-1", turnSeq: 1, phase: "finished", outcome: "cancelled" }
        }
      });
    }
    if (url.endsWith("/api/sessions/session-1/children")) {
      return json({
        children: [{
          id: "child-1",
          parentSessionId: "session-1",
          status: "idle",
          depth: 1,
          costUsd: 0,
          createdAt: "2026-07-10T00:00:00.000Z",
          updatedAt: "2026-07-10T00:01:00.000Z",
          lastRun: { sessionId: "child-1", runId: "child-run-1", turnSeq: 1, phase: "finished", outcome: "succeeded" }
        }]
      });
    }
    if (url.endsWith("/api/sessions/child-1/children")) return json({ children: [] });
    if (url.endsWith("/api/sessions/child-1/events")) return json({ events: [] });
    if (url.endsWith("/api/sessions/session-1")) return json({ session });
    if (url.endsWith("/api/sessions")) return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }, 201);
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
    evt(1025, "RUN_FINISHED", { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } })
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

describe("start<T> — typed schema-decode outcome (WS10)", () => {
  it("returns a decoded value from an aex.result.decoded event", async () => {
    SOCKET_EVENTS = [
      evt(1024, "TEXT_MESSAGE_CONTENT", { text: "{...}", messageId: "m1" }),
      evt(1025, "CUSTOM", { name: "aex.result.decoded", value: { value: { answer: 42 } } }),
      evt(1026, "RUN_FINISHED", { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } })
    ];
    const { client } = makeEnv();
    const result = await client.start<{ answer: number }>({
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
      evt(1026, "RUN_FINISHED", { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } })
    ];
    const { client } = makeEnv();
    const result = await client.start({
      model: "claude-haiku-4-5",
      message: "give me the number",
      apiKeys: { anthropic: "sk-ant" },
      responseFormat: { kind: "json_schema", schema: { type: "object" } }
    });
    expect(result.outcome).toEqual({ kind: "refused", reason: "schema_violation", detail: "bad json" });
  });
});

describe("sessions.get — failed run projection", () => {
  it("keeps the resumable session in error and preserves the run outcome and failure", async () => {
    const { client } = makeEnv({
      id: "session-1",
      status: "error",
      acceptsMessages: true,
      lastRun: { sessionId: "session-1", runId: "run-1", turnSeq: 1, phase: "error", outcome: "failed" },
      failureClass: "provider-permanent",
      errorMessage: "llm provider rejected the request (HTTP 401) - not retryable",
      costUsd: 0,
      usage: {}
    });

    const session = await client.sessions.get("session-1");

    expect(session.status).toBe("error");
    expect(session.lastRun?.outcome).toBe("failed");
    expect(session.failureClass).toBe("provider-permanent");
    expect(session.errorMessage).toMatch(/401/);
  });
});

describe("HITL approval gate (WS10)", () => {
  it("requestApproval → awaiting_approval; approve → running; deny → idle with a cancelled run", async () => {
    const { client } = makeEnv();
    const session = await client.sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    await session.requestApproval();
    expect(session.record.status).toBe("awaiting_approval");
    await session.approve();
    expect(session.record.status).toBe("running");
    await session.deny();
    expect(session.record.status).toBe("idle");
    expect(session.record.lastRun?.outcome).toBe("cancelled");
  });

  it("threads a declarative approvalGate into the submission wire body", async () => {
    const { client, bodies } = makeEnv();
    await client.sessions.create({
      model: "claude-haiku-4-5",
      apiKeys: { anthropic: "sk-ant" },
      approvalGate: { tools: ["delete_file"] }
    });
    const create = bodies.find((b) => b.submission);
    expect((create!.submission as Record<string, unknown>).approvalGate).toEqual({ tools: ["delete_file"] });
  });
});

describe("subagent children (WS8)", () => {
  it("session.children() returns read-only handles whose observation resources work", async () => {
    const { client } = makeEnv();
    const session = await client.sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
    const children = await session.children();
    expect(children).toHaveLength(1);
    const child = children[0]!;
    expect(child).toBeInstanceOf(ChildSessionHandle);
    expect(child.id).toBe("child-1");
    expect(child.parentSessionId).toBe("session-1");
    expect(child.depth).toBe(1);
    expect(child.status).toBe("idle");
    expect(child.ref.lastRun?.outcome).toBe("succeeded");
    expect("get" in child).toBe(false);
    expect("cancel" in child).toBe(false);
    const noGet: "get" extends keyof ChildSessionHandle ? true : false = false;
    const noCancel: "cancel" extends keyof ChildSessionHandle ? true : false = false;
    expect({ noGet, noCancel }).toEqual({ noGet: false, noCancel: false });
    expect(await child.events.list()).toEqual([]);
    const files = await child.files.list();
    expect(files.files).toEqual([]);
    expect(await child.children()).toEqual([]);
  });
});

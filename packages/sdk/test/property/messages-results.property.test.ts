import fc, { type Arbitrary } from "fast-check";
import { describe, expect, it, setDefaultTimeout } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import type {
  AexEvent,
  JsonValue,
  TurnTrace,
  SessionMessage,
  SessionMessageSender
} from "@aexhq/contracts";
import { asAexEventViews } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";
import { Aex, type Message, type SessionResult, type SessionRunResult } from "../../src/index.js";

interface CapturedRequest {
  readonly method: string;
  readonly pathname: string;
  readonly search: string;
  readonly body: unknown;
}

type FuzzEvent = AexEvent & {
  readonly seq?: number;
  readonly recordedAt?: string;
};

type ExpectedMessage = Pick<Message, "id" | "sender" | "text" | "timestamp" | "turnSeq" | "sequence">;

interface TextAtom {
  readonly kind: "text";
  readonly text: string;
  readonly messageId?: string;
  readonly turnSeq?: number;
}

interface ToolStartAtom {
  readonly kind: "tool-start";
  readonly toolId: string;
  readonly name: string;
  readonly args: Readonly<Record<string, JsonValue>>;
  readonly messageId?: string;
}

interface ToolResultAtom {
  readonly kind: "tool-result";
  readonly toolId: string;
  readonly isError: boolean;
  readonly content: JsonValue;
}

type StreamAtom = TextAtom | ToolStartAtom | ToolResultAtom;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function statusResponse(status: number): Response {
  return new Response(JSON.stringify({ error: `missing ${status}` }), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function urlOf(input: string | URL | Request): URL {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  return new URL(raw);
}

function methodOf(init: RequestInit | undefined): string {
  return (init?.method ?? "GET").toString().toUpperCase();
}

function bodyOf(init: RequestInit | undefined): unknown {
  if (typeof init?.body !== "string") return init?.body;
  return JSON.parse(init.body) as unknown;
}

function recordCall(input: string | URL | Request, init: RequestInit | undefined): CapturedRequest {
  const url = urlOf(input);
  return {
    method: methodOf(init),
    pathname: url.pathname,
    search: url.search,
    body: bodyOf(init)
  };
}

function sessionRecord(status = "idle") {
  return {
    id: "sess_1",
    status,
    acceptsMessages: status === "idle",
    lastRun: {
      sessionId: "sess_1",
      runId: "run_1",
      turnSeq: 1,
      phase: "finished",
      outcome: "succeeded"
    },
    costUsd: 0,
    costTelemetry: { providerUsage: [] }
  };
}

function captureMessagesClient(messages: readonly SessionMessage[]): {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
} {
  const calls: CapturedRequest[] = [];
  const fetchImpl: FetchLike = async (input, init) => {
    const call = recordCall(input, init);
    calls.push(call);
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1") {
      return json({ session: sessionRecord() });
    }
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1/messages") {
      return json({ messages });
    }
    throw new Error(`unexpected SDK request: ${call.method} ${call.pathname}${call.search}`);
  };
  return {
    client: new Aex({ apiKey: "tkn_property", baseUrl: "https://api.example.test", fetch: fetchImpl, retry: false }),
    calls
  };
}

function captureMissingMessagesClient(args: {
  readonly missingStatus: number;
  readonly eventPages: readonly (readonly FuzzEvent[])[];
}): {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
} {
  const calls: CapturedRequest[] = [];
  const fetchImpl: FetchLike = async (input, init) => {
    const call = recordCall(input, init);
    calls.push(call);
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1") {
      return json({ session: sessionRecord() });
    }
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1/messages") {
      return statusResponse(args.missingStatus);
    }
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1/events") {
      const cursor = new URLSearchParams(call.search).get("cursor");
      const index = cursor === null ? 0 : Number(cursor);
      const events = args.eventPages[index] ?? [];
      const nextCursor = index + 1 < args.eventPages.length ? String(index + 1) : undefined;
      return json({
        events,
        ...(nextCursor !== undefined ? { nextCursor } : {})
      });
    }
    throw new Error(`unexpected SDK request: ${call.method} ${call.pathname}${call.search}`);
  };
  return {
    client: new Aex({ apiKey: "tkn_property", baseUrl: "https://api.example.test", fetch: fetchImpl, retry: false }),
    calls
  };
}

function captureSessionClient(firstSeq: number): {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
} {
  const calls: CapturedRequest[] = [];
  const sockets: FakeWebSocket[] = [];
  const fetchImpl: FetchLike = async (input, init) => {
    const call = recordCall(input, init);
    calls.push(call);
    if (call.method === "POST" && call.pathname === "/api/sessions") {
      return json({ session: { id: "sess_1", status: "idle", acceptsMessages: true } });
    }
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1") {
      return json({ session: sessionRecord("idle") });
    }
    if (call.method === "POST" && call.pathname === "/api/sessions/sess_1/messages") {
      return json({
        session: sessionRecord("running"),
        run: { sessionId: "sess_1", turnSeq: 1, runId: "run_1", phase: "running", eventCursor: firstSeq },
        eventCursor: firstSeq
      });
    }
    if (call.method === "POST" && call.pathname === "/api/sessions/sess_1/events/ticket") {
      return json({
        wsUrl: "wss://events.example.test/sessions/sess_1",
        ticket: "ticket",
        expiresAtMs: Date.now() + 60_000
      });
    }
    if (call.method === "GET" && call.pathname === "/api/sessions/sess_1/files") {
      return json({
        revision: { checkpointId: "cp_1", runId: "run_1", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: firstSeq },
        files: []
      });
    }
    throw new Error(`unexpected SDK request: ${call.method} ${call.pathname}${call.search}`);
  };
  return {
    client: new Aex({ apiKey: "tkn_property", baseUrl: "https://api.example.test", fetch: fetchImpl, retry: false }),
    calls,
    sockets,
    webSocketFactory: (url: string) => {
      const socket = new FakeWebSocket(url);
      sockets.push(socket);
      return socket;
    }
  };
}

async function flush(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

async function waitForSocket(sockets: readonly FakeWebSocket[]): Promise<FakeWebSocket> {
  for (let i = 0; i < 20; i++) {
    if (sockets[0] !== undefined) return sockets[0];
    await flush();
  }
  throw new Error("SDK did not open the coordinator WebSocket");
}

function isoAt(ms: number): string {
  return new Date(ms).toISOString();
}

function aexEvent(args: {
  readonly sessionId: string;
  readonly sequence: number;
  readonly time: string;
  readonly source: AexEvent["source"];
  readonly type: AexEvent["type"];
  readonly data: Readonly<Record<string, JsonValue>>;
}): FuzzEvent {
  return {
    specversion: "1.0",
    id: `${args.sessionId}:${args.sequence}`,
    source: args.source,
    type: args.type,
    subject: args.sessionId,
    threadId: args.sessionId,
    runId: "run_1",
    time: args.time,
    sequence: args.sequence,
    seq: args.sequence,
    recordedAt: args.time,
    data: args.data
  };
}

function terminalEvent(sequence: number, time: string): FuzzEvent {
  return aexEvent({
    sessionId: "sess_1",
    sequence,
    time,
    source: "runtime",
    type: "RUN_FINISHED",
    data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
  });
}

function streamEvents(atoms: readonly StreamAtom[], firstSeq: number, baseMs: number): readonly FuzzEvent[] {
  return atoms.map((atom, index) => {
    const sequence = firstSeq + index;
    const time = isoAt(baseMs + index * 137);
    if (atom.kind === "text") {
      return aexEvent({
        sessionId: "sess_1",
        sequence,
        time,
        source: "agent",
        type: "TEXT_MESSAGE_CONTENT",
        data: {
          text: atom.text,
          ...(atom.messageId !== undefined ? { messageId: atom.messageId } : {}),
          ...(atom.turnSeq !== undefined ? { turnSeq: atom.turnSeq } : {})
        }
      });
    }
    if (atom.kind === "tool-start") {
      return aexEvent({
        sessionId: "sess_1",
        sequence,
        time,
        source: "agent",
        type: "TOOL_CALL_START",
        data: {
          id: atom.toolId,
          name: atom.name,
          arguments: atom.args,
          ...(atom.messageId !== undefined ? { messageId: atom.messageId } : {})
        }
      });
    }
    return aexEvent({
      sessionId: "sess_1",
      sequence,
      time,
      source: "agent",
      type: "TOOL_CALL_RESULT",
      data: {
        id: atom.toolId,
        isError: atom.isError,
        content: atom.content
      }
    });
  });
}

function eventPages(events: readonly FuzzEvent[], splitAt: number): readonly (readonly FuzzEvent[])[] {
  if (events.length === 0) return [[]];
  const split = Math.max(0, Math.min(events.length, splitAt));
  if (split === 0 || split === events.length) return [events];
  return [events.slice(0, split), events.slice(split)];
}

function wireMessage(message: SessionMessage): ExpectedMessage {
  return {
    id: message.id,
    sender: message.sender,
    text: message.text,
    ...(message.timestamp !== undefined ? { timestamp: message.timestamp } : {}),
    ...(message.turnSeq !== undefined ? { turnSeq: message.turnSeq } : {}),
    ...(message.sequence !== undefined ? { sequence: message.sequence } : {})
  };
}

function textData(event: FuzzEvent): Record<string, unknown> {
  return event.data && typeof event.data === "object" && !Array.isArray(event.data)
    ? event.data as Record<string, unknown>
    : {};
}

function projectedMessages(events: readonly FuzzEvent[]): readonly ExpectedMessage[] {
  const out: ExpectedMessage[] = [];
  const byMessageId = new Map<string, number>();
  for (let i = 0; i < events.length; i++) {
    const event = events[i]!;
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = textData(event);
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const messageId = typeof data.messageId === "string" && data.messageId ? data.messageId : undefined;
    const timestamp = event.time ?? event.recordedAt;
    const sequence = event.sequence ?? event.seq;
    const turnSeq = typeof data.turnSeq === "number" ? data.turnSeq : undefined;
    if (messageId !== undefined) {
      const existing = byMessageId.get(messageId);
      if (existing !== undefined) {
        const current = out[existing]!;
        out[existing] = {
          ...current,
          text: `${current.text}${text}`,
          ...(timestamp !== undefined ? { timestamp } : {}),
          ...(sequence !== undefined ? { sequence } : {}),
          ...(turnSeq !== undefined ? { turnSeq } : {})
        };
        continue;
      }
      byMessageId.set(messageId, out.length);
    }
    out.push({
      id: messageId ?? event.id,
      sender: "assistant",
      text,
      ...(timestamp !== undefined ? { timestamp } : {}),
      ...(sequence !== undefined ? { sequence } : {}),
      ...(turnSeq !== undefined ? { turnSeq } : {})
    });
  }
  return out;
}

function assistantText(events: readonly FuzzEvent[]): string {
  return events
    .filter((event) => event.type === "TEXT_MESSAGE_CONTENT")
    .map((event) => textData(event).text)
    .filter((text): text is string => typeof text === "string")
    .join("");
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function durationMs(start: string | undefined, end: string | undefined): number | undefined {
  if (start === undefined || end === undefined) return undefined;
  const delta = Date.parse(end) - Date.parse(start);
  return Number.isFinite(delta) && delta >= 0 ? delta : undefined;
}

function expectedTrace(events: readonly FuzzEvent[]): TurnTrace {
  const text: TurnTrace["text"] = events
    .filter((event) => event.type === "TEXT_MESSAGE_CONTENT")
    .map((event) => {
      const data = textData(event);
      const chunk = typeof data.text === "string" ? data.text : undefined;
      if (chunk === undefined) return undefined;
      return {
        text: chunk,
        ...(typeof data.messageId === "string" ? { messageId: data.messageId } : {}),
        ...(typeof event.seq === "number" ? { seq: event.seq } : {}),
        ...(typeof event.recordedAt === "string" ? { recordedAt: event.recordedAt } : {})
      };
    })
    .filter((entry): entry is TurnTrace["text"][number] => entry !== undefined);
  const order: string[] = [];
  const byId = new Map<string, TurnTrace["toolCalls"][number]>();
  for (const event of events) {
    const data = textData(event);
    if (event.type === "TOOL_CALL_START") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const trace: TurnTrace["toolCalls"][number] = {
        id,
        name: typeof data.name === "string" ? data.name : "",
        args: asRecord(data.arguments),
        ...(typeof data.messageId === "string" ? { messageId: data.messageId } : {}),
        ...(typeof event.seq === "number" ? { startSeq: event.seq } : {}),
        ...(typeof event.recordedAt === "string" ? { startedAt: event.recordedAt } : {})
      };
      if (!byId.has(id)) order.push(id);
      byId.set(id, trace);
      continue;
    }
    if (event.type === "TOOL_CALL_RESULT") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const result: NonNullable<TurnTrace["toolCalls"][number]["result"]> = {
        isError: data.isError === true,
        content: data.content ?? null,
        ...(typeof event.seq === "number" ? { seq: event.seq } : {}),
        ...(typeof event.recordedAt === "string" ? { recordedAt: event.recordedAt } : {})
      };
      const previous = byId.get(id);
      const duration = durationMs(previous?.startedAt, result.recordedAt);
      const next: TurnTrace["toolCalls"][number] = previous === undefined
        ? { id, name: "", args: {}, result }
        : {
          ...previous,
          result,
          ...(duration !== undefined
            ? { durationMs: duration }
            : {})
        };
      if (previous === undefined) order.push(id);
      byId.set(id, next);
    }
  }
  return {
    text,
    toolCalls: order.map((id) => byId.get(id)!),
    usage: {}
  };
}

async function collectSessionSend(events: readonly FuzzEvent[], firstSeq: number): Promise<SessionRunResult> {
  const harness = captureSessionClient(firstSeq);
  const session = await harness.client.sessions.open("sess_1");
  const promise = session.messages.send("continue", {
    webSocketFactory: harness.webSocketFactory,
    idleTimeoutMs: 0,
    pingIntervalMs: 0
  }).finished();
  const socket = await waitForSocket(harness.sockets);
  for (const event of events) socket.message(event);
  return promise;
}

async function collectRun(events: readonly FuzzEvent[], firstSeq: number): Promise<SessionResult> {
  const harness = captureSessionClient(firstSeq);
  const promise = harness.client.start(
    {
      model: "anthropic/claude-haiku-4-5",
      message: "start",
    },
    {
      webSocketFactory: harness.webSocketFactory,
      idleTimeoutMs: 0,
      pingIntervalMs: 0
    }
  );
  const socket = await waitForSocket(harness.sockets);
  for (const event of events) socket.message(event);
  return promise;
}

const idString = fc.string({ minLength: 1, maxLength: 16 }).filter((value) => value.trim().length > 0);
const smallText = fc.string({ maxLength: 24 });
const isoString = fc.integer({ min: 0, max: 4_102_444_800_000 }).map(isoAt);
// The projection emits only these two; the wire schema is strict about it.
const sender = fc.constantFrom<SessionMessageSender>("user", "assistant");
const jsonScalar: Arbitrary<JsonValue> = fc.oneof(
  fc.string({ maxLength: 16 }),
  fc.integer(),
  fc.boolean(),
  fc.constant(null)
);
const jsonValue: Arbitrary<JsonValue> = fc.oneof(
  jsonScalar,
  fc.array(jsonScalar, { maxLength: 3 }),
  fc.dictionary(idString, jsonScalar, { maxKeys: 3 })
);
const jsonObject: Arbitrary<Record<string, JsonValue>> = fc.dictionary(idString, jsonValue, { maxKeys: 3 });

const sessionMessage = fc.record({
  id: idString,
  sender,
  text: smallText,
  // Required on the wire: the projection emits all three on every message.
  // Generating them as absent modelled a response the server does not produce.
  timestamp: isoString,
  sequence: fc.integer({ min: 0, max: 1_000_000 }),
  content: fc.array(jsonValue, { maxLength: 3 }),
  turnSeq: fc.option(fc.integer({ min: 0, max: 200 }), { nil: undefined }),
  messageId: fc.option(idString, { nil: undefined })
}).map((raw): SessionMessage => ({
  id: raw.id,
  sender: raw.sender,
  text: raw.text,
  timestamp: raw.timestamp,
  sequence: raw.sequence,
  content: raw.content,
  ...(raw.turnSeq !== undefined ? { turnSeq: raw.turnSeq } : {}),
  ...(raw.messageId !== undefined ? { messageId: raw.messageId } : {})
}));

const messageId = fc.constantFrom("msg_0", "msg_1", "msg_2", "msg_3");
const maybeMessageId = fc.option(messageId, { nil: undefined });
const toolId = fc.constantFrom("tool_0", "tool_1", "tool_2", "tool_3");

const textAtom = fc.record({
  kind: fc.constant("text" as const),
  text: smallText,
  messageId: maybeMessageId,
  turnSeq: fc.option(fc.integer({ min: 0, max: 10 }), { nil: undefined })
}).map((raw): TextAtom => ({
  kind: "text",
  text: raw.text,
  ...(raw.messageId !== undefined ? { messageId: raw.messageId } : {}),
  ...(raw.turnSeq !== undefined ? { turnSeq: raw.turnSeq } : {})
}));

const toolStartAtom = fc.record({
  kind: fc.constant("tool-start" as const),
  toolId,
  name: idString,
  args: jsonObject,
  messageId: maybeMessageId
}).map((raw): ToolStartAtom => ({
  kind: "tool-start",
  toolId: raw.toolId,
  name: raw.name,
  args: raw.args,
  ...(raw.messageId !== undefined ? { messageId: raw.messageId } : {})
}));

const toolResultAtom = fc.record({
  kind: fc.constant("tool-result" as const),
  toolId,
  isError: fc.boolean(),
  content: jsonValue
}).map((raw): ToolResultAtom => ({
  kind: "tool-result",
  toolId: raw.toolId,
  isError: raw.isError,
  content: raw.content
}));

const streamAtom: Arbitrary<StreamAtom> = fc.oneof(textAtom, toolStartAtom, toolResultAtom);

function textChunk(text: string, id: string, turnSeq: number | undefined): TextAtom {
  return {
    kind: "text",
    text,
    messageId: id,
    ...(turnSeq !== undefined ? { turnSeq } : {})
  };
}

const repeatedMessageCase = fc.record({
  before: fc.array(streamAtom, { maxLength: 4 }),
  between: fc.array(streamAtom, { maxLength: 4 }),
  after: fc.array(streamAtom, { maxLength: 4 }),
  repeatedId: messageId,
  firstText: smallText,
  secondText: smallText,
  firstTurnSeq: fc.option(fc.integer({ min: 0, max: 10 }), { nil: undefined }),
  secondTurnSeq: fc.option(fc.integer({ min: 0, max: 10 }), { nil: undefined }),
  firstSeq: fc.integer({ min: 1, max: 50_000 }),
  baseMs: fc.integer({ min: 0, max: 4_102_444_800_000 })
}).map((raw) => {
  const atoms: readonly StreamAtom[] = [
    ...raw.before,
    textChunk(raw.firstText, raw.repeatedId, raw.firstTurnSeq),
    ...raw.between,
    textChunk(raw.secondText, raw.repeatedId, raw.secondTurnSeq),
    ...raw.after
  ];
  return {
    firstSeq: raw.firstSeq,
    events: streamEvents(atoms, raw.firstSeq, raw.baseMs)
  };
});

const resultStreamCase = fc.record({
  before: fc.array(streamAtom, { maxLength: 4 }),
  after: fc.array(streamAtom, { maxLength: 4 }),
  requiredText: textAtom,
  requiredToolId: toolId,
  requiredToolName: idString,
  requiredToolArgs: jsonObject,
  requiredToolMessageId: maybeMessageId,
  requiredToolIsError: fc.boolean(),
  requiredToolContent: jsonValue,
  firstSeq: fc.integer({ min: 1, max: 50_000 }),
  baseMs: fc.integer({ min: 0, max: 4_102_444_800_000 })
}).map((raw) => {
  const requiredStart: ToolStartAtom = {
    kind: "tool-start",
    toolId: raw.requiredToolId,
    name: raw.requiredToolName,
    args: raw.requiredToolArgs,
    ...(raw.requiredToolMessageId !== undefined ? { messageId: raw.requiredToolMessageId } : {})
  };
  const requiredResult: ToolResultAtom = {
    kind: "tool-result",
    toolId: raw.requiredToolId,
    isError: raw.requiredToolIsError,
    content: raw.requiredToolContent
  };
  const atoms: readonly StreamAtom[] = [
    ...raw.before,
    raw.requiredText,
    requiredStart,
    requiredResult,
    ...raw.after
  ];
  const firstSeq = raw.firstSeq;
  const baseMs = raw.baseMs;
  const events = streamEvents(atoms, firstSeq, baseMs);
  const terminal = terminalEvent(firstSeq + events.length, isoAt(baseMs + events.length * 137));
  return { firstSeq, events: [...events, terminal] as readonly FuzzEvent[] };
});

// bun's describe() takes no options object; this file-wide default replaces the
// former vitest describe-level { timeout: 30_000 } (single suite spans the file).
setDefaultTimeout(30_000);

describe("slim session messages/results properties", () => {
  it("returns intuitive Message objects from the session messages endpoint", async () => {
    await fc.assert(
      fc.asyncProperty(fc.array(sessionMessage, { maxLength: 30 }), async (wireMessages) => {
        const { client, calls } = captureMessagesClient(wireMessages);
        const session = await client.sessions.open("sess_1");
        const expected = wireMessages.map(wireMessage);

        await expect(session.messages.list()).resolves.toEqual(expected);
        await expect(session.messages.first()).resolves.toEqual(expected[0]);
        await expect(session.messages.last()).resolves.toEqual(expected.at(-1));
        await expect(session.messages.list()).resolves.toEqual(expected);

        const messageCalls = calls.filter((call) => call.pathname === "/api/sessions/sess_1/messages");
        expect(messageCalls.map((call) => call.method)).toEqual(["GET", "GET", "GET", "GET"]);
      }),
      { numRuns: 180 }
    );
  });

  it("keeps run and session-send results consistent for text, messages, events, and trace", async () => {
    await fc.assert(
      fc.asyncProperty(resultStreamCase, async (generated) => {
        const sessionSendResult = await collectSessionSend(generated.events, generated.firstSeq);
        const runResult = await collectRun(generated.events, generated.firstSeq);
        const expectedEvents = generated.events;
        const expectedText = assistantText(expectedEvents);
        const expectedMessages = projectedMessages(expectedEvents);

        expect(sessionSendResult.text).toBe(expectedText);
        expect(runResult.text).toBe(expectedText);
        expect(sessionSendResult.messages).toEqual(expectedMessages);
        expect(runResult.messages).toEqual(expectedMessages);
        // The SDK yields prototype-carrying views over the same wire events;
        // wrap the expectation identically so the types (and shapes) agree.
        expect(sessionSendResult.events).toEqual(asAexEventViews(expectedEvents));
        expect(runResult.events).toEqual(asAexEventViews(expectedEvents));
        expect(runResult.trace).toEqual(expectedTrace(expectedEvents));
      }),
      { numRuns: 120 }
    );
  });
});

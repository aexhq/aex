import fc, { type Arbitrary } from "fast-check";
import { describe, expect, it } from "vitest";
import { Aex, type Message, type SessionResult, type SessionInput, type SessionStartOptions } from "../../src/index.js";
import type { AexEvent, JsonValue } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";

const BASE_URL = "https://api.example.test";
const SESSION_ID = "sess_property";
const MODEL = "claude-haiku-4-5";
const API_KEYS = { anthropic: "sk-ant-fuzztestkey0123456789" } as const;
const TEXT_SEQUENCE_START = 1024;
const PROPERTY_RUNS = { numRuns: 150 };
const EVENT_PROPERTY_RUNS = { numRuns: 100 };
const EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

interface TextEventSpec {
  readonly text: string;
  readonly messageId?: string;
}

interface Harness {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
}

function headersToObject(headers: HeadersInit | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (headers === undefined) return out;
  if (headers instanceof Headers) {
    for (const [key, value] of headers.entries()) out[key.toLowerCase()] = value;
    return out;
  }
  if (Array.isArray(headers)) {
    for (const [key, value] of headers) out[String(key).toLowerCase()] = String(value);
    return out;
  }
  for (const [key, value] of Object.entries(headers)) out[key.toLowerCase()] = String(value);
  return out;
}

function requestUrl(input: string | URL | Request): string {
  return typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
}

function requestBody(body: BodyInit | null | undefined): unknown {
  if (typeof body === "string") return JSON.parse(body) as unknown;
  return body;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function makeHarness(): Harness {
  const calls: CapturedRequest[] = [];
  const sockets: FakeWebSocket[] = [];

  const fetchImpl: typeof globalThis.fetch = async (input, init) => {
    const url = requestUrl(input);
    const method = String(init?.method ?? "GET").toUpperCase();
    calls.push({
      url,
      method,
      headers: headersToObject(init?.headers),
      body: requestBody(init?.body)
    });

    if (method === "POST" && url.endsWith("/api/sessions")) {
      return json({ session: { id: SESSION_ID, status: "idle", acceptsMessages: true } });
    }
    if (method === "POST" && url.endsWith(`/api/sessions/${SESSION_ID}/messages`)) {
      return json({
        session: { id: SESSION_ID, status: "running", acceptsMessages: false },
        run: { sessionId: SESSION_ID, turnSeq: 1, runId: "run_property", phase: "running", eventCursor: TEXT_SEQUENCE_START },
        eventCursor: TEXT_SEQUENCE_START
      });
    }
    if (method === "POST" && url.endsWith(`/api/sessions/${SESSION_ID}/events/ticket`)) {
      return json({
        wsUrl: `wss://events.example.test/sessions/${SESSION_ID}`,
        ticket: "ticket",
        expiresAtMs: 1
      });
    }
    if (method === "GET" && url.includes(`/api/sessions/${SESSION_ID}/files?checkpointId=cp_property`)) {
      return json({
        revision: { checkpointId: "cp_property", runId: "run_property", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: TEXT_SEQUENCE_START },
        files: [{
          id: "out_property",
          checkpointId: "cp_property",
          filename: "answer.txt",
          sizeBytes: 0,
          sha256: EMPTY_SHA256
        }]
      });
    }
    if (method === "GET" && url.endsWith(`/api/sessions/${SESSION_ID}`)) {
      return json({
        session: {
          id: SESSION_ID,
          status: "idle",
          acceptsMessages: true,
          lastRun: {
            sessionId: SESSION_ID,
            runId: "run_property",
            turnSeq: 1,
            phase: "finished",
            outcome: "succeeded"
          },
          costTelemetry: {
            providerUsage: [{ inputTokens: 3, outputTokens: 5, totalTokens: 8 }]
          },
          costUsd: 0.001
        }
      });
    }
    throw new Error(`unexpected SDK network call in property harness: ${method} ${url}`);
  };

  const webSocketFactory = (url: string): FakeWebSocket => {
    const socket = new FakeWebSocket(url);
    sockets.push(socket);
    return socket;
  };

  return {
    client: new Aex({ apiKey: "test-token", baseUrl: BASE_URL, fetch: fetchImpl }),
    calls,
    sockets,
    webSocketFactory
  };
}

function idempotencyKey(call: CapturedRequest): string | undefined {
  return call.headers["idempotency-key"];
}

function createCalls(harness: Harness): CapturedRequest[] {
  return harness.calls.filter((call) => call.method === "POST" && call.url.endsWith("/api/sessions"));
}

function messageCalls(harness: Harness): CapturedRequest[] {
  return harness.calls.filter((call) => call.method === "POST" && call.url.endsWith(`/api/sessions/${SESSION_ID}/messages`));
}

function runOptions(input: SessionInput, keys: IdempotencyCase = {}): SessionStartOptions {
  return {
    model: MODEL,
    message: input,
    apiKeys: API_KEYS,
    ...(keys.createKey !== undefined ? { idempotencyKey: keys.createKey } : {}),
    ...(keys.messageKey !== undefined ? { messageIdempotencyKey: keys.messageKey } : {})
  };
}

async function waitForSocket(sockets: readonly FakeWebSocket[], count: number): Promise<FakeWebSocket> {
  for (let i = 0; i < 200 && sockets.length < count; i++) {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
  const socket = sockets[count - 1];
  if (socket === undefined) throw new Error(`socket ${count} never opened`);
  return socket;
}

async function finishTurn(harness: Harness, textEvents: readonly TextEventSpec[] = []): Promise<void> {
  const socket = await waitForSocket(harness.sockets, harness.sockets.length + 1);
  for (let i = 0; i < textEvents.length; i++) {
    socket.message(textEvent(textEvents[i]!, TEXT_SEQUENCE_START + i));
  }
  socket.message(idleEvent(TEXT_SEQUENCE_START + textEvents.length));
}

function eventTime(sequence: number): string {
  return new Date(sequence).toISOString();
}

function textEvent(spec: TextEventSpec, sequence: number): AexEvent {
  const data: Record<string, JsonValue> = { text: spec.text };
  if (spec.messageId !== undefined) data.messageId = spec.messageId;
  return {
    specversion: "1.0",
    id: `${SESSION_ID}:${sequence}`,
    source: "agent",
    type: "TEXT_MESSAGE_CONTENT",
    subject: SESSION_ID,
    threadId: SESSION_ID,
    runId: "run_property",
    time: eventTime(sequence),
    sequence,
    data
  };
}

function idleEvent(sequence: number): AexEvent {
  return {
    specversion: "1.0",
    id: `${SESSION_ID}:${sequence}`,
    source: "runtime",
    type: "RUN_FINISHED",
    subject: SESSION_ID,
    threadId: SESSION_ID,
    runId: "run_property",
    time: eventTime(sequence),
    sequence,
    data: {
      outcome: "succeeded",
      costUsd: 0.001,
      providerUsage: [{ inputTokens: 3, outputTokens: 5, totalTokens: 8 }],
      checkpoint: { checkpointId: "cp_property" }
    }
  };
}

function expectSessionKeys(harness: Harness, keys: IdempotencyCase): void {
  const create = createCalls(harness);
  const messages = messageCalls(harness);
  expect(create).toHaveLength(1);
  expect(messages).toHaveLength(1);

  const createKey = idempotencyKey(create[0]!);
  const messageKey = idempotencyKey(messages[0]!);
  if (keys.createKey !== undefined) {
    expect(createKey).toBe(keys.createKey);
  } else {
    expect(createKey).toEqual(expect.stringMatching(/\S+/));
  }
  if (keys.messageKey !== undefined) {
    expect(messageKey).toBe(keys.messageKey);
  } else {
    expect(messageKey).toBe(`${createKey}:message`);
  }
}

function expectedMessages(specs: readonly TextEventSpec[]): readonly Message[] {
  const messages: Message[] = [];
  const byMessageId = new Map<string, number>();

  for (let i = 0; i < specs.length; i++) {
    const spec = specs[i]!;
    const sequence = TEXT_SEQUENCE_START + i;
    const timestamp = eventTime(sequence);
    if (spec.messageId !== undefined) {
      const existing = byMessageId.get(spec.messageId);
      if (existing !== undefined) {
        const current = messages[existing]!;
        messages[existing] = {
          ...current,
          text: `${current.text}${spec.text}`,
          timestamp,
          sequence
        };
        continue;
      }
      byMessageId.set(spec.messageId, messages.length);
    }
    messages.push({
      id: spec.messageId ?? `${SESSION_ID}:${sequence}`,
      sender: "assistant",
      text: spec.text,
      timestamp,
      sequence
    });
  }

  return messages;
}

function expectedTraceText(specs: readonly TextEventSpec[]): SessionResult["trace"]["text"] {
  return specs.map((spec, i) => {
    const sequence = TEXT_SEQUENCE_START + i;
    return {
      text: spec.text,
      ...(spec.messageId !== undefined ? { messageId: spec.messageId } : {}),
      seq: sequence,
      recordedAt: eventTime(sequence)
    };
  });
}

function assertTurnEventProjection(result: SessionResult, specs: readonly TextEventSpec[]): void {
  expect(result.sessionId).toBe(SESSION_ID);
  expect(result.sessionId).toBe(SESSION_ID);
  expect(result.status).toBe("succeeded");
  expect(result.session?.status).toBe("idle");
  expect(result.ok).toBe(true);
  expect(result.text).toBe(specs.map((spec) => spec.text).join(""));
  expect(result.messages).toEqual(expectedMessages(specs));
  expect(result.events.map((event) => event.type)).toEqual([
    ...specs.map(() => "TEXT_MESSAGE_CONTENT"),
    "RUN_FINISHED"
  ]);
  expect(result.trace.text).toEqual(expectedTraceText(specs));
  expect(result.trace.toolCalls).toEqual([]);
  expect(result.files).toEqual([{
    id: "out_property",
    checkpointId: "cp_property",
    filename: "answer.txt",
    sizeBytes: 0,
    sha256: EMPTY_SHA256
  }]);
  expect(result.usage).toEqual({ inputTokens: 3, outputTokens: 5, totalTokens: 8 });
  expect(result.costUsd).toBe(0.001);
}

function isValidSessionInput(value: unknown): value is SessionInput {
  if (typeof value === "string") return value.length > 0;
  return Array.isArray(value) && value.length > 0 && value.every((segment) => typeof segment === "string" && segment.length > 0);
}

interface IdempotencyCase {
  readonly createKey?: string;
  readonly messageKey?: string;
}

const idChar = fc.constantFrom(
  "a", "b", "c", "d", "e", "f", "g", "h",
  "i", "j", "k", "m", "n", "p", "q", "r",
  "s", "t", "u", "v", "w", "x", "y", "z",
  "0", "1", "2", "3", "4", "5", "6", "7",
  "8", "9", "-", "_"
);
const nonEmptyString = fc.string({ minLength: 1, maxLength: 80 });
const validSessionInput: Arbitrary<SessionInput> = fc.oneof(
  nonEmptyString,
  fc.array(nonEmptyString, { minLength: 1, maxLength: 6 })
);
const keyString = fc.array(idChar, { minLength: 1, maxLength: 24 }).map((chars) => `idem_${chars.join("")}`);
const idempotencyCase: Arbitrary<IdempotencyCase> = fc.oneof(
  fc.constant({}),
  keyString.map((createKey) => ({ createKey })),
  keyString.map((messageKey) => ({ messageKey })),
  fc.tuple(keyString, keyString).map(([createKey, messageKey]) => ({ createKey, messageKey }))
);
const invalidArrayInput: Arbitrary<unknown> = fc.oneof(
  fc.constant([]),
  fc.array(
    fc.oneof(
      nonEmptyString,
      fc.constant(""),
      fc.integer(),
      fc.boolean(),
      fc.constant(null),
      fc.record({ text: fc.string({ maxLength: 10 }) })
    ),
    { minLength: 1, maxLength: 6 }
  ).filter((value) => !isValidSessionInput(value))
);
const invalidSessionInput: Arbitrary<unknown> = fc.oneof(
  fc.constant(""),
  invalidArrayInput,
  fc.constant(undefined),
  fc.constant(null),
  fc.boolean(),
  fc.integer(),
  fc.double({ noNaN: true }),
  fc.record({ message: fc.string({ maxLength: 20 }) })
);
const messageId = fc.array(idChar, { minLength: 1, maxLength: 10 }).map((chars) => `msg_${chars.join("")}`);
const textEventSpec = fc.tuple(
  fc.string({ maxLength: 40 }),
  fc.option(messageId, { nil: undefined })
).map(([text, maybeMessageId]): TextEventSpec => (
  maybeMessageId === undefined ? { text } : { text, messageId: maybeMessageId }
));
const textEventSequence = fc.array(textEventSpec, { minLength: 0, maxLength: 8 });

describe("SDK run/send SessionInput properties", { timeout: 30_000 }, () => {
  it("Aex.start serializes valid message inputs and uses predictable idempotency keys", async () => {
    await fc.assert(
      fc.asyncProperty(validSessionInput, idempotencyCase, async (input, keys) => {
        const harness = makeHarness();
        const promise = harness.client.start(runOptions(input, keys), { webSocketFactory: harness.webSocketFactory });
        await finishTurn(harness);
        const result = await promise;

        expect(messageCalls(harness)[0]!.body).toEqual({ input });
        expectSessionKeys(harness, keys);
        expect(result.text).toBe("");
      }),
      PROPERTY_RUNS
    );
  });

  it("session.send serializes valid inputs and preserves caller-supplied or generated message keys", async () => {
    await fc.assert(
      fc.asyncProperty(validSessionInput, fc.option(keyString, { nil: undefined }), async (input, providedKey) => {
        const harness = makeHarness();
        const session = await harness.client.sessions.open(SESSION_ID);
        harness.calls.length = 0;

        const promise = session.messages.send(input, {
          webSocketFactory: harness.webSocketFactory,
          ...(providedKey !== undefined ? { idempotencyKey: providedKey } : {})
        }).finished();
        await finishTurn(harness);
        const result = await promise;
        const messages = messageCalls(harness);

        expect(messages).toHaveLength(1);
        expect(messages[0]!.body).toEqual({ input });
        expect(idempotencyKey(messages[0]!)).toEqual(providedKey ?? expect.stringMatching(/\S+/));
        expect(result.text).toBe("");
        expect(createCalls(harness)).toHaveLength(0);
      }),
      PROPERTY_RUNS
    );
  });

  it("rejects invalid Aex.start message inputs before any HTTP request", async () => {
    await fc.assert(
      fc.asyncProperty(invalidSessionInput, async (input) => {
        const harness = makeHarness();

        await expect(
          harness.client.start({ ...runOptions("fallback"), message: input } as unknown as SessionStartOptions)
        ).rejects.toBeInstanceOf(Error);
        expect(harness.calls).toHaveLength(0);
      }),
      PROPERTY_RUNS
    );
  });

  it("rejects invalid session.send inputs before sending any HTTP request", async () => {
    await fc.assert(
      fc.asyncProperty(invalidSessionInput, async (input) => {
        const harness = makeHarness();
        const session = await harness.client.sessions.open(SESSION_ID);
        harness.calls.length = 0;

        expect(() => session.messages.send(input as SessionInput)).toThrow(Error);
        expect(harness.calls).toHaveLength(0);
      }),
      PROPERTY_RUNS
    );
  });

  it("Aex.start is create plus send and projects fuzzed TEXT_MESSAGE_CONTENT streams consistently", async () => {
    await fc.assert(
      fc.asyncProperty(validSessionInput, textEventSequence, async (input, specs) => {
        const harness = makeHarness();
        const promise = harness.client.start(
          runOptions(input, { createKey: "create_key" }),
          { webSocketFactory: harness.webSocketFactory }
        );
        await finishTurn(harness, specs);
        const result = await promise;
        const posts = harness.calls.filter((call) => call.method === "POST");

        expect(posts[0]!.url).toBe(`${BASE_URL}/api/sessions`);
        expect(posts[1]!.url).toBe(`${BASE_URL}/api/sessions/${SESSION_ID}/messages`);
        expect(messageCalls(harness)[0]!.body).toEqual({ input });
        expectSessionKeys(harness, { createKey: "create_key" });
        assertTurnEventProjection(result, specs);
      }),
      EVENT_PROPERTY_RUNS
    );
  });

  it("session.replayLast reuses the last send input and idempotency key for fuzzed valid inputs", async () => {
    await fc.assert(
      fc.asyncProperty(validSessionInput, fc.option(keyString, { nil: undefined }), async (input, providedKey) => {
        const harness = makeHarness();
        const session = await harness.client.sessions.open(SESSION_ID);
        harness.calls.length = 0;

        const first = session.messages.send(input, {
          webSocketFactory: harness.webSocketFactory,
          ...(providedKey !== undefined ? { idempotencyKey: providedKey } : {})
        }).finished();
        await finishTurn(harness);
        await first;

        const replay = session.messages.replayLast({ webSocketFactory: harness.webSocketFactory }).finished();
        await finishTurn(harness);
        await replay;

        const messages = messageCalls(harness);
        expect(messages).toHaveLength(2);
        expect(messages[0]!.body).toEqual({ input });
        expect(messages[1]!.body).toEqual({ input });
        expect(idempotencyKey(messages[0]!)).toEqual(providedKey ?? expect.stringMatching(/\S+/));
        expect(idempotencyKey(messages[1]!)).toBe(idempotencyKey(messages[0]!));
      }),
      PROPERTY_RUNS
    );
  });
});

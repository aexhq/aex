import { describe, expect, it } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import { Aex, SessionConfigValidationError, type SessionResult } from "../../src/index.js";
import type { AexEvent } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";
import {
  unvalidatedCreateOptions,
  unvalidatedSendOptions,
  unvalidatedStartControls,
  unvalidatedStartOptions
} from "../helpers/unvalidated.js";

const EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

interface CapturedRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Record<string, string>;
  readonly body: unknown;
}

function captureThrown(operation: () => unknown): unknown {
  try {
    operation();
  } catch (error) {
    return error;
  }
  throw new Error("expected operation to throw");
}

async function captureRejected(operation: () => Promise<unknown>): Promise<unknown> {
  return operation().then(
    () => undefined,
    (error: unknown) => error
  );
}

function expectConfigError(error: unknown, field: string): void {
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toMatchObject({
    name: "SessionConfigValidationError",
    code: "SESSION_CONFIG_INVALID"
  });
  expect((error as SessionConfigValidationError).details).toEqual({ field });
  expect((error as Error).message.trim().length).toBeGreaterThan(0);
}

function expectExactConfigError(error: unknown, field: string, message: string): void {
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toMatchObject({
    name: "SessionConfigValidationError",
    code: "SESSION_CONFIG_INVALID",
    message,
    details: { field }
  });
}

function event(sequence: number, patch: Partial<AexEvent> = {}): AexEvent {
  return {
    specversion: "1.0",
    id: `sess_1:${sequence}`,
    source: "agent",
    type: "TEXT_MESSAGE_CONTENT",
    subject: "sess_1",
    threadId: "sess_1",
    runId: "run_1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { text: "hello" },
    ...patch
  };
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

function makeClient(options: {
  readonly getSessionStatus?: string;
  readonly committedLastRun?: "matching" | "missing" | "stale";
} = {}): {
  readonly client: Aex;
  readonly calls: CapturedRequest[];
  readonly sockets: FakeWebSocket[];
  readonly webSocketFactory: (url: string) => FakeWebSocket;
} {
  const calls: CapturedRequest[] = [];
  const sockets: FakeWebSocket[] = [];
  const fetch: FetchLike = async (input, init) => {
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
      return json({ session: { id: "sess_1", status: "idle", acceptsMessages: true } });
    }
    if (url.endsWith("/api/sessions/sess_1/messages")) {
      return json({
        session: { id: "sess_1", status: "running", acceptsMessages: false },
        run: { sessionId: "sess_1", turnSeq: 1, runId: "run_1", phase: "running", eventCursor: 4096 },
        eventCursor: 4096
      });
    }
    if (url.endsWith("/api/sessions/sess_1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/sess_1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.includes("/api/sessions/sess_1/files?checkpointId=cp_1")) {
      return json({
        revision: { checkpointId: "cp_1", runId: "run_1", turnSeq: 1, committedAt: "2026-07-10T00:00:00Z", throughSeq: 4097 },
        files: [{
          id: "out_1",
          checkpointId: "cp_1",
          filename: "answer.txt",
          sizeBytes: 0,
          sha256: EMPTY_SHA256
        }]
      });
    }
    if (url.endsWith("/api/sessions/sess_1") && (init?.method ?? "GET").toString() === "DELETE") {
      // DELETE answers with its own body. This stub used to reply with the GET
      // one, which made the two counters the server always sends look optional.
      return json({
        session: { id: "sess_1", status: "deleted", acceptsMessages: false },
        purgedSessionFileObjects: 0,
        cleanupComplete: true
      });
    }
    if (url.endsWith("/api/sessions/sess_1")) {
      // Terminal billing is committed on the first read; a `running` override
      // deliberately exercises an inconsistent post-terminal response.
      const lastRun = options.committedLastRun === "missing"
        ? undefined
        : {
            sessionId: "sess_1",
            runId: options.committedLastRun === "stale" ? "run_0" : "run_1",
            turnSeq: 1,
            phase: "finished",
            outcome: "succeeded"
          };
      return json({ session: {
        id: "sess_1",
        status: options.getSessionStatus ?? "idle",
        acceptsMessages: options.getSessionStatus !== "running",
        ...(lastRun === undefined ? {} : { lastRun }),
        costUsd: 0,
        costTelemetry: { providerUsage: [] }
      } });
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
  it("start creates a session, sends one message, and stops at RUN_FINISHED", async () => {
    const { client, calls, sockets, webSocketFactory } = makeClient();
    const promise = client.start({
      model: "anthropic/claude-haiku-4-5",
      message: "say hello",
      stream: { webSocketFactory }
    });

    await flush();
    expect(sockets).toHaveLength(1);
    expect(sockets[0]!.url).toBe("wss://events.example.test/sessions/sess_1?ticket=ticket&from=4096");
    sockets[0]!.message(event(4096));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "RUN_FINISHED",
      data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
    }));

    const result: SessionResult = await promise;
    expect(result.sessionId).toBe("sess_1");
    // The RESULT status is the terminal OUTCOME (a clean park ⇒ succeeded); the
    // resumable lifecycle `idle` stays on the session record.
    expect(result.status).toBe("succeeded");
    expect(result.session?.status).toBe("idle");
    expect(result.text).toBe("hello");
    expect(result.events.map((evt) => evt.sequence)).toEqual([4096, 4097]);
    expect(result.files).toEqual([{
      id: "out_1",
      checkpointId: "cp_1",
      filename: "answer.txt",
      sizeBytes: 0,
      sha256: EMPTY_SHA256
    }]);
    expect(calls.map((call) => `${call.method} ${call.url}`)).toContain(
      "POST https://api.example.test/api/sessions/sess_1/messages"
    );
    const create = calls.find((call) => call.method === "POST" && call.url.endsWith("/api/sessions"));
    expect((create!.body as Record<string, unknown>).retention).toEqual({ idleTtl: "3m" });
    expect(calls.map((call) => `${call.method} ${call.url}`)).toContain(
      "GET https://api.example.test/api/sessions/sess_1/files?checkpointId=cp_1"
    );
  });

  it("fails closed when RUN_FINISHED is visible before the session state is committed", async () => {
    const { client, sockets, webSocketFactory } = makeClient({ getSessionStatus: "running" });
    const promise = client.start({
      model: "anthropic/claude-haiku-4-5",
      message: "say hello",
      stream: { webSocketFactory }
    });

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "RUN_FINISHED",
      data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
    }));

    await expect(promise).rejects.toThrow(/before the session state was committed/);
  });

  it.each(["missing", "stale"] as const)(
    "fails closed when RUN_FINISHED is followed by a %s lastRun projection",
    async (committedLastRun) => {
      const { client, sockets, webSocketFactory } = makeClient({ committedLastRun });
      const promise = client.start({
        model: "anthropic/claude-haiku-4-5",
        message: "say hello",
        stream: { webSocketFactory }
      });

      await flush();
      sockets[0]!.message(event(4097, {
        source: "runtime",
        type: "RUN_FINISHED",
        data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
      }));

      await expect(promise).rejects.toThrow(/lastRun/);
    }
  );

  it("session.send can be consumed as an async event stream", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    const seen: number[] = [];
    const consume = (async () => {
      for await (const evt of session.messages.send("continue", { webSocketFactory })) {
        if (evt.replayable !== false) {
          seen.push(evt.sequence);
        }
      }
    })();

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "RUN_FINISHED",
      data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
    }));
    await consume;

    expect(seen).toEqual([4096, 4097]);
  });

  it("excludes replayed events from earlier runs from the accepted run stream and result", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    const turn = session.messages.send("continue", { webSocketFactory });
    const seenRunIds: string[] = [];
    const consume = (async () => {
      for await (const evt of turn) seenRunIds.push(evt.runId);
    })();

    await flush();
    sockets[0]!.message(event(4095, { runId: "run_0", data: { text: "old answer" } }));
    sockets[0]!.message(event(4096, { data: { text: "new answer" } }));
    sockets[0]!.message(event(4097, {
      source: "runtime",
      type: "RUN_FINISHED",
      data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
    }));

    await consume;
    const result = await turn.finished();
    expect(seenRunIds).toEqual(["run_1", "run_1"]);
    expect(result.text).toBe("new answer");
    expect(result.events.map((evt) => evt.runId)).toEqual(["run_1", "run_1"]);
  });

  it("yields events carrying the is*() type-guard methods, and narrows in the branch", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    const texts: string[] = [];
    const toolNames: string[] = [];
    let sawFinished = false;
    const consume = (async () => {
      for await (const evt of session.messages.send("continue", { webSocketFactory })) {
        if (evt.isTextMessage()) {
          // `evt.data.text` is narrowed to `string` — no cast needed.
          texts.push(evt.data.text);
        } else if (evt.isToolCallStart()) {
          toolNames.push(evt.data.name);
        }
        if (evt.isRunFinished()) sawFinished = true;
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
        type: "RUN_FINISHED",
        data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
      })
    );
    await consume;

    expect(texts).toEqual(["hello"]);
    expect(toolNames).toEqual(["read_file"]);
    expect(sawFinished).toBe(true);
  });

  it("collected result.events carry the is*() methods too", async () => {
    const { client, sockets, webSocketFactory } = makeClient();
    const promise = client.start({
      model: "anthropic/claude-haiku-4-5",
      message: "say hello",
      stream: { webSocketFactory }
    });

    await flush();
    sockets[0]!.message(event(4096));
    sockets[0]!.message(
      event(4097, {
        source: "runtime",
        type: "RUN_FINISHED",
        data: { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp_1" } }
      })
    );

    const result = await promise;
    expect(result.events[0]!.isTextMessage()).toBe(true);
    expect(result.events[1]!.isRunFinished()).toBe(true);
  });

  it("sessions.open rehydrates an existing session handle", async () => {
    const { client, calls } = makeClient();
    const session = await client.sessions.open("sess_1");

    expect(session.id).toBe("sess_1");
    expect(calls.map((call) => `${call.method} ${call.url}`)).toContain(
      "GET https://api.example.test/api/sessions/sess_1"
    );
  });

  it("sessions.delete deletes an id-addressed session", async () => {
    const { client, calls } = makeClient();

    await client.sessions.delete("sess_1");

    const req = calls.find((call) => call.method === "DELETE" && call.url.endsWith("/api/sessions/sess_1"));
    expect(req).toBeTruthy();
    expect(req!.headers["Idempotency-Key"] ?? req!.headers["idempotency-key"]).toBeUndefined();
  });

  it("sessions.create lets callers override the idle-to-suspend TTL", async () => {
    const { client, calls } = makeClient();
    await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
      overrides: { idleTtl: "10m" },
    });

    const create = calls.find((call) => call.method === "POST" && call.url.endsWith("/api/sessions"));
    expect((create!.body as Record<string, unknown>).retention).toEqual({
      idleTtl: "10m"
    });
  });

  it("rejects the old idleSuspendAfter override", async () => {
    const { client } = makeClient();
    const error = await captureRejected(() => client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        overrides: { idleSuspendAfter: "10m" },
      })));
    expectConfigError(error, "overrides.idleSuspendAfter");
  });

  it("rejects legacy one-shot prompt input before any HTTP request", async () => {
    const { client, calls } = makeClient();

    const error = await captureRejected(() => client.start(unvalidatedStartOptions({
        model: "anthropic/claude-haiku-4-5",
        prompt: "legacy one-shot input",
      })));
    expectConfigError(error, "prompt");
    expect(calls).toHaveLength(0);
  });

  it("rejects unknown one-shot options before any HTTP request", async () => {
    const { client, calls } = makeClient();

    const error = await captureRejected(() => client.start(unvalidatedStartOptions({
        model: "anthropic/claude-haiku-4-5",
        message: "hello",
        totallyUnknownOption: { nope: true }
      })));
    expectExactConfigError(
      error,
      "totallyUnknownOption",
      "Aex.start: totallyUnknownOption is not a supported option"
    );
    expect(calls).toHaveLength(0);
  });

  it("rejects unknown Aex.start control and stream keys with exact pre-transport errors", async () => {
    const { client, calls, sockets } = makeClient();
    const input = {
      model: "anthropic/claude-haiku-4-5",
      message: "hello",
    } as const;

    const controlError = await captureRejected(() => client.start(
      input,
      unvalidatedStartControls({ futureControl: true })
    ));
    expectExactConfigError(
      controlError,
      "options.futureControl",
      "Aex.start: options.futureControl is not a supported option"
    );
    expect(calls).toHaveLength(0);
    expect(sockets).toHaveLength(0);

    const streamError = await captureRejected(() => client.start(unvalidatedStartOptions({
      ...input,
      stream: { idempotencyKey: "wire-only-here" }
    })));
    expectExactConfigError(
      streamError,
      "stream.idempotencyKey",
      "Aex.start: stream.idempotencyKey is not a supported option"
    );
    expect(calls).toHaveLength(0);
    expect(sockets).toHaveLength(0);
  });

  it("rejects invalid one-shot messages before any HTTP request", async () => {
    const { client, calls } = makeClient();

    const invalidMessages = [
      () => client.start(unvalidatedStartOptions({
        model: "anthropic/claude-haiku-4-5",
      })),
      () => client.start({ model: "anthropic/claude-haiku-4-5", message: "" }),
      () => client.start({ model: "anthropic/claude-haiku-4-5", message: "  \n\t " }),
      () => client.start({
        model: "anthropic/claude-haiku-4-5",
        message: ["ok", ""],
      }),
      () => client.start({
        model: "anthropic/claude-haiku-4-5",
        message: ["  ", "\n"],
      })
    ];
    for (const operation of invalidMessages) {
      expectConfigError(await captureRejected(operation), "message");
    }
    expect(calls).toHaveLength(0);
  });

  it("rejects invalid session.send input before sending a message request", async () => {
    const { client, calls } = makeClient();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    calls.length = 0;

    for (const input of ["", "  \n\t ", [], ["ok", ""], ["  ", "\n"]] as const) {
      expectConfigError(captureThrown(() => session.messages.send(input)), "input");
    }
    expect(() => session.messages.send(["keep formatting", "  \n"])).not.toThrow();
    expect(calls).toHaveLength(0);
  });

  it("rejects public AbortSignal controls on the session API", async () => {
    const { client } = makeClient();
    const signal = new AbortController().signal;
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });

    expectConfigError(captureThrown(() => session.messages.send("continue", unvalidatedSendOptions({ signal }))), "signal");
    const error = await captureRejected(() => client.start(unvalidatedStartOptions({
        model: "anthropic/claude-haiku-4-5",
        message: "continue",
        stream: { signal }
      })));
    expectConfigError(error, "stream.signal");
  });

  it("keeps replay cursors on session.events instead of message sends", async () => {
    const { client, calls } = makeClient();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    calls.length = 0;

    expectConfigError(captureThrown(() => session.messages.send("continue", unvalidatedSendOptions({ from: 0 }))), "from");
    expect(calls).toHaveLength(0);
  });

  it("rejects an unknown send key with the exact synchronous error", async () => {
    const { client, calls, sockets } = makeClient();
    const session = await client.sessions.create({
      model: "anthropic/claude-haiku-4-5",
    });
    calls.length = 0;

    const error = captureThrown(() => session.messages.send("continue", unvalidatedSendOptions({ futureSend: true })));
    expectExactConfigError(
      error,
      "futureSend",
      "session.messages.send: futureSend is not a supported option"
    );
    expect(calls).toHaveLength(0);
    expect(sockets).toHaveLength(0);
  });
});

import { describe, expect, it } from "vitest";
import { Aex, type RunResult } from "../../src/index.js";
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
    id: `run-1:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: "run-1",
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
    if (url.endsWith("/api/sessions/run-1/events/ticket")) {
      return json({ wsUrl: "wss://events.example.test/sessions/run-1", ticket: "ticket", expiresAtMs: 1 });
    }
    if (url.endsWith("/api/sessions/run-1/outputs")) {
      return json({ outputs: [{ id: "o1", filename: "report.txt" }] });
    }
    if (url.endsWith("/api/sessions/run-1/messages")) {
      return json({
        session: { id: "run-1", status: "running", turnSeq: 1 },
        turn: { sessionId: "run-1", turnSeq: 1 },
        eventCursor: 1024
      });
    }
    if (url.endsWith("/api/sessions/run-1")) {
      return json({ session });
    }
    if (url.endsWith("/api/sessions")) {
      return json({ session: { id: "run-1", status: "idle", turnSeq: 0 } });
    }
    return json({});
  };
  const factory = (url: string): FakeWebSocket => {
    const ws = new FakeWebSocket(url);
    sockets.push(ws);
    return ws;
  };
  return { client: new Aex({ apiToken: "tkn", baseUrl: "https://x", fetch }), urls, sockets, webSocketFactory: factory };
}

async function collectRun(session: Record<string, unknown>): Promise<{
  readonly result: RunResult;
  readonly urls: readonly string[];
}> {
  const { client, urls, sockets, webSocketFactory } = runClient(session);
  const promise = client.run(
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
  sockets[0]!.message(evt(1026, "CUSTOM", { name: session.status === "error" ? "aex.session.error" : "aex.session.idle", value: { turnSeq: 1 } }));

  return { result: await promise, urls };
}

describe("Aex.run -> one-shot session RunResult", () => {
  it("returns a run-compatible result for a parked session turn", async () => {
    const { result, urls } = await collectRun({
      id: "run-1",
      status: "idle",
      usage: { inputTokens: 10, outputTokens: 5, totalTokens: 15 },
      costUsd: 0.0123
    });

    expect(result.runId).toBe("run-1");
    expect(result.sessionId).toBe("run-1");
    expect(result.ok).toBe(true);
    expect(result.status).toBe("idle");
    expect(result.run.status).toBe("idle");
    expect(result.text).toBe("hello world");
    expect(result.events.map((e) => e.type)).toEqual(["TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_CONTENT", "CUSTOM"]);
    expect(result.trace.text.map((t) => t.text)).toEqual(["hello ", "world"]);
    expect(result.outputs).toEqual([{ id: "o1", filename: "report.txt" }]);
    expect(result.usage).toEqual({ inputTokens: 10, outputTokens: 5, totalTokens: 15 });
    expect(result.costUsd).toBe(0.0123);
    expect(result.error).toBeUndefined();
    expect(urls.some((url) => url.includes("/api/runs"))).toBe(false);
  });

  it("returns ok:false with error for an error session by default", async () => {
    const { result } = await collectRun({ id: "run-1", status: "error", errorMessage: "boom" });
    expect(result.ok).toBe(false);
    expect(result.status).toBe("error");
    expect(result.error).toBe("boom");
  });

  it("waits for the settled record so costUsd/usage survive the park-event → settle race", async () => {
    // Live-observed on dev: the park EVENT ends the stream seconds BEFORE the
    // settle lambda flips the record and stamps costTelemetry/costUsd, so a
    // single immediate read returned costUsd: undefined on virtually every
    // fresh run — despite RunResult documenting the settle-time showback.
    const urls: string[] = [];
    const sockets: FakeWebSocket[] = [];
    let sessionReads = 0;
    const fetch: typeof globalThis.fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
      urls.push(`${(init?.method ?? "GET").toString()} ${url}`);
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
        sessionReads += 1;
        // Read 1 (post-stream): settle hasn't landed — record still `running`,
        // no costUsd. Read 2+: settled.
        return json({
          session:
            sessionReads < 2
              ? { id: "run-1", status: "running", turnSeq: 1 }
              : { id: "run-1", status: "idle", turnSeq: 1, costUsd: 0.0042, usage: { inputTokens: 3, outputTokens: 2, totalTokens: 5 } }
        });
      }
      if (url.endsWith("/api/sessions")) {
        return json({ session: { id: "run-1", status: "idle", turnSeq: 0 } });
      }
      return json({});
    };
    const factory = (url: string): FakeWebSocket => {
      const ws = new FakeWebSocket(url);
      sockets.push(ws);
      return ws;
    };
    const client = new Aex({ apiToken: "tkn", baseUrl: "https://x", fetch });
    const promise = client.run(
      { model: "claude-haiku-4-5", message: "p", apiKeys: { anthropic: "sk-ant" } },
      { webSocketFactory: factory, settleConsistent: true }
    );

    await flush();
    sockets[0]!.message(evt(1024, "TEXT_MESSAGE_CONTENT", { text: "hi", messageId: "m1" }));
    sockets[0]!.message(evt(1025, "CUSTOM", { name: "aex.session.idle", value: { turnSeq: 1 } }));

    const result = await promise;
    expect(result.ok).toBe(true);
    expect(result.costUsd).toBe(0.0042);
    expect(result.usage).toEqual({ inputTokens: 3, outputTokens: 2, totalTokens: 5 });
    expect(result.session?.status).toBe("idle");
  });

  it("throws when throwOnFailure is set and the session turn did not park cleanly", async () => {
    const { client, sockets, webSocketFactory } = runClient({ id: "run-1", status: "error", errorMessage: "boom" });
    const promise = client.run(
      {
        model: "claude-haiku-4-5",
        message: "p",
        apiKeys: { anthropic: "sk-ant" }
      },
      { throwOnFailure: true, webSocketFactory }
    );

    await flush();
    sockets[0]!.message(evt(1024, "CUSTOM", { name: "aex.session.error", value: { turnSeq: 1 } }));

    await expect(promise).rejects.toThrow(/session run-1 ended error: boom/);
  });
});

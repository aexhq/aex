/**
 * DX3: `aex tail` / `aex inspect` over the coordinator WS envelope stream.
 * Fully offline — a fake `webSocketFactory` drives frames and a fake `fetchImpl`
 * answers the ticket broker + getSession.
 */
import { describe, expect, it } from "bun:test";
import { executeCli } from "../src/main.js";
import { type CliIO } from "../src/internal.js";
import type { AexEvent, WebSocketLike } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";

const evt = (
  sequence: number,
  type: AexEvent["type"] = "TEXT_MESSAGE_CONTENT",
  data: Record<string, unknown> = {},
  extra: Partial<AexEvent> = {}
): AexEvent => {
  const terminalData = type === "RUN_FINISHED"
    ? { outcome: "succeeded", costUsd: 0, providerUsage: [], checkpoint: { checkpointId: "cp-1" } }
    : type === "RUN_ERROR"
      ? { outcome: "failed", failureClass: "internal", failureMessage: "run failed", costUsd: 0, providerUsage: [] }
      : type === "TOOL_CALL_START"
        ? { id: `call-${sequence}` }
        : type === "TOOL_CALL_RESULT"
          ? { id: `call-${sequence}`, content: null }
          : {};
  return {
    specversion: "1.0",
    id: `session-x:${sequence}`,
    source: "agent",
    type,
    subject: "session-x",
    threadId: "session-x",
    runId: "run-1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: { ...terminalData, ...data } as AexEvent["data"],
    ...extra
  };
};

const flush = async (n = 6): Promise<void> => {
  for (let i = 0; i < n; i++) await new Promise<void>((r) => setTimeout(r, 0));
};

function makeIo(opts: {
  argv: readonly string[];
  finalStatuses?: readonly string[];
  sessionReadErrors?: Readonly<Record<number, {
    readonly status: number;
    readonly body: Readonly<Record<string, unknown>>;
  }>>;
  sessionStatus?: string;
  runless?: boolean;
  noWs?: boolean;
}): {
  io: CliIO;
  out: () => string;
  err: () => string;
  exit: () => number | null;
  nextSocket: (minCount?: number) => Promise<FakeWebSocket>;
  fireSigint: () => void;
  socketUrls: string[];
  requests: Array<{ readonly method: string; readonly path: string; readonly body: unknown }>;
} {
  const state = { stdout: "", stderr: "", exit: null as number | null };
  const sockets: FakeWebSocket[] = [];
  const socketUrls: string[] = [];
  const requests: Array<{ readonly method: string; readonly path: string; readonly body: unknown }> = [];
  const finalStatuses = opts.finalStatuses ?? [opts.sessionStatus ?? "idle"];
  let finalReadCount = 0;
  let waiters: Array<() => void> = [];
  let sigint: (() => void) | undefined;
  const io: CliIO = {
    argv: ["bun", "/aex/aex", ...opts.argv],
    readFile: async () => {
      throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
    },
    writeFile: async () => {},
    cwd: () => "/tmp",
    fetchImpl: async (url, init) => {
      const u = String(url);
      const parsed = new URL(u);
      const method = String(init?.method ?? "GET").toUpperCase();
      let body: unknown;
      if (typeof init?.body === "string") {
        try {
          body = JSON.parse(init.body) as unknown;
        } catch {
          body = init.body;
        }
      }
      requests.push({ method, path: parsed.pathname, body });
      if (parsed.pathname === "/api/sessions" && method === "POST") {
        return new Response(
          JSON.stringify({
            session: {
              id: "session-x",
              status: "running",
              acceptsMessages: false,
              provider: "anthropic",
              model: "claude-haiku-4-5",
              runtimeSize: "0.25cpu-1gb",
              createdAt: "2026-01-01T00:00:00Z"
            }
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
      if (parsed.pathname === "/api/sessions/session-x/messages" && method === "POST") {
        return new Response(
          JSON.stringify({
            session: {
              id: "session-x",
              status: "running",
              acceptsMessages: false,
              provider: "anthropic",
              model: "claude-haiku-4-5",
              runtimeSize: "0.25cpu-1gb",
              createdAt: "2026-01-01T00:00:00Z"
            },
            run: { sessionId: "session-x", runId: "run-1", turnSeq: 1, phase: "running" },
            eventCursor: 1024
          }),
          { status: 202, headers: { "content-type": "application/json" } }
        );
      }
      if (u.endsWith("/events/ticket")) {
        return new Response(
          JSON.stringify({ wsUrl: "wss://co/sessions/session-x/subscribe", ticket: "tkt", expiresAtMs: Date.now() + 60_000 }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
      // getSession (final record read; sessionId === sessionId)
      const readIndex = finalReadCount;
      finalReadCount += 1;
      const readError = opts.sessionReadErrors?.[readIndex];
      if (readError) {
        return new Response(JSON.stringify(readError.body), {
          status: readError.status,
          headers: { "content-type": "application/json" }
        });
      }
      const status = finalStatuses[Math.min(readIndex, finalStatuses.length - 1)]!;
      const run = {
        sessionId: "session-x",
        runId: "run-1",
        turnSeq: 1,
        phase: status === "running" ? "running" : "finished",
        ...(status === "running" ? {} : { outcome: "succeeded" })
      };
      return new Response(JSON.stringify({ session: {
        id: "session-x",
        status,
        acceptsMessages: status !== "running",
        model: "claude-haiku-4-5",
        createdAt: "2026-01-01T00:00:00Z",
        ...(opts.runless ? {} : status === "running" ? { currentRun: run } : { lastRun: run })
      } }), {
        status: 200,
        headers: { "content-type": "application/json" }
      });
      void init;
    },
    stdout: (c) => {
      state.stdout += c;
    },
    stderr: (c) => {
      state.stderr += c;
    },
    exit: (code) => {
      state.exit = code;
    },
    onSignal: (_sig, handler) => {
      sigint = handler;
    }
  };
  if (!opts.noWs) {
    (io as { webSocketFactory?: (url: string) => WebSocketLike }).webSocketFactory = (url: string) => {
      const ws = new FakeWebSocket(url);
      sockets.push(ws);
      socketUrls.push(url);
      waiters.splice(0).forEach((w) => w());
      return ws;
    };
  }
  async function nextSocket(minCount = 1): Promise<FakeWebSocket> {
    while (sockets.length < minCount) {
      await new Promise<void>((r) => {
        waiters.push(r);
        setTimeout(r, 5);
      });
    }
    return sockets[sockets.length - 1]!;
  }
  return {
    io,
    out: () => state.stdout,
    err: () => state.stderr,
    exit: () => state.exit,
    nextSocket,
    fireSigint: () => sigint?.(),
    socketUrls,
    requests
  };
}

const COMMON = ["--api-key", "tok", "--aex-url", "https://dash.example"];

describe("aex tail", () => {
  it("renders pretty lines in order and exits 0 when the thread returns idle", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", ...COMMON], sessionStatus: "idle" });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "hello world" }));
    ws.message(evt(2, "TOOL_CALL_START", { name: "bash", input: { cmd: "ls" } }));
    ws.message(evt(3, "TOOL_CALL_RESULT", { name: "bash", content: "file.txt" }));
    ws.message(evt(4, "RUN_FINISHED"));
    await done;
    expect(cap.exit()).toBe(0);
    const lines = cap.out().trim().split("\n");
    expect(lines[0]).toContain("run started");
    expect(lines[1]).toBe("hello world");
    expect(lines[2]).toContain("tool bash");
    expect(lines[3]).toContain("← bash");
    expect(lines[4]).toContain("run finished");
  });

  it("emits raw envelope NDJSON under --json", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", "--json", ...COMMON] });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "hi" }));
    ws.message(evt(1, "RUN_FINISHED"));
    await done;
    expect(cap.exit()).toBe(0);
    const first = JSON.parse(cap.out().trim().split("\n")[0]!) as AexEvent;
    expect(first.type).toBe("TEXT_MESSAGE_CONTENT");
    expect(first.sequence).toBe(0);
  });

  it("filters to the requested AG-UI types", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", "--filter", "TOOL_CALL_START,TOOL_CALL_RESULT", ...COMMON] });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "ignored" }));
    ws.message(evt(2, "TOOL_CALL_START", { name: "bash" }));
    ws.message(evt(3, "RUN_FINISHED"));
    await done;
    const out = cap.out();
    expect(out).toContain("tool bash");
    expect(out).not.toContain("ignored");
    expect(out).not.toContain("run started");
  });

  it("rejects an unknown --filter token with USAGE_ERR + did-you-mean", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", "--filter", "TOOL_CALL_STAR", ...COMMON] });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("unknown --filter token");
    expect(cap.err()).toContain("did you mean");
  });

  it("returns exit 1 when the resumable thread reports an error", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", ...COMMON], sessionStatus: "error" });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_ERROR", { failureMessage: "boom", failureClass: "provider_error" }));
    await done;
    expect(cap.exit()).toBe(1);
    // jump-to-failure line on stderr
    expect(cap.err()).toContain("✗ run error: boom");
  });

  it("returns exit 1 for a cancelled run even when the session returns idle", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", ...COMMON], sessionStatus: "idle" });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_FINISHED", { outcome: "cancelled" }));
    await done;
    expect(cap.exit()).toBe(1);
  });

  it("enriches a failed final-status read and preserves the consistency check", async () => {
    const cap = makeIo({
      argv: ["tail", "session-x", "--json", ...COMMON],
      sessionReadErrors: {
        1: {
          status: 403,
          body: {
            error: "insufficient_scope",
            message: "the token does not carry the required scope",
            requiredScope: "sessions:read"
          }
        }
      }
    });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "kept" }));
    ws.message(evt(1, "RUN_FINISHED"));
    await done;

    expect(cap.exit()).toBe(1);
    expect(cap.out().trim().split("\n").map((line) => (JSON.parse(line) as AexEvent).sequence)).toEqual([0, 1]);
    const diagnostics = cap.err().trim().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(diagnostics).toHaveLength(2);
    expect(diagnostics[0]).toMatchObject({
      error: "tail_failed",
      sessionId: "session-x",
      lastSeq: 1,
      status: 403,
      remedy: "token lacks permission for this workspace/action"
    });
    expect(diagnostics[0]!.message).toMatch(/^final status fetch failed: /);
    expect(diagnostics[1]).toEqual({
      error: "run_terminal_inconsistent",
      sessionId: "session-x",
      lastSeq: 1,
      status: "unknown"
    });
  });

  it("times out a quiet stream with exit 3", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", "--timeout", "10ms", ...COMMON] });
    const done = executeCli(cap.io);
    await cap.nextSocket();
    // never send a terminal — the timeout must fire
    await done;
    expect(cap.exit()).toBe(3);
    expect(cap.err()).toContain("tail_timeout");
  });

  it("exits 130 with an (interrupted) note on SIGINT before terminal", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", ...COMMON] });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "partial" }));
    await flush();
    cap.fireSigint();
    await done;
    expect(cap.exit()).toBe(130);
    expect(cap.err()).toContain("(interrupted)");
  });

  it("errors actionably when no WebSocket is available", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", ...COMMON], noWs: true });
    await executeCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("websocket_unavailable");
    expect(cap.err()).toContain("Node >= 22");
  });

  it("resumes from lastSeq+1 after a transport drop (no dup/no gap)", async () => {
    const cap = makeIo({ argv: ["tail", "session-x", "--json", ...COMMON] });
    const done = executeCli(cap.io);
    const ws1 = await cap.nextSocket();
    expect(cap.socketUrls[0]).toContain("from=0");
    ws1.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "a" }));
    await flush();
    ws1.close(); // transport drop mid-stream → reconnect
    const ws2 = await cap.nextSocket(2);
    // second connect resumes from seq 1 (lastSeq 0 + 1)
    expect(cap.socketUrls[1]).toContain("from=1");
    ws2.message(evt(1, "RUN_FINISHED"));
    await done;
    const seqs = cap.out().trim().split("\n").map((l) => (JSON.parse(l) as AexEvent).sequence);
    expect(seqs).toEqual([0, 1]);
  });
});

describe("aex start --follow", () => {
  it("exits 130 when SIGINT interrupts a run before its terminal", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--follow",
        ...COMMON
      ],
      sessionStatus: "running"
    });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "partial" }));
    await flush();
    cap.fireSigint();
    await done;

    expect(cap.exit()).toBe(130);
    expect(cap.err()).toContain("(interrupted)");
  });

  it("streams live coordinator envelopes as NDJSON after submit", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--follow",
        ...COMMON
      ],
      sessionStatus: "idle"
    });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "live" }));
    ws.message(evt(2, "RUN_FINISHED"));
    await done;

    expect(cap.exit()).toBe(0);
    expect(cap.requests.some((request) => request.path === "/api/sessions/session-x/messages")).toBe(true);
    expect(cap.requests.find((request) => request.path === "/api/sessions/session-x/messages")?.body).toEqual({ input: ["hi"] });
    expect(cap.socketUrls[0]).toContain("from=0");
    const lines = cap.out().trim().split("\n");
    const accepted = JSON.parse(lines[0]!) as { id: string; status: string };
    const firstEvent = JSON.parse(lines[1]!) as AexEvent;
    const secondEvent = JSON.parse(lines[2]!) as AexEvent;
    const final = JSON.parse(lines.at(-1)!) as { id: string; status: string };
    expect(accepted).toMatchObject({ id: "session-x", status: "running" });
    expect(firstEvent.type).toBe("RUN_STARTED");
    expect(secondEvent).toMatchObject({ type: "TEXT_MESSAGE_CONTENT", sequence: 1 });
    expect(final).toMatchObject({ id: "session-x", status: "idle" });
  });

  it("includes the accepted session state when follow times out", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--follow",
        "--timeout", "0ms",
        ...COMMON
      ],
      sessionStatus: "running"
    });
    await executeCli(cap.io);

    expect(cap.exit()).toBe(3);
    expect(JSON.parse(cap.err().trim())).toMatchObject({
      error: "session_follow_timeout",
      sessionId: "session-x",
      sessionStatus: "running",
      turnSeq: 1
    });
  });

  it("fails immediately when the post-terminal session read is inconsistent", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--follow",
        ...COMMON
      ],
      finalStatuses: ["running", "idle"]
    });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "live" }));
    ws.message(evt(2, "RUN_FINISHED"));
    await done;

    expect(cap.exit()).toBe(1);
    const getSessionReads = cap.requests.filter((request) => request.method === "GET" && request.path === "/api/sessions/session-x");
    expect(getSessionReads).toHaveLength(1);
    const final = JSON.parse(cap.out().trim().split("\n").at(-1)!) as { id: string; status: string };
    expect(final).toMatchObject({ id: "session-x", status: "running" });
    expect(cap.err()).toContain("run_terminal_inconsistent");
  });

  it("retains streamed output and enriches a failed final-status read", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "anthropic/claude-haiku-4-5",
        "--prompt", "hi",
        "--follow",
        ...COMMON
      ],
      sessionReadErrors: {
        0: {
          status: 404,
          body: { error: "session_not_found", message: "session not found" }
        }
      }
    });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "kept" }));
    ws.message(evt(1, "RUN_FINISHED"));
    await done;

    expect(cap.exit()).toBe(1);
    const stdoutLines = cap.out().trim().split("\n");
    expect(stdoutLines).toHaveLength(3);
    expect(JSON.parse(stdoutLines[0]!)).toMatchObject({ id: "session-x", status: "running" });
    expect(stdoutLines.slice(1).map((line) => (JSON.parse(line) as AexEvent).sequence)).toEqual([0, 1]);
    expect(JSON.parse(cap.err())).toMatchObject({
      error: "session_failed",
      message: expect.stringMatching(/^final status fetch failed: /),
      sessionId: "session-x",
      status: 404,
      remedy: "no such run/resource — verify the id"
    });
  });
});

describe("aex inspect", () => {
  it("returns a bounded empty timeline for an idle session with no runs", async () => {
    const cap = makeIo({ argv: ["inspect", "session-x", "--json", ...COMMON], sessionStatus: "idle", runless: true });

    await executeCli(cap.io);

    expect(cap.exit()).toBe(0);
    expect(cap.socketUrls).toEqual([]);
    expect(JSON.parse(cap.out().trim())).toMatchObject({
      session: { id: "session-x", status: "idle" },
      events: []
    });
  });
  it("prints a header, the full timeline, and a cost/usage footer", async () => {
    const cap = makeIo({ argv: ["inspect", "session-x", ...COMMON], sessionStatus: "idle" });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "done" }));
    ws.message(evt(2, "RUN_FINISHED", { outcome: "succeeded" }, { source: "aex" }));
    await done;
    expect(cap.exit()).toBe(0);
    const out = cap.out();
    expect(out).toContain("session session-x · idle");
    expect(out).toContain("run started");
    expect(out).toContain("done");
  });

  it("emits one machine document under --json", async () => {
    const cap = makeIo({ argv: ["inspect", "session-x", "--json", ...COMMON], sessionStatus: "idle" });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "x" }));
    ws.message(evt(1, "RUN_FINISHED", { outcome: "succeeded" }, { source: "aex" }));
    await done;
    const doc = JSON.parse(cap.out().trim()) as { session: { id: string }; events: AexEvent[] };
    expect(doc.session.id).toBe("session-x");
    expect(doc.events.length).toBeGreaterThanOrEqual(1);
  });

  it("surfaces a jump-to-failure footer + exit 1 on a failed run", async () => {
    const cap = makeIo({ argv: ["inspect", "session-x", ...COMMON], sessionStatus: "error" });
    const done = executeCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_ERROR", { failureMessage: "kaboom", failureClass: "timeout" }));
    await done;
    expect(cap.exit()).toBe(1);
    expect(cap.out()).toContain("✗ kaboom");
    expect(cap.out()).toContain("[timeout]");
  });
});

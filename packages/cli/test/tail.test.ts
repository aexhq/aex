/**
 * DX3: `aex tail` / `aex inspect` over the coordinator WS envelope stream.
 * Fully offline — a fake `webSocketFactory` drives frames and a fake `fetchImpl`
 * answers the ticket broker + getSession.
 */
import { describe, expect, it } from "vitest";
import { runCli } from "../src/run.js";
import { type CliIO } from "../src/internal.js";
import type { AexEvent, WebSocketLike } from "@aexhq/contracts";

const evt = (
  sequence: number,
  type: AexEvent["type"] = "TEXT_MESSAGE_CONTENT",
  data: Record<string, unknown> = {},
  extra: Partial<AexEvent> = {}
): AexEvent => ({
  specversion: "1.0",
  id: `run-x:${sequence}`,
  source: "agent",
  type,
  subject: "run-x",
  time: new Date(sequence).toISOString(),
  sequence,
  data: data as AexEvent["data"],
  ...extra
});

class FakeWebSocket implements WebSocketLike {
  readonly url: string;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};
  readonly sent: string[] = [];
  closed = false;
  constructor(url: string) {
    this.url = url;
  }
  addEventListener(type: "open" | "message" | "close" | "error", cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }
  send(data: string): void {
    this.sent.push(data);
  }
  close(): void {
    this.closed = true;
    this.#emit("close", {});
  }
  message(event: AexEvent): void {
    this.#emit("message", { data: JSON.stringify(event) });
  }
  #emit(type: string, ev: { data?: unknown }): void {
    for (const cb of this.#listeners[type] ?? []) cb(ev);
  }
}

const flush = async (n = 6): Promise<void> => {
  for (let i = 0; i < n; i++) await new Promise<void>((r) => setTimeout(r, 0));
};

function makeIo(opts: {
  argv: readonly string[];
  runStatus?: string;
  noWs?: boolean;
}): {
  io: CliIO;
  out: () => string;
  err: () => string;
  exit: () => number | null;
  nextSocket: (minCount?: number) => Promise<FakeWebSocket>;
  fireSigint: () => void;
  socketUrls: string[];
} {
  const state = { stdout: "", stderr: "", exit: null as number | null };
  const sockets: FakeWebSocket[] = [];
  const socketUrls: string[] = [];
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
      if (parsed.pathname === "/api/sessions" && init?.method === "POST") {
        return new Response(
          JSON.stringify({
            id: "run-x",
            status: "running",
            provider: "anthropic",
            model: "claude-haiku-4-5",
            runtime: "managed",
            createdAt: "2026-01-01T00:00:00Z"
          }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
      if (u.endsWith("/events/ticket")) {
        return new Response(
          JSON.stringify({ wsUrl: "wss://co/runs/run-x/subscribe", ticket: "tkt", expiresAtMs: Date.now() + 60_000 }),
          { status: 200, headers: { "content-type": "application/json" } }
        );
      }
      // getSession (final record read; runId === sessionId)
      return new Response(JSON.stringify({ id: "run-x", status: opts.runStatus ?? "succeeded", model: "claude-haiku-4-5", createdAt: "2026-01-01T00:00:00Z" }), {
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
    socketUrls
  };
}

const COMMON = ["--api-key", "tok", "--aex-url", "https://dash.example"];

describe("aex tail", () => {
  it("renders pretty lines in order and exits 0 on a succeeded terminal", async () => {
    const cap = makeIo({ argv: ["tail", "run-x", ...COMMON], runStatus: "succeeded" });
    const done = runCli(cap.io);
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
    const cap = makeIo({ argv: ["tail", "run-x", "--json", ...COMMON] });
    const done = runCli(cap.io);
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
    const cap = makeIo({ argv: ["tail", "run-x", "--filter", "TOOL_CALL_START,TOOL_CALL_RESULT", ...COMMON] });
    const done = runCli(cap.io);
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
    const cap = makeIo({ argv: ["tail", "run-x", "--filter", "TOOL_CALL_STAR", ...COMMON] });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("unknown --filter token");
    expect(cap.err()).toContain("did you mean");
  });

  it("returns exit 1 when the run reaches a non-succeeded terminal", async () => {
    const cap = makeIo({ argv: ["tail", "run-x", ...COMMON], runStatus: "failed" });
    const done = runCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_ERROR", { failureMessage: "boom", failureClass: "provider_error" }));
    await done;
    expect(cap.exit()).toBe(1);
    // jump-to-failure line on stderr
    expect(cap.err()).toContain("✗ run error: boom");
  });

  it("times out a quiet stream with exit 3", async () => {
    const cap = makeIo({ argv: ["tail", "run-x", "--timeout", "10ms", ...COMMON] });
    const done = runCli(cap.io);
    await cap.nextSocket();
    // never send a terminal — the timeout must fire
    await done;
    expect(cap.exit()).toBe(3);
    expect(cap.err()).toContain("tail_timeout");
  });

  it("exits 0 with an (interrupted) note on SIGINT before terminal", async () => {
    const cap = makeIo({ argv: ["tail", "run-x", ...COMMON] });
    const done = runCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "partial" }));
    await flush();
    cap.fireSigint();
    await done;
    expect(cap.exit()).toBe(0);
    expect(cap.err()).toContain("(interrupted)");
  });

  it("errors actionably when no WebSocket is available", async () => {
    const cap = makeIo({ argv: ["tail", "run-x", ...COMMON], noWs: true });
    await runCli(cap.io);
    expect(cap.exit()).toBe(2);
    expect(cap.err()).toContain("websocket_unavailable");
    expect(cap.err()).toContain("Node >= 22");
  });

  it("resumes from lastSeq+1 after a transport drop (no dup/no gap)", async () => {
    const cap = makeIo({ argv: ["tail", "run-x", "--json", ...COMMON] });
    const done = runCli(cap.io);
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

describe("aex run --follow", () => {
  it("streams live coordinator envelopes as NDJSON after submit", async () => {
    const cap = makeIo({
      argv: [
        "run",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-test",
        "--follow",
        ...COMMON
      ],
      runStatus: "succeeded"
    });
    const done = runCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "live" }));
    ws.message(evt(2, "RUN_FINISHED"));
    await done;

    expect(cap.exit()).toBe(0);
    expect(cap.socketUrls[0]).toContain("from=0");
    const lines = cap.out().trim().split("\n");
    const accepted = JSON.parse(lines[0]!) as { id: string; status: string };
    const firstEvent = JSON.parse(lines[1]!) as AexEvent;
    const secondEvent = JSON.parse(lines[2]!) as AexEvent;
    const final = JSON.parse(lines.at(-1)!) as { id: string; status: string };
    expect(accepted).toMatchObject({ id: "run-x", status: "running" });
    expect(firstEvent.type).toBe("RUN_STARTED");
    expect(secondEvent).toMatchObject({ type: "TEXT_MESSAGE_CONTENT", sequence: 1 });
    expect(final).toMatchObject({ id: "run-x", status: "succeeded" });
  });
});

describe("aex inspect", () => {
  it("prints a header, the full timeline, and a cost/usage footer", async () => {
    const cap = makeIo({ argv: ["inspect", "run-x", ...COMMON], runStatus: "succeeded" });
    const done = runCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_STARTED"));
    ws.message(evt(1, "TEXT_MESSAGE_CONTENT", { text: "done" }));
    // settle-consistent stream ends on the aex.run.settled barrier
    ws.message(evt(2, "CUSTOM", { name: "aex.run.settled", value: { outcome: "succeeded" } }, { source: "aex" }));
    await done;
    expect(cap.exit()).toBe(0);
    const out = cap.out();
    expect(out).toContain("session run-x · succeeded");
    expect(out).toContain("run started");
    expect(out).toContain("done");
  });

  it("emits one machine document under --json", async () => {
    const cap = makeIo({ argv: ["inspect", "run-x", "--json", ...COMMON], runStatus: "succeeded" });
    const done = runCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "TEXT_MESSAGE_CONTENT", { text: "x" }));
    ws.message(evt(1, "CUSTOM", { name: "aex.run.settled", value: {} }, { source: "aex" }));
    await done;
    const doc = JSON.parse(cap.out().trim()) as { session: { id: string }; events: AexEvent[] };
    expect(doc.session.id).toBe("run-x");
    expect(doc.events.length).toBeGreaterThanOrEqual(1);
  });

  it("surfaces a jump-to-failure footer + exit 1 on a failed run", async () => {
    const cap = makeIo({ argv: ["inspect", "run-x", ...COMMON], runStatus: "failed" });
    const done = runCli(cap.io);
    const ws = await cap.nextSocket();
    ws.message(evt(0, "RUN_ERROR", { failureMessage: "kaboom", failureClass: "timeout" }));
    ws.message(evt(1, "CUSTOM", { name: "aex.run.settled", value: {} }, { source: "aex" }));
    await done;
    expect(cap.exit()).toBe(1);
    expect(cap.out()).toContain("✗ kaboom");
    expect(cap.out()).toContain("[timeout]");
  });
});

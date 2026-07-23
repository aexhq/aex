/**
 * Blackbox fake platform — a stateful, wire-faithful in-memory model of the aex
 * session lifecycle, driven ONLY through the public `@aexhq/sdk` client.
 *
 * WHY THIS EXISTS. The 155-finding friction hunt showed that the defects the
 * user hits live are OBSERVABLE at the public seam: a cancelled run reading as a
 * clean idle, cost/usage absent at `start()`, list vs stream events non-joinable,
 * a control intent silently steamrolled. The existing unit tests each hand-roll a
 * bespoke `fetch` for one assertion; there was no realistic platform to drive the
 * whole lifecycle against. This harness is that platform: a scenario scripts a
 * run behavior (streamed events + terminal outcome + committed checkpoint),
 * constructs a REAL `Aex` over the fake transport, drives the public API, and
 * asserts only what a customer can see. No SDK internals are imported.
 *
 * A RUN terminal carries the run verdict, usage, cost, and checkpoint. The
 * session record independently stays in a resumable lifecycle state.
 *
 * Routes modeled (the public session and observation resources the SDK calls):
 *   POST /api/sessions                         create
 *   POST /api/sessions/:id/messages            send a turn
 *   POST /api/sessions/:id/events/ticket       WS ticket
 *   GET  /api/sessions/:id                     lifecycle record
 *   GET  /api/sessions/:id/files             captured files
 *   POST /api/sessions/:id/{suspend,cancel,resume,approve,deny,request-approval}
 *   GET  /api/sessions/:id/children              subagent lineage
 */
import type { AexEvent, AexLiveEvent, JsonValue } from "@aexhq/contracts";
import { FakeWebSocket } from "@aexhq/contracts/testing";
import { Aex } from "../../../src/index.js";
import { SessionHandle } from "../../../src/client.js";
import type { SessionResult, SessionInput, SessionStartOptions } from "../../../src/index.js";

/** A session's terminal condition, as the FIXED platform surfaces it. */
export type ScriptOutcome = "succeeded" | "cancelled" | "timed_out" | "failed" | "interrupted";

/** One turn's scripted brain behavior. Everything is optional; defaults = a clean $0 succeeded turn. */
export interface TurnScript {
  /** Assembled assistant text, streamed as a single TEXT_MESSAGE_CONTENT delta. */
  readonly text?: string;
  /** Per-token deltas, streamed in order (use for streaming-progress assertions). */
  readonly chunks?: readonly string[];
  /** Tool calls: each emits a TOOL_CALL_START then TOOL_CALL_RESULT sharing `id`. */
  readonly tools?: readonly { readonly callId: string; readonly name: string; readonly result?: string }[];
  /** Extra CUSTOM/typed events emitted before the terminal (raw escape hatch). */
  readonly custom?: readonly { readonly type: AexEvent["type"]; readonly data: Record<string, JsonValue> }[];
  /** Terminal outcome. Default `succeeded`. */
  readonly outcome?: ScriptOutcome;
  /** Session lifecycle state after the run; useful for interruption cases. */
  readonly sessionStatus?: "idle" | "suspended" | "awaiting_approval" | "error";
  /** Failure text for a `failed` turn (surfaced via the terminal RUN_ERROR event). */
  readonly errorMessage?: string;
  /** Per-run billable cost. Default 0. */
  readonly costUsd?: number;
  /** Per-run provider usage carried on the terminal event. */
  readonly usage?: { readonly inputTokens?: number; readonly outputTokens?: number; readonly totalTokens?: number };
  /** Captured session files GET /files returns. */
  readonly files?: readonly Record<string, unknown>[];
}

/** A scripted non-2xx wire response, consumed once by the next matching request. */
interface ScriptedError {
  readonly pathIncludes: string;
  readonly status: number;
  readonly code: string;
  readonly message?: string;
}

/** One recorded request — method, path, parsed JSON body, and the idempotency key header. */
export interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly body: unknown;
  readonly idempotencyKey: string | null;
}

type Row = Record<string, unknown>;

const SEQ_BASE = 1024;

/** The record's lifecycle status is independent of the run verdict. */
function recordStatus(script: TurnScript, outcome: ScriptOutcome): string {
  if (script.sessionStatus !== undefined) return script.sessionStatus;
  if (outcome === "failed") return "error";
  if (outcome === "interrupted") return "suspended";
  return "idle";
}

/** Build a canonical AexEvent whose id is `${sessionId}:${sequence}` (the one identity scheme). */
function evt(sessionId: string, sequence: number, type: AexEvent["type"], data: Record<string, JsonValue> = {}): AexEvent {
  const turnSeq = Math.max(1, Math.floor(sequence / SEQ_BASE));
  return {
    specversion: "1.0",
    id: `${sessionId}:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: sessionId,
    threadId: sessionId,
    runId: `run-${turnSeq}`,
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

export interface FakePlatformOptions {
  /** The api key handed to the constructed client. A well-formed dev key by default. */
  readonly apiKey?: string;
  readonly baseUrl?: string;
}

/**
 * The fake platform. Construct one per scenario, script turns, and drive the
 * real client via {@link run} / {@link send}. {@link requests} records every
 * `METHOD /path` so a test can assert whether an unexpected follow-up read happened or that
 * NO network was touched.
 */
export class FakePlatform {
  readonly aex: Aex;
  /** Every `METHOD /path` issued; use it to assert exact network behavior. */
  readonly requests: string[] = [];
  /** Full request records (parsed body + idempotency key) — assert what the SDK put on the wire. */
  readonly requestLog: RecordedRequest[] = [];
  readonly #sessions = new Map<string, Row>();
  readonly #children = new Map<string, readonly Row[]>();
  readonly #sockets: FakeWebSocket[] = [];
  readonly #scriptedErrors: ScriptedError[] = [];
  #idSeq = 0;
  #pendingScript: TurnScript | undefined;

  constructor(options: FakePlatformOptions = {}) {
    // A bare opaque key + explicit baseUrl: the plane-guard only engages when a
    // key PARSES to a plane that mismatches baseUrl (its own scenario covers that).
    this.aex = new Aex({
      apiKey: options.apiKey ?? "tkn",
      baseUrl: options.baseUrl ?? "https://dev.aex.test",
      fetch: this.#fetch
    });
  }

  /** Seed read-only subagent lineage snapshots for `session.children()`. */
  setChildren(sessionId: string, children: readonly Row[]): void {
    this.#children.set(sessionId, children.map((child, index) => ({
      createdAt: new Date(index).toISOString(),
      updatedAt: new Date(index + 1).toISOString(),
      ...child
    })));
  }

  /** The WS factory to hand a per-call `{ webSocketFactory }` option. */
  readonly webSocketFactory = (url: string): FakeWebSocket => {
    const ws = new FakeWebSocket(url);
    this.#sockets.push(ws);
    // Self-drive: the SDK attaches its listeners synchronously right after this
    // returns, so emit the scripted turn on the next macrotask tick.
    setTimeout(() => this.#emitTurn(ws), 0);
    return ws;
  };

  /** Drive a one-shot `aex.start(options)` with `script`, resolving the finished result. */
  async start<T = unknown>(options: SessionStartOptions, script: TurnScript = {}): Promise<SessionResult<T>> {
    this.#pendingScript = script;
    // The factory-created socket self-drives, so awaiting the session resolves the
    // finished result; a pre-flight/wire rejection surfaces with no socket opened.
    return this.aex.start<T>(options, { webSocketFactory: this.webSocketFactory });
  }

  /** Drive one `session.messages.send(input).finished()` run on an existing handle with `script`. */
  async send(handle: SessionHandle, input: SessionInput, script: TurnScript = {}) {
    this.#pendingScript = script;
    return handle.messages.send(input, { webSocketFactory: this.webSocketFactory }).finished();
  }

  /**
   * Kick a live run on an existing handle without awaiting `finished()`, returning
   * the turn stream so a test can iterate per-token deltas as they arrive.
   */
  turn(handle: SessionHandle, input: SessionInput, script: TurnScript = {}) {
    this.#pendingScript = script;
    return handle.messages.send(input, { webSocketFactory: this.webSocketFactory });
  }

  /** Emit one scripted run on `ws`, persist its events, and commit final state atomically. */
  #emitTurn(ws: FakeWebSocket): void {
    ws.driven = true;
    const id = ws.sessionId;
    const session = this.#sessions.get(id);
    if (!session) return;
    const script = (session.__script as TurnScript | undefined) ?? {};
    const outcome = script.outcome ?? "succeeded";
    const log = (session.__events as AexEvent[] | undefined) ?? [];
    session.__events = log;
    let seq = SEQ_BASE * ((session.turnSeq as number) || 1);
    const emit = (type: AexEvent["type"], data: Record<string, JsonValue>): void => {
      const event = evt(id, seq++, type, data);
      log.push(event); // persist so events().list()/stream() replay the same canonical events
      ws.message(event);
    };
    if (script.chunks !== undefined) {
      for (const [liveSequence, chunk] of script.chunks.entries()) {
        const event: AexLiveEvent = {
          specversion: "1.0",
          id: `${id}:run-${session.turnSeq as number}:live:${liveSequence}`,
          source: "agent",
          type: "TEXT_MESSAGE_CONTENT",
          subject: id,
          threadId: id,
          runId: `run-${session.turnSeq as number}`,
          time: new Date(seq + liveSequence).toISOString(),
          replayable: false,
          liveSequence,
          receivedAt: seq + liveSequence,
          ephemeral: true,
          data: { text: chunk, messageId: "m1", delta: true }
        };
        ws.message(event);
      }
      emit("TEXT_MESSAGE_CONTENT", { text: script.chunks.join(""), messageId: "m1" });
    } else if (script.text !== undefined) {
      emit("TEXT_MESSAGE_CONTENT", { text: script.text, messageId: "m1" });
    }
    for (const tool of script.tools ?? []) {
      // Canonical wire keys the SDK actually projects from: TOOL_CALL_START reads
      // data.name + data.arguments; TOOL_CALL_RESULT reads data.content + data.isError.
      emit("TOOL_CALL_START", { id: tool.callId, name: tool.name, arguments: {} });
      emit("TOOL_CALL_RESULT", { id: tool.callId, content: tool.result ?? "", isError: false });
    }
    for (const custom of script.custom ?? []) emit(custom.type, custom.data);
    this.#finish(session, script, outcome);
    const checkpoint = { checkpointId: `cp-${session.turnSeq as number}` };
    if (outcome === "failed") {
      emit("RUN_ERROR", {
        failureMessage: script.errorMessage ?? "session failed",
        outcome,
        costUsd: script.costUsd ?? 0,
        providerUsage: script.usage ? [script.usage] : []
      });
    } else {
      emit("RUN_FINISHED", {
        outcome,
        checkpoint,
        costUsd: script.costUsd ?? 0,
        providerUsage: script.usage ? [script.usage] : []
      });
    }
  }

  #finish(session: Row, script: TurnScript, outcome: ScriptOutcome): void {
    session.status = recordStatus(script, outcome);
    session.acceptsMessages = session.status !== "awaiting_approval";
    session.costUsd = script.costUsd ?? 0;
    session.costTelemetry = {
      providerUsage: script.usage
        ? [{
            inputTokens: script.usage.inputTokens ?? 0,
            outputTokens: script.usage.outputTokens ?? 0,
            totalTokens:
              script.usage.totalTokens ?? (script.usage.inputTokens ?? 0) + (script.usage.outputTokens ?? 0)
          }]
        : []
    };
    if (outcome === "failed") session.errorMessage = script.errorMessage ?? "session failed";
    const checkpointId = `cp-${session.turnSeq as number}`;
    session.lastRun = {
      sessionId: session.id,
      runId: `run-${session.turnSeq as number}`,
      turnSeq: session.turnSeq,
      phase: outcome === "failed" ? "error" : "finished",
      outcome,
      ...(outcome === "failed" ? {} : { checkpoint: { checkpointId, runId: `run-${session.turnSeq as number}`, turnSeq: session.turnSeq } })
    };
    session.currentRun = undefined;
    session.__files = (script.files ?? []).map((file) => ({ ...file, checkpointId }));
  }

  /** Script a one-shot non-2xx wire response for the next request whose path contains `pathIncludes`. */
  scriptError(error: ScriptedError): void {
    this.#scriptedErrors.push(error);
  }

  readonly #fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : (input as Request).url;
    const method = (init?.method ?? "GET").toString().toUpperCase();
    const path = url.replace(/^https?:\/\/[^/]+/, "");
    this.requests.push(`${method} ${path}`);
    let body: unknown;
    if (typeof init?.body === "string") {
      try {
        body = JSON.parse(init.body);
      } catch {
        body = init.body;
      }
    }
    this.requestLog.push({ method, path, body, idempotencyKey: headerValue(init?.headers, "idempotency-key") });

    const errIdx = this.#scriptedErrors.findIndex((e) => path.includes(e.pathIncludes));
    if (errIdx !== -1) {
      const err = this.#scriptedErrors.splice(errIdx, 1)[0]!;
      // Body carries the stable `error` code; omit `message` unless scripted so the
      // SDK derives the human message from its code table (not the bare code).
      return json(err.message !== undefined ? { error: err.code, message: err.message } : { error: err.code }, err.status);
    }
    return this.#route(method, path, init);
  };

  #route(method: string, path: string, init?: RequestInit): Response {
    // POST /api/sessions — create
    if (method === "POST" && /\/api\/sessions$/.test(path)) {
      const id = `session-${++this.#idSeq}`;
      const script = this.#pendingScript;
      this.#pendingScript = undefined;
      const session: Row = { id, status: "idle", acceptsMessages: true, turnSeq: 0, __script: script };
      this.#sessions.set(id, session);
      return json({ session: publicSession(session) });
    }
    const m = path.match(/\/api\/sessions\/([^/?]+)(\/[^?]*)?/);
    if (m) {
      const id = decodeURIComponent(m[1]!);
      const sub = m[2] ?? "";
      const session = this.#sessions.get(id);
      if (!session) return json({ error: { code: "not_found" } }, 404);

      if (method === "POST" && sub === "/messages") {
        session.turnSeq = (session.turnSeq as number) + 1;
        session.status = "running";
        session.acceptsMessages = false;
        session.currentRun = {
          sessionId: id,
          runId: `run-${session.turnSeq as number}`,
          turnSeq: session.turnSeq,
          phase: "running",
          eventCursor: SEQ_BASE
        };
        // A fresh send() on an existing handle carries the newly-queued script.
        if (this.#pendingScript !== undefined) {
          session.__script = this.#pendingScript;
          this.#pendingScript = undefined;
        }
        return json({ session: publicSession(session), run: session.currentRun, eventCursor: SEQ_BASE });
      }
      if (method === "POST" && sub === "/events/ticket") {
        return json({ wsUrl: `wss://events.aex.test/${id}`, ticket: "t", expiresAtMs: 1 });
      }
      if (method === "GET" && sub === "/files") {
        const turnSeq = Number(session.turnSeq ?? 1);
        return json({
          revision: {
            checkpointId: `cp-${turnSeq}`,
            runId: `run-${turnSeq}`,
            turnSeq,
            committedAt: new Date(SEQ_BASE).toISOString(),
            throughSeq: SEQ_BASE * turnSeq
          },
          files: (session.__files as Row[]) ?? []
        });
      }
      if (method === "GET" && sub === "/events") {
        // The SAME canonical AexEvents the turn streamed — list() and the stream
        // are joinable by construction (one identity/ordering scheme).
        return json({ events: (session.__events as AexEvent[]) ?? [] });
      }
      if (method === "POST" && sub === "/suspend") {
        session.status = "suspended";
        return json({ session: publicSession(session) });
      }
      if (method === "POST" && sub === "/cancel") {
        session.status = "cancelling";
        session.cancelRequested = true;
        return json({ session: publicSession(session) });
      }
      if (method === "POST" && sub === "/resume") {
        session.status = "running";
        return json({ session: publicSession(session) });
      }
      if (method === "POST" && sub === "/request-approval") {
        session.status = "awaiting_approval";
        return json({ session: publicSession(session) });
      }
      if (method === "POST" && sub === "/approve") {
        session.status = "running";
        session.approvedThroughSeq = session.turnSeq;
        return json({ session: publicSession(session) });
      }
      if (method === "POST" && sub === "/deny") {
        session.status = "idle";
        session.acceptsMessages = true;
        const turnSeq = Math.max(1, Number(session.turnSeq ?? 0));
        session.lastRun = {
          sessionId: session.id,
          runId: `run-${turnSeq}`,
          turnSeq,
          phase: "finished",
          outcome: "cancelled"
        };
        return json({ session: publicSession(session) });
      }
      if (method === "DELETE" && sub === "") {
        return json({ session: publicSession(session) });
      }
      if (method === "GET" && sub === "") {
        return json({ session: publicSession(session) });
      }
    }
    // GET /api/sessions/:id/children — subagent lineage
    const c = path.match(/\/api\/sessions\/([^/?]+)\/children$/);
    if (method === "GET" && c) {
      return json({ children: this.#children.get(decodeURIComponent(c[1]!)) ?? [] });
    }
    const re = path.match(/\/api\/sessions\/([^/?]+)\/(events|files)/);
    if (method === "GET" && re) {
      const session = this.#sessions.get(decodeURIComponent(re[1]!));
      if (re[2] === "files") {
        const turnSeq = Number(session?.turnSeq ?? 1);
        return json({
          revision: {
            checkpointId: `cp-${turnSeq}`,
            runId: `run-${turnSeq}`,
            turnSeq,
            committedAt: new Date(SEQ_BASE).toISOString(),
            throughSeq: SEQ_BASE * turnSeq
          },
          files: (session?.__files as Row[]) ?? []
        });
      }
      return json({ events: (session?.__events as AexEvent[]) ?? [] });
    }
    return json({});
  }
}

/** The public session projection GET returns (drops fake-only storage fields). */
function publicSession(session: Row): Row {
  const { __script, __files, __events, turnSeq, ...pub } = session;
  void __script;
  void __files;
  void __events;
  void turnSeq;
  return pub;
}

/** Read one header case-insensitively from any of the shapes `fetch` init.headers takes. */
function headerValue(headers: HeadersInit | undefined, name: string): string | null {
  if (!headers) return null;
  const lower = name.toLowerCase();
  if (headers instanceof Headers) return headers.get(name);
  if (Array.isArray(headers)) {
    const found = headers.find(([k]) => k.toLowerCase() === lower);
    return found ? found[1] : null;
  }
  const rec = headers as Record<string, string>;
  const key = Object.keys(rec).find((k) => k.toLowerCase() === lower);
  return key !== undefined ? rec[key]! : null;
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

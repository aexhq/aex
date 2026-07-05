/**
 * Blackbox fake platform — a stateful, wire-faithful in-memory model of the aex
 * session lifecycle, driven ONLY through the public `@aexhq/sdk` client.
 *
 * WHY THIS EXISTS. The 155-finding friction hunt showed that the defects the
 * user hits live are OBSERVABLE at the public seam: a cancelled run reading as a
 * clean idle, cost/usage absent at `run()`, list vs stream events non-joinable,
 * a control intent silently steamrolled. The existing unit tests each hand-roll a
 * bespoke `fetch` for one assertion; there was no realistic platform to drive the
 * whole lifecycle against. This harness is that platform: a scenario scripts a
 * turn's brain behavior (streamed events + terminal outcome + settle stamp),
 * constructs a REAL `Aex` over the fake transport, drives the public API, and
 * asserts only what a customer can see. No SDK internals are imported.
 *
 * The one invariant it models is the fix's spine: a turn streams to a PARK event,
 * then SETTLES (costUsd + usage + lastTurnOutcome + settledAt written atomically),
 * so a default await-settle `run()`/`done()` always reads cost, usage, and the
 * real terminal OUTCOME — never a lossy `idle`. The record's lifecycle `status`
 * stays `idle`/`suspended` for a resumable session; a terminal turn writes the
 * outcome (`succeeded`/`cancelled`/`timed_out`/`failed`).
 *
 * Routes modeled (the session + run facade the SDK actually calls):
 *   POST /api/sessions                         create
 *   POST /api/sessions/:id/messages            send a turn
 *   POST /api/sessions/:id/events/ticket       WS ticket
 *   GET  /api/sessions/:id                     settle poll (settled record)
 *   GET  /api/sessions/:id/outputs             captured outputs
 *   POST /api/sessions/:id/{suspend,cancel,resume,approve,deny,request-approval}
 *   GET  /api/runs/:id/children                subagent lineage
 *   GET  /api/runs/:id                          run-facade resolve (child)
 */
import type { AexEvent, JsonValue, WebSocketLike } from "@aexhq/contracts";
import { Aex, SessionHandle } from "../../../src/index.js";
import type { BatchResult, RunResult, SessionInput, SessionRunOptions } from "../../../src/index.js";

/** A run's terminal condition, as the FIXED platform surfaces it. */
export type ScriptOutcome = "succeeded" | "cancelled" | "timed_out" | "failed" | "suspended";

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
  /** Failure text for a `failed` turn (surfaced via the terminal RUN_ERROR event). */
  readonly errorMessage?: string;
  /** Settle-stamped billable cost. Default 0 (a $0 turn still settles, never hangs). */
  readonly costUsd?: number;
  /** Settle-stamped token usage → costTelemetry.providerUsage (the single token SSoT). */
  readonly usage?: { readonly inputTokens?: number; readonly outputTokens?: number; readonly totalTokens?: number };
  /** Captured output files GET /outputs returns. */
  readonly outputs?: readonly Record<string, unknown>[];
  /**
   * Model the real park→settle LAG: the park event fires with the record still
   * UNSETTLED; GET /api/sessions/:id only returns the settled record after a poll.
   * Use to prove run()/done() actually AWAIT the settle commit (default: settle
   * synchronously, for fast common-case tests).
   */
  readonly settleLag?: boolean;
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

/** The terminal CUSTOM `aex.session.<name>` event name for a park outcome. */
function parkName(outcome: ScriptOutcome): string {
  // A clean turn parks the session `idle` (resumable); the SDK reads idle→succeeded.
  return outcome === "succeeded" ? "idle" : outcome;
}

/** The record's lifecycle `status` after a settled turn (idle/suspended stay resumable). */
function recordStatus(outcome: ScriptOutcome): string {
  if (outcome === "succeeded") return "idle";
  return outcome; // cancelled | timed_out | failed | suspended
}

/** Build a canonical AexEvent whose id is `${runId}:${sequence}` (the one identity scheme). */
function evt(runId: string, sequence: number, type: AexEvent["type"], data: Record<string, JsonValue> = {}): AexEvent {
  return {
    specversion: "1.0",
    id: `${runId}:${sequence}`,
    source: type === "CUSTOM" ? "runtime" : "agent",
    type,
    subject: runId,
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

/** A minimal in-memory WebSocket matching the SDK's `WebSocketLike` contract. */
class FakeWebSocket implements WebSocketLike {
  readonly url: string;
  readonly sessionId: string;
  driven = false;
  readonly #listeners: Record<string, Array<(ev: { data?: unknown }) => void>> = {};

  constructor(url: string) {
    this.url = url;
    // The coordinator URL carries `?ticket=&from=` query params — strip them so
    // the last path segment is the bare session id.
    this.sessionId = (url.split("?")[0] ?? url).split("/").pop() ?? "";
  }

  addEventListener(type: "open" | "message" | "close" | "error", cb: (ev: { data?: unknown }) => void): void {
    (this.#listeners[type] ??= []).push(cb);
  }

  removeEventListener(): void {
    /* no-op for the fake */
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

export interface FakePlatformOptions {
  /** The api key handed to the constructed client. A well-formed dev key by default. */
  readonly apiKey?: string;
  readonly baseUrl?: string;
}

/**
 * The fake platform. Construct one per scenario, script turns, and drive the
 * real client via {@link run} / {@link send}. {@link requests} records every
 * `METHOD /path` so a test can assert (e.g.) that a settle poll happened or that
 * NO network was touched.
 */
export class FakePlatform {
  readonly aex: Aex;
  /** Every `METHOD /path` issued — assert a settle poll happened, or that NO network was touched. */
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

  /** Seed subagent children for a run, resolvable via `session.children()`. */
  setChildren(runId: string, children: readonly Row[]): void {
    this.#children.set(runId, children);
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

  /** Drive a one-shot `aex.run(options)` with `script`, resolving the settled result. */
  async run<T = unknown>(options: SessionRunOptions, script: TurnScript = {}): Promise<RunResult<T>> {
    this.#pendingScript = script;
    // The factory-created socket self-drives, so awaiting the run resolves the
    // settled result; a pre-flight/wire rejection surfaces with no socket opened.
    return this.aex.run<T>(options, { webSocketFactory: this.webSocketFactory });
  }

  /** Drive one `session.send(input).done()` turn on an existing handle with `script`. */
  async send(handle: SessionHandle, input: SessionInput, script: TurnScript = {}) {
    this.#pendingScript = script;
    return handle.send(input, { webSocketFactory: this.webSocketFactory }).done();
  }

  /**
   * Kick a LIVE turn on an existing handle WITHOUT awaiting `done()`, returning
   * the turn stream so a test can iterate per-token deltas as they arrive.
   */
  turn(handle: SessionHandle, input: SessionInput, script: TurnScript = {}) {
    this.#pendingScript = script;
    return handle.send(input, { webSocketFactory: this.webSocketFactory });
  }

  /**
   * Drive `aex.batch(items)` where every item runs `script` (or a per-index
   * script). `batch()` calls its internal `run()` with the DEFAULT WebSocket
   * (no per-call factory), so install the fake globally for the duration; the
   * per-session script is keyed by creation order.
   */
  async batch<T = unknown>(
    items: readonly SessionRunOptions[],
    scripts: readonly TurnScript[] | TurnScript = {}
  ): Promise<BatchResult<T>> {
    const scriptFor = (i: number): TurnScript => (Array.isArray(scripts) ? (scripts[i] ?? {}) : (scripts as TurnScript));
    let created = 0;
    this.#onCreate = (): TurnScript => scriptFor(created++);
    this.#installGlobalSocket();
    try {
      return await this.aex.batch<T>(items);
    } finally {
      this.#restoreGlobalSocket();
      this.#onCreate = undefined;
    }
  }

  #onCreate: (() => TurnScript) | undefined;
  #priorWebSocket: unknown;

  /** Route `new WebSocket(url)` (the default, factory-less path `batch` uses) through the fake. */
  #installGlobalSocket(): void {
    const factory = this.webSocketFactory;
    const g = globalThis as { WebSocket?: unknown };
    this.#priorWebSocket = g.WebSocket;
    g.WebSocket = class {
      constructor(url: string) {
        return factory(url) as unknown as object;
      }
    };
  }

  #restoreGlobalSocket(): void {
    (globalThis as { WebSocket?: unknown }).WebSocket = this.#priorWebSocket;
  }

  /** Emit one scripted turn on `ws`, PERSIST the events (for list()/stream()), and SETTLE atomically. */
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
    const chunks = script.chunks ?? (script.text != null ? [script.text] : []);
    for (const chunk of chunks) emit("TEXT_MESSAGE_CONTENT", { text: chunk, messageId: "m1" });
    for (const tool of script.tools ?? []) {
      // Canonical wire keys the SDK actually projects from: TOOL_CALL_START reads
      // data.name + data.arguments; TOOL_CALL_RESULT reads data.content + data.isError.
      emit("TOOL_CALL_START", { id: tool.callId, name: tool.name, arguments: {} });
      emit("TOOL_CALL_RESULT", { id: tool.callId, content: tool.result ?? "", isError: false });
    }
    for (const custom of script.custom ?? []) emit(custom.type, custom.data);
    if (script.settleLag) {
      // Model the real park→settle LAG: emit the park now with the record still
      // UNSETTLED (no settledAt/costUsd/lastTurnOutcome), and let GET /api/sessions/:id
      // flip it to settled only after a poll — so a caller that fails to await the
      // settle commit reads an incomplete record. Settle-await is exercised for real.
      session.__pendingSettle = { script, outcome, reads: 0 };
      session.status = "idle";
    } else {
      // SETTLE synchronously BEFORE the terminal event so the fast common-case
      // tests do not pay a poll round-trip; the dedicated settle-lag scenario
      // opts into the deferred path above.
      this.#settle(session, script, outcome);
    }
    if (outcome === "failed") {
      emit("RUN_ERROR", { failureMessage: script.errorMessage ?? "run failed" });
    } else {
      emit("CUSTOM", { name: `aex.session.${parkName(outcome)}`, value: { turnSeq: Number(session.turnSeq ?? 1) } });
    }
  }

  #settle(session: Row, script: TurnScript, outcome: ScriptOutcome): void {
    session.status = recordStatus(outcome);
    session.lastTurnOutcome = outcome;
    session.costUsd = script.costUsd ?? 0;
    session.settledAt = new Date(SEQ_BASE).toISOString();
    if (script.usage) {
      session.costTelemetry = {
        providerUsage: [
          {
            inputTokens: script.usage.inputTokens ?? 0,
            outputTokens: script.usage.outputTokens ?? 0,
            totalTokens:
              script.usage.totalTokens ?? (script.usage.inputTokens ?? 0) + (script.usage.outputTokens ?? 0)
          }
        ]
      };
    }
    if (outcome === "failed") session.errorMessage = script.errorMessage ?? "run failed";
    session.__outputs = script.outputs ?? [];
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
      const id = `run-${++this.#idSeq}`;
      const script = this.#onCreate ? this.#onCreate() : this.#pendingScript;
      this.#pendingScript = undefined;
      const session: Row = { id, sessionId: id, status: "idle", turnSeq: 0, __script: script };
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
        // A fresh send() on an existing handle carries the newly-queued script.
        if (this.#pendingScript !== undefined) {
          session.__script = this.#pendingScript;
          this.#pendingScript = undefined;
        }
        return json({ session: publicSession(session), turn: { sessionId: id, turnSeq: session.turnSeq }, eventCursor: SEQ_BASE });
      }
      if (method === "POST" && sub === "/events/ticket") {
        return json({ wsUrl: `wss://events.aex.test/${id}`, ticket: "t", expiresAtMs: 1 });
      }
      if (method === "GET" && sub === "/outputs") {
        return json({ outputs: (session.__outputs as Row[]) ?? [] });
      }
      if (method === "GET" && sub === "/events") {
        // The SAME canonical AexEvents the turn streamed — list() and the stream
        // are joinable by construction (one identity/ordering scheme).
        return json({ events: (session.__events as AexEvent[]) ?? [] });
      }
      if (method === "POST" && sub === "/suspend") {
        session.status = "suspended";
        session.lastTurnOutcome = undefined;
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
        session.status = "cancelled";
        session.lastTurnOutcome = "cancelled";
        return json({ session: publicSession(session) });
      }
      if (method === "DELETE" && sub === "") {
        return json({ session: publicSession(session) });
      }
      if (method === "GET" && sub === "") {
        // Deferred settle: the first post-park read sees the UNSETTLED record; a
        // later poll flips it to settled — so run()/done() must await the commit.
        const pending = session.__pendingSettle as { script: TurnScript; outcome: ScriptOutcome; reads: number } | undefined;
        if (pending) {
          pending.reads += 1;
          if (pending.reads >= 2) {
            this.#settle(session, pending.script, pending.outcome);
            session.__pendingSettle = undefined;
          }
        }
        return json({ session: publicSession(session) });
      }
    }
    // GET /api/runs/:id/children — subagent lineage
    const c = path.match(/\/api\/runs\/([^/?]+)\/children$/);
    if (method === "GET" && c) {
      return json({ children: this.#children.get(decodeURIComponent(c[1]!)) ?? [] });
    }
    // GET /api/runs/:id — run-facade resolve (a child)
    const r = path.match(/\/api\/runs\/([^/?]+)$/);
    if (method === "GET" && r) {
      const id = decodeURIComponent(r[1]!);
      const session = this.#sessions.get(id);
      return json({ run: session ? publicSession(session) : { id, status: "idle" } });
    }
    const re = path.match(/\/api\/runs\/([^/?]+)\/(events|outputs)/);
    if (method === "GET" && re) {
      const session = this.#sessions.get(decodeURIComponent(re[1]!));
      if (re[2] === "outputs") return json({ outputs: (session?.__outputs as Row[]) ?? [] });
      return json({ events: (session?.__events as AexEvent[]) ?? [] });
    }
    return json({});
  }
}

/** The public session projection GET returns (drops the internal `__script`). */
function publicSession(session: Row): Row {
  const { __script, __outputs, __events, __pendingSettle, ...pub } = session;
  void __script;
  void __outputs;
  void __events;
  void __pendingSettle;
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

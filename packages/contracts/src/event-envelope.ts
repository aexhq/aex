/**
 * The unified aex event envelope.
 *
 * One versioned, self-describing record that every subscriber sees, derived
 * from the unified {@link RunnerEvent}. Managed runtime adapters emit
 * byte-identical envelopes for the same logical event by construction. This is
 * the shape the coordinator (Phase 2) appends, broadcasts, and archives.
 *
 *   - **CloudEvents-shaped** self-describing envelope: a stable `id`, the
 *     coarse `source`, the AG-UI-aligned `type`, the `subject` (run), a
 *     `time`, the `sequence` cursor, and the typed `data`.
 *   - **AG-UI vocabulary** for `type` where it maps; aex-specific events
 *     ride AG-UI's reserved `CUSTOM` carrier under an `aex.*` name, so an
 *     off-the-shelf AG-UI client reads an aex run with no glue.
 *   - Two aex extensions: a coarse `source` (filter first by origin) and
 *     an optional human `message` (log / CLI / dashboard rendering).
 *
 * Unified observability spine:
 * the envelope additionally carries four ordering attributes so BOTH the typed
 * event stream and the high-volume hosted-platform log stream ride one per-run
 * coordinator:
 *   - `channel`   — "event" (the typed AG-UI stream) or "log" (a verbose log
 *                   line). The single axis a consumer splits the unified stream
 *                   on. Absent ⇒ "event" (back-compat for existing producers).
 *   - `sourceSeq` — a per-SOURCE monotonic counter assigned AT the source. The
 *                   DO's hard guarantee is that records of the same source are
 *                   never reordered relative to their `sourceSeq`.
 *   - `emittedAt` — source wall-clock ms at emit. Carried, never trusted for
 *                   cross-source ordering (clocks are independent; the coordinator gives
 *                   no synchronized clock). A client may re-sort by it for a
 *                   best-effort time view.
 * The coordinator remains the single serial ordering authority: it assigns the global
 * `sequence` on arrival, which is the canonical stream order. `sourceSeq` /
 * `emittedAt` are CARRIED through broadcast + archive, not used to reorder.
 *
 * This module is **additive**: it does not change {@link RunnerEvent} or any
 * existing wire shape. It is a pure projection plus honest guards and a strict
 * AG-UI projection for consumers.
 */

import type { JsonValue } from "./submission.js";
import type { RunnerEvent } from "./runner-event.js";

/** CloudEvents `specversion` the envelope conforms to. */
export const AEX_EVENT_SPECVERSION = "1.0" as const;

/**
 * Mapping version. Bump when the RunnerEvent → envelope projection changes
 * shape (new `source`/`type`, renamed data field). Independent of
 * {@link AEX_EVENT_SPECVERSION} (the CloudEvents version) and of
 * `RUNNER_EVENT_VERSION` (the upstream wire version).
 */
export const AEX_EVENT_MAP_VERSION = 1 as const;

/**
 * Coarse origin classifier — the first axis a consumer filters on.
 *   - `agent`   — the model: text, reasoning, builtin tool calls/results.
 *   - `worker`  — the hosted aex edge itself.
 *   - `runtime` — the execution runtime: lifecycle, diagnostics, non-fatal
 *                 stream errors.
 *   - `mcp`     — an MCP server (a tool call/result routed through MCP).
 *   - `aex` — the platform: skills, files, and other aex-native events.
 *   - `workflow`— the orchestration layer.
 *   - `host`    — the managed host the runtime executes on; distinct from the
 *                 runtime process itself.
 */
export const AEX_EVENT_SOURCES = ["agent", "worker", "runtime", "mcp", "aex", "workflow", "host"] as const;
export type AexEventSource = (typeof AEX_EVENT_SOURCES)[number];

/**
 * The channel a record rides on the unified per-run stream:
 *   - `event` — the typed, low-volume, fully-replayed AG-UI event stream.
 *   - `log`   — a high-volume verbose log line (level + message + fields). The
 *               coordinator prunes flushed log rows after evidence archival
 *               (logs are append-only, events are kept). Absent on the wire ⇒
 *               `event`.
 */
export const AEX_EVENT_CHANNELS = ["event", "log"] as const;
export type AexEventChannel = (typeof AEX_EVENT_CHANNELS)[number];

/**
 * Log severity carried by a `channel: "log"` record (the `LOG` event type).
 * NOTE: the platform owner's canonical term for the middle level is "warning";
 * we keep "warn" for consistency with the existing in-code vocabulary — the
 * mapping is `warning ≡ warn`.
 */
export const AEX_LOG_LEVELS = ["info", "warn", "error"] as const;
export type AexLogLevel = (typeof AEX_LOG_LEVELS)[number];

/**
 * The AG-UI-aligned `type` vocabulary the envelope emits. A subset of the
 * full AG-UI protocol — the events aex actually produces today — plus
 * `CUSTOM`, AG-UI's reserved carrier for aex-native events.
 */
export const AEX_EVENT_TYPES = [
  "RUN_STARTED",
  "RUN_FINISHED",
  "RUN_ERROR",
  "TEXT_MESSAGE_CONTENT",
  "TOOL_CALL_START",
  "TOOL_CALL_RESULT",
  "CUSTOM",
  // The carrier type for a `channel: "log"` record. Kept out of the AG-UI
  // typed-event vocabulary on purpose: a `LOG` is never a run-lifecycle signal,
  // so terminal detection (RUN_FINISHED/RUN_ERROR) is unaffected and an
  // off-the-shelf AG-UI client filters logs out by `channel`.
  "LOG"
] as const;
export type AexEventType = (typeof AEX_EVENT_TYPES)[number];

/**
 * One event on the unified log. CloudEvents core attributes (`specversion`,
 * `id`, `source`, `type`, `subject`, `time`) plus the `sequence` extension
 * (the ordering cursor) and the typed `data`.
 */
export interface AexEvent {
  /** CloudEvents specversion. Always {@link AEX_EVENT_SPECVERSION}. */
  readonly specversion: typeof AEX_EVENT_SPECVERSION;
  /** Stable, globally-unique event id: `${runId}:${sequence}`. Dedupe key. */
  readonly id: string;
  /** Coarse origin classifier. */
  readonly source: AexEventSource;
  /** AG-UI-aligned event type. */
  readonly type: AexEventType;
  /** The run this event belongs to (CloudEvents `subject`). */
  readonly subject: string;
  /** ISO-8601 event time (run base + the RunnerEvent's relative `tMs`). */
  readonly time: string;
  /**
   * Monotonic ordering cursor within the run — the GLOBAL `seq` the coordinator
   * assigns on arrival. This is the canonical stream order and the dedupe key.
   */
  readonly sequence: number;
  /**
   * Which sub-stream this record rides. Absent ⇒ `"event"` (existing typed
   * producers don't set it; the coordinator defaults it on ingest).
   */
  readonly channel?: AexEventChannel;
  /**
   * Per-SOURCE monotonic counter assigned at the source. The coordinator
   * preserves the order of same-source records by `sourceSeq` and never
   * reorders them; cross-source order is by arrival (`sequence`) only.
   */
  readonly sourceSeq?: number;
  /** Source wall-clock ms at emit. Carried for a best-effort client time view. */
  readonly emittedAt?: number;
  /**
   * The DO's authoritative receive time (wall-clock ms), stamped at ingest
   * alongside the global {@link sequence}. This is the authoritative wall-clock
   * companion to `sequence` — distinct from `emittedAt` (the SOURCE's clock) and
   * `time` (the LOGICAL time = run base + relative tMs).
   *
   * NOTE: some edge clocks are coarsened/frozen-at-I/O, so `receivedAt` is
   * precisely "the coordinator's last-I/O wall-clock at ingest" — that is fine
   * as the authoritative receive marker. It does NOT need to exceed `emittedAt`:
   * the source runs on a different host with an independent clock, so cross-host
   * drift can leave `receivedAt < emittedAt` and that is expected, not an error.
   * Absent on records produced before this field existed (back-compat with
   * archived records).
   */
  readonly receivedAt?: number;
  /**
   * Log severity, first-class on a `channel: "log"` record (mirrors
   * `data.level`). Optional: only `log`-channel records carry it; typed `event`
   * records omit it. ("warn" ≡ the owner's "warning" — see {@link AEX_LOG_LEVELS}.)
   */
  readonly level?: AexLogLevel;
  /** Optional human-readable summary for log / CLI / dashboard rendering. */
  readonly message?: string;
  /**
   * Typed payload. For `CUSTOM` events this is `{ name, value }` (AG-UI's
   * custom carrier, `name` = `aex.<kind>`); for typed events it is the
   * RunnerEvent's own data, projected to AG-UI fields by {@link toAGUI}.
   */
  readonly data: Readonly<Record<string, JsonValue>>;
}

/** Context the mapper needs to stamp absolute identity/time onto an event. */
export interface AexEventContext {
  readonly runId: string;
  /**
   * Run-start epoch ms. The RunnerEvent's `tMs` is relative to this, so
   * `time = new Date(baseMs + tMs)`. Pass the run's `createdAtMs`.
   */
  readonly baseMs: number;
}

interface Projection {
  readonly type: AexEventType;
  readonly source: AexEventSource;
  readonly message?: string;
  readonly data: Record<string, JsonValue>;
}

/**
 * Project a {@link RunnerEvent} onto the unified envelope. Pure and total:
 * every RunnerEvent kind maps to exactly one envelope. Both runtimes feed
 * RunnerEvents through this same function, so identical logical events
 * produce identical envelopes.
 */
export function runnerEventToAexEvent(evt: RunnerEvent, ctx: AexEventContext): AexEvent {
  const projection = project(evt);
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: `${ctx.runId}:${evt.seq}`,
    source: projection.source,
    type: projection.type,
    subject: ctx.runId,
    time: new Date(ctx.baseMs + evt.tMs).toISOString(),
    sequence: evt.seq,
    ...(projection.message !== undefined ? { message: projection.message } : {}),
    data: Object.freeze(projection.data)
  };
}

function project(evt: RunnerEvent): Projection {
  const data = evt.data;
  switch (evt.kind) {
    case "runtime_started":
      return { type: "RUN_STARTED", source: "runtime", message: "run started", data: { ...data } };
    case "assistant_text": {
      const text = str(data.text);
      return {
        type: "TEXT_MESSAGE_CONTENT",
        source: "agent",
        ...(text ? { message: clip(text) } : {}),
        data: { ...data }
      };
    }
    case "tool_request": {
      const name = str(data.name);
      return {
        type: "TOOL_CALL_START",
        source: data.extension ? "mcp" : "agent",
        ...(name ? { message: `tool ${name}` } : {}),
        data: { ...data }
      };
    }
    case "tool_response":
      return {
        type: "TOOL_CALL_RESULT",
        source: data.extension ? "mcp" : "agent",
        data: { ...data }
      };
    case "skill_loaded":
      return custom("aex.skill_loaded", "aex", data, "skill loaded");
    case "file_uploaded":
      return custom("aex.file_uploaded", "aex", data, "file uploaded");
    case "notification":
      return custom("aex.notification", "runtime", data, str(data.reason) || undefined);
    case "stream_error":
      return custom("aex.stream_error", "runtime", data, str(data.message) || "stream error");
    case "runtime_terminal": {
      const reason = str(data.reason);
      return {
        // The event's own reason types it (error vs finished); the
        // authoritative run *status* is still owned by the orchestrator and
        // is never re-derived from this event.
        type: reason === "error" ? "RUN_ERROR" : "RUN_FINISHED",
        source: "runtime",
        message: reason ? `run ${reason}` : "run finished",
        data: { ...data }
      };
    }
  }
}

// --- Log channel ----------------------------------------------------------
// A worker/workflow log line is a record on the unified stream's `log` channel,
// distinct from the typed `event` channel. The coordinator is still the seq
// authority; the producer supplies `source`/`sourceSeq`/`emittedAt` and a
// minimal payload (level + message + optional fields).

/** A log line as a producer hands it to {@link logToInbound} (pre-coordinator). */
export interface AexLogLine {
  readonly level: AexLogLevel;
  readonly message: string;
  readonly fields?: Readonly<Record<string, JsonValue>>;
  /** Source wall-clock ms at emit. */
  readonly emittedAt: number;
  /** Per-source monotonic counter assigned at the source. */
  readonly sourceSeq: number;
}

/**
 * The inbound (pre-seq) envelope a producer POSTs for a `channel: "log"` line.
 * The coordinator stamps `specversion`/`id`/`subject`/`sequence` (the ordering
 * authority) AND `receivedAt` (its authoritative receive time) on ingest, so a
 * producer never supplies any of them.
 */
export type AexInboundLog = Omit<
  AexEvent,
  "specversion" | "id" | "subject" | "sequence" | "receivedAt"
>;

/**
 * Project a log line onto an inbound coordinator envelope. The coordinator
 * stamps `specversion`/`id`/`subject`/`sequence` on ingest (it is the ordering
 * authority); everything else — `channel`, `source`, `sourceSeq`, `emittedAt`,
 * and the `LOG` payload — is supplied here.
 */
export function logToInbound(source: AexEventSource, line: AexLogLine): AexInboundLog {
  return {
    source,
    type: "LOG",
    channel: "log",
    sourceSeq: line.sourceSeq,
    emittedAt: line.emittedAt,
    // First-class severity on the envelope (not only inside `data`). `data.level`
    // is kept too so an existing `data`-reading consumer still works.
    level: line.level,
    time: new Date(line.emittedAt).toISOString(),
    message: line.message,
    data: {
      level: line.level,
      message: line.message,
      ...(line.fields ? { fields: { ...line.fields } } : {})
    }
  };
}

function custom(
  name: string,
  source: AexEventSource,
  value: Readonly<Record<string, JsonValue>>,
  message?: string
): Projection {
  return {
    type: "CUSTOM",
    source,
    ...(message !== undefined ? { message } : {}),
    data: { name, value: { ...value } }
  };
}

// --- Honest guards over the emitted vocabulary --------------------------------
// These match the vocabulary a consumer of the unified stream actually receives.

export function isRunStarted(e: AexEvent): boolean {
  return e.type === "RUN_STARTED";
}
export function isRunFinished(e: AexEvent): boolean {
  return e.type === "RUN_FINISHED";
}
export function isRunError(e: AexEvent): boolean {
  return e.type === "RUN_ERROR";
}
/** A terminal event of either flavour (finished or error). */
export function isRunTerminal(e: AexEvent): boolean {
  return e.type === "RUN_FINISHED" || e.type === "RUN_ERROR";
}
export function isTextMessage(e: AexEvent): boolean {
  return e.type === "TEXT_MESSAGE_CONTENT";
}
export function isToolCallStart(e: AexEvent): boolean {
  return e.type === "TOOL_CALL_START";
}
export function isToolCallResult(e: AexEvent): boolean {
  return e.type === "TOOL_CALL_RESULT";
}
export function isCustom(e: AexEvent): boolean {
  return e.type === "CUSTOM";
}
/** The `aex.*` name of a CUSTOM event, or null for typed events. */
export function customName(e: AexEvent): string | null {
  return e.type === "CUSTOM" ? str(e.data.name) || null : null;
}
export function isFromSource(e: AexEvent, source: AexEventSource): boolean {
  return e.source === source;
}
/** The channel a record rides, defaulting an absent value to `"event"`. */
export function channelOf(e: AexEvent): AexEventChannel {
  return e.channel ?? "event";
}
/** True when a record is a log line (the `log` channel / `LOG` type). */
export function isLog(e: AexEvent): boolean {
  return channelOf(e) === "log";
}
/** True when a record is a typed AG-UI event (the `event` channel). */
export function isEventChannel(e: AexEvent): boolean {
  return channelOf(e) === "event";
}

// --- Oversized-payload rule (2 MB SQLite row cap) -----------------------------

/**
 * The coordinator's embedded store caps a
 * single row at 2 MB. Events whose serialized form exceeds the budget must be
 * split before insert (the coordinator's responsibility); the archive uses the
 * same bound. A conservative margin under the hard 2 MiB leaves room for row
 * overhead and column framing.
 */
export const MAX_SQLITE_ROW_BYTES = 2_000_000 as const;

/** Serialized UTF-8 byte length of an event (the size the row must hold). */
export function serializedEventBytes(e: AexEvent): number {
  return new TextEncoder().encode(JSON.stringify(e)).byteLength;
}

/** True when an event's serialized form exceeds the row budget and must be split. */
export function exceedsRowBudget(e: AexEvent, max: number = MAX_SQLITE_ROW_BYTES): boolean {
  return serializedEventBytes(e) > max;
}

// --- Strict AG-UI projection --------------------------------------------------

/**
 * AG-UI events aex projects to. Each carries the AG-UI `type` discriminant
 * and a numeric `timestamp` (ms), plus the type-specific fields an off-the-shelf
 * AG-UI client expects. Aex's envelope extensions (`source`, `message`, the
 * CloudEvents framing) are dropped — `rawEvent` carries the original for clients
 * that want it.
 */
export type AguiEvent =
  | { type: "RUN_STARTED"; timestamp: number; threadId: string; runId: string }
  | { type: "RUN_FINISHED"; timestamp: number; threadId: string; runId: string }
  | { type: "RUN_ERROR"; timestamp: number; message: string; code?: string }
  | { type: "TEXT_MESSAGE_CONTENT"; timestamp: number; messageId: string; delta: string }
  | { type: "TOOL_CALL_START"; timestamp: number; toolCallId: string; toolCallName: string }
  | { type: "TOOL_CALL_RESULT"; timestamp: number; messageId: string; toolCallId: string; content: JsonValue }
  | { type: "CUSTOM"; timestamp: number; name: string; value: JsonValue };

/**
 * Project an aex envelope to a strict AG-UI event so an off-the-shelf
 * AG-UI client can consume an aex run with no glue. This is the
 * client-side projection the SDK exposes.
 */
export function toAGUI(e: AexEvent): AguiEvent {
  const timestamp = Date.parse(e.time);
  const d = e.data;
  switch (e.type) {
    case "RUN_STARTED":
      return { type: "RUN_STARTED", timestamp, threadId: e.subject, runId: e.subject };
    case "RUN_FINISHED":
      return { type: "RUN_FINISHED", timestamp, threadId: e.subject, runId: e.subject };
    case "RUN_ERROR": {
      const code = str(d.failureClass);
      return {
        type: "RUN_ERROR",
        timestamp,
        message: str(d.failureMessage) || e.message || "run error",
        ...(code ? { code } : {})
      };
    }
    case "TEXT_MESSAGE_CONTENT":
      return {
        type: "TEXT_MESSAGE_CONTENT",
        timestamp,
        messageId: str(d.messageId) || str(d.eventId) || e.id,
        delta: str(d.text)
      };
    case "TOOL_CALL_START":
      return {
        type: "TOOL_CALL_START",
        timestamp,
        toolCallId: str(d.id) || e.id,
        toolCallName: str(d.name)
      };
    case "TOOL_CALL_RESULT":
      return {
        type: "TOOL_CALL_RESULT",
        timestamp,
        messageId: str(d.messageId) || e.id,
        toolCallId: str(d.id) || e.id,
        content: d.content ?? null
      };
    case "CUSTOM":
      return { type: "CUSTOM", timestamp, name: str(d.name), value: d.value ?? null };
    case "LOG":
      // Logs ride the `log` channel and are normally filtered out before
      // projection. If a consumer projects one anyway, carry it under AG-UI's
      // reserved CUSTOM so the client still receives a valid record.
      return { type: "CUSTOM", timestamp, name: "aex.log", value: { ...d } };
  }
}

function str(v: JsonValue | undefined): string {
  return typeof v === "string" ? v : "";
}

function clip(s: string, max = 200): string {
  return s.length <= max ? s : `${s.slice(0, max - 1)}…`;
}

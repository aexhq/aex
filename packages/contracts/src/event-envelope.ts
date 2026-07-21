/**
 * The unified aex event envelope.
 *
 * One versioned, self-describing record that every subscriber sees, derived
 * from the unified {@link RunnerEvent}. Managed runtime adapters emit
 * byte-identical envelopes for the same logical event by construction. This is
 * the shape the coordinator (Phase 2) appends, broadcasts, and archives.
 *
 *   - **CloudEvents-shaped** self-describing envelope: a stable `id`, the
 *     coarse `source`, the AG-UI-aligned `type`, the `subject` (session), a
 *     `time`, the `sequence` cursor, and the typed `data`.
 *   - **AG-UI vocabulary** for `type` where it maps; aex-specific events
 *     ride AG-UI's reserved `CUSTOM` carrier under an `aex.*` name, so an
 *     off-the-shelf AG-UI client reads an aex start with no glue.
 *   - Two aex extensions: a coarse `source` (filter first by origin) and
 *     an optional human `message` (log / CLI / dashboard rendering).
 *
 * Unified observability spine:
 * the envelope additionally carries four ordering attributes so BOTH the typed
 * event stream and the high-volume hosted-platform log stream ride one per-session
 * coordinator:
 *   - `channel`   — "event" (the typed AG-UI stream) or "log" (a verbose log
 *                   line). The single axis a consumer splits the unified stream
 *                   on.
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
 * This module is a pure projection plus honest guards and a strict AG-UI
 * projection for consumers.
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
export const AEX_EVENT_MAP_VERSION = 2 as const;

/**
 * Coarse origin classifier — the first axis a consumer filters on.
 *   - `agent`   — the model: text, reasoning, builtin tool calls/results.
 *   - `api`     — the hosted aex API path.
 *   - `runtime` — the execution runtime: lifecycle, diagnostics, non-fatal
 *                 stream errors.
 *   - `mcp`     — an MCP server (a tool call/result routed through MCP).
 *   - `aex` — the platform: skills, files, and other aex-native events.
 *   - `workflow`— the orchestration layer.
 *   - `host`    — the managed host the runtime executes on; distinct from the
 *                 runtime process itself.
 */
export const AEX_EVENT_SOURCES = ["agent", "api", "runtime", "mcp", "aex", "workflow", "host"] as const;
export type AexEventSource = (typeof AEX_EVENT_SOURCES)[number] | string;

/**
 * The channel a record rides on the unified per-session stream:
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
  // typed-event vocabulary on purpose: a `LOG` is never a session-lifecycle signal,
  // so terminal detection (RUN_FINISHED/RUN_ERROR) is unaffected and an
  // off-the-shelf AG-UI client filters logs out by `channel`.
  "LOG"
] as const;
export type AexEventType = (typeof AEX_EVENT_TYPES)[number] | string;

/**
 * Opt-in producer identity for at-least-once ingest. This is distinct from the
 * coarse envelope `source`/`sourceSeq`: only producers maintaining a
 * session-scoped monotonic counter set this pair.
 */
export interface AexEventDedup {
  /** Free-form producer identity, not the coarse envelope source. */
  readonly source: string;
  /** Producer-monotonic integer scoped to `source` within the session. */
  readonly sourceSeq: number;
}

/** Fields shared by durable events and provisional live-only stream frames. */
export interface AexEventBase {
  /** CloudEvents specversion. Always {@link AEX_EVENT_SPECVERSION}. */
  readonly specversion: typeof AEX_EVENT_SPECVERSION;
  /** Stable, globally unique event id. This is the stream dedupe key. */
  readonly id: string;
  /** Coarse origin classifier. */
  readonly source: AexEventSource;
  /** AG-UI-aligned event type. */
  readonly type: AexEventType;
  /** The session this event belongs to (CloudEvents `subject`). */
  readonly subject: string;
  /** AG-UI thread identity. Equal to {@link subject} for an aex session. */
  readonly threadId: string;
  /** AG-UI run identity. A new id is allocated for every session turn. */
  readonly runId: string;
  /** ISO-8601 event time (session base + the RunnerEvent's relative `tMs`). */
  readonly time: string;
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
   * `time` (the LOGICAL time = session base + relative tMs).
   *
   * NOTE: some edge clocks are coarsened/frozen-at-I/O, so `receivedAt` is
   * precisely "the coordinator's last-I/O wall-clock at ingest" — that is fine
   * as the authoritative receive marker. It does NOT need to exceed `emittedAt`:
   * the source runs on a different host with an independent clock, so cross-host
   * drift can leave `receivedAt < emittedAt` and that is expected, not an error.
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
  /** Optional producer identity used by the coordinator's idempotent ingest. */
  readonly dedup?: AexEventDedup;
  /** True only for producer-authored broadcast-only frames. */
  readonly ephemeral?: boolean;
  /**
   * Typed payload. For `CUSTOM` events this is `{ name, value }` (AG-UI's
   * custom carrier, `name` = `aex.<kind>`); for typed events it is the
   * RunnerEvent's own data, projected to AG-UI fields by {@link toAGUI}.
   */
  readonly data: Readonly<Record<string, JsonValue>>;
}

/**
 * A durable, replayable event. `sequence` is the session-wide persistence and
 * reconnect cursor used by list/archive APIs and coordinator replay.
 */
export interface AexEvent extends AexEventBase {
  readonly replayable?: true;
  readonly liveSequence?: never;
  readonly sequence: number;
}

/**
 * A provisional live-only frame. It is never returned by list/archive APIs and
 * cannot advance a durable reconnect cursor. `liveSequence` orders frames only
 * within this run's live delivery; `id` is the stable duplicate-suppression key.
 */
export interface AexLiveEvent extends AexEventBase {
  readonly replayable: false;
  readonly liveSequence: number;
  readonly sequence?: never;
  readonly receivedAt: number;
  readonly ephemeral: true;
}

/** Events a live coordinator WebSocket may yield. */
export type AexStreamEvent = AexEvent | AexLiveEvent;

/** True only for a provisional frame outside the durable replay sequence. */
export function isAexLiveEvent(event: AexStreamEvent): event is AexLiveEvent {
  return event.replayable === false;
}

/** True only for a durable event carrying a replay cursor. */
export function isReplayableEvent(event: AexStreamEvent): event is AexEvent {
  return event.replayable !== false && typeof event.sequence === "number";
}

/** Compatibility spelling used by the hosted platform. */
export const isReplayableAexEvent = isReplayableEvent;

/** Context the mapper needs to stamp absolute identity/time onto an event. */
export interface AexEventContext {
  readonly sessionId: string;
  /** The real AG-UI run id for the session turn being projected. */
  readonly runId: string;
  /**
   * Run-start epoch ms. The RunnerEvent's `tMs` is relative to this, so
   * `time = new Date(baseMs + tMs)`. Pass the session's `createdAtMs`.
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
 * Project a {@link RunnerEvent} onto the unified public envelope. Internal
 * runtime completion has no public projection: only the platform-authored
 * RUN_FINISHED/RUN_ERROR event is a completion boundary.
 */
export function runnerEventToAexEvent(evt: RunnerEvent, ctx: AexEventContext): AexEvent | null {
  const projection = project(evt);
  if (projection === null) return null;
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: `${ctx.sessionId}:${evt.seq}`,
    source: projection.source,
    type: projection.type,
    subject: ctx.sessionId,
    threadId: ctx.sessionId,
    runId: ctx.runId,
    time: new Date(ctx.baseMs + evt.tMs).toISOString(),
    sequence: evt.seq,
    ...(evt.sourceSeq !== undefined ? { sourceSeq: evt.sourceSeq } : {}),
    ...(evt.emittedAt !== undefined ? { emittedAt: evt.emittedAt } : {}),
    ...(projection.message !== undefined ? { message: projection.message } : {}),
    data: Object.freeze(projection.data)
  };
}

function project(evt: RunnerEvent): Projection | null {
  const data = evt.data;
  switch (evt.kind) {
    case "runtime_started":
      return { type: "RUN_STARTED", source: "runtime", message: "turn started", data: { ...data } };
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
    case "runtime_terminal":
      return null;
  }
}

// --- Log channel ----------------------------------------------------------
// A hosted API/workflow log line is a record on the unified stream's `log` channel,
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
  "specversion" | "id" | "subject" | "threadId" | "runId" | "sequence" | "receivedAt"
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

export function isRunStarted(e: AexEventBase): boolean {
  return e.type === "RUN_STARTED";
}
export function isRunFinished(e: AexEventBase): boolean {
  return e.type === "RUN_FINISHED";
}
export function isRunError(e: AexEventBase): boolean {
  return e.type === "RUN_ERROR";
}
/** A terminal event of either flavour (finished or error). */
export function isRunTerminal(e: AexEventBase): boolean {
  return e.type === "RUN_FINISHED" || e.type === "RUN_ERROR";
}
export function isTextMessage(e: AexEventBase): boolean {
  return e.type === "TEXT_MESSAGE_CONTENT";
}
export function isToolCallStart(e: AexEventBase): boolean {
  return e.type === "TOOL_CALL_START";
}
export function isToolCallResult(e: AexEventBase): boolean {
  return e.type === "TOOL_CALL_RESULT";
}
export function isCustom(e: AexEventBase): boolean {
  return e.type === "CUSTOM";
}
/** The `aex.*` name of a CUSTOM event, or null for typed events. */
export function customName(e: AexEventBase): string | null {
  return e.type === "CUSTOM" ? str(e.data.name) || null : null;
}
/**
 * The CUSTOM `data.name` of the HITL write-gate park: the session has reached the
 * `awaiting_approval` state before a gated action and is holding for an
 * `approve()`/`deny()`. Structural (independent of model prose).
 */
export const AEX_SESSION_AWAITING_APPROVAL_NAME = "aex.session.awaiting_approval";
/** The CUSTOM `data.name` carrying a schema-decoded value (`{ value }`). */
export const AEX_RESULT_DECODED_NAME = "aex.result.decoded";
/** The CUSTOM `data.name` carrying a typed decode refusal (`{ reason, detail? }`). */
export const AEX_RESULT_REFUSED_NAME = "aex.result.refused";

/** True for the HITL `awaiting_approval` gate event. */
export function isAwaitingApproval(e: AexEventBase): boolean {
  return customName(e) === AEX_SESSION_AWAITING_APPROVAL_NAME;
}
/** True for a schema-decoded terminal result event. */
export function isResultDecoded(e: AexEventBase): boolean {
  return customName(e) === AEX_RESULT_DECODED_NAME;
}
/** True for a typed decode-refusal terminal result event. */
export function isResultRefused(e: AexEventBase): boolean {
  return customName(e) === AEX_RESULT_REFUSED_NAME;
}
export function isFromSource(e: AexEventBase, source: AexEventSource): boolean {
  return e.source === source;
}
/** The channel a record rides, defaulting an absent value to `"event"`. */
export function channelOf(e: AexEventBase): AexEventChannel {
  return e.channel ?? "event";
}
/** True when a record is a log line (the `log` channel / `LOG` type). */
export function isLog(e: AexEventBase): boolean {
  return channelOf(e) === "log";
}
/** True when a record is a typed AG-UI event (the `event` channel). */
export function isEventChannel(e: AexEventBase): boolean {
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
  | { type: "RUN_FINISHED"; timestamp: number; threadId: string; runId: string; result?: JsonValue }
  | { type: "RUN_ERROR"; timestamp: number; message: string; code?: string }
  | { type: "TEXT_MESSAGE_CONTENT"; timestamp: number; messageId: string; delta: string }
  | { type: "TOOL_CALL_START"; timestamp: number; toolCallId: string; toolCallName: string }
  | { type: "TOOL_CALL_RESULT"; timestamp: number; messageId: string; toolCallId: string; content: JsonValue }
  | { type: "CUSTOM"; timestamp: number; name: string; value: JsonValue };

/**
 * Project an aex envelope to a strict AG-UI event so an off-the-shelf
 * AG-UI client can consume an aex start with no glue. This is the
 * client-side projection the SDK exposes.
 */
export function toAGUI(e: AexEventBase): AguiEvent {
  const timestamp = Date.parse(e.time);
  const d = e.data;
  switch (e.type) {
    case "RUN_STARTED":
      return { type: "RUN_STARTED", timestamp, threadId: e.threadId, runId: e.runId };
    case "RUN_FINISHED": {
      const result = d.result;
      return {
        type: "RUN_FINISHED",
        timestamp,
        threadId: e.threadId,
        runId: e.runId,
        ...(result !== undefined ? { result } : {})
      };
    }
    case "RUN_ERROR": {
      const code = str(d.failureClass);
      return {
        type: "RUN_ERROR",
        timestamp,
        message: str(d.failureMessage) || e.message || "turn error",
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
    default:
      // Raw AexEvent stays open and untouched for forward compatibility. AG-UI
      // has a reserved CUSTOM carrier for event types a client does not know.
      return { type: "CUSTOM", timestamp, name: e.type, value: { ...d } };
  }
}

function str(v: JsonValue | undefined): string {
  return typeof v === "string" ? v : "";
}

function clip(s: string, max = 200): string {
  return s.length <= max ? s : `${s.slice(0, max - 1)}…`;
}

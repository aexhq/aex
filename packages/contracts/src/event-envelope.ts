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
import { SESSION_TERMINAL_OUTCOMES, type SessionTerminalOutcome } from "./status.js";
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

/** Payload carried by a valid public `RUN_STARTED` event. */
export type AexRunStartedData = Readonly<Record<string, JsonValue>> & {
  readonly source?: string;
  readonly turnSeq?: number;
  readonly mode?: string;
  readonly provider?: string;
  readonly model?: string;
};

/** Payload carried by a valid public `RUN_FINISHED` event. */
export type AexRunFinishedData = Readonly<Record<string, JsonValue>> & {
  readonly outcome: SessionTerminalOutcome;
  readonly result?: JsonValue;
};

/** Payload carried by a valid public `RUN_ERROR` event. */
export type AexRunErrorData = Readonly<Record<string, JsonValue>> & {
  readonly outcome: SessionTerminalOutcome;
  readonly failureClass: string;
  readonly failureMessage: string;
};

/** Payload carried by a valid public assistant-text event. */
export type AexTextMessageData = Readonly<Record<string, JsonValue>> & {
  readonly text: string;
  readonly messageId?: string;
  readonly eventId?: string;
  /** True only on a provisional, non-replayable live delta. */
  readonly delta?: boolean;
  readonly truncated?: boolean;
};

/** Payload carried by a valid public tool-call start. */
export type AexToolCallStartData = Readonly<Record<string, JsonValue>> & {
  readonly id: string;
  readonly name: string;
  readonly arguments?: Readonly<Record<string, JsonValue>>;
  readonly messageId?: string;
};

/** Payload carried by a valid public tool-call result. */
export type AexToolCallResultData = Readonly<Record<string, JsonValue>> & {
  readonly id: string;
  readonly content: JsonValue;
  readonly isError?: boolean;
  readonly messageId?: string;
};

/** Payload carried by AG-UI's CUSTOM carrier. */
export type AexCustomData<Name extends string = string> = Readonly<Record<string, JsonValue>> & {
  readonly name: Name;
  readonly value: JsonValue;
};

/** Payload carried by a public log-channel record. */
export type AexLogData = Readonly<Record<string, JsonValue>> & {
  readonly level: AexLogLevel;
  readonly message: string;
  readonly fields?: Readonly<Record<string, JsonValue>>;
};

export type AexRunStartedEvent = AexEventBase & {
  readonly type: "RUN_STARTED";
  readonly data: AexRunStartedData;
};
export type AexRunFinishedEvent = AexEventBase & {
  readonly type: "RUN_FINISHED";
  readonly data: AexRunFinishedData;
};
export type AexRunErrorEvent = AexEventBase & {
  readonly type: "RUN_ERROR";
  readonly data: AexRunErrorData;
};
export type AexTextMessageEvent = AexEventBase & {
  readonly type: "TEXT_MESSAGE_CONTENT";
  readonly data: AexTextMessageData;
};
export type AexToolCallStartEvent = AexEventBase & {
  readonly type: "TOOL_CALL_START";
  readonly data: AexToolCallStartData;
};
export type AexToolCallResultEvent = AexEventBase & {
  readonly type: "TOOL_CALL_RESULT";
  readonly data: AexToolCallResultData;
};
export type AexCustomEvent<Name extends string = string> = AexEventBase & {
  readonly type: "CUSTOM";
  readonly data: AexCustomData<Name>;
};
export type AexLogEvent = AexEventBase & {
  readonly type: "LOG";
  readonly channel: "log";
  readonly level: AexLogLevel;
  readonly data: AexLogData;
};

/** All public event shapes this package currently understands. */
export type KnownAexEventBase =
  | AexRunStartedEvent
  | AexRunFinishedEvent
  | AexRunErrorEvent
  | AexTextMessageEvent
  | AexToolCallStartEvent
  | AexToolCallResultEvent
  | AexCustomEvent
  | AexLogEvent;

/** A validated known durable event. Raw {@link AexEvent} deliberately remains open. */
export type KnownAexEvent = AexEvent & KnownAexEventBase;

/** A validated known provisional event. */
export type KnownAexLiveEvent = AexLiveEvent & KnownAexEventBase;

/** A validated known durable or provisional stream event. */
export type KnownAexStreamEvent = AexStreamEvent & KnownAexEventBase;

/** Stable diagnostic for a recognized type whose payload violates its contract. */
export interface MalformedAexEventIssue {
  readonly code: "malformed_known_event";
  readonly type: (typeof AEX_EVENT_TYPES)[number];
  readonly path: string;
  readonly expected: string;
}

/** Explicit failure thrown when a malformed known event is projected. */
export class MalformedAexEventError extends Error {
  readonly code = "malformed_known_event" as const;
  readonly issue: MalformedAexEventIssue;

  constructor(issue: MalformedAexEventIssue) {
    super(`Malformed ${issue.type} event: ${issue.path} must be ${issue.expected}`);
    this.name = "MalformedAexEventError";
    this.issue = issue;
  }
}

/**
 * Result of classifying an open event without changing it. Unknown future types
 * remain raw; malformed recognized types are a separate, explicit state.
 */
export type AexEventClassification<T extends AexEventBase = AexEventBase> =
  | { readonly kind: "known"; readonly event: T & KnownAexEventBase }
  | { readonly kind: "unknown"; readonly event: T }
  | { readonly kind: "malformed_known"; readonly event: T; readonly issue: MalformedAexEventIssue };

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
  const event: AexEvent = {
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
  const classified = classifyAexEvent(event);
  if (classified.kind === "malformed_known") {
    throw new MalformedAexEventError(classified.issue);
  }
  return event;
}

function project(evt: RunnerEvent): Projection | null {
  const data = evt.data;
  switch (evt.kind) {
    case "runtime_started":
      return { type: "RUN_STARTED", source: "runtime", message: "turn started", data: { ...data } };
    case "assistant_text": {
      const text = typeof data.text === "string" ? data.text : undefined;
      return {
        type: "TEXT_MESSAGE_CONTENT",
        source: "agent",
        ...(text ? { message: clip(text) } : {}),
        data: { ...data }
      };
    }
    case "tool_request": {
      const name = typeof data.name === "string" ? data.name : undefined;
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
      return custom(
        "aex.notification",
        "runtime",
        data,
        typeof data.reason === "string" && data.reason.length > 0 ? data.reason : undefined
      );
    case "stream_error":
      return custom(
        "aex.stream_error",
        "runtime",
        data,
        typeof data.message === "string" && data.message.length > 0 ? data.message : "stream error"
      );
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

// --- Known/unknown classification and honest guards ---------------------------

const TERMINAL_OUTCOMES = new Set<string>(SESSION_TERMINAL_OUTCOMES);
const LOG_LEVELS = new Set<string>(AEX_LOG_LEVELS);

function malformed(
  type: (typeof AEX_EVENT_TYPES)[number],
  path: string,
  expected: string
): MalformedAexEventIssue {
  return { code: "malformed_known_event", type, path, expected };
}

function optionalString(data: Readonly<Record<string, JsonValue>>, key: string): boolean {
  return data[key] === undefined || typeof data[key] === "string";
}

function optionalBoolean(data: Readonly<Record<string, JsonValue>>, key: string): boolean {
  return data[key] === undefined || typeof data[key] === "boolean";
}

function isJsonRecord(value: JsonValue | undefined): value is Readonly<Record<string, JsonValue>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function knownIssue(e: AexEventBase): MalformedAexEventIssue | null | undefined {
  const d = e.data;
  switch (e.type) {
    case "RUN_STARTED":
      for (const key of ["source", "mode", "provider", "model"] as const) {
        if (!optionalString(d, key)) {
          return malformed("RUN_STARTED", `data.${key}`, "a string when present");
        }
      }
      return d.turnSeq === undefined || (
        typeof d.turnSeq === "number" && Number.isInteger(d.turnSeq) && d.turnSeq >= 0
      )
        ? undefined
        : malformed("RUN_STARTED", "data.turnSeq", "a non-negative integer when present");
    case "RUN_FINISHED":
      return typeof d.outcome === "string" && TERMINAL_OUTCOMES.has(d.outcome)
        ? undefined
        : malformed("RUN_FINISHED", "data.outcome", `one of ${SESSION_TERMINAL_OUTCOMES.join(", ")}`);
    case "RUN_ERROR":
      if (typeof d.outcome !== "string" || !TERMINAL_OUTCOMES.has(d.outcome)) {
        return malformed("RUN_ERROR", "data.outcome", `one of ${SESSION_TERMINAL_OUTCOMES.join(", ")}`);
      }
      if (typeof d.failureClass !== "string" || d.failureClass.length === 0) {
        return malformed("RUN_ERROR", "data.failureClass", "a non-empty string");
      }
      return typeof d.failureMessage === "string" && d.failureMessage.length > 0
        ? undefined
        : malformed("RUN_ERROR", "data.failureMessage", "a non-empty string");
    case "TEXT_MESSAGE_CONTENT":
      if (typeof d.text !== "string") {
        return malformed("TEXT_MESSAGE_CONTENT", "data.text", "a string");
      }
      if (!optionalString(d, "messageId")) {
        return malformed("TEXT_MESSAGE_CONTENT", "data.messageId", "a string when present");
      }
      if (!optionalString(d, "eventId")) {
        return malformed("TEXT_MESSAGE_CONTENT", "data.eventId", "a string when present");
      }
      if (!optionalBoolean(d, "delta")) {
        return malformed("TEXT_MESSAGE_CONTENT", "data.delta", "a boolean when present");
      }
      return optionalBoolean(d, "truncated")
        ? undefined
        : malformed("TEXT_MESSAGE_CONTENT", "data.truncated", "a boolean when present");
    case "TOOL_CALL_START":
      if (typeof d.id !== "string" || d.id.length === 0) {
        return malformed("TOOL_CALL_START", "data.id", "a non-empty string");
      }
      if (typeof d.name !== "string" || d.name.length === 0) {
        return malformed("TOOL_CALL_START", "data.name", "a non-empty string");
      }
      if (d.arguments !== undefined && !isJsonRecord(d.arguments)) {
        return malformed("TOOL_CALL_START", "data.arguments", "a JSON object when present");
      }
      return optionalString(d, "messageId")
        ? undefined
        : malformed("TOOL_CALL_START", "data.messageId", "a string when present");
    case "TOOL_CALL_RESULT":
      if (typeof d.id !== "string" || d.id.length === 0) {
        return malformed("TOOL_CALL_RESULT", "data.id", "a non-empty string");
      }
      if (!Object.hasOwn(d, "content")) {
        return malformed("TOOL_CALL_RESULT", "data.content", "present JSON data");
      }
      if (!optionalBoolean(d, "isError")) {
        return malformed("TOOL_CALL_RESULT", "data.isError", "a boolean when present");
      }
      return optionalString(d, "messageId")
        ? undefined
        : malformed("TOOL_CALL_RESULT", "data.messageId", "a string when present");
    case "CUSTOM":
      if (typeof d.name !== "string" || d.name.length === 0) {
        return malformed("CUSTOM", "data.name", "a non-empty string");
      }
      return Object.hasOwn(d, "value")
        ? undefined
        : malformed("CUSTOM", "data.value", "present JSON data");
    case "LOG":
      if (e.channel !== "log") return malformed("LOG", "channel", '"log"');
      if (typeof e.level !== "string" || !LOG_LEVELS.has(e.level)) {
        return malformed("LOG", "level", `one of ${AEX_LOG_LEVELS.join(", ")}`);
      }
      if (d.level !== e.level) return malformed("LOG", "data.level", "the first-class event level");
      if (typeof d.message !== "string") return malformed("LOG", "data.message", "a string");
      return d.fields === undefined || isJsonRecord(d.fields)
        ? undefined
        : malformed("LOG", "data.fields", "a JSON object when present");
    default:
      return null;
  }
}

/** Classify an open event without cloning or rewriting it. */
export function classifyAexEvent<T extends AexEventBase>(event: T): AexEventClassification<T> {
  const issue = knownIssue(event);
  if (issue === null) return { kind: "unknown", event };
  if (issue !== undefined) return { kind: "malformed_known", event, issue };
  return { kind: "known", event: event as T & KnownAexEventBase };
}

/** True only for a currently understood event with a valid payload. */
export function isKnownAexEvent<T extends AexEventBase>(e: T): e is T & KnownAexEventBase {
  return knownIssue(e) === undefined;
}

export function isRunStarted<T extends AexEventBase>(e: T): e is T & AexRunStartedEvent {
  return e.type === "RUN_STARTED" && knownIssue(e) === undefined;
}
export function isRunFinished<T extends AexEventBase>(e: T): e is T & AexRunFinishedEvent {
  return e.type === "RUN_FINISHED" && knownIssue(e) === undefined;
}
export function isRunError<T extends AexEventBase>(e: T): e is T & AexRunErrorEvent {
  return e.type === "RUN_ERROR" && knownIssue(e) === undefined;
}
/** A valid terminal event of either flavour (finished or error). */
export function isRunTerminal<T extends AexEventBase>(
  e: T
): e is T & (AexRunFinishedEvent | AexRunErrorEvent) {
  return (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") && knownIssue(e) === undefined;
}
export function isTextMessage<T extends AexEventBase>(e: T): e is T & AexTextMessageEvent {
  return e.type === "TEXT_MESSAGE_CONTENT" && knownIssue(e) === undefined;
}
export function isToolCallStart<T extends AexEventBase>(e: T): e is T & AexToolCallStartEvent {
  return e.type === "TOOL_CALL_START" && knownIssue(e) === undefined;
}
export function isToolCallResult<T extends AexEventBase>(e: T): e is T & AexToolCallResultEvent {
  return e.type === "TOOL_CALL_RESULT" && knownIssue(e) === undefined;
}
export function isCustom<T extends AexEventBase>(e: T): e is T & AexCustomEvent {
  return e.type === "CUSTOM" && knownIssue(e) === undefined;
}
/** The `aex.*` name of a valid CUSTOM event, or null otherwise. */
export function customName(e: AexEventBase): string | null {
  return isCustom(e) ? e.data.name : null;
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
export function isAwaitingApproval<T extends AexEventBase>(
  e: T
): e is T & AexCustomEvent<typeof AEX_SESSION_AWAITING_APPROVAL_NAME> {
  return isCustom(e) && e.data.name === AEX_SESSION_AWAITING_APPROVAL_NAME;
}
/** True for a schema-decoded terminal result event. */
export function isResultDecoded<T extends AexEventBase>(
  e: T
): e is T & AexCustomEvent<typeof AEX_RESULT_DECODED_NAME> {
  return isCustom(e) && e.data.name === AEX_RESULT_DECODED_NAME;
}
/** True for a typed decode-refusal terminal result event. */
export function isResultRefused<T extends AexEventBase>(
  e: T
): e is T & AexCustomEvent<typeof AEX_RESULT_REFUSED_NAME> {
  return isCustom(e) && e.data.name === AEX_RESULT_REFUSED_NAME;
}
export function isFromSource(e: AexEventBase, source: AexEventSource): boolean {
  return e.source === source;
}
/** The channel a record rides, defaulting an absent value to `"event"`. */
export function channelOf(e: AexEventBase): AexEventChannel {
  return e.channel ?? "event";
}
/** True when a record is a valid log line (the `log` channel / `LOG` type). */
export function isLog<T extends AexEventBase>(e: T): e is T & AexLogEvent {
  return e.type === "LOG" && knownIssue(e) === undefined;
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
  const classified = classifyAexEvent(e);
  if (classified.kind === "malformed_known") {
    throw new MalformedAexEventError(classified.issue);
  }
  if (classified.kind === "unknown") {
    return { type: "CUSTOM", timestamp, name: e.type, value: { ...e.data } };
  }
  const known = classified.event;
  switch (known.type) {
    case "RUN_STARTED":
      return { type: "RUN_STARTED", timestamp, threadId: known.threadId, runId: known.runId };
    case "RUN_FINISHED": {
      const result = known.data.result;
      return {
        type: "RUN_FINISHED",
        timestamp,
        threadId: known.threadId,
        runId: known.runId,
        ...(result !== undefined ? { result } : {})
      };
    }
    case "RUN_ERROR":
      return {
        type: "RUN_ERROR",
        timestamp,
        message: known.data.failureMessage,
        code: known.data.failureClass
      };
    case "TEXT_MESSAGE_CONTENT":
      return {
        type: "TEXT_MESSAGE_CONTENT",
        timestamp,
        messageId: known.data.messageId ?? known.data.eventId ?? known.id,
        delta: known.data.text
      };
    case "TOOL_CALL_START":
      return {
        type: "TOOL_CALL_START",
        timestamp,
        toolCallId: known.data.id,
        toolCallName: known.data.name
      };
    case "TOOL_CALL_RESULT":
      return {
        type: "TOOL_CALL_RESULT",
        timestamp,
        messageId: known.data.messageId ?? known.id,
        toolCallId: known.data.id,
        content: known.data.content
      };
    case "CUSTOM":
      return { type: "CUSTOM", timestamp, name: known.data.name, value: known.data.value };
    case "LOG":
      // Logs ride the `log` channel and are normally filtered out before
      // projection. If a consumer projects one anyway, carry it under AG-UI's
      // reserved CUSTOM so the client still receives a valid record.
      return { type: "CUSTOM", timestamp, name: "aex.log", value: { ...known.data } };
  }
}

function clip(s: string, max = 200): string {
  return s.length <= max ? s : `${s.slice(0, max - 1)}…`;
}

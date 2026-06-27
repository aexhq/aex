/**
 * Typed decoders for a `listEvents(runId)` stream.
 *
 * `listEvents` returns the loose {@link RunEvent} wire shape: `type` at the top
 * level, but the tool name at `data.name`, the call args at `data.arguments`,
 * assistant text at `data.text`, and — the awkward part — a `TOOL_CALL_RESULT`
 * carries no name, so a consumer must correlate each result back to its
 * `TOOL_CALL_START` by `data.id` (the tool-call id). Every consumer ended up
 * re-implementing that correlation; these helpers do it once, typed and pure.
 *
 * They are total and side-effect-free: pass the array `listEvents` returns and
 * get back structured traces. Unknown event types are ignored (forward-compat),
 * a result with no matching start surfaces as an orphan (never dropped silently),
 * and timing falls back gracefully when a `recordedAt` is absent.
 *
 * `summarizeRunUsage` remains tolerant of historical/internal `aex.usage`
 * CUSTOM records when a caller has them, but the normal public `listEvents`
 * stream is not the live usage-reporting surface. Settled provider/runtime
 * usage is exposed through the run's `costTelemetry`.
 */

import type { UsageSummary } from "./runtime-types.js";

/**
 * One decoded tool call: the `TOOL_CALL_START` and its correlated
 * `TOOL_CALL_RESULT` (when present), with timing. `result` is undefined while a
 * call is still in flight (the start has arrived but not the result), so a
 * mid-run decode shows in-progress calls honestly rather than dropping them.
 */
export interface ToolCallTrace {
  /** The tool-call id (`data.id`) that pairs the start with its result. */
  readonly id: string;
  /** The tool name (from the `TOOL_CALL_START`). */
  readonly name: string;
  /** The call arguments (`data.arguments`), or `{}` when absent. */
  readonly args: Readonly<Record<string, unknown>>;
  /** The id of the assistant message that issued the call, when present. */
  readonly messageId?: string;
  /** Event sequence of the `TOOL_CALL_START` (ordering within the run). */
  readonly startSeq?: number;
  /** ISO-8601 time of the `TOOL_CALL_START`, when the event carried one. */
  readonly startedAt?: string;
  /** The correlated result; undefined while the call is still in flight. */
  readonly result?: ToolCallResult;
  /** Result wall-clock minus start wall-clock (ms); undefined if either time is missing. */
  readonly durationMs?: number;
}

/** The result half of a {@link ToolCallTrace}, decoded from `TOOL_CALL_RESULT`. */
export interface ToolCallResult {
  /** True when the tool reported an error (`data.isError`). */
  readonly isError: boolean;
  /** The tool's result content, passed through verbatim (`data.content`). */
  readonly content: unknown;
  /** Event sequence of the `TOOL_CALL_RESULT`. */
  readonly seq?: number;
  /** ISO-8601 time of the `TOOL_CALL_RESULT`, when the event carried one. */
  readonly recordedAt?: string;
}

/** One assistant text block, decoded from a `TEXT_MESSAGE_CONTENT` event. */
export interface AssistantTextEntry {
  readonly text: string;
  readonly messageId?: string;
  readonly seq?: number;
  readonly recordedAt?: string;
}

/**
 * A decoded view of a run's event stream: the correlated tool calls, the
 * aggregate token usage, and the assistant text — everything a consumer
 * previously hand-decoded from `listEvents`.
 */
export interface RunTrace {
  readonly toolCalls: readonly ToolCallTrace[];
  readonly usage: UsageSummary;
  readonly text: readonly AssistantTextEntry[];
}

/** The loose event shape these decoders read — the {@link RunEvent} subset they touch. */
interface TraceEvent {
  readonly type: string;
  readonly seq?: number;
  readonly recordedAt?: string;
  readonly data?: unknown;
  readonly [key: string]: unknown;
}

const CUSTOM_USAGE_NAME = "aex.usage";

/** snake_case `aex.usage` field → the camelCase {@link UsageSummary} field. */
const USAGE_FIELD_MAP = {
  input_tokens: "inputTokens",
  output_tokens: "outputTokens",
  cache_read_input_tokens: "cacheReadInputTokens",
  cache_creation_input_tokens: "cacheCreationInputTokens"
} as const;

/**
 * Decode a `listEvents` stream into correlated tool-call traces, in start order.
 *
 * Each `TOOL_CALL_START` opens a trace keyed by `data.id`; the matching
 * `TOOL_CALL_RESULT` (same `data.id`) fills in `result` + `durationMs`. A result
 * whose id never had a start is surfaced as an orphan trace (empty `name`, no
 * `args`) rather than dropped, so a partial/mis-ordered stream never hides a
 * result. Pure — no I/O, input is not mutated.
 */
export function decodeToolCalls(events: readonly TraceEvent[]): readonly ToolCallTrace[] {
  const order: string[] = [];
  const byId = new Map<string, Mutable<ToolCallTrace>>();

  for (const event of events) {
    const data = asRecord(event.data);
    if (event.type === "TOOL_CALL_START") {
      const id = asString(data.id);
      if (id === undefined) continue;
      const trace: Mutable<ToolCallTrace> = {
        id,
        name: asString(data.name) ?? "",
        args: asRecord(data.arguments)
      };
      const messageId = asString(data.messageId);
      if (messageId !== undefined) trace.messageId = messageId;
      if (typeof event.seq === "number") trace.startSeq = event.seq;
      if (typeof event.recordedAt === "string") trace.startedAt = event.recordedAt;
      if (!byId.has(id)) order.push(id);
      byId.set(id, trace);
      continue;
    }
    if (event.type === "TOOL_CALL_RESULT") {
      const id = asString(data.id);
      if (id === undefined) continue;
      const result: Mutable<ToolCallResult> = {
        isError: data.isError === true,
        content: data.content ?? null
      };
      if (typeof event.seq === "number") result.seq = event.seq;
      if (typeof event.recordedAt === "string") result.recordedAt = event.recordedAt;
      let trace = byId.get(id);
      if (trace === undefined) {
        // Orphan result (no matching start) — surface it, never drop it.
        trace = { id, name: "", args: {} };
        order.push(id);
        byId.set(id, trace);
      }
      trace.result = result;
      const duration = durationMs(trace.startedAt, result.recordedAt);
      if (duration !== undefined) trace.durationMs = duration;
      continue;
    }
  }

  return order.map((id) => byId.get(id)!);
}

/**
 * Sum any `aex.usage` CUSTOM events present in the supplied stream into one
 * {@link UsageSummary}. This is mainly for historical/internal event arrays;
 * current public reads expose settled provider usage through cost telemetry.
 * `totalTokens` is the sum of input + output tokens. Pure.
 */
export function summarizeRunUsage(events: readonly TraceEvent[]): UsageSummary {
  const totals = { inputTokens: 0, outputTokens: 0, cacheReadInputTokens: 0, cacheCreationInputTokens: 0 };
  let seen = false;
  for (const event of events) {
    if (event.type !== "CUSTOM") continue;
    const data = asRecord(event.data);
    if (asString(data.name) !== CUSTOM_USAGE_NAME) continue;
    const value = asRecord(data.value);
    for (const [snake, camel] of Object.entries(USAGE_FIELD_MAP)) {
      const n = value[snake];
      if (typeof n === "number" && Number.isFinite(n)) {
        totals[camel] += n;
        seen = true;
      }
    }
  }
  if (!seen) return {};
  return {
    inputTokens: totals.inputTokens,
    outputTokens: totals.outputTokens,
    cacheReadInputTokens: totals.cacheReadInputTokens,
    cacheCreationInputTokens: totals.cacheCreationInputTokens,
    totalTokens: totals.inputTokens + totals.outputTokens
  };
}

/**
 * The run's final assistant text: every `TEXT_MESSAGE_CONTENT` block in stream
 * order, concatenated. The one-line "what did the agent say" accessor over
 * {@link decodeAssistantText} (buffered mode yields whole messages, stream mode
 * yields token deltas — both concatenate correctly). Pure.
 */
export function textOf(events: readonly TraceEvent[]): string {
  return decodeAssistantText(events).map((entry) => entry.text).join("");
}

/** Decode the assistant text blocks (`TEXT_MESSAGE_CONTENT`) in stream order. Pure. */
export function decodeAssistantText(events: readonly TraceEvent[]): readonly AssistantTextEntry[] {
  const out: AssistantTextEntry[] = [];
  for (const event of events) {
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    const text = asString(data.text);
    if (text === undefined) continue;
    const entry: Mutable<AssistantTextEntry> = { text };
    const messageId = asString(data.messageId);
    if (messageId !== undefined) entry.messageId = messageId;
    if (typeof event.seq === "number") entry.seq = event.seq;
    if (typeof event.recordedAt === "string") entry.recordedAt = event.recordedAt;
    out.push(entry);
  }
  return out;
}

/**
 * Decode a whole `listEvents` stream in one pass: correlated tool calls,
 * aggregate {@link UsageSummary}, and assistant text. Convenience over the three
 * focused decoders; pure.
 */
export function summarizeRunTrace(events: readonly TraceEvent[]): RunTrace {
  return {
    toolCalls: decodeToolCalls(events),
    usage: summarizeRunUsage(events),
    text: decodeAssistantText(events)
  };
}

type Mutable<T> = { -readonly [K in keyof T]: T[K] };

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function durationMs(start: string | undefined, end: string | undefined): number | undefined {
  if (start === undefined || end === undefined) return undefined;
  const a = Date.parse(start);
  const b = Date.parse(end);
  if (!Number.isFinite(a) || !Number.isFinite(b)) return undefined;
  const delta = b - a;
  return delta >= 0 ? delta : undefined;
}

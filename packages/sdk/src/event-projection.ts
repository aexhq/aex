import {
  SessionStateError,
  usageFromProviderUsage,
  type AexEvent,
  type AexEventView,
  type Session,
  type SessionCheckpointRevision,
  type SessionCostProviderUsage,
  type SessionFile,
  type SessionMessage,
  type SessionRun,
  type SessionRunOutcome,
  type SessionStatus,
  type TurnOutcome,
  type TurnTrace,
  type UsageSummary
} from "@aexhq/contracts";
import type { Message, SessionRunResult } from "./client-types.js";

export function messageFromWire(message: SessionMessage): Message {
  return {
    id: message.id,
    sender: message.sender,
    text: message.text,
    ...(message.timestamp !== undefined ? { timestamp: message.timestamp } : {}),
    ...(message.turnSeq !== undefined ? { turnSeq: message.turnSeq } : {}),
    ...(message.sequence !== undefined ? { sequence: message.sequence } : {})
  };
}

export function projectAssistantMessages(events: readonly AexEvent[]): readonly Message[] {
  const out: Message[] = [];
  const byMessageId = new Map<string, number>();
  for (let i = 0; i < events.length; i++) {
    const event = events[i] as MessageEventLike;
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    if (data.delta === true) continue;
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const messageId = typeof data.messageId === "string" && data.messageId ? data.messageId : undefined;
    const sequence = event.sequence ?? event.seq;
    const timestamp = event.time ?? event.recordedAt ?? timestampFromEpochMs(event.receivedAt);
    const turnSeq = typeof data.turnSeq === "number" ? data.turnSeq : undefined;
    if (messageId !== undefined) {
      const existing = byMessageId.get(messageId);
      if (existing !== undefined) {
        const current = out[existing]!;
        out[existing] = {
          ...current,
          text: `${current.text}${text}`,
          ...(timestamp !== undefined ? { timestamp } : {}),
          ...(sequence !== undefined ? { sequence } : {}),
          ...(turnSeq !== undefined ? { turnSeq } : {})
        };
        continue;
      }
      byMessageId.set(messageId, out.length);
    }
    out.push({
      id: messageId ?? (typeof event.id === "string" && event.id ? event.id : `message-${i}`),
      sender: "assistant",
      text,
      ...(timestamp !== undefined ? { timestamp } : {}),
      ...(sequence !== undefined ? { sequence } : {}),
      ...(turnSeq !== undefined ? { turnSeq } : {})
    });
  }
  return out;
}

function assistantTextFromEvents(events: readonly AexEvent[]): string {
  return assistantTextEntriesFromEvents(events).map((entry) => entry.text).join("");
}

export function turnTraceFromEvents(events: readonly AexEvent[]): TurnTrace {
  return {
    toolCalls: toolCallsFromEvents(events),
    usage: usageFromEvents(events),
    text: assistantTextEntriesFromEvents(events)
  };
}

function assistantTextEntriesFromEvents(events: readonly AexEvent[]): TurnTrace["text"] {
  const out: Array<Mutable<TurnTrace["text"][number]>> = [];
  for (const event of events) {
    if (event.type !== "TEXT_MESSAGE_CONTENT") continue;
    const data = asRecord(event.data);
    if (data.delta === true) continue;
    const text = typeof data.text === "string" ? data.text : undefined;
    if (text === undefined) continue;
    const entry: Mutable<TurnTrace["text"][number]> = { text };
    const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
    if (messageId !== undefined) entry.messageId = messageId;
    if (typeof event.sequence === "number") entry.seq = event.sequence;
    if (typeof event.time === "string") entry.recordedAt = event.time;
    out.push(entry);
  }
  return out;
}

function toolCallsFromEvents(events: readonly AexEvent[]): TurnTrace["toolCalls"] {
  const order: string[] = [];
  const byId = new Map<string, Mutable<TurnTrace["toolCalls"][number]>>();
  for (const event of events) {
    const data = asRecord(event.data);
    if (event.type === "TOOL_CALL_START") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const trace: Mutable<TurnTrace["toolCalls"][number]> = {
        id,
        name: typeof data.name === "string" ? data.name : "",
        args: asRecord(data.arguments)
      };
      const messageId = typeof data.messageId === "string" ? data.messageId : undefined;
      if (messageId !== undefined) trace.messageId = messageId;
      if (typeof event.sequence === "number") trace.startSeq = event.sequence;
      if (typeof event.time === "string") trace.startedAt = event.time;
      if (!byId.has(id)) order.push(id);
      byId.set(id, trace);
      continue;
    }
    if (event.type === "TOOL_CALL_RESULT") {
      const id = typeof data.id === "string" ? data.id : undefined;
      if (id === undefined) continue;
      const result: Mutable<NonNullable<TurnTrace["toolCalls"][number]["result"]>> = {
        isError: data.isError === true,
        content: data.content ?? null
      };
      if (typeof event.sequence === "number") result.seq = event.sequence;
      if (typeof event.time === "string") result.recordedAt = event.time;
      let trace = byId.get(id);
      if (trace === undefined) {
        trace = { id, name: "", args: {} };
        order.push(id);
        byId.set(id, trace);
      }
      trace.result = result;
      const duration = durationMs(trace.startedAt, result.recordedAt);
      if (duration !== undefined) trace.durationMs = duration;
    }
  }
  return order.map((id) => byId.get(id)!);
}

function usageFromEvents(events: readonly AexEvent[]): UsageSummary {
  const totals = { inputTokens: 0, outputTokens: 0, cacheReadInputTokens: 0, cacheCreationInputTokens: 0 };
  let seen = false;
  for (const event of events) {
    if (event.type !== "CUSTOM") continue;
    const data = asRecord(event.data);
    if (data.name !== "aex.usage") continue;
    const value = asRecord(data.value);
    const fields = [
      ["input_tokens", "inputTokens"],
      ["output_tokens", "outputTokens"],
      ["cache_read_input_tokens", "cacheReadInputTokens"],
      ["cache_creation_input_tokens", "cacheCreationInputTokens"]
    ] as const;
    for (const [wireName, apiName] of fields) {
      const n = value[wireName];
      if (typeof n === "number" && Number.isFinite(n)) {
        totals[apiName] += n;
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

interface MessageEventLike {
  readonly id?: string;
  readonly type?: string;
  readonly seq?: number;
  readonly sequence?: number;
  readonly recordedAt?: string;
  readonly time?: string;
  readonly receivedAt?: number;
  readonly data?: unknown;
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function timestampFromEpochMs(value: unknown): string | undefined {
  return typeof value === "number" && Number.isFinite(value)
    ? new Date(value).toISOString()
    : undefined;
}

type Mutable<T> = { -readonly [K in keyof T]: T[K] };

function durationMs(start: string | undefined, end: string | undefined): number | undefined {
  if (start === undefined || end === undefined) return undefined;
  const a = Date.parse(start);
  const b = Date.parse(end);
  if (!Number.isFinite(a) || !Number.isFinite(b)) return undefined;
  const delta = b - a;
  return delta >= 0 ? delta : undefined;
}

/**
 * The verdict carried by a run terminal event. Session lifecycle states never
 * appear here; a suspension or approval hold interrupts the run.
 */
const SESSION_TERMINAL_READS = new Set<string>([
  "succeeded",
  "failed",
  "timed_out",
  "cancelled",
  "interrupted"
]);

/** The terminal event's explicit outcome. */
function carriedOutcome(event: AexEvent): SessionRunOutcome | undefined {
  const outcome = event.data.outcome;
  return typeof outcome === "string" && SESSION_TERMINAL_READS.has(outcome)
    ? (outcome as SessionRunOutcome)
    : undefined;
}

export function isSessionRunTerminalEvent(event: AexEvent, runId: string): boolean {
  return event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR");
}

/** Read and validate the committed terminal event's explicit run outcome. */
export function terminalSessionStatusFromEvents(events: readonly AexEvent[], runId: string): SessionRunOutcome {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (!isSessionRunTerminalEvent(event, runId)) continue;
    const carried = carriedOutcome(event);
    if (carried === undefined) {
      throw new SessionStateError("RUN terminal is missing a valid explicit outcome", { runId });
    }
    if (event.type === "RUN_ERROR" && carried !== "failed") {
      throw new SessionStateError("RUN_ERROR must carry outcome=failed", { runId });
    }
    if (event.type === "RUN_FINISHED" && carried === "failed") {
      throw new SessionStateError("a failed run must terminate with RUN_ERROR", { runId });
    }
    return carried;
  }
  throw new SessionStateError(`run ${runId} ended without a matching RUN_FINISHED or RUN_ERROR event`, { runId });
}

/** True only for a completed successful run. */
function isTerminalReadOk(read: SessionRunOutcome): boolean {
  return read === "succeeded";
}

export const PROGRESSING_SESSION_STATUSES = new Set<SessionStatus>([
  "creating",
  "running",
  "suspending",
  "cancelling",
  "deleting"
]);

/** Validate the session projection observed immediately after a durable terminal. */
export function assertSessionCommittedAfterRun(
  session: Session,
  run: SessionRun,
  outcome: SessionRunOutcome
): void {
  if (PROGRESSING_SESSION_STATUSES.has(session.status) || session.currentRun?.runId === run.runId) {
    throw new SessionStateError("RUN terminal was observed before the session state was committed", {
      sessionId: session.id,
      runId: run.runId,
      status: session.status
    });
  }
  if (session.lastRun?.runId !== run.runId) {
    throw new SessionStateError("RUN terminal does not match the session's lastRun", {
      sessionId: session.id,
      runId: run.runId,
      lastRunId: session.lastRun?.runId
    });
  }
  if (outcome === "succeeded" && (session.status !== "idle" || session.acceptsMessages !== true)) {
    throw new SessionStateError("a succeeded RUN_FINISHED must leave the session idle and accepting messages", {
      sessionId: session.id,
      runId: run.runId,
      status: session.status,
      acceptsMessages: session.acceptsMessages
    });
  }
}

function runBillingFromEvents(
  events: readonly AexEvent[],
  runId: string
): { readonly costUsd: number; readonly usage: UsageSummary } {
  const terminal = [...events].reverse().find(
    (event) => event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
  );
  if (terminal === undefined) {
    throw new SessionStateError(`run ${runId} ended without a matching terminal event`, { runId });
  }
  const data = asRecord(terminal.data);
  const costUsd = data.costUsd;
  const providerUsage = data.providerUsage;
  if (typeof costUsd !== "number" || !Number.isFinite(costUsd) || costUsd < 0 || !Array.isArray(providerUsage)) {
    throw new SessionStateError("RUN terminal is missing valid per-run cost and provider usage", { runId });
  }
  return {
    costUsd,
    usage: usageFromProviderUsage(providerUsage as readonly SessionCostProviderUsage[])
  };
}

/**
 * The IMMEDIATE authoritative failure text — the terminal `RUN_ERROR` event's
 * `data.failureMessage`. `result.error` reads this FIRST so a failed (e.g.
 * bad-BYOK) session's error is never empty even before the session-record mirror
 * exposes `errorMessage`.
 */
function failureFromEvents(events: readonly AexEvent[]): string | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.type !== "RUN_ERROR") continue;
    const data = asRecord(event.data);
    for (const key of ["failureMessage", "message", "error"]) {
      const value = data[key];
      if (typeof value === "string" && value.length > 0) return value;
    }
  }
  return undefined;
}

/** The typed schema-decode outcome from the terminal `aex.result.*` event, if any. */
function outcomeFromEvents<T = unknown>(events: readonly AexEventView[]): TurnOutcome<T> | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]!;
    if (event.isResultDecoded()) {
      const value = asRecord(event.data).value;
      const decoded =
        value && typeof value === "object" && !Array.isArray(value) && "value" in (value as object)
          ? (value as { readonly value: unknown }).value
          : value;
      return { kind: "decoded", value: decoded as T };
    }
    if (event.isResultRefused()) {
      const payload = asRecord(asRecord(event.data).value);
      const reason = payload.reason;
      const detail = typeof payload.detail === "string" ? payload.detail : undefined;
      return {
        kind: "refused",
        reason: reason === "schema_violation" || reason === "uncertain" || reason === "refused" ? reason : "refused",
        ...(detail !== undefined ? { detail } : {})
      };
    }
  }
  return undefined;
}

/**
 * Build the unified finished run result (shared by `session.messages.send().finished()`
 * and `Aex.start`): the terminal outcome `status`, `ok`, `costUsd` (>= 0),
 * `usage` (from the run terminal), and event-first `error`.
 */
export function buildTurnResult(
  sessionId: string,
  session: Session,
  run: SessionRun,
  events: readonly AexEventView[],
  files: readonly SessionFile[],
  checkpoint: SessionCheckpointRevision | undefined,
  messages: readonly Message[],
  read: SessionRunOutcome
): SessionRunResult {
  const status = read;
  const ok = isTerminalReadOk(read);
  const { usage, costUsd } = runBillingFromEvents(events, run.runId);
  const error =
    failureFromEvents(events) ??
    (!ok && typeof session.errorMessage === "string" && session.errorMessage ? session.errorMessage : undefined);
  const outcome = outcomeFromEvents(events);
  return {
    sessionId,
    session,
    run,
    status,
    ok,
    costUsd,
    usage,
    ...(error !== undefined ? { error } : {}),
    text: assistantTextFromEvents(events),
    events,
    files,
    ...(checkpoint !== undefined ? { checkpoint } : {}),
    messages,
    ...(outcome !== undefined ? { outcome } : {})
  };
}

export function assertRunCheckpoint(
  events: readonly AexEvent[],
  runId: string,
  revision: SessionCheckpointRevision
): void {
  const terminal = [...events].reverse().find(
    (event) => event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
  );
  const checkpoint = terminal ? asRecord(terminal.data.checkpoint) : {};
  if (
    terminal === undefined ||
    revision.runId !== runId ||
    checkpoint.checkpointId !== revision.checkpointId
  ) {
    throw new SessionStateError("RUN terminal and session files resolved to different checkpoints", {
      runId,
      terminalCheckpointId: checkpoint.checkpointId,
      fileCheckpointId: revision.checkpointId,
      fileCheckpointRunId: revision.runId
    });
  }
}

export function terminalCheckpointId(events: readonly AexEvent[], runId: string): string | undefined {
  const terminal = [...events].reverse().find(
    (event) => event.runId === runId && (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR")
  );
  const checkpointId = terminal ? asRecord(terminal.data.checkpoint).checkpointId : undefined;
  if (typeof checkpointId !== "string" || checkpointId.length === 0) {
    if (terminal?.type === "RUN_ERROR") return undefined;
    throw new SessionStateError("RUN_FINISHED is missing its committed checkpoint", { runId });
  }
  return checkpointId;
}

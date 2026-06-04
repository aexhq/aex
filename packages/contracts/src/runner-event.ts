/**
 * Unified runner event schema. The managed runtime feeds one shape into
 * the hosted aex event pipeline:
 *
 *   - **Goose Managed** — the per-run managed runtime POSTs batches of
 *     NDJSON events to `/runs/{id}/runner/events`; the Goose adapter
 *     translates each event into one or more `RunnerEvent`s.
 *
 * The downstream subscribers (dashboard, SDK `streamEvents`, observable
 * spans) never see runtime-specific wire shapes — they only see
 * `RunnerEvent`s.
 *
 * This is the public event contract consumed by SDK and CLI clients.
 */

import type { JsonValue } from "./submission.js";

/**
 * Schema version. Bump when the shape of `RunnerEvent`, its kind set,
 * or the batch envelope changes. Subscribers gate on this so a future
 * v2 wire shape can land behind a feature flag.
 */
export const RUNNER_EVENT_VERSION = 1 as const;

/**
 * The set of event kinds emitted into the unified stream. Adapters
 * fold runtime-specific events into one of these — anything that
 * doesn't fit is mapped to `notification` so the data is captured
 * even when no UI handler exists yet.
 *
 *   - `runtime_started`  — either runtime announced "ready" (Fly
 *                          machine running goose; Anthropic session
 *                          accepted the first turn).
 *   - `assistant_text`   — model text delta.
 *   - `tool_request`     — model emitted a tool_use / function call.
 *   - `tool_response`    — tool result delivered back to the model.
 *   - `skill_loaded`     — a skill was loaded (Anthropic Skills API
 *                          ref OR a workspace folder mount).
 *   - `file_uploaded`    — a file became available to the agent
 *                          (Files API id OR workspace path).
 *   - `notification`     — runtime/extension notification; catch-all
 *                          for diagnostic data.
 *   - `stream_error`     — stream-level error (non-fatal). Subscribers
 *                          may surface this as a UI warning; the run
 *                          continues unless `runtime_terminal` follows.
 *   - `runtime_terminal` — the run reached a terminal state. The
 *                          adapter MUST emit exactly one of these per
 *                          run; subscribers gate on it for end-of-stream.
 */
export const RUNNER_EVENT_KINDS = [
  "runtime_started",
  "assistant_text",
  "tool_request",
  "tool_response",
  "skill_loaded",
  "file_uploaded",
  "notification",
  "stream_error",
  "runtime_terminal"
] as const;

export type RunnerEventKind = (typeof RUNNER_EVENT_KINDS)[number];

/**
 * One event in the unified stream. `seq` is monotonically increasing
 * within a single run (the adapter is responsible for assigning seqs
 * — no two events with the same `seq` for the same `runId`); `tMs` is
 * a millisecond-resolution timestamp that is also monotonically
 * non-decreasing within a single run (an event's `tMs` is never less
 * than the previous event's `tMs`). `data` carries the runtime- and
 * kind-specific payload, JSON-typed so the batch round-trips through
 * the database column without ad-hoc serialization.
 */
export interface RunnerEvent {
  readonly seq: number;
  readonly tMs: number;
  readonly kind: RunnerEventKind;
  readonly data: Readonly<Record<string, JsonValue>>;
}

/**
 * Batch envelope. The runner ships one or more events per POST so the
 * inbox writes them atomically. `events` MUST be sorted by `seq`
 * ascending; api.aex.dev rejects malformed batches before touching
 * Postgres.
 */
export interface RunnerEventBatch {
  readonly v: typeof RUNNER_EVENT_VERSION;
  readonly runId: string;
  readonly events: readonly RunnerEvent[];
}

/**
 * Maximum number of events per batch. Bounds the size of the body
 * api.aex.dev accepts and the size of the downstream Postgres / KV
 * write. Larger streams are split into multiple batches; the runner is
 * responsible for chunking.
 */
export const RUNNER_EVENT_BATCH_MAX_EVENTS = 256 as const;

/**
 * Validation outcome for an inbound batch. The `ok` branch returns
 * the parsed batch with frozen events; the `error` branch returns the
 * code + a short human message suitable for an HTTP 400 body. Codes
 * are stable strings — the dashboard / SDK may branch on them.
 */
export type RunnerEventBatchValidation =
  | { readonly ok: true; readonly batch: RunnerEventBatch }
  | { readonly ok: false; readonly code: RunnerEventBatchValidationCode; readonly message: string };

export const RUNNER_EVENT_BATCH_VALIDATION_CODES = [
  "invalid_envelope",
  "version_mismatch",
  "missing_run_id",
  "empty_batch",
  "batch_too_large",
  "invalid_event",
  "seq_not_monotonic",
  "t_ms_not_monotonic"
] as const;
export type RunnerEventBatchValidationCode = (typeof RUNNER_EVENT_BATCH_VALIDATION_CODES)[number];

/**
 * Parse + validate an inbound runner event batch (untrusted input).
 * Used at the api.aex.dev ingress so adapters never have to
 * re-check the wire shape, and used by tests to assert the contract.
 *
 * Successful validation guarantees:
 *   - top-level envelope matches {@link RunnerEventBatch}
 *   - `v === RUNNER_EVENT_VERSION`
 *   - `runId` is a non-empty string
 *   - `events` is a non-empty array of at most
 *     {@link RUNNER_EVENT_BATCH_MAX_EVENTS} entries
 *   - each event is a {@link RunnerEvent} with a known `kind`
 *   - `seq` is strictly increasing across the batch
 *   - `tMs` is non-decreasing across the batch
 */
export function validateRunnerEventBatch(input: unknown): RunnerEventBatchValidation {
  if (!isRecord(input)) {
    return invalid("invalid_envelope", "batch must be a JSON object");
  }
  if (input.v !== RUNNER_EVENT_VERSION) {
    return invalid(
      "version_mismatch",
      `batch.v must equal ${RUNNER_EVENT_VERSION} (got ${JSON.stringify(input.v)})`
    );
  }
  if (typeof input.runId !== "string" || input.runId.length === 0) {
    return invalid("missing_run_id", "batch.runId must be a non-empty string");
  }
  if (!Array.isArray(input.events) || input.events.length === 0) {
    return invalid("empty_batch", "batch.events must be a non-empty array");
  }
  if (input.events.length > RUNNER_EVENT_BATCH_MAX_EVENTS) {
    return invalid(
      "batch_too_large",
      `batch.events has ${input.events.length} entries; max is ${RUNNER_EVENT_BATCH_MAX_EVENTS}`
    );
  }
  let lastSeq = Number.NEGATIVE_INFINITY;
  let lastTMs = Number.NEGATIVE_INFINITY;
  const events: RunnerEvent[] = [];
  for (let i = 0; i < input.events.length; i++) {
    const evt = input.events[i];
    if (!isRecord(evt)) {
      return invalid("invalid_event", `events[${i}] must be a JSON object`);
    }
    if (
      typeof evt.seq !== "number" ||
      !Number.isFinite(evt.seq) ||
      !Number.isInteger(evt.seq) ||
      evt.seq < 0
    ) {
      return invalid("invalid_event", `events[${i}].seq must be a non-negative integer`);
    }
    if (
      typeof evt.tMs !== "number" ||
      !Number.isFinite(evt.tMs) ||
      !Number.isInteger(evt.tMs) ||
      evt.tMs < 0
    ) {
      return invalid("invalid_event", `events[${i}].tMs must be a non-negative integer`);
    }
    if (
      typeof evt.kind !== "string" ||
      !(RUNNER_EVENT_KINDS as readonly string[]).includes(evt.kind)
    ) {
      return invalid(
        "invalid_event",
        `events[${i}].kind must be one of: ${RUNNER_EVENT_KINDS.join(", ")} (got ${JSON.stringify(evt.kind)})`
      );
    }
    if (!isRecord(evt.data)) {
      return invalid("invalid_event", `events[${i}].data must be a JSON object`);
    }
    if (!isJsonRecord(evt.data)) {
      return invalid("invalid_event", `events[${i}].data must be JSON-serializable`);
    }
    if (evt.seq <= lastSeq) {
      return invalid(
        "seq_not_monotonic",
        `events[${i}].seq=${evt.seq} must be strictly greater than the previous seq=${lastSeq}`
      );
    }
    if (evt.tMs < lastTMs) {
      return invalid(
        "t_ms_not_monotonic",
        `events[${i}].tMs=${evt.tMs} must be >= the previous tMs=${lastTMs}`
      );
    }
    lastSeq = evt.seq;
    lastTMs = evt.tMs;
    events.push({
      seq: evt.seq,
      tMs: evt.tMs,
      kind: evt.kind as RunnerEventKind,
      data: Object.freeze({ ...evt.data }) as Readonly<Record<string, JsonValue>>
    });
  }
  return {
    ok: true,
    batch: { v: RUNNER_EVENT_VERSION, runId: input.runId, events: Object.freeze(events) }
  };
}

function invalid(code: RunnerEventBatchValidationCode, message: string): RunnerEventBatchValidation {
  return { ok: false, code, message };
}

function isRecord(input: unknown): input is Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}

function isJsonRecord(input: Record<string, unknown>): input is Record<string, JsonValue> {
  for (const value of Object.values(input)) {
    if (!isJsonValue(value)) return false;
  }
  return true;
}

function isJsonValue(input: unknown): input is JsonValue {
  if (input === null) return true;
  const t = typeof input;
  if (t === "string" || t === "boolean") return true;
  if (t === "number") return Number.isFinite(input as number);
  if (Array.isArray(input)) return input.every(isJsonValue);
  if (isRecord(input)) {
    for (const v of Object.values(input)) {
      if (!isJsonValue(v)) return false;
    }
    return true;
  }
  return false;
}

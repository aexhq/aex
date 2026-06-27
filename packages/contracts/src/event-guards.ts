/**
 * Type-narrowing guards over the loose {@link RunEvent} `listEvents` shape.
 *
 * The envelope guards in `event-envelope.ts` operate on the coordinator
 * {@link import("./event-envelope.js").AexEvent} and return a plain `boolean`.
 * These mirror the same AG-UI discriminants but operate on the {@link RunEvent}
 * snapshot shape returned by `listEvents` / `RunResult.events` and NARROW
 * `event.data` to the fields that event type carries, so a consumer reads
 * `event.data.text` (etc.) without a manual cast.
 *
 * They are DISCRIMINANT-only at runtime (they test `event.type`), matching the
 * envelope guards: the narrowed `data` shape is a typing convenience, not a deep
 * runtime validation. Parse/verify the payload fields before trusting them when
 * the producer is untrusted.
 *
 * The parameter is the minimal `{ type }` discriminant both the {@link RunEvent}
 * snapshot and the coordinator `AexEvent` envelope satisfy, so the guards accept
 * either shape while narrowing to the {@link RunEvent}-typed payloads.
 */

import type { RunEvent } from "./runtime-types.js";

/** A `TEXT_MESSAGE_CONTENT` run event with its assistant-text payload narrowed. */
export interface TextMessageRunEvent extends RunEvent {
  readonly type: "TEXT_MESSAGE_CONTENT";
  readonly data: { readonly text: string; readonly messageId?: string };
}

/** A `TOOL_CALL_START` run event with the tool-call id/name/args narrowed. */
export interface ToolCallStartRunEvent extends RunEvent {
  readonly type: "TOOL_CALL_START";
  readonly data: { readonly id: string; readonly name: string; readonly arguments?: Record<string, unknown> };
}

/** A `TOOL_CALL_RESULT` run event with the correlating id + result narrowed. */
export interface ToolCallResultRunEvent extends RunEvent {
  readonly type: "TOOL_CALL_RESULT";
  readonly data: { readonly id: string; readonly content?: unknown; readonly isError?: boolean };
}

/** A terminal `RUN_FINISHED` run event. */
export interface RunFinishedRunEvent extends RunEvent {
  readonly type: "RUN_FINISHED";
  readonly data: Record<string, unknown>;
}

/** True for an assistant text event; narrows `event.data.text` to `string`. */
export function isTextMessage(event: { readonly type: string }): event is TextMessageRunEvent {
  return event.type === "TEXT_MESSAGE_CONTENT";
}

/** True for a tool-call start event; narrows `event.data` to `{ id, name }`. */
export function isToolCallStart(event: { readonly type: string }): event is ToolCallStartRunEvent {
  return event.type === "TOOL_CALL_START";
}

/** True for a tool-call result event; narrows `event.data` to `{ id, content }`. */
export function isToolCallResult(event: { readonly type: string }): event is ToolCallResultRunEvent {
  return event.type === "TOOL_CALL_RESULT";
}

/** True for the terminal success event; narrows `event.data` to a record. */
export function isRunFinished(event: { readonly type: string }): event is RunFinishedRunEvent {
  return event.type === "RUN_FINISHED";
}

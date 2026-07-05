/**
 * Event methods over the coordinator {@link AexEvent} the SDK yields.
 *
 * The SDK hands users an {@link AexEventView} — the coordinator envelope enriched
 * with one type-predicate METHOD per standardized event type, so a consumer
 * branches on the event with `event.isTextMessage()` / `event.isToolCallStart()`
 * / … instead of a free-function guard or a raw `event.type === "…"` compare.
 *
 * `isTextMessage()` / `isToolCallStart()` / `isToolCallResult()` are TS type
 * predicates: inside the guarded branch `event.data` NARROWS to that event type's
 * payload fields (e.g. `event.data.text` is `string`), exactly what the former
 * free `is*` guards gave. Like those guards the narrowing is DISCRIMINANT-only at
 * runtime (it tests `type`); the narrowed `data` shape is a typing convenience,
 * not deep runtime validation. Parse/verify the payload fields before trusting
 * them when the producer is untrusted.
 *
 * The view is a thin wrapper: {@link asAexEventView} sets the prototype and
 * copies the envelope's own fields, so every {@link AexEvent} field (`type`,
 * `data`, `source`, `sequence`, …) reads exactly as before, the methods live on
 * the prototype (they never serialize), and an `AexEventView` is still a
 * structural {@link AexEvent}.
 */

import type { AexEvent, AexEventSource } from "./event-envelope.js";
import { channelOf, isRunSettled } from "./event-envelope.js";
import type { JsonValue } from "./submission.js";

/** A `TEXT_MESSAGE_CONTENT` event with its assistant-text payload narrowed. */
export interface TextMessageEventView extends AexEventView {
  readonly type: "TEXT_MESSAGE_CONTENT";
  readonly data: Readonly<Record<string, JsonValue>> & { readonly text: string; readonly messageId?: string };
}

/** A `TOOL_CALL_START` event with the tool-call id/name/args narrowed. */
export interface ToolCallStartEventView extends AexEventView {
  readonly type: "TOOL_CALL_START";
  readonly data: Readonly<Record<string, JsonValue>> & {
    readonly id: string;
    readonly name: string;
    readonly arguments?: { readonly [key: string]: JsonValue };
  };
}

/** A `TOOL_CALL_RESULT` event with the correlating id + result narrowed. */
export interface ToolCallResultEventView extends AexEventView {
  readonly type: "TOOL_CALL_RESULT";
  readonly data: Readonly<Record<string, JsonValue>> & {
    readonly id: string;
    readonly content?: JsonValue;
    readonly isError?: boolean;
  };
}

/**
 * A coordinator {@link AexEvent} with the standardized-event-type guards carried
 * as METHODS (`event.isTextMessage()`, `event.isToolCallStart()`, …). Extends
 * {@link AexEvent} by declaration merging, so it exposes every envelope field
 * unchanged. Construct one with {@link asAexEventView}.
 */
export class AexEventView {
  /** True for the run-start lifecycle event. */
  isRunStarted(): boolean {
    return this.type === "RUN_STARTED";
  }
  /** True for the terminal success event. */
  isRunFinished(): boolean {
    return this.type === "RUN_FINISHED";
  }
  /** True for the terminal error event. */
  isRunError(): boolean {
    return this.type === "RUN_ERROR";
  }
  /** True for a terminal event of either flavour (finished or error). */
  isRunTerminal(): boolean {
    return this.type === "RUN_FINISHED" || this.type === "RUN_ERROR";
  }
  /** True for an assistant text event; narrows `data.text` to `string`. */
  isTextMessage(): this is TextMessageEventView {
    return this.type === "TEXT_MESSAGE_CONTENT";
  }
  /** True for a tool-call start event; narrows `data` to `{ id, name }`. */
  isToolCallStart(): this is ToolCallStartEventView {
    return this.type === "TOOL_CALL_START";
  }
  /** True for a tool-call result event; narrows `data` to `{ id, content }`. */
  isToolCallResult(): this is ToolCallResultEventView {
    return this.type === "TOOL_CALL_RESULT";
  }
  /** True for an aex-native CUSTOM event (an `aex.*` name under `data.name`). */
  isCustom(): boolean {
    return this.type === "CUSTOM";
  }
  /** True when the record is a log line (the `log` channel / `LOG` type). */
  isLog(): boolean {
    return channelOf(this) === "log";
  }
  /** True when the record is a typed AG-UI event (the `event` channel). */
  isEventChannel(): boolean {
    return channelOf(this) === "event";
  }
  /**
   * True for the settle-consistency barrier event AND for a managed-runtime
   * session-park terminal (idle/error/suspended) — the one check that reliably
   * means "this stream is done and the record is authoritative".
   */
  isRunSettled(): boolean {
    return isRunSettled(this);
  }
  /** True when the record originates from the given coarse source. */
  isFromSource(source: AexEventSource): boolean {
    return this.source === source;
  }
}
// Declaration merge: the view instance carries every AexEvent field (ambient, so
// no class-field initializer is required) plus the methods above.
export interface AexEventView extends AexEvent {}

/**
 * Wrap a coordinator {@link AexEvent} as an {@link AexEventView}: the methods
 * live on the prototype and the envelope's own fields are copied across, so the
 * returned value reads identically to the raw envelope and serializes to the
 * same bytes (prototype methods are not own-enumerable).
 */
export function asAexEventView(event: AexEvent): AexEventView {
  const view = Object.create(AexEventView.prototype) as AexEventView;
  return Object.assign(view, event);
}

/** Wrap each event of a list as an {@link AexEventView}. */
export function asAexEventViews(events: readonly AexEvent[]): AexEventView[] {
  return events.map(asAexEventView);
}

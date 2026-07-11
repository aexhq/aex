/** Guard-bearing views over durable and provisional coordinator events. */

import type {
  AexEvent,
  AexEventBase,
  AexEventSource,
  AexLiveEvent,
  AexStreamEvent
} from "./event-envelope.js";
import { channelOf, isAwaitingApproval, isResultDecoded, isResultRefused } from "./event-envelope.js";
import type { JsonValue } from "./submission.js";

type TextMessageFields = {
  readonly type: "TEXT_MESSAGE_CONTENT";
  readonly data: Readonly<Record<string, JsonValue>> & {
    readonly text: string;
    readonly messageId?: string;
    /** True only on a provisional, non-replayable live delta. */
    readonly delta?: boolean;
  };
};

type ToolCallStartFields = {
  readonly type: "TOOL_CALL_START";
  readonly data: Readonly<Record<string, JsonValue>> & {
    readonly id: string;
    readonly name: string;
    readonly arguments?: { readonly [key: string]: JsonValue };
  };
  toolCallId(): string;
};

type ToolCallResultFields = {
  readonly type: "TOOL_CALL_RESULT";
  readonly data: Readonly<Record<string, JsonValue>> & {
    readonly id: string;
    readonly content?: JsonValue;
    readonly isError?: boolean;
  };
  toolCallId(): string;
};

/** Shared methods mixed into both durable and live-only event views. */
class EventViewPrototype {
  isRunStarted(): boolean {
    return this.type === "RUN_STARTED";
  }
  isRunFinished(): boolean {
    return this.type === "RUN_FINISHED";
  }
  isRunError(): boolean {
    return this.type === "RUN_ERROR";
  }
  isRunTerminal(): boolean {
    return this.type === "RUN_FINISHED" || this.type === "RUN_ERROR";
  }
  isTextMessage(): this is EventViewPrototype & TextMessageFields {
    return this.type === "TEXT_MESSAGE_CONTENT";
  }
  isToolCallStart(): this is EventViewPrototype & ToolCallStartFields {
    return this.type === "TOOL_CALL_START";
  }
  isToolCallResult(): this is EventViewPrototype & ToolCallResultFields {
    return this.type === "TOOL_CALL_RESULT";
  }
  toolCallId(): string | undefined {
    const id = this.data.id;
    return typeof id === "string" ? id : undefined;
  }
  isCustom(): boolean {
    return this.type === "CUSTOM";
  }
  isAwaitingApproval(): boolean {
    return isAwaitingApproval(this);
  }
  isResultDecoded(): boolean {
    return isResultDecoded(this);
  }
  isResultRefused(): boolean {
    return isResultRefused(this);
  }
  isLog(): boolean {
    return channelOf(this) === "log";
  }
  isEventChannel(): boolean {
    return channelOf(this) === "event";
  }
  isFromSource(source: AexEventSource): boolean {
    return this.source === source;
  }
}

interface EventViewPrototype extends AexEventBase {}

/** A durable event view. List, archive, and finished-result APIs return this. */
export type AexEventView = EventViewPrototype & AexEvent;

/** A provisional live-only event view with no durable `sequence`. */
export type AexLiveEventView = EventViewPrototype & AexLiveEvent;

/** A view yielded by live coordinator streams. */
export type AexStreamEventView = AexEventView | AexLiveEventView;

export type TextMessageEventView = AexStreamEventView & TextMessageFields;
export type ToolCallStartEventView = AexStreamEventView & ToolCallStartFields;
export type ToolCallResultEventView = AexStreamEventView & ToolCallResultFields;

/** Constructor value for `instanceof`; event fields are mixed in. */
export const AexEventView = EventViewPrototype;

/** Wrap a durable coordinator event while preserving its precise replayable type. */
export function asAexEventView(event: AexEvent): AexEventView {
  return wrap(event) as AexEventView;
}

/** Wrap a live stream event while preserving durable-vs-live discrimination. */
export function asAexStreamEventView(event: AexStreamEvent): AexStreamEventView {
  return wrap(event) as AexStreamEventView;
}

/** Wrap each durable event in a list. */
export function asAexEventViews(events: readonly AexEvent[]): AexEventView[] {
  return events.map(asAexEventView);
}

function wrap(event: AexStreamEvent): EventViewPrototype {
  const view = Object.create(EventViewPrototype.prototype) as EventViewPrototype;
  return Object.assign(view, event);
}

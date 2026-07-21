/** Guard-bearing views over durable and provisional coordinator events. */

import type {
  AexCustomEvent,
  AexEvent,
  AexEventBase,
  AexEventSource,
  AexLiveEvent,
  AexLogEvent,
  AexRunErrorEvent,
  AexRunFinishedEvent,
  AexRunStartedEvent,
  AexStreamEvent,
  AexTextMessageEvent,
  AexToolCallResultEvent,
  AexToolCallStartEvent
} from "./event-envelope.js";
import {
  AEX_RESULT_DECODED_NAME,
  AEX_RESULT_REFUSED_NAME,
  AEX_SESSION_AWAITING_APPROVAL_NAME,
  isAwaitingApproval as canonicalIsAwaitingApproval,
  isCustom as canonicalIsCustom,
  isEventChannel as canonicalIsEventChannel,
  isFromSource as canonicalIsFromSource,
  isLog as canonicalIsLog,
  isResultDecoded as canonicalIsResultDecoded,
  isResultRefused as canonicalIsResultRefused,
  isRunError as canonicalIsRunError,
  isRunFinished as canonicalIsRunFinished,
  isRunStarted as canonicalIsRunStarted,
  isRunTerminal as canonicalIsRunTerminal,
  isTextMessage as canonicalIsTextMessage,
  isToolCallResult as canonicalIsToolCallResult,
  isToolCallStart as canonicalIsToolCallStart
} from "./event-envelope.js";

type ToolCallIdMethod = {
  toolCallId(): string;
};

/** Shared methods mixed into both durable and live-only event views. */
class EventViewPrototype {
  isRunStarted(): this is EventViewPrototype & AexRunStartedEvent {
    return canonicalIsRunStarted(this);
  }
  isRunFinished(): this is EventViewPrototype & AexRunFinishedEvent {
    return canonicalIsRunFinished(this);
  }
  isRunError(): this is EventViewPrototype & AexRunErrorEvent {
    return canonicalIsRunError(this);
  }
  isRunTerminal(): this is EventViewPrototype & (AexRunFinishedEvent | AexRunErrorEvent) {
    return canonicalIsRunTerminal(this);
  }
  isTextMessage(): this is EventViewPrototype & AexTextMessageEvent {
    return canonicalIsTextMessage(this);
  }
  isToolCallStart(): this is EventViewPrototype & AexToolCallStartEvent & ToolCallIdMethod {
    return canonicalIsToolCallStart(this);
  }
  isToolCallResult(): this is EventViewPrototype & AexToolCallResultEvent & ToolCallIdMethod {
    return canonicalIsToolCallResult(this);
  }
  toolCallId(): string | undefined {
    const id = this.data.id;
    return typeof id === "string" ? id : undefined;
  }
  isCustom(): this is EventViewPrototype & AexCustomEvent {
    return canonicalIsCustom(this);
  }
  isAwaitingApproval(): this is EventViewPrototype & AexCustomEvent<typeof AEX_SESSION_AWAITING_APPROVAL_NAME> {
    return canonicalIsAwaitingApproval(this);
  }
  isResultDecoded(): this is EventViewPrototype & AexCustomEvent<typeof AEX_RESULT_DECODED_NAME> {
    return canonicalIsResultDecoded(this);
  }
  isResultRefused(): this is EventViewPrototype & AexCustomEvent<typeof AEX_RESULT_REFUSED_NAME> {
    return canonicalIsResultRefused(this);
  }
  isLog(): this is EventViewPrototype & AexLogEvent {
    return canonicalIsLog(this);
  }
  isEventChannel(): boolean {
    return canonicalIsEventChannel(this);
  }
  isFromSource(source: AexEventSource): boolean {
    return canonicalIsFromSource(this, source);
  }
}

interface EventViewPrototype extends AexEventBase {}

/** A durable event view. List, archive, and finished-result APIs return this. */
export type AexEventView = EventViewPrototype & AexEvent;

/** A provisional live-only event view with no durable `sequence`. */
export type AexLiveEventView = EventViewPrototype & AexLiveEvent;

/** A view yielded by live coordinator streams. */
export type AexStreamEventView = AexEventView | AexLiveEventView;

export type TextMessageEventView = AexStreamEventView & AexTextMessageEvent;
export type ToolCallStartEventView = AexStreamEventView & AexToolCallStartEvent & ToolCallIdMethod;
export type ToolCallResultEventView = AexStreamEventView & AexToolCallResultEvent & ToolCallIdMethod;

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

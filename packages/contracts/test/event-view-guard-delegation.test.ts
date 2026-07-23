import { readFileSync } from "node:fs";
import { describe, expect, it } from "bun:test";
import {
  AEX_EVENT_SPECVERSION,
  asAexEventView,
  isAwaitingApproval,
  isCustom,
  isLog,
  isResultDecoded,
  isResultRefused,
  isRunError,
  isRunFinished,
  isRunStarted,
  isRunTerminal,
  isTextMessage,
  isToolCallResult,
  isToolCallStart,
  type AexCustomEvent,
  type AexEvent,
  type AexEventView,
  type AexLogEvent,
  type AexRunErrorEvent,
  type AexRunFinishedEvent,
  type AexRunStartedEvent,
  type AexStreamEventView,
  type AexTextMessageEvent,
  type AexToolCallResultEvent,
  type AexToolCallStartEvent,
  type JsonValue
} from "../src/index.js";

function event(type: string, data: Readonly<Record<string, JsonValue>>, extra: Partial<AexEvent> = {}): AexEvent {
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: "ses_view_guards:3",
    source: "runtime",
    type,
    subject: "ses_view_guards",
    threadId: "ses_view_guards",
    runId: "run_view_guards",
    time: "2026-07-21T12:00:00.000Z",
    sequence: 3,
    data,
    ...extra
  };
}

/** Compile-time proof that every view predicate exposes SC-4's validated shape. */
function assertHonestNarrowing(view: AexStreamEventView): void {
  if (view.isRunStarted()) {
    const narrowed: AexRunStartedEvent = view;
    const source: string | undefined = view.data.source;
    void narrowed;
    void source;
  }
  if (view.isRunFinished()) {
    const narrowed: AexRunFinishedEvent = view;
    const outcome: "succeeded" | "failed" | "timed_out" | "cancelled" | "interrupted" = view.data.outcome;
    void narrowed;
    void outcome;
  }
  if (view.isRunError()) {
    const narrowed: AexRunErrorEvent = view;
    const failureClass: string = view.data.failureClass;
    void narrowed;
    void failureClass;
  }
  if (view.isRunTerminal()) {
    const narrowed: AexRunFinishedEvent | AexRunErrorEvent = view;
    const outcome: "succeeded" | "failed" | "timed_out" | "cancelled" | "interrupted" = view.data.outcome;
    void narrowed;
    void outcome;
  }
  if (view.isTextMessage()) {
    const narrowed: AexTextMessageEvent = view;
    const text: string = view.data.text;
    void narrowed;
    void text;
  }
  if (view.isToolCallStart()) {
    const narrowed: AexToolCallStartEvent = view;
    const name: string = view.data.name;
    const callId: string = view.data.id;
    void narrowed;
    void name;
    void callId;
  }
  if (view.isToolCallResult()) {
    const narrowed: AexToolCallResultEvent = view;
    const content: JsonValue = view.data.content;
    const callId: string = view.data.id;
    void narrowed;
    void content;
    void callId;
  }
  if (view.isCustom()) {
    const narrowed: AexCustomEvent = view;
    const name: string = view.data.name;
    void narrowed;
    void name;
  }
  if (view.isAwaitingApproval()) {
    const name: "aex.session.awaiting_approval" = view.data.name;
    void name;
  }
  if (view.isResultDecoded()) {
    const name: "aex.result.decoded" = view.data.name;
    void name;
  }
  if (view.isResultRefused()) {
    const name: "aex.result.refused" = view.data.name;
    void name;
  }
  if (view.isLog()) {
    const narrowed: AexLogEvent = view;
    const channel: "log" = view.channel;
    void narrowed;
    void channel;
  }
}

void assertHonestNarrowing;

const guards = [
  ["isRunStarted", isRunStarted],
  ["isRunFinished", isRunFinished],
  ["isRunError", isRunError],
  ["isRunTerminal", isRunTerminal],
  ["isTextMessage", isTextMessage],
  ["isToolCallStart", isToolCallStart],
  ["isToolCallResult", isToolCallResult],
  ["isCustom", isCustom],
  ["isAwaitingApproval", isAwaitingApproval],
  ["isResultDecoded", isResultDecoded],
  ["isResultRefused", isResultRefused],
  ["isLog", isLog]
] as const;

const malformedSamples = [
  event("RUN_STARTED", { turnSeq: -1 }),
  event("RUN_FINISHED", {}),
  event("RUN_ERROR", { outcome: "failed", failureClass: "provider_permanent" }),
  event("TEXT_MESSAGE_CONTENT", { text: 42 }),
  event("TOOL_CALL_START", { id: "call_1" }),
  event("TOOL_CALL_RESULT", { id: "call_1" }),
  event("CUSTOM", { name: "aex.session.awaiting_approval" }),
  event("LOG", { level: "warn", message: "retry" })
] as const;

const samples = [
  event("RUN_STARTED", { source: "session" }),
  event("RUN_FINISHED", { outcome: "succeeded" }),
  event("RUN_ERROR", { outcome: "failed", failureClass: "provider_permanent", failureMessage: "failed" }),
  event("TEXT_MESSAGE_CONTENT", { text: "hello" }),
  event("TOOL_CALL_START", { id: "call_1", name: "read_file" }),
  event("TOOL_CALL_RESULT", { id: "call_1", content: null }),
  event("CUSTOM", { name: "aex.session.awaiting_approval", value: {} }),
  event("CUSTOM", { name: "aex.result.decoded", value: 42 }),
  event("CUSTOM", { name: "aex.result.refused", value: { reason: "schema" } }),
  event("LOG", { level: "warn", message: "retry" }, { channel: "log", level: "warn" }),
  event("FUTURE_EVENT_V2", { opaque: ["preserve"] }),
  ...malformedSamples
] as const;

describe("AexEventView canonical guard delegation", () => {
  it.each(guards)("keeps %s byte-for-byte aligned with its canonical free guard", (method, freeGuard) => {
    for (const raw of samples) {
      const view = asAexEventView(raw);
      const before = JSON.stringify(raw);
      expect((view[method] as () => boolean)()).toBe(freeGuard(raw));
      expect(JSON.stringify(view)).toBe(before);
    }
  });

  it("rejects malformed recognized payloads through method guards", () => {
    const malformed = malformedSamples.map(asAexEventView);
    expect(malformed[0]!.isRunStarted()).toBe(false);
    expect(malformed[1]!.isRunFinished()).toBe(false);
    expect(malformed[1]!.isRunTerminal()).toBe(false);
    expect(malformed[2]!.isRunError()).toBe(false);
    expect(malformed[2]!.isRunTerminal()).toBe(false);
    expect(malformed[3]!.isTextMessage()).toBe(false);
    expect(malformed[4]!.isToolCallStart()).toBe(false);
    expect(malformed[5]!.isToolCallResult()).toBe(false);
    expect(malformed[6]!.isCustom()).toBe(false);
    expect(malformed[6]!.isAwaitingApproval()).toBe(false);
    expect(malformed[7]!.isLog()).toBe(false);
  });

  it("keeps each runtime predicate method as a direct canonical delegation", () => {
    const source = readFileSync(new URL("../src/event-view.ts", import.meta.url), "utf8");
    for (const [method] of guards) {
      const canonical = `canonical${method[0]!.toUpperCase()}${method.slice(1)}`;
      expect(source).toMatch(new RegExp(`${method}\\([^)]*\\)[^{]*\\{\\s*return ${canonical}\\(this\\);\\s*\\}`));
    }
    expect(source).toMatch(/isEventChannel\(\): boolean \{\s*return canonicalIsEventChannel\(this\);\s*\}/);
    expect(source).toMatch(
      /isFromSource\(source: AexEventSource\): boolean \{\s*return canonicalIsFromSource\(this, source\);\s*\}/
    );
    expect(source).not.toContain("this.type ===");
  });

  it("preserves the public durable view type", () => {
    const view: AexEventView = asAexEventView(samples[0]);
    expect(view.sequence).toBe(3);
  });
});

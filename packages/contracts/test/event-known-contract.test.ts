import { describe, expect, it } from "vitest";
import {
  AEX_EVENT_SPECVERSION,
  MalformedAexEventError,
  classifyAexEvent,
  isCustom,
  isKnownAexEvent,
  isLog,
  isRunError,
  isRunFinished,
  isRunStarted,
  isRunTerminal,
  isTextMessage,
  isToolCallResult,
  isToolCallStart,
  toAGUI,
  type AexEvent,
  type JsonValue
} from "../src/index.js";

function knownPayloadFact(raw: AexEvent): string {
  if (!isKnownAexEvent(raw)) return "not-known";
  switch (raw.type) {
    case "RUN_STARTED":
      return raw.data.source ?? "started";
    case "RUN_FINISHED":
      return raw.data.outcome;
    case "RUN_ERROR":
      return raw.data.failureClass;
    case "TEXT_MESSAGE_CONTENT":
      return raw.data.text;
    case "TOOL_CALL_START":
      return raw.data.name;
    case "TOOL_CALL_RESULT":
      return raw.data.id;
    case "CUSTOM":
      return raw.data.name;
    case "LOG":
      return raw.data.message;
    default: {
      const exhaustive: never = raw;
      return exhaustive;
    }
  }
}

function event(type: string, data: Readonly<Record<string, JsonValue>>, extra: Partial<AexEvent> = {}): AexEvent {
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: "ses_known:7",
    source: "runtime",
    type,
    subject: "ses_known",
    threadId: "ses_known",
    runId: "run_known",
    time: "2026-07-21T10:00:00.000Z",
    sequence: 7,
    data,
    ...extra
  };
}

describe("known AexEvent payload contracts", () => {
  it("narrows every current event type to its validated payload", () => {
    const started = event("RUN_STARTED", { source: "session" });
    if (!isRunStarted(started)) throw new Error("expected RUN_STARTED");
    const startedType: "RUN_STARTED" = started.type;

    const finished = event("RUN_FINISHED", { outcome: "succeeded", result: { answer: 42 } });
    if (!isRunFinished(finished)) throw new Error("expected RUN_FINISHED");
    const outcome: "succeeded" | "failed" | "timed_out" | "cancelled" | "interrupted" = finished.data.outcome;
    const result: JsonValue | undefined = finished.data.result;

    const failed = event("RUN_ERROR", {
      outcome: "failed",
      failureClass: "provider_permanent",
      failureMessage: "provider refused the request",
      providerFault: { kind: "provider_error", status: 400 }
    });
    if (!isRunError(failed)) throw new Error("expected RUN_ERROR");
    const failureClass: string = failed.data.failureClass;
    const failureMessage: string = failed.data.failureMessage;
    expect(failed.data.providerFault?.kind).toBe("provider_error");
    if (!isRunTerminal(failed)) throw new Error("expected a terminal");
    const terminalType: "RUN_FINISHED" | "RUN_ERROR" = failed.type;

    const text = event("TEXT_MESSAGE_CONTENT", { text: "hello", delta: true, messageId: "msg_1" });
    if (!isTextMessage(text)) throw new Error("expected text");
    const textValue: string = text.data.text;
    const delta: boolean | undefined = text.data.delta;

    const start = event("TOOL_CALL_START", { id: "call_1", name: "read_file", arguments: { path: "/x" } });
    if (!isToolCallStart(start)) throw new Error("expected tool start");
    const callName: string = start.data.name;
    const args: Readonly<Record<string, JsonValue>> | undefined = start.data.arguments;

    const toolResult = event("TOOL_CALL_RESULT", { id: "call_1", content: [{ type: "text", text: "ok" }] });
    if (!isToolCallResult(toolResult)) throw new Error("expected tool result");
    const content: JsonValue = toolResult.data.content;

    const custom = event("CUSTOM", { name: "aex.notification", value: { reason: "waiting" } });
    if (!isCustom(custom)) throw new Error("expected custom");
    const customName: string = custom.data.name;
    const customValue: JsonValue = custom.data.value;

    const log = event(
      "LOG",
      { level: "warn", message: "retrying", fields: { attempt: 2 } },
      { channel: "log", level: "warn", message: "retrying" }
    );
    if (!isLog(log)) throw new Error("expected log");
    const logLevel: "info" | "warn" | "error" = log.data.level;
    const logMessage: string = log.data.message;

    expect([
      startedType,
      outcome,
      result,
      failureClass,
      failureMessage,
      terminalType,
      textValue,
      delta,
      callName,
      args,
      content,
      customName,
      customValue,
      logLevel,
      logMessage
    ]).toHaveLength(15);
  });

  it("classifies a valid known event without replacing it", () => {
    const raw = event("TEXT_MESSAGE_CONTENT", { text: "hello" });
    const classified = classifyAexEvent(raw);
    expect(classified.kind).toBe("known");
    if (classified.kind !== "known") throw new Error("expected known");
    expect(classified.event).toBe(raw);
    expect(knownPayloadFact(raw)).toBe("hello");
  });
});

describe("malformed known AexEvent handling", () => {
  it.each([
    ["RUN_STARTED", { turnSeq: -1 }, "data.turnSeq"],
    ["RUN_FINISHED", {}, "data.outcome"],
    ["RUN_ERROR", { outcome: "failed", failureClass: "provider_permanent" }, "data.failureMessage"],
    ["RUN_ERROR", {
      outcome: "failed",
      failureClass: "transient-provider",
      failureMessage: "provider unavailable",
      providerFault: { kind: "rate_limit", statusCode: 429 }
    }, "data.providerFault"],
    ["TEXT_MESSAGE_CONTENT", { text: 42 }, "data.text"],
    ["TOOL_CALL_START", { id: "call_1" }, "data.name"],
    ["TOOL_CALL_RESULT", { id: "call_1" }, "data.content"],
    ["CUSTOM", { name: "aex.notification" }, "data.value"],
    ["LOG", { level: "warn", message: "retry" }, "channel"]
  ] as const)("surfaces malformed %s instead of projecting a fallback", (type, data, path) => {
    const raw = event(type, data);
    const classified = classifyAexEvent(raw);
    expect(classified).toMatchObject({ kind: "malformed_known", issue: { type, path } });
    expect(() => toAGUI(raw)).toThrow(MalformedAexEventError);
    expect(() => toAGUI(raw)).toThrow(new RegExp(path.replace(".", "\\.")));
  });

  it("does not narrow malformed payloads merely because their type matches", () => {
    const malformedText = event("TEXT_MESSAGE_CONTENT", { text: 42 });
    const malformedFinished = event("RUN_FINISHED", {});
    const malformedCustom = event("CUSTOM", { name: "aex.notification" });

    expect(isTextMessage(malformedText)).toBe(false);
    expect(isRunFinished(malformedFinished)).toBe(false);
    expect(isRunTerminal(malformedFinished)).toBe(false);
    expect(isCustom(malformedCustom)).toBe(false);
  });
});

describe("unknown future AexEvent preservation", () => {
  it("keeps the exact raw object and bytes while using AG-UI CUSTOM as a view", () => {
    const raw = {
      ...event("STATE_SNAPSHOT_V2", { revision: 2, opaque: { keep: ["all", "bytes"] } }),
      futureEnvelopeField: { untouched: true }
    };
    const before = JSON.stringify(raw);
    const classified = classifyAexEvent(raw);

    expect(classified.kind).toBe("unknown");
    if (classified.kind !== "unknown") throw new Error("expected unknown");
    expect(classified.event).toBe(raw);
    expect(toAGUI(raw)).toEqual({
      type: "CUSTOM",
      timestamp: Date.parse(raw.time),
      name: "STATE_SNAPSHOT_V2",
      value: raw.data
    });
    expect(JSON.stringify(raw)).toBe(before);
    expect(raw.futureEnvelopeField).toEqual({ untouched: true });
  });
});

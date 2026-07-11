/**
 * WS2/WS9/WS10 class-killer: ONE canonical event surface — AexEvent with a
 * non-optional `sequence`, the loose `TurnEvent` retired (import fails),
 * `toolCallId()` returns `data.id`, and the new guard methods exist.
 */
import { describe, expect, it } from "vitest";
import { asAexEventView, asAexStreamEventView, type AexEvent, type AexEventView } from "../src/index.js";

// @ts-expect-error — the loose `TurnEvent` type is RETIRED; importing it must fail.
import type { TurnEvent } from "../src/runtime-types.js";

function toolStart(id: string): AexEvent {
  return {
    specversion: "1.0",
    id: "ses_1:2048",
    source: "agent",
    type: "TOOL_CALL_START",
    subject: "ses_1",
    threadId: "ses_1",
    runId: "run_1",
    time: "2026-07-05T00:00:00.000Z",
    sequence: 2048,
    data: { id, name: "write_file" }
  };
}

describe("one canonical, guard-bearing event surface (WS2)", () => {
  it("[compile-time] AexEvent.sequence is a required number (not optional)", () => {
    const ev = toolStart("call_1");
    const seq: number = ev.sequence; // would error if sequence were `number | undefined`
    expect(seq).toBe(2048);
  });

  it("toolCallId() returns data.id on tool-call views", () => {
    const view = asAexEventView(toolStart("call_1"));
    expect(view.isToolCallStart()).toBe(true);
    if (view.isToolCallStart()) {
      expect(view.toolCallId()).toBe("call_1");
      expect(view.data.id).toBe("call_1");
    }
  });

  it("exposes guard methods and an honest non-replayable delta discriminator", () => {
    const view = asAexStreamEventView({
      specversion: "1.0",
      id: "ses_1:run_1:live:5",
      source: "agent",
      type: "TEXT_MESSAGE_CONTENT",
      subject: "ses_1",
      threadId: "ses_1",
      runId: "run_1",
      time: "2026-07-05T00:00:00.000Z",
      replayable: false,
      liveSequence: 5,
      data: { text: "hi", delta: true }
    });
    expect(view.isTextMessage()).toBe(true);
    if (view.isTextMessage()) {
      expect(view.data.delta).toBe(true);
    }
    expect("sequence" in view).toBe(false);
    expect(view.isAwaitingApproval()).toBe(false);
    expect(view.isResultDecoded()).toBe(false);
    expect(view.isResultRefused()).toBe(false);
  });
});

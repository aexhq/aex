import { describe, expect, it } from "vitest";
import { expectEventStream } from "../src/event-stream.js";

describe("expectEventStream", () => {
  it("returns started+terminal indices on a well-formed stream", () => {
    const events = [
      { type: "RUN_STARTED" },
      { type: "CUSTOM" },
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "RUN_FINISHED" }
    ];
    const { startedIdx, terminalIdx } = expectEventStream(events);
    expect(startedIdx).toBe(0);
    expect(terminalIdx).toBe(3);
  });

  it("tolerates notifications after the terminal (Anthropic batch trailers)", () => {
    const events = [
      { type: "RUN_STARTED" },
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "RUN_FINISHED" },
      { type: "CUSTOM" },
      { type: "CUSTOM" }
    ];
    expect(() => expectEventStream(events)).not.toThrow();
  });

  it("throws when runtime_started is missing", () => {
    const events = [{ type: "TEXT_MESSAGE_CONTENT" }, { type: "RUN_FINISHED" }];
    expect(() => expectEventStream(events)).toThrow(/RUN_STARTED missing/);
  });

  it("throws when runtime_terminal is missing", () => {
    const events = [{ type: "RUN_STARTED" }, { type: "TEXT_MESSAGE_CONTENT" }];
    expect(() => expectEventStream(events)).toThrow(/terminal/);
  });

  it("throws when a signal event precedes RUN_STARTED", () => {
    const events = [
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "RUN_STARTED" },
      { type: "RUN_FINISHED" }
    ];
    expect(() => expectEventStream(events)).toThrow(
      /signal event TEXT_MESSAGE_CONTENT at idx 0 precedes RUN_STARTED/
    );
  });

  it("throws when a signal event follows runtime_terminal", () => {
    const events = [
      { type: "RUN_STARTED" },
      { type: "RUN_FINISHED" },
      { type: "TOOL_CALL_START" }
    ];
    expect(() => expectEventStream(events)).toThrow(
      /signal event TOOL_CALL_START at idx 2 comes after the terminal/
    );
  });

  it("respects allowAfterTerminal carve-out", () => {
    const events = [
      { type: "RUN_STARTED" },
      { type: "RUN_FINISHED" },
      { type: "stream_error" }
    ];
    // stream_error AFTER terminal is normally a violation (it's a signal kind).
    expect(() => expectEventStream(events)).toThrow();
    // But when explicitly allowed, the matcher accepts it.
    expect(() =>
      expectEventStream(events, { allowAfterTerminal: ["stream_error", "CUSTOM"] })
    ).not.toThrow();
  });
});

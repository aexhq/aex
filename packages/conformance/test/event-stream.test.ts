import { describe, expect, it } from "vitest";
import { expectEventStream } from "../src/event-stream.js";

describe("expectEventStream", () => {
  it("returns start and terminal indices on a well-formed run", () => {
    const events = [
      { type: "RUN_STARTED" },
      { type: "CUSTOM" },
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "RUN_FINISHED" }
    ];
    expect(expectEventStream(events)).toEqual({ startedIdx: 0, terminalIdx: 3 });
  });

  it("rejects any event after the committed terminal", () => {
    const events = [
      { type: "RUN_STARTED" },
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "RUN_FINISHED" },
      { type: "CUSTOM" }
    ];
    expect(() => expectEventStream(events)).toThrow(/follows the committed terminal/);
  });

  it("requires RUN_STARTED", () => {
    expect(() => expectEventStream([{ type: "TEXT_MESSAGE_CONTENT" }, { type: "RUN_FINISHED" }]))
      .toThrow(/RUN_STARTED missing/);
  });

  it("requires a RUN terminal", () => {
    expect(() => expectEventStream([{ type: "RUN_STARTED" }, { type: "TEXT_MESSAGE_CONTENT" }]))
      .toThrow(/terminal/);
  });

  it("rejects a signal before RUN_STARTED", () => {
    const events = [
      { type: "TEXT_MESSAGE_CONTENT" },
      { type: "RUN_STARTED" },
      { type: "RUN_ERROR" }
    ];
    expect(() => expectEventStream(events)).toThrow(/precedes RUN_STARTED/);
  });
});

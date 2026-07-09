import { describe, expect, it } from "vitest";
import { expectTerminalEvent } from "../src/terminal.js";

describe("expectTerminalEvent", () => {
  it("returns the matched terminal when reason matches", () => {
    const events = [
      { type: "TURN_STARTED", data: {} },
      { type: "TEXT_MESSAGE_CONTENT", data: { text: "hi" } },
      { type: "TURN_FINISHED", data: { reason: "complete", stopReason: "end_turn" } }
    ];
    const terminal = expectTerminalEvent(events, { reason: "complete" });
    expect(terminal.data["reason"]).toBe("complete");
  });

  it("throws when no terminal is present", () => {
    const events = [{ type: "TURN_STARTED", data: {} }];
    expect(() => expectTerminalEvent(events, { reason: "complete" })).toThrow(
      /no terminal/
    );
  });

  it("throws when more than one sessiontime_terminal is present (deduplication failure)", () => {
    const events = [
      { type: "TURN_FINISHED", data: { reason: "complete" } },
      { type: "TURN_FINISHED", data: { reason: "complete" } }
    ];
    expect(() => expectTerminalEvent(events, { reason: "complete" })).toThrow(
      /2 terminal events — exactly one expected/
    );
  });

  it("throws when reason does NOT match", () => {
    const events = [{ type: "TURN_FINISHED", data: { reason: "error" } }];
    expect(() => expectTerminalEvent(events, { reason: "complete" })).toThrow(
      /reason="error" but expected "complete"/
    );
  });

  it("checks failureClass when reason === 'error' and failureClass is given", () => {
    const events = [
      { type: "TURN_FINISHED", data: { reason: "error", failureClass: "session_no_idle" } }
    ];
    expect(() =>
      expectTerminalEvent(events, { reason: "error", failureClass: "session_no_idle" })
    ).not.toThrow();
    expect(() =>
      expectTerminalEvent(events, { reason: "error", failureClass: "internal_error" })
    ).toThrow(/failureClass="session_no_idle" but expected "internal_error"/);
  });

  it("interpolates context into failure messages", () => {
    expect(() =>
      expectTerminalEvent([{ type: "TURN_FINISHED", data: { reason: "error" } }], {
        reason: "complete",
        context: "anthropic-managed:happy-path"
      })
    ).toThrow(/\[anthropic-managed:happy-path\]/);
  });
});

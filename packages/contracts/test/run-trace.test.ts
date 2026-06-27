import { describe, expect, it } from "vitest";
import {
  decodeAssistantText,
  decodeToolCalls,
  summarizeRunTrace,
  summarizeRunUsage,
  textOf
} from "../src/run-trace.js";

// The exact loose `RunEvent` wire shapes `listEvents` returns (see
// envelopeToRunEvent + the projection): tool name at data.name, args at
// data.arguments, results carry no name and pair by data.id.
const TOOL_ID_A = "3:call_00_XbouiyygAt88xlF826mf7967";
const TOOL_ID_B = "3:call_01_Wm4Tz8Qa55ycN217pq9d3344";

function start(id: string, name: string, args: Record<string, unknown>, seq: number, recordedAt: string) {
  return {
    id: `run:${seq}`,
    type: "TOOL_CALL_START",
    seq,
    recordedAt,
    data: { id, name, arguments: args, status: "success", messageId: "msg_1" }
  };
}

function result(id: string, content: unknown, isError: boolean, seq: number, recordedAt: string) {
  return {
    id: `run:${seq}`,
    type: "TOOL_CALL_RESULT",
    seq,
    recordedAt,
    data: { id, status: isError ? "error" : "success", content, isError }
  };
}

function usage(value: Record<string, number>, seq: number) {
  return { id: `run:${seq}`, type: "CUSTOM", seq, data: { name: "aex.usage", value } };
}

function text(t: string, seq: number, recordedAt: string) {
  return {
    id: `run:${seq}`,
    type: "TEXT_MESSAGE_CONTENT",
    seq,
    recordedAt,
    data: { role: "assistant", text: t, messageId: "msg_1" }
  };
}

describe("decodeToolCalls", () => {
  it("correlates each TOOL_CALL_RESULT to its START by data.id, with timing", () => {
    const events = [
      start(TOOL_ID_A, "web_search", { query: "broll" }, 1, "2026-01-01T00:00:00.000Z"),
      start(TOOL_ID_B, "bash", { command: "ls" }, 2, "2026-01-01T00:00:00.500Z"),
      result(TOOL_ID_A, [{ type: "text", text: "ok" }], false, 3, "2026-01-01T00:00:01.000Z"),
      result(TOOL_ID_B, [{ type: "text", text: "err" }], true, 4, "2026-01-01T00:00:02.500Z")
    ];
    const calls = decodeToolCalls(events);
    expect(calls).toHaveLength(2);

    expect(calls[0]!.id).toBe(TOOL_ID_A);
    expect(calls[0]!.name).toBe("web_search");
    expect(calls[0]!.args).toEqual({ query: "broll" });
    expect(calls[0]!.messageId).toBe("msg_1");
    expect(calls[0]!.result?.isError).toBe(false);
    expect(calls[0]!.result?.content).toEqual([{ type: "text", text: "ok" }]);
    expect(calls[0]!.durationMs).toBe(1000);

    expect(calls[1]!.id).toBe(TOOL_ID_B);
    expect(calls[1]!.name).toBe("bash");
    expect(calls[1]!.result?.isError).toBe(true);
    expect(calls[1]!.durationMs).toBe(2000);
  });

  it("preserves start order and keeps parallel ids distinct", () => {
    const events = [
      start(TOOL_ID_B, "b", {}, 1, "2026-01-01T00:00:00.000Z"),
      start(TOOL_ID_A, "a", {}, 2, "2026-01-01T00:00:00.000Z"),
      result(TOOL_ID_A, null, false, 3, "2026-01-01T00:00:00.000Z")
    ];
    const calls = decodeToolCalls(events);
    expect(calls.map((c) => c.id)).toEqual([TOOL_ID_B, TOOL_ID_A]);
  });

  it("surfaces an in-flight call (start, no result) with result undefined", () => {
    const calls = decodeToolCalls([start(TOOL_ID_A, "web_search", {}, 1, "2026-01-01T00:00:00.000Z")]);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.result).toBeUndefined();
    expect(calls[0]!.durationMs).toBeUndefined();
  });

  it("surfaces an orphan result (no matching start) rather than dropping it", () => {
    const calls = decodeToolCalls([result(TOOL_ID_A, "x", false, 1, "2026-01-01T00:00:00.000Z")]);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.id).toBe(TOOL_ID_A);
    expect(calls[0]!.name).toBe("");
    expect(calls[0]!.result?.content).toBe("x");
  });

  it("ignores unknown event types and is pure (input not mutated)", () => {
    const events = [
      { id: "run:0", type: "RUN_STARTED", seq: 0, data: {} },
      start(TOOL_ID_A, "a", { k: 1 }, 1, "2026-01-01T00:00:00.000Z")
    ];
    const frozen = JSON.parse(JSON.stringify(events));
    const calls = decodeToolCalls(events);
    expect(calls).toHaveLength(1);
    expect(events).toEqual(frozen);
  });
});

describe("summarizeRunUsage", () => {
  it("sums the per-turn aex.usage events into a UsageSummary", () => {
    const events = [
      usage({ input_tokens: 100, output_tokens: 40, cache_read_input_tokens: 7 }, 1),
      usage({ input_tokens: 200, output_tokens: 60, cache_creation_input_tokens: 3 }, 2)
    ];
    expect(summarizeRunUsage(events)).toEqual({
      inputTokens: 300,
      outputTokens: 100,
      cacheReadInputTokens: 7,
      cacheCreationInputTokens: 3,
      totalTokens: 400
    });
  });

  it("returns an empty summary when no usage events are present", () => {
    expect(summarizeRunUsage([text("hi", 1, "2026-01-01T00:00:00.000Z")])).toEqual({});
  });

  it("ignores non-aex.usage CUSTOM events", () => {
    const events = [{ id: "run:1", type: "CUSTOM", seq: 1, data: { name: "aex.diag", value: { input_tokens: 999 } } }];
    expect(summarizeRunUsage(events)).toEqual({});
  });
});

describe("decodeAssistantText / summarizeRunTrace", () => {
  it("decodes assistant text blocks in order", () => {
    const entries = decodeAssistantText([
      text("first", 1, "2026-01-01T00:00:00.000Z"),
      text("second", 2, "2026-01-01T00:00:01.000Z")
    ]);
    expect(entries.map((e) => e.text)).toEqual(["first", "second"]);
    expect(entries[0]!.messageId).toBe("msg_1");
  });

  it("summarizeRunTrace decodes calls + usage + text in one pass", () => {
    const events = [
      start(TOOL_ID_A, "web_search", { q: "x" }, 1, "2026-01-01T00:00:00.000Z"),
      usage({ input_tokens: 10, output_tokens: 5 }, 2),
      result(TOOL_ID_A, "ok", false, 3, "2026-01-01T00:00:00.500Z"),
      text("done", 4, "2026-01-01T00:00:01.000Z")
    ];
    const trace = summarizeRunTrace(events);
    expect(trace.toolCalls).toHaveLength(1);
    expect(trace.toolCalls[0]!.durationMs).toBe(500);
    expect(trace.usage.totalTokens).toBe(15);
    expect(trace.text.map((t) => t.text)).toEqual(["done"]);
  });
});

describe("textOf", () => {
  it("concatenates the assistant text blocks in stream order", () => {
    expect(
      textOf([
        text("hello ", 1, "2026-01-01T00:00:00.000Z"),
        result(TOOL_ID_A, "ok", false, 2, "2026-01-01T00:00:00.500Z"),
        text("world", 3, "2026-01-01T00:00:01.000Z")
      ])
    ).toBe("hello world");
  });

  it("is empty when no assistant text events are present", () => {
    expect(textOf([usage({ input_tokens: 1 }, 1)])).toBe("");
  });
});

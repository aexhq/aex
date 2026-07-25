import { describe, expect, it } from "bun:test";
import { newId } from "@aexhq/contracts";
import {
  SessionConfigValidationError,
  SessionStateError,
  asAexEventView,
  type AexEvent,
  type Session,
  type SessionRun
} from "@aexhq/contracts";
import {
  assertRunCheckpoint,
  buildTurnResult,
  projectAssistantMessages,
  terminalCheckpointId,
  terminalSessionStatusFromEvents,
  turnTraceFromEvents
} from "../../src/event-projection.js";
import {
  assertSupportedSessionFields,
  normaliseSessionInput
} from "../../src/session-validate.js";
import {
  fileCaptureForWire,
  mergeMcpServers,
  sessionEnvironmentForWire,
  sessionRetentionForWire
} from "../../src/submission-wire.js";
import { McpServer } from "../../src/mcp-server.js";

/** Minted from the id owner; the SDK guard now accepts only that shape. */
const WORKSPACE_MCP_ID = newId("mcp");

function event(
  sequence: number,
  type: AexEvent["type"],
  data: AexEvent["data"]
): AexEvent {
  const time = new Date(sequence * 1_000).toISOString();
  return {
    specversion: "1.0",
    id: `evt_${sequence}`,
    source: "runtime",
    type,
    subject: "sess_1",
    threadId: "sess_1",
    runId: "run_1",
    time,
    sequence,
    data
  } as AexEvent;
}

describe("SDK client private leaves", () => {
  it("keeps event ordering, merging, trace, terminal billing, and checkpoint projection deterministic", () => {
    const events = [
      event(1, "TEXT_MESSAGE_CONTENT", { text: "hel", messageId: "msg_1", turnSeq: 1 }),
      event(2, "TOOL_CALL_RESULT", { id: "tool_1", isError: false, content: "ok" }),
      event(3, "TOOL_CALL_START", { id: "tool_1", name: "search", arguments: { q: "aex" } }),
      event(4, "TEXT_MESSAGE_CONTENT", { text: "lo", messageId: "msg_1", turnSeq: 1 }),
      event(5, "CUSTOM", { name: "aex.usage", value: { input_tokens: 3, output_tokens: 2 } }),
      event(6, "FUTURE_PROJECTION_ROW", { text: "must not be projected", nested: { retained: true } }),
      event(7, "CUSTOM", { name: "aex.result.decoded", value: { value: { answer: 42 } } }),
      event(8, "RUN_FINISHED", {
        outcome: "succeeded",
        costUsd: 0.25,
        providerUsage: [],
        checkpoint: { checkpointId: "cp_1" }
      })
    ];
    const views = events.map(asAexEventView);
    const messages = projectAssistantMessages(events);

    expect(messages).toEqual([expect.objectContaining({ id: "msg_1", text: "hello", turnSeq: 1, sequence: 4 })]);
    expect(turnTraceFromEvents(events)).toMatchObject({
      text: [{ text: "hel", messageId: "msg_1", seq: 1 }, { text: "lo", messageId: "msg_1", seq: 4 }],
      toolCalls: [{ id: "tool_1", name: "search", args: { q: "aex" } }],
      usage: { inputTokens: 3, outputTokens: 2, totalTokens: 5 }
    });
    expect(terminalSessionStatusFromEvents(events, "run_1")).toBe("succeeded");
    expect(terminalCheckpointId(events, "run_1")).toBe("cp_1");
    expect(() => assertRunCheckpoint(events, "run_1", {
      checkpointId: "cp_other",
      runId: "run_1"
    } as never)).toThrow(SessionStateError);

    const session = {
      id: "sess_1",
      status: "idle",
      acceptsMessages: true,
      lastRun: { runId: "run_1" }
    } as Session;
    const run = { sessionId: "sess_1", runId: "run_1" } as SessionRun;
    const result = buildTurnResult("sess_1", session, run, views, [], undefined, messages, "succeeded");
    expect(result).toMatchObject({
      sessionId: "sess_1",
      status: "succeeded",
      ok: true,
      costUsd: 0.25,
      text: "hello",
      messages,
      outcome: { kind: "decoded", value: { answer: 42 } }
    });
    expect(result.events).toBe(views);
    expect(result.events[5]).toBe(views[5]);
  });

  it("preserves exact validation identity, first-error selection, and input normalization", () => {
    const input = ["hello", "world"] as const;
    const normalized = normaliseSessionInput(input, "session.messages.send", "input");
    expect(normalized).toEqual(input);
    expect(normalized).not.toBe(input);

    expect(() => assertSupportedSessionFields({
      model: "anthropic/claude-haiku-4-5",
      runtime: { kind: "container", tier: "large" }
    } as never, "aex.sessions.create", false)).toThrowError(expect.objectContaining({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: "aex.sessions.create: runtime.tier is not a supported option; use runtime.kind or runtime.size",
      details: { field: "runtime.tier" }
    }));
    expect(() => normaliseSessionInput("   ", "Aex.start", "message")).toThrow(SessionConfigValidationError);
  });

  it("preserves wire omission, defaults, environment mapping, and MCP ordering/conflicts", () => {
    expect(fileCaptureForWire({ allowedDirs: ["", "/workspace"], deniedDirs: [] })).toEqual({
      allowedDirs: ["/workspace"]
    });
    expect(fileCaptureForWire({ allowedDirs: [], deniedDirs: [] })).toBeUndefined();
    expect(sessionRetentionForWire({ model: "anthropic/claude-haiku-4-5" })).toEqual({ idleTtl: "3m" });
    expect(sessionEnvironmentForWire({
      variables: { MODE: "test" },
      networking: { mode: "limited", allowedHosts: ["example.test"] }
    })).toEqual({
      envVars: { MODE: "test" },
      networking: { mode: "limited", allowedHosts: ["example.test"] }
    });

    const first = McpServer.remote({
      name: "docs",
      url: "https://docs.example.test/mcp",
      headers: { Authorization: "Bearer placeholder" }
    });
    const second = McpServer.fromId(WORKSPACE_MCP_ID);
    expect(mergeMcpServers([first, second], []).submissionMcpServers).toEqual([
      { name: "docs", url: "https://docs.example.test/mcp" },
      { kind: "workspace", id: WORKSPACE_MCP_ID }
    ]);
    expect(() => mergeMcpServers([
      first,
      McpServer.remote({ name: "docs", url: "https://other.example.test/mcp", headers: { Authorization: "x" } })
    ], [])).toThrowError(expect.objectContaining({ details: { field: "mcpServers[1].url" } }));
  });
});

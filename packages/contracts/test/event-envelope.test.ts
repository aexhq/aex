import { describe, expect, it } from "vitest";
import {
  AEX_EVENT_SPECVERSION,
  MAX_SQLITE_ROW_BYTES,
  channelOf,
  customName,
  exceedsRowBudget,
  isCustom,
  isEventChannel,
  isFromSource,
  isLog,
  isRunError,
  isRunFinished,
  isRunSettled,
  isRunStarted,
  isRunTerminal,
  isSessionParked,
  isTextMessage,
  isToolCallResult,
  isToolCallStart,
  logToInbound,
  runnerEventToAexEvent,
  serializedEventBytes,
  toAGUI,
  type AexEvent,
  type RunnerEvent
} from "../src/index.js";

const ctx = { runId: "run_env", baseMs: 1_000_000 };

const map = (evt: RunnerEvent) => runnerEventToAexEvent(evt, ctx);
const ev = (seq: number, kind: RunnerEvent["kind"], data: RunnerEvent["data"] = {}): RunnerEvent => ({
  seq,
  tMs: seq,
  kind,
  data
});

describe("runnerEventToAexEvent — CloudEvents framing", () => {
  it("stamps stable identity, subject, sequence, and ISO time", () => {
    const out = map(ev(7, "assistant_text", { text: "hi" }));
    expect(out.specversion).toBe(AEX_EVENT_SPECVERSION);
    expect(out.id).toBe("run_env:7");
    expect(out.subject).toBe("run_env");
    expect(out.sequence).toBe(7);
    expect(out.time).toBe(new Date(ctx.baseMs + 7).toISOString());
    expect(Object.isFrozen(out.data)).toBe(true);
  });
});

describe("runnerEventToAexEvent — per-kind projection (type + source)", () => {
  it("runtime_started → RUN_STARTED / runtime", () => {
    const out = map(ev(0, "runtime_started", { source: "managed-runtime" }));
    expect([out.type, out.source]).toEqual(["RUN_STARTED", "runtime"]);
    expect(isRunStarted(out)).toBe(true);
  });

  it("assistant_text → TEXT_MESSAGE_CONTENT / agent, with a clipped message", () => {
    const out = map(ev(1, "assistant_text", { role: "assistant", text: "Hello." }));
    expect([out.type, out.source]).toEqual(["TEXT_MESSAGE_CONTENT", "agent"]);
    expect(out.message).toBe("Hello.");
    expect(isTextMessage(out)).toBe(true);
  });

  it("tool_request → TOOL_CALL_START; source is agent for builtin, mcp when an extension is present", () => {
    const builtin = map(ev(2, "tool_request", { id: "t1", name: "read_file", arguments: {} }));
    expect([builtin.type, builtin.source]).toEqual(["TOOL_CALL_START", "agent"]);
    expect(builtin.message).toBe("tool read_file");
    expect(isToolCallStart(builtin)).toBe(true);

    const mcp = map(ev(3, "tool_request", { id: "t2", name: "echo", arguments: {}, extension: "srv_mcp" }));
    expect(mcp.source).toBe("mcp");
    expect(isFromSource(mcp, "mcp")).toBe(true);
  });

  it("tool_response → TOOL_CALL_RESULT, mcp when extension present", () => {
    const out = map(ev(4, "tool_response", { id: "t2", status: "success", content: [], extension: "srv_mcp" }));
    expect([out.type, out.source]).toEqual(["TOOL_CALL_RESULT", "mcp"]);
    expect(isToolCallResult(out)).toBe(true);
  });

  it("skill_loaded / file_uploaded → CUSTOM / aex under an aex.* name", () => {
    const skill = map(ev(5, "skill_loaded", { name: "pdf" }));
    expect([skill.type, skill.source]).toEqual(["CUSTOM", "aex"]);
    expect(customName(skill)).toBe("aex.skill_loaded");
    // The original payload is preserved verbatim under `value` (AG-UI custom carrier).
    expect(skill.data.value).toEqual({ name: "pdf" });

    const file = map(ev(6, "file_uploaded", { path: "out.txt" }));
    expect(customName(file)).toBe("aex.file_uploaded");
  });

  it("notification → CUSTOM / runtime; stream_error → CUSTOM / runtime (non-fatal, not RUN_ERROR)", () => {
    const note = map(ev(7, "notification", { source: "anthropic", reason: "model_request_end" }));
    expect([note.type, note.source]).toEqual(["CUSTOM", "runtime"]);
    expect(customName(note)).toBe("aex.notification");

    const err = map(ev(8, "stream_error", { source: "runtime", message: "echo timed out" }));
    expect([err.type, err.source]).toEqual(["CUSTOM", "runtime"]);
    expect(customName(err)).toBe("aex.stream_error");
    // A non-fatal stream error must NOT masquerade as the terminal RUN_ERROR.
    expect(isRunError(err)).toBe(false);
    expect(isRunTerminal(err)).toBe(false);
    // ...but a session-park CUSTOM is a managed-runtime terminal (F19).
    expect(isSessionParked(err)).toBe(false);
  });

  it("isSessionParked / isRunSettled recognize the managed session-park terminals (F19)", () => {
    const parked = (name: string): AexEvent => ({
      specversion: AEX_EVENT_SPECVERSION,
      id: "run_env:0",
      source: "runtime",
      type: "CUSTOM",
      subject: "run_env",
      time: new Date().toISOString(),
      sequence: 0,
      data: { name, value: {} }
    });
    // WS1 clean-cut: the settled park vocabulary is the resumable parks plus the
    // terminal outcomes; bare `aex.session.error` is retired (→ `failed`).
    for (const name of [
      "aex.session.idle",
      "aex.session.suspended",
      "aex.session.succeeded",
      "aex.session.failed",
      "aex.session.timed_out",
      "aex.session.cancelled"
    ]) {
      expect(isSessionParked(parked(name))).toBe(true);
      // settleConsistent may terminate at the park if no later aex.run.settled
      // barrier is delivered to this stream.
      expect(isRunSettled(parked(name))).toBe(true);
    }
    // The retired bare `error` park is no longer recognized (→ `failed`).
    expect(isSessionParked(parked("aex.session.error"))).toBe(false);
    // A HELD approval gate ends the turn but the RUN is NOT settled.
    expect(isRunSettled(parked("aex.session.awaiting_approval"))).toBe(false);
    // A non-park CUSTOM (e.g. skill_loaded) is neither.
    expect(isSessionParked(parked("aex.skill_loaded"))).toBe(false);
    expect(isRunSettled(parked("aex.skill_loaded"))).toBe(false);
    // A plain typed event is not a session park.
    expect(isSessionParked(map(ev(11, "runtime_terminal", { reason: "complete" })))).toBe(false);
  });

  it("runtime_terminal → RUN_FINISHED normally, RUN_ERROR when the reason is error", () => {
    const done = map(ev(9, "runtime_terminal", { reason: "complete", totalTokens: 10 }));
    expect(done.type).toBe("RUN_FINISHED");
    expect(isRunTerminal(done)).toBe(true);

    const cancelled = map(ev(10, "runtime_terminal", { reason: "cancelled" }));
    expect(cancelled.type).toBe("RUN_FINISHED"); // the orchestrator owns the cancelled *status*

    const failed = map(ev(11, "runtime_terminal", { reason: "error", failureClass: "session_failed", failureMessage: "boom" }));
    expect(failed.type).toBe("RUN_ERROR");
    expect(isRunError(failed)).toBe(true);
  });
});

describe("honest guards close the Phase-0 gap", () => {
  it("a guard fires on every emitted kind (the dead raw-provider guards never did)", () => {
    const kinds: RunnerEvent["kind"][] = [
      "runtime_started",
      "assistant_text",
      "tool_request",
      "tool_response",
      "skill_loaded",
      "file_uploaded",
      "notification",
      "stream_error",
      "runtime_terminal"
    ];
    const guards = [isRunStarted, isRunFinished, isRunError, isTextMessage, isToolCallStart, isToolCallResult, isCustom];
    for (const kind of kinds) {
      const out = map(ev(0, kind, kind === "runtime_terminal" ? { reason: "complete" } : {}));
      expect(guards.some((g) => g(out))).toBe(true);
    }
  });
});

describe("toAGUI — strict AG-UI projection", () => {
  it("TEXT_MESSAGE_CONTENT carries messageId + delta", () => {
    const out = toAGUI(map(ev(1, "assistant_text", { text: "hi", messageId: "m1" })));
    expect(out).toEqual({ type: "TEXT_MESSAGE_CONTENT", timestamp: ctx.baseMs + 1, messageId: "m1", delta: "hi" });
  });

  it("TOOL_CALL_START carries toolCallId + toolCallName", () => {
    const out = toAGUI(map(ev(2, "tool_request", { id: "tc1", name: "read_file", arguments: {} })));
    expect(out).toEqual({ type: "TOOL_CALL_START", timestamp: ctx.baseMs + 2, toolCallId: "tc1", toolCallName: "read_file" });
  });

  it("RUN_ERROR carries message + code from the failure fields", () => {
    const out = toAGUI(map(ev(3, "runtime_terminal", { reason: "error", failureClass: "session_failed", failureMessage: "boom" })));
    expect(out).toEqual({ type: "RUN_ERROR", timestamp: ctx.baseMs + 3, message: "boom", code: "session_failed" });
  });

  it("CUSTOM round-trips name + value", () => {
    const out = toAGUI(map(ev(4, "skill_loaded", { name: "pdf", id: "sk1" })));
    expect(out).toEqual({ type: "CUSTOM", timestamp: ctx.baseMs + 4, name: "aex.skill_loaded", value: { name: "pdf", id: "sk1" } });
  });
});

describe("cross-runtime parity (same logical event ⇒ same envelope shape)", () => {
  // Representative assistant-text RunnerEvents as each adapter emits them:
  // One adapter tags the message id `messageId`; another adapter tags it
  // `eventId`. The envelope type/source are identical, and toAGUI normalizes
  // both to a populated `messageId`.
  it("different assistant_text adapters map to the same type/source and a normalized AG-UI message", () => {
    const runtime = map(ev(0, "assistant_text", { role: "assistant", text: "Done", messageId: "msg_runtime" }));
    const anthropic = map(ev(0, "assistant_text", { role: "assistant", text: "Done", eventId: "evt_anthropic" }));

    expect([runtime.type, runtime.source]).toEqual([anthropic.type, anthropic.source]);

    // toMatchObject avoids union-narrowing control flow while still asserting
    // the type-specific AG-UI fields.
    expect(toAGUI(runtime)).toMatchObject({ type: "TEXT_MESSAGE_CONTENT", delta: "Done", messageId: "msg_runtime" });
    expect(toAGUI(anthropic)).toMatchObject({ type: "TEXT_MESSAGE_CONTENT", delta: "Done", messageId: "evt_anthropic" });
  });

  it("identical RunnerEvents from either runtime produce byte-identical envelopes", () => {
    const a = map(ev(2, "tool_request", { id: "t", name: "echo", arguments: { x: 1 } }));
    const b = map(ev(2, "tool_request", { id: "t", name: "echo", arguments: { x: 1 } }));
    expect(JSON.stringify(a)).toBe(JSON.stringify(b));
  });
});

describe("oversized-payload rule (2 MB SQLite row cap)", () => {
  it("a small event is well under the budget", () => {
    const out = map(ev(0, "assistant_text", { text: "hi" }));
    expect(serializedEventBytes(out)).toBeLessThan(MAX_SQLITE_ROW_BYTES);
    expect(exceedsRowBudget(out)).toBe(false);
  });

  it("flags an event whose serialized form exceeds the budget", () => {
    const big = map(ev(0, "assistant_text", { text: "x".repeat(MAX_SQLITE_ROW_BYTES + 10) }));
    expect(serializedEventBytes(big)).toBeGreaterThan(MAX_SQLITE_ROW_BYTES);
    expect(exceedsRowBudget(big)).toBe(true);
  });
});

describe("log channel — logToInbound + guards (unified stream)", () => {
  const line = {
    level: "warn" as const,
    message: "retrying poll",
    fields: { attempt: 2 },
    emittedAt: 1_700_000_000_000,
    sourceSeq: 5
  };

  it("projects a log line to an inbound LOG record on the log channel", () => {
    const rec = logToInbound("api", line);
    expect(rec.type).toBe("LOG");
    expect(rec.channel).toBe("log");
    expect(rec.source).toBe("api");
    expect(rec.sourceSeq).toBe(5);
    expect(rec.emittedAt).toBe(line.emittedAt);
    expect(rec.time).toBe(new Date(line.emittedAt).toISOString());
    expect(rec.message).toBe("retrying poll");
    expect(rec.data).toMatchObject({ level: "warn", message: "retrying poll", fields: { attempt: 2 } });
  });

  it("surfaces level FIRST-CLASS (not only inside data)", () => {
    const rec = logToInbound("api", line);
    expect(rec.level).toBe("warn");
    // data.level is kept too so an existing data-reading consumer still works.
    expect((rec.data as { level?: string }).level).toBe("warn");
  });

  it("omits fields when none are supplied", () => {
    const { fields: _drop, ...noFields } = line;
    const rec = logToInbound("workflow", noFields);
    expect(rec.data.fields).toBeUndefined();
    expect(rec.source).toBe("workflow");
  });

  it("channelOf defaults an absent channel to event; isLog/isEventChannel split", () => {
    const typed = map(ev(0, "assistant_text", { text: "hi" }));
    expect(channelOf(typed)).toBe("event");
    expect(isEventChannel(typed)).toBe(true);
    expect(isLog(typed)).toBe(false);
    const log = { ...logToInbound("api", line), specversion: AEX_EVENT_SPECVERSION, id: "r:0", subject: "r", sequence: 0 } as const;
    expect(channelOf(log)).toBe("log");
    expect(isLog(log)).toBe(true);
    expect(isEventChannel(log)).toBe(false);
  });

  it("toAGUI carries a LOG under the reserved CUSTOM as aex.log", () => {
    const log = { ...logToInbound("api", line), specversion: AEX_EVENT_SPECVERSION, id: "r:0", subject: "r", sequence: 0 } as const;
    const agui = toAGUI(log);
    expect(agui.type).toBe("CUSTOM");
    expect(agui).toMatchObject({ name: "aex.log" });
  });

  it("workflow is a recognized source", () => {
    expect(logToInbound("workflow", line).source).toBe("workflow");
  });

  it("host is a recognized source (managed-host logs)", () => {
    expect(logToInbound("host", line).source).toBe("host");
  });
});

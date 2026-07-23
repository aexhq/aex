import { describe, expect, it } from "bun:test";
import {
  AEX_EVENT_SPECVERSION,
  RUNNER_EVENT_VERSION,
  runnerEventToAexEvent,
  toAGUI,
  validateRunnerEventBatch,
  type AexEvent,
  type AexLiveEvent,
  type RunnerEvent
} from "../src/index.js";

const context = { sessionId: "ses_golden", runId: "turn_golden", baseMs: 1_700_000_000_000 };

function project(event: RunnerEvent): AexEvent | null {
  return runnerEventToAexEvent(event, context);
}

describe("canonical AexEvent projection goldens", () => {
  it.each([
    ["runtime_started", {}, "RUN_STARTED", "runtime", "turn started"],
    ["assistant_text", { text: "hello" }, "TEXT_MESSAGE_CONTENT", "agent", "hello"],
    ["tool_request", { id: "call_1", name: "read_file" }, "TOOL_CALL_START", "agent", "tool read_file"],
    ["tool_response", { id: "call_1", content: "ok" }, "TOOL_CALL_RESULT", "agent", undefined],
    ["skill_loaded", { name: "pdf" }, "CUSTOM", "aex", "skill loaded"],
    ["file_uploaded", { path: "out.txt" }, "CUSTOM", "aex", "file uploaded"],
    ["notification", { reason: "waiting" }, "CUSTOM", "runtime", "waiting"],
    ["stream_error", { message: "retry" }, "CUSTOM", "runtime", "retry"]
  ] as const)("projects %s without platform-specific logic", (kind, data, type, source, message) => {
    const event = project({ seq: 7, tMs: 25, kind, data });

    expect(event).toMatchObject({
      specversion: AEX_EVENT_SPECVERSION,
      id: "ses_golden:7",
      subject: "ses_golden",
      threadId: "ses_golden",
      runId: "turn_golden",
      sequence: 7,
      time: "2023-11-14T22:13:20.025Z",
      type,
      source,
      ...(message === undefined ? {} : { message })
    });
  });

  it("does not project the runtime-internal terminal marker", () => {
    expect(project({ seq: 9, tMs: 30, kind: "runtime_terminal", data: { reason: "complete" } })).toBeNull();
  });

  it("validates and carries source ordering metadata only when supplied", () => {
    const result = validateRunnerEventBatch({
      v: RUNNER_EVENT_VERSION,
      sessionId: "ses_golden",
      events: [{
        seq: 3,
        tMs: 4,
        sourceSeq: 55,
        emittedAt: 1_700_000_000_123,
        kind: "assistant_text",
        data: { text: "hi" }
      }]
    });
    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error(result.message);

    expect(result.batch.events[0]).toMatchObject({ sourceSeq: 55, emittedAt: 1_700_000_000_123 });
    expect(project(result.batch.events[0]!)).toMatchObject({
      sourceSeq: 55,
      emittedAt: 1_700_000_000_123
    });
    expect(project({ seq: 3, tMs: 4, kind: "assistant_text", data: { text: "hi" } }))
      .not.toHaveProperty("sourceSeq");
  });

  it.each([
    ["sourceSeq", -1],
    ["sourceSeq", 1.5],
    ["emittedAt", -1],
    ["emittedAt", Number.NaN]
  ] as const)("rejects invalid optional runner metadata %s=%s", (field, value) => {
    const result = validateRunnerEventBatch({
      v: RUNNER_EVENT_VERSION,
      sessionId: "ses_golden",
      events: [{ seq: 1, tMs: 2, kind: "assistant_text", data: {}, [field]: value }]
    });

    expect(result).toMatchObject({ ok: false, code: "invalid_event" });
  });

  it("describes the dedup and live fields already emitted by the hosted stream", () => {
    const live = {
      specversion: AEX_EVENT_SPECVERSION,
      id: "ses_golden:live:1",
      replayable: false,
      liveSequence: 1,
      source: "agent",
      type: "TEXT_MESSAGE_CONTENT",
      subject: "ses_golden",
      threadId: "ses_golden",
      runId: "turn_golden",
      time: "2023-11-14T22:13:20.000Z",
      receivedAt: 1_700_000_000_001,
      ephemeral: true,
      dedup: { source: "runtime:ses_golden", sourceSeq: 1 },
      data: { role: "assistant", text: "h", delta: true }
    } satisfies AexLiveEvent;

    expect(live.dedup).toEqual({ source: "runtime:ses_golden", sourceSeq: 1 });
    expect(live.ephemeral).toBe(true);
  });
});

describe("forward-compatible unknown events", () => {
  it("keeps the raw event byte-for-byte and separately projects it through AG-UI CUSTOM", () => {
    const raw: AexEvent & { readonly futureEnvelopeField: { readonly untouched: boolean } } = {
      specversion: AEX_EVENT_SPECVERSION,
      id: "ses_golden:42",
      source: "future-runtime",
      type: "STATE_SNAPSHOT_V2",
      subject: "ses_golden",
      threadId: "ses_golden",
      runId: "turn_golden",
      time: "2023-11-14T22:13:20.042Z",
      sequence: 42,
      data: { revision: 2, opaque: { keep: ["all", "bytes"] } },
      futureEnvelopeField: { untouched: true }
    };
    const event: AexEvent = raw;
    const before = JSON.stringify(raw);

    expect(toAGUI(event)).toEqual({
      type: "CUSTOM",
      timestamp: 1_700_000_000_042,
      name: "STATE_SNAPSHOT_V2",
      value: raw.data
    });
    expect(JSON.stringify(raw)).toBe(before);
    expect(raw.futureEnvelopeField).toEqual({ untouched: true });
  });
});

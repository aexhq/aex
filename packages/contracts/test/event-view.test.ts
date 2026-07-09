import { describe, expect, it } from "vitest";
import {
  AEX_EVENT_SPECVERSION,
  AEX_SESSION_SETTLED_NAME,
  asAexEventView,
  asAexEventViews,
  runnerEventToAexEvent,
  type AexEvent,
  type RunnerEvent
} from "../src/index.js";

const ctx = { sessionId: "ses_view", baseMs: 2_000_000 };
const map = (evt: RunnerEvent) => runnerEventToAexEvent(evt, ctx);
const ev = (seq: number, kind: RunnerEvent["kind"], data: RunnerEvent["data"] = {}): RunnerEvent => ({
  seq,
  tMs: seq,
  kind,
  data
});

/** A hand-built envelope (for the shapes `runnerEventToAexEvent` never emits). */
const envelope = (over: Partial<AexEvent> & Pick<AexEvent, "type">): AexEvent => ({
  specversion: AEX_EVENT_SPECVERSION,
  id: "ses_view:0",
  source: "runtime",
  subject: "ses_view",
  time: new Date(ctx.baseMs).toISOString(),
  sequence: 0,
  data: {},
  ...over
});

describe("asAexEventView — one type-predicate method per standardized event type", () => {
  it("isTurnStarted / isTurnFinished / isTurnError / isTurnTerminal fire on the lifecycle types", () => {
    const started = asAexEventView(map(ev(0, "runtime_started")));
    expect(started.isTurnStarted()).toBe(true);
    expect(started.isTurnTerminal()).toBe(false);

    const finished = asAexEventView(map(ev(1, "runtime_terminal", { reason: "complete" })));
    expect(finished.isTurnFinished()).toBe(true);
    expect(finished.isTurnError()).toBe(false);
    expect(finished.isTurnTerminal()).toBe(true);

    const errored = asAexEventView(map(ev(2, "runtime_terminal", { reason: "error", failureMessage: "boom" })));
    expect(errored.isTurnError()).toBe(true);
    expect(errored.isTurnFinished()).toBe(false);
    expect(errored.isTurnTerminal()).toBe(true);
  });

  it("isCustom is true only for CUSTOM events", () => {
    expect(asAexEventView(map(ev(3, "skill_loaded", { name: "pdf" }))).isCustom()).toBe(true);
    expect(asAexEventView(map(ev(4, "assistant_text", { text: "hi" }))).isCustom()).toBe(false);
  });

  it("isLog / isEventChannel split the unified stream by channel", () => {
    const typed = asAexEventView(map(ev(5, "assistant_text", { text: "hi" })));
    expect(typed.isLog()).toBe(false);
    expect(typed.isEventChannel()).toBe(true);

    const log = asAexEventView(envelope({ type: "LOG", channel: "log", level: "warn", data: { message: "retry" } }));
    expect(log.isLog()).toBe(true);
    expect(log.isEventChannel()).toBe(false);
  });

  it("isFromSource matches the coarse origin", () => {
    const mcp = asAexEventView(map(ev(6, "tool_request", { id: "t", name: "echo", arguments: {}, extension: "srv" })));
    expect(mcp.isFromSource("mcp")).toBe(true);
    expect(mcp.isFromSource("agent")).toBe(false);
  });

  it("isSessionSettled is true for the settle barrier and for a managed session-park terminal", () => {
    const barrier = asAexEventView(envelope({ type: "CUSTOM", data: { name: AEX_SESSION_SETTLED_NAME, value: {} } }));
    expect(barrier.isSessionSettled()).toBe(true);

    // WS1: resumable parks + terminal outcomes are all settled parks; bare `error` retired.
    for (const name of [
      "aex.session.idle",
      "aex.session.suspended",
      "aex.session.succeeded",
      "aex.session.failed",
      "aex.session.timed_out",
      "aex.session.cancelled"
    ]) {
      const parked = asAexEventView(envelope({ type: "CUSTOM", data: { name, value: {} } }));
      expect(parked.isSessionSettled()).toBe(true);
    }
    expect(asAexEventView(map(ev(7, "assistant_text", { text: "hi" }))).isSessionSettled()).toBe(false);
  });
});

describe("asAexEventView — data narrowing (type predicates)", () => {
  it("isTextMessage narrows data.text to string", () => {
    const view = asAexEventView(map(ev(0, "assistant_text", { text: "hello", messageId: "m1" })));
    if (view.isTextMessage()) {
      // `data.text` is typed `string` here — no cast, no `typeof` guard.
      const upper: string = view.data.text.toUpperCase();
      expect(upper).toBe("HELLO");
      expect(view.data.messageId).toBe("m1");
    } else {
      throw new Error("expected a text message");
    }
  });

  it("isToolCallStart narrows data to { id, name }", () => {
    const view = asAexEventView(map(ev(1, "tool_request", { id: "tc1", name: "read_file", arguments: { path: "/x" } })));
    if (view.isToolCallStart()) {
      const label: string = `${view.data.name}#${view.data.id}`;
      expect(label).toBe("read_file#tc1");
      expect(view.data.arguments).toEqual({ path: "/x" });
    } else {
      throw new Error("expected a tool-call start");
    }
  });

  it("isToolCallResult narrows data to { id, content }", () => {
    const view = asAexEventView(map(ev(2, "tool_response", { id: "tc1", content: [{ type: "text", text: "ok" }], isError: false })));
    if (view.isToolCallResult()) {
      const id: string = view.data.id;
      expect(id).toBe("tc1");
      expect(view.data.isError).toBe(false);
    } else {
      throw new Error("expected a tool-call result");
    }
  });
});

describe("asAexEventView — the wrapper is a transparent AexEvent", () => {
  it("reads every envelope field unchanged and serializes to identical bytes", () => {
    const raw = map(ev(9, "assistant_text", { text: "hi", messageId: "m1" }));
    const view = asAexEventView(raw);
    expect(view.type).toBe(raw.type);
    expect(view.source).toBe(raw.source);
    expect(view.sequence).toBe(raw.sequence);
    expect(view.id).toBe(raw.id);
    expect(view.data).toEqual(raw.data);
    // Methods live on the prototype, so they never leak into the serialized event.
    expect(JSON.stringify(view)).toBe(JSON.stringify(raw));
    expect(Object.prototype.hasOwnProperty.call(view, "isTextMessage")).toBe(false);
  });

  it("asAexEventViews wraps a whole list", () => {
    const raws = [map(ev(0, "assistant_text", { text: "a" })), map(ev(1, "tool_request", { id: "t", name: "bash" }))];
    const views = asAexEventViews(raws);
    expect(views).toHaveLength(2);
    expect(views[0]!.isTextMessage()).toBe(true);
    expect(views[1]!.isToolCallStart()).toBe(true);
  });
});

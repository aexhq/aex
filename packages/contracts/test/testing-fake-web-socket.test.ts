import { describe, expect, it } from "vitest";
import { FakeWebSocket } from "../src/testing.js";
import type { WebSocketFactory } from "../src/index.js";

describe("FakeWebSocket", () => {
  it("records the url and derives the bare session id from the last path segment", () => {
    const ws = new FakeWebSocket("wss://co/sessions/session-9/subscribe?ticket=t&from=0");
    expect(ws.url).toBe("wss://co/sessions/session-9/subscribe?ticket=t&from=0");
    expect(ws.sessionId).toBe("subscribe");
    expect(new FakeWebSocket("wss://events.aex.test/session-1?ticket=t").sessionId).toBe("session-1");
  });

  it("satisfies the WebSocketLike factory contract", () => {
    const factory: WebSocketFactory = (url) => new FakeWebSocket(url);
    const socket = factory("wss://co/x");
    expect(socket).toBeInstanceOf(FakeWebSocket);
    expect((socket as FakeWebSocket).url).toBe("wss://co/x");
  });

  it("delivers message() payloads JSON-encoded to every message listener", () => {
    const ws = new FakeWebSocket("wss://co/s");
    const seen: unknown[] = [];
    ws.addEventListener("message", (ev) => seen.push(ev.data));
    ws.addEventListener("message", (ev) => seen.push(ev.data));
    ws.message({ sequence: 7 });
    expect(seen).toEqual(['{"sequence":7}', '{"sequence":7}']);
  });

  it("pong() delivers a raw non-JSON keep-alive frame", () => {
    const ws = new FakeWebSocket("wss://co/s");
    const frames: unknown[] = [];
    ws.addEventListener("message", (ev) => frames.push(ev.data));
    ws.pong();
    ws.pong("custom-frame");
    expect(frames).toEqual(["aex:pong", "custom-frame"]);
  });

  it("open() and close() emit their lifecycle events and close() marks closed", () => {
    const ws = new FakeWebSocket("wss://co/s");
    const events: string[] = [];
    ws.addEventListener("open", () => events.push("open"));
    ws.addEventListener("close", () => events.push("close"));
    expect(ws.closed).toBe(false);
    ws.open();
    ws.close();
    expect(events).toEqual(["open", "close"]);
    expect(ws.closed).toBe(true);
  });

  it("send() records outbound frames for keep-alive assertions", () => {
    const ws = new FakeWebSocket("wss://co/s");
    ws.send("aex:ping");
    ws.send("aex:ping");
    expect(ws.sent).toEqual(["aex:ping", "aex:ping"]);
  });

  it("removeEventListener is a no-op and driven defaults to false for driver harnesses", () => {
    const ws = new FakeWebSocket("wss://co/s");
    expect(ws.driven).toBe(false);
    ws.removeEventListener();
    ws.driven = true;
    expect(ws.driven).toBe(true);
  });

  it("dispatches nothing for event types without listeners", () => {
    const ws = new FakeWebSocket("wss://co/s");
    expect(() => {
      ws.open();
      ws.message({});
      ws.close();
    }).not.toThrow();
  });
});

/**
 * BLACKBOX — the canonical-event-identity class (WS2 H2·H3·T11, WS8 children).
 *
 * The findings this pins closed, all through the public event surface:
 *   H3   every event carries a FINITE numeric `sequence` — sorting never yields
 *        NaN (the loose-projection `sequence===undefined` runtime lie).
 *   H2   one identity scheme: `id === ${sessionId}:${sequence}`, so `list()` and the
 *        turn stream are joinable — the same logical event has ONE identity.
 *   T11a every event is a guard-BEARING view: `isTextMessage()`/`isToolCallStart()`
 *        /`isToolCallResult()` work and NARROW `data`.
 *   T11c the tool START/RESULT join key is discoverable via `toolCallId()`.
 *   WS8  a session's subagent children expose read-only observation handles with lineage.
 */
import { describe, expect, it } from "vitest";
import type { SessionStartOptions } from "../../../src/index.js";
import { FakePlatform } from "./fake-platform.js";

const SESSION: SessionStartOptions = {
  model: "claude-haiku-4-5",
  message: "write a file",
  apiKeys: { anthropic: "sk-ant" }
};

describe("blackbox: canonical event identity + lineage", () => {
  it("every event has a finite sequence, a canonical id, and working guard methods", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(SESSION, {
      text: "here is your file",
      tools: [{ callId: "call_1", name: "write_file", result: "ok" }],
      costUsd: 0.01
    });

    const events = result.events;
    expect(events.length).toBeGreaterThan(0);

    // H3 — sequence is a real number everywhere; a sort produces no NaN.
    for (const e of events) expect(Number.isFinite(e.sequence)).toBe(true);
    const sorted = [...events].sort((a, b) => a.sequence - b.sequence);
    expect(sorted.map((e) => e.sequence)).toEqual([...events].map((e) => e.sequence).sort((a, b) => a - b));

    // H2 — one identity scheme: id === `${sessionId}:${sequence}`.
    for (const e of events) expect(e.id).toBe(`${result.sessionId}:${e.sequence}`);

    // T11a — guard methods exist and narrow.
    const text = events.find((e) => e.isTextMessage());
    expect(text).toBeDefined();
    expect(text!.data.text).toBe("here is your file"); // narrowed to string

    // T11c — the tool START/RESULT share a discoverable join key.
    const start = events.find((e) => e.isToolCallStart());
    const toolResult = events.find((e) => e.isToolCallResult());
    expect(start).toBeDefined();
    expect(toolResult).toBeDefined();
    expect(start!.toolCallId()).toBe("call_1");
    expect(toolResult!.toolCallId()).toBe("call_1");

    // The SDK projects the canonical tool-call wire keys into the trace: the tool
    // NAME comes from data.name (not the wrong key) — proving the decode path.
    expect(result.trace.toolCalls.map((t) => t.name)).toEqual(["write_file"]);
  });

  it("events().list() and the turn stream are JOINABLE by id (one identity, two surfaces)", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(SESSION, {
      text: "joinable",
      tools: [{ callId: "call_x", name: "grep", result: "found" }],
      costUsd: 0.01
    });

    // Re-open the session and read the snapshot the LIST endpoint serves.
    const handle = await platform.aex.sessions.open(result.sessionId);
    const listed = await handle.events.list();

    const listIds = new Set(listed.map((e) => e.id));
    const streamIds = result.events.map((e) => e.id);
    // Every streamed event id is present in the list snapshot — non-empty overlap,
    // and the ids are the same canonical scheme on both surfaces.
    expect(streamIds.length).toBeGreaterThan(0);
    for (const id of streamIds) expect(listIds.has(id)).toBe(true);
    for (const e of listed) {
      expect(Number.isFinite(e.sequence)).toBe(true);
      expect(e.id).toBe(`${result.sessionId}:${e.sequence}`);
    }
  });

  it("children() resolves each subagent child with its lineage + outcome", async () => {
    const platform = new FakePlatform();
    const result = await platform.start(SESSION, { text: "spawned a subagent", costUsd: 0.02 });

    platform.setChildren(result.sessionId, [
      { id: "child-1", parentSessionId: result.sessionId, depth: 1, status: "idle" },
      { id: "child-2", parentSessionId: result.sessionId, depth: 1, status: "error" }
    ]);

    const handle = await platform.aex.sessions.open(result.sessionId);
    const children = await handle.children();

    expect(children.map((c) => c.id)).toEqual(["child-1", "child-2"]);
    for (const c of children) {
      expect(c.parentSessionId).toBe(result.sessionId);
      expect(c.depth).toBe(1);
      expect("get" in c).toBe(false);
      expect("cancel" in c).toBe(false);
    }
    expect(children.map((c) => c.status)).toEqual(["idle", "error"]);
  });
});

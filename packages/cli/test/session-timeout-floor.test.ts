import { describe, expect, it } from "vitest";
import { executeCli } from "../src/main.js";
import { makeIo } from "./support.js";

const COMMON = ["--api-key", "tok-1", "--aex-url", "https://dash.example/"];

function runSubmitHandler(call: { readonly url: string; readonly init: RequestInit }): Response {
  const path = new URL(call.url).pathname;
  if (path === "/api/sessions") {
    return new Response(JSON.stringify({ session: { id: "s1", status: "idle", acceptsMessages: true } }), {
      status: 201,
      headers: { "content-type": "application/json" }
    });
  }
  if (path === "/api/sessions/s1/messages") {
    return new Response(JSON.stringify({
      session: { id: "s1", status: "running", acceptsMessages: false },
      run: { sessionId: "s1", runId: "run-1", turnSeq: 1, phase: "running" },
      eventCursor: 1
    }), {
      status: 202,
      headers: { "content-type": "application/json" }
    });
  }
  return new Response(JSON.stringify({ error: "unexpected", path }), {
    status: 404,
    headers: { "content-type": "application/json" }
  });
}

describe("aex start --session-timeout floor (T6d — SSoT parser, sync reject)", () => {
  it("rejects a sub-1m --session-timeout synchronously, firing NO network call", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        "--session-timeout", "30s",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    // The SSoT `parseSessionTimeout` enforces the [1m, 8h] floor (not a format-only check).
    expect(cap.stderr).toContain("--session-timeout");
    expect(cap.stderr).toMatch(/at least .*1m|60000ms/);
    // No session was created — the reject is fully client-side.
    expect(cap.calls).toHaveLength(0);
  });

  it("accepts a valid --session-timeout above the floor", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        "--session-timeout", "2h",
        ...COMMON
      ],
      fetchHandler: runSubmitHandler
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(2);
    expect(new URL(cap.calls[0]!.url).pathname).toBe("/api/sessions");
    expect(cap.calls[0]!.body).toMatchObject({ timeout: "2h" });
  });
});

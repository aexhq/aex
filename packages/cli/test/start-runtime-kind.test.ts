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
    }), { status: 202, headers: { "content-type": "application/json" } });
  }
  return new Response(JSON.stringify({ error: "unexpected", path }), {
    status: 404,
    headers: { "content-type": "application/json" }
  });
}

describe("aex start --runtime-kind", () => {
  it("forwards a valid --runtime-kind to the create request", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        "--runtime-kind", "spot_container",
        ...COMMON
      ],
      fetchHandler: runSubmitHandler
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(new URL(cap.calls[0]!.url).pathname).toBe("/api/sessions");
    expect(cap.calls[0]!.body).toMatchObject({ runtimeKind: "spot_container" });
  });

  it("does not send runtimeKind when the flag is omitted (container default downstream)", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        ...COMMON
      ],
      fetchHandler: runSubmitHandler
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls[0]!.body).not.toHaveProperty("runtimeKind");
  });

  it("rejects an invalid --runtime-kind synchronously, firing NO network call", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "claude-haiku-4-5",
        "--prompt", "hi",
        "--anthropic-api-key", "sk-ant-1",
        "--runtime-kind", "fargate",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(2);
    expect(cap.stderr).toContain("--runtime-kind");
    expect(cap.stderr).toMatch(/container, spot_container, lambda/);
    expect(cap.calls).toHaveLength(0);
  });
});

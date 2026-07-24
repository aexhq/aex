import { describe, expect, it } from "bun:test";
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

describe("aex start model policy (gateway slugs — server arbitrates, no closed catalog)", () => {
  it("forwards a well-formed but unknown model slug to the server", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "newvendor/future-model-x",
        "--prompt", "hi",
        ...COMMON
      ],
      fetchHandler: runSubmitHandler
    });
    await executeCli(cap.io);
    expect(cap.exitCode).toBe(0);
    expect(cap.calls).toHaveLength(2);
    const body = cap.calls[0]!.body as { provider?: string; submission?: { model?: string } };
    expect(body.provider).toBeUndefined();
    expect(body.submission?.model).toBe("newvendor/future-model-x");
  });

  it("rejects a bare (non-slug) model id at the boundary with no network call", async () => {
    const cap = makeIo({
      argv: [
        "start",
        "--model", "deepseek-v4-flsh",
        "--prompt", "hi",
        ...COMMON
      ]
    });
    await executeCli(cap.io);
    expect(cap.exitCode).not.toBe(0);
    expect(cap.stderr).toContain("--model");
    expect(cap.calls).toHaveLength(0);
  });
});

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const SCRIPT = String.raw`
import { deepStrictEqual, ok, strictEqual } from "node:assert/strict";
import { Aex } from "@aexhq/sdk";

const calls = [];
let status = "idle";
const revision = {
  checkpointId: "cp-1",
  runId: "run-1",
  turnSeq: 1,
  committedAt: "2026-07-10T00:00:00.000Z",
  throughSeq: 11
};
const json = (value, statusCode = 200) => Response.json(value, { status: statusCode });
const fetch = async (input, init = {}) => {
  const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const parsed = new URL(url);
  const method = String(init.method ?? "GET").toUpperCase();
  const body = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
  calls.push({ path: parsed.pathname, search: parsed.search, method, body, headers: new Headers(init.headers) });
  if (parsed.pathname === "/api/sessions" && method === "POST") {
    return json({ session: { id: "session-1", status: "idle", acceptsMessages: true } }, 201);
  }
  if (parsed.pathname === "/api/sessions/session-1/messages" && method === "POST") {
    status = "running";
    return json({
      session: { id: "session-1", status, acceptsMessages: false },
      run: { sessionId: "session-1", runId: "run-1", turnSeq: 1, phase: "running", eventCursor: 10 },
      eventCursor: 10
    }, 202);
  }
  if (parsed.pathname === "/api/sessions/session-1/events/ticket" && method === "POST") {
    return json({ wsUrl: "wss://events.example/session-1", ticket: "ticket", expiresAtMs: Date.now() + 60_000 });
  }
  if (parsed.pathname === "/api/sessions/session-1/files" && method === "GET") {
    strictEqual(parsed.searchParams.get("checkpointId"), "cp-1");
    return json({ revision, files: [{ id: "file-1", checkpointId: "cp-1", filename: "answer.txt", sizeBytes: 12, sha256: "7509e5bda0c762d2bac7f90d758b5b2263fa01ccbc542ab5e3df163be08e6ca9" }] });
  }
  if (parsed.pathname === "/api/sessions/session-1/events" && method === "GET") {
    return json({ events: [] });
  }
  if (parsed.pathname === "/api/sessions/session-1" && method === "GET") {
    status = "idle";
    return json({ session: {
      id: "session-1",
      status,
      acceptsMessages: true,
      lastRun: { sessionId: "session-1", runId: "run-1", turnSeq: 1, phase: "finished", outcome: "succeeded", checkpoint: revision }
    }});
  }
  if (parsed.pathname === "/api/sessions/session-1/suspend" && method === "POST") {
    status = "suspended";
    return json({ session: { id: "session-1", status, acceptsMessages: true } });
  }
  if (parsed.pathname === "/api/sessions/session-1/resume" && method === "POST") {
    status = "idle";
    return json({ session: { id: "session-1", status, acceptsMessages: true } });
  }
  return json({ error: "unexpected", path: parsed.pathname, method }, 404);
};

const terminal = {
  specversion: "1.0",
  id: "session-1:11",
  source: "workflow",
  type: "RUN_FINISHED",
  subject: "session-1",
  threadId: "session-1",
  runId: "run-1",
  time: "2026-07-10T00:00:00.000Z",
  sequence: 11,
  data: { outcome: "succeeded", checkpoint: { checkpointId: "cp-1" }, costUsd: 0.002, providerUsage: [{ inputTokens: 3, outputTokens: 2, totalTokens: 5 }] }
};
const text = {
  ...terminal,
  id: "session-1:10",
  type: "TEXT_MESSAGE_CONTENT",
  sequence: 10,
  data: { text: "hello from chat", messageId: "message-1" }
};

class Socket {
  constructor(url) {
    this.url = url;
    this.listeners = {};
    queueMicrotask(() => this.emit("open", {}));
    setTimeout(() => {
      this.emit("message", { data: JSON.stringify(text) });
      this.emit("message", { data: JSON.stringify(terminal) });
      this.emit("close", {});
    }, 0);
  }
  addEventListener(type, listener) { (this.listeners[type] ??= []).push(listener); }
  removeEventListener(type, listener) { this.listeners[type] = (this.listeners[type] ?? []).filter((entry) => entry !== listener); }
  send() {}
  close() { this.emit("close", {}); }
  emit(type, event) { for (const listener of this.listeners[type] ?? []) listener(event); }
}

const client = new Aex({ apiKey: "aex_test", baseUrl: "https://api.example", fetch });
const session = await client.sessions.create({ model: "claude-haiku-4-5", apiKeys: { anthropic: "sk-ant" } });
const result = await session.messages.send(["hello", "again"], {
  idempotencyKey: "stable-message",
  webSocketFactory: (url) => new Socket(url)
}).finished();

strictEqual(result.status, "succeeded");
strictEqual(result.session.status, "idle");
strictEqual(result.run.runId, "run-1");
strictEqual(result.text, "hello from chat");
strictEqual(result.costUsd, 0.002);
deepStrictEqual(result.usage, { inputTokens: 3, outputTokens: 2, totalTokens: 5 });
strictEqual(result.checkpoint.checkpointId, "cp-1");
strictEqual(result.files[0].checkpointId, "cp-1");
deepStrictEqual(result.events.map((event) => event.type), ["TEXT_MESSAGE_CONTENT", "RUN_FINISHED"]);
strictEqual(calls.find((call) => call.path.endsWith("/messages")).headers.get("idempotency-key"), "stable-message");

await session.suspend();
strictEqual(session.record.status, "suspended");
await session.resume();
strictEqual(session.record.status, "idle");
ok(typeof session.messages === "object" && typeof session.events === "object" && typeof session.files === "object");

process.stdout.write(JSON.stringify({
  resultStatus: result.status,
  sessionStatus: result.session.status,
  eventTypes: result.events.map((event) => event.type),
  checkpointId: result.checkpoint.checkpointId,
  costUsd: result.costUsd
}));
`;

describe("installed SDK resumable session", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => install?.cleanup());

  it("finishes on RUN_FINISHED and reads the matching checkpoint", async () => {
    const path = join(install.installDir, "sdk-session-run.mjs");
    writeFileSync(path, SCRIPT);
    const child = await runCommand(getBunCommand(), [path], { cwd: install.installDir, timeoutMs: 120_000 });
    if (child.exitCode !== 0) throw new Error(`sdk-session-run.mjs exited ${child.exitCode}\n${child.stderr}`);
    expect(JSON.parse(child.stdout)).toEqual({
      resultStatus: "succeeded",
      sessionStatus: "idle",
      eventTypes: ["TEXT_MESSAGE_CONTENT", "RUN_FINISHED"],
      checkpointId: "cp-1",
      costUsd: 0.002
    });
  });
});

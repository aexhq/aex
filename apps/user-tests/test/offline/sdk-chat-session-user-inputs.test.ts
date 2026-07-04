/**
 * Blackbox SDK session coverage through a clean installed package.
 *
 * The child scripts run from the user-test install tempdir, import
 * `@aexhq/sdk` from the packed/published artifact, and use fake fetch/WS
 * transports to assert the public wire shape without dispatching a live run.
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";

const CHILD_HARNESS = String.raw`
import { deepStrictEqual, match, ok, strictEqual } from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Skills are ingested as TOOLS now: build a skill-tool from a temp directory
// containing a SKILL.md whose YAML frontmatter carries the tool name +
// description, then pass it via Tools.fromSkillDir.
function makeSkillDir(name, description) {
  const dir = mkdtempSync(join(tmpdir(), "aex-skill-"));
  writeFileSync(
    join(dir, "SKILL.md"),
    "---\nname: " + name + "\ndescription: " + description + "\n---\n# " + name + "\n" + description + "\n"
  );
  return dir;
}

function assetIdFromHash(hash) {
  const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
  return "asset_" + hex;
}

function headersToObject(headers) {
  const out = {};
  if (!headers) return out;
  if (headers instanceof Headers) {
    for (const [key, value] of headers.entries()) out[key.toLowerCase()] = value;
    return out;
  }
  if (Array.isArray(headers)) {
    for (const [key, value] of headers) out[String(key).toLowerCase()] = String(value);
    return out;
  }
  for (const [key, value] of Object.entries(headers)) out[String(key).toLowerCase()] = String(value);
  return out;
}

async function decodeBody(body) {
  if (body === undefined || body === null) return undefined;
  if (typeof body === "string") {
    try { return JSON.parse(body); } catch { return body; }
  }
  if (body instanceof Uint8Array) return { kind: "Uint8Array", byteLength: body.byteLength };
  try {
    const text = await new Response(body).text();
    try { return JSON.parse(text); } catch { return text; }
  } catch {
    return String(body);
  }
}

function json(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" }
  });
}

function makeHarness() {
  const calls = [];
  const sockets = [];
  let sessionCounter = 0;
  let turnSeq = 0;
  let sessionStatus = "idle";

  const fetchFake = async (input, init = {}) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    const parsed = new URL(url);
    const method = String(init.method ?? "GET").toUpperCase();
    const headers = headersToObject(init.headers);
    const body = await decodeBody(init.body);
    calls.push({ url, path: parsed.pathname, search: parsed.search, method, headers, body });

    if (parsed.pathname.endsWith("/assets/presign")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return json({
        ok: true,
        exists: false,
        assetId: "asset_" + hex,
        contentHash: hash,
        uploadUrl: "https://object-storage.example.test/assets/" + hex,
        requiredHeaders: { "x-amz-checksum-sha256": "Y2hlY2tzdW0=" }
      }, 201);
    }
    if (parsed.hostname === "object-storage.example.test") return new Response("", { status: 200 });
    if (parsed.pathname.endsWith("/assets/finalize")) {
      const hash = body && typeof body.hash === "string" ? body.hash : "sha256:" + "a".repeat(64);
      const hex = hash.startsWith("sha256:") ? hash.slice("sha256:".length) : hash;
      return json({ ok: true, assetId: "asset_" + hex, contentHash: hash, sizeBytes: body?.sizeBytes ?? 1 });
    }

    if (parsed.pathname === "/api/sessions" && method === "POST") {
      sessionCounter += 1;
      sessionStatus = "idle";
      turnSeq = 0;
      return json({ session: { id: "sess_user_" + sessionCounter, status: sessionStatus, turnSeq } }, 201);
    }
    if (parsed.pathname === "/api/sessions" && method === "GET") {
      return json({
        sessions: [
          { id: "sess_user_1", status: sessionStatus, turnSeq, createdAt: new Date(0).toISOString(), updatedAt: new Date(0).toISOString() }
        ]
      });
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/messages" && method === "POST") {
      turnSeq += 1;
      sessionStatus = "running";
      return json({
        session: { id: "sess_user_1", status: sessionStatus, turnSeq },
        turn: { sessionId: "sess_user_1", turnSeq, turnId: "turn_" + turnSeq, eventCursor: 10 },
        eventCursor: 10
      }, 202);
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/events/ticket" && method === "POST") {
      return json({
        ok: true,
        wsUrl: "wss://events.example.test/api/runs/sess_user_1/subscribe",
        ticket: "ticket-" + turnSeq,
        expiresAtMs: Date.now() + 60000
      });
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/events" && method === "GET") {
      return json({
        events: [
          event(0, "RUN_STARTED", { source: "session" }),
          event(10, "TEXT_MESSAGE_CONTENT", { text: "snapshot text" }),
          sessionEvent(11, "aex.session.idle")
        ]
      });
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/outputs" && method === "GET") {
      return json({ outputs: [{ id: "out_1", filename: "answer.txt", sizeBytes: 12 }] });
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/suspend" && method === "POST") {
      sessionStatus = "suspended";
      return json({ session: { id: "sess_user_1", status: sessionStatus, turnSeq } }, 202);
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/cancel" && method === "POST") {
      sessionStatus = "idle";
      return json({ session: { id: "sess_user_1", status: sessionStatus, turnSeq } }, 202);
    }
    if (parsed.pathname === "/api/sessions/sess_user_1/resume" && method === "POST") {
      sessionStatus = "idle";
      return json({ session: { id: "sess_user_1", status: sessionStatus, turnSeq } });
    }
    if (parsed.pathname === "/api/sessions/sess_user_1" && method === "DELETE") {
      sessionStatus = "deleted";
      return json({ session: { id: "sess_user_1", status: sessionStatus, turnSeq } });
    }
    if (parsed.pathname === "/api/sessions/sess_user_1" && method === "GET") {
      if (sessionStatus === "running") sessionStatus = "idle";
      return json({ session: { id: "sess_user_1", status: sessionStatus, turnSeq } });
    }

    return json({ ok: true });
  };

  class FakeWebSocket {
    constructor(url) {
      this.url = url;
      this.listeners = {};
      sockets.push(this);
      queueMicrotask(() => this.emit("open", {}));
      setTimeout(() => {
        this.message(event(10, "TEXT_MESSAGE_CONTENT", { text: "hello from chat" }));
        this.message(sessionEvent(11, "aex.session.idle"));
      }, 0);
    }
    addEventListener(type, cb) {
      (this.listeners[type] ??= []).push(cb);
    }
    close() {
      this.emit("close", {});
    }
    message(evt) {
      this.emit("message", { data: JSON.stringify(evt) });
    }
    emit(type, ev) {
      for (const cb of this.listeners[type] ?? []) cb(ev);
    }
  }

  return {
    calls,
    sockets,
    fetch: fetchFake,
    webSocketFactory: (url) => new FakeWebSocket(url)
  };
}

function event(sequence, type, data) {
  return {
    specversion: "1.0",
    id: "sess_user_1:" + sequence,
    source: "agent",
    type,
    subject: "sess_user_1",
    time: new Date(sequence).toISOString(),
    sequence,
    data
  };
}

function sessionEvent(sequence, name) {
  return event(sequence, "CUSTOM", { name, value: { sessionId: "sess_user_1", turnSeq: 1 } });
}

function callsFor(calls, method, path) {
  return calls.filter((call) => call.method === method && call.path === path);
}

function onlyCall(calls, method, path) {
  const matches = callsFor(calls, method, path);
  strictEqual(matches.length, 1, method + " " + path);
  return matches[0];
}

async function expectReject(label, fn, pattern) {
  try {
    await fn();
  } catch (err) {
    const message = err && err.message ? err.message : String(err);
    match(message, pattern, label + " message");
    return;
  }
  throw new Error(label + " unexpectedly resolved");
}
`;

describe("SDK sessions (installed package)", () => {
  let install: InstallResult;

  beforeAll(async () => {
    install = await installAex();
  }, 240_000);

  afterAll(() => {
    install?.cleanup();
  });

  async function runChild(script: string, fileName: string): Promise<Record<string, unknown>> {
    const scriptPath = join(install.installDir, fileName);
    writeFileSync(scriptPath, script);
    const child = await runCommand(getBunCommand(), [scriptPath], {
      cwd: install.installDir,
      timeoutMs: 120_000
    });
    if (child.exitCode !== 0) {
      throw new Error(
        `${fileName} exited ${child.exitCode}\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
      );
    }
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  }

  it("serializes openSession and session state operations on the public session routes", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex, AgentsMd, File, Secret, Tools } = await import("@aexhq/sdk");

const h = makeHarness();
const client = new Aex({ apiKey: "aex_chat_token", baseUrl: "https://example.invalid", fetch: h.fetch });
const skill = await Tools.fromSkillDir(makeSkillDir("chat-skill", "Follow instructions."), { name: "chat-skill" });
const rules = await AgentsMd.fromContent("# Chat rules\nKeep it short.\n", { name: "chat-rules" });
const file = await File.fromBytes({
  name: "chat-note",
  bytes: new TextEncoder().encode("note"),
  mountPath: "/workspace/input/chat-note.txt"
});

const session = await client.openSession({
  provider: "anthropic",
  model: "claude-haiku-4-5",
  system: "System instructions for the whole session.",
  tools: [skill],
  agentsMd: [rules],
  files: [file],
  environment: {
    variables: { CHAT_MODE: "test" },
    secrets: { CHAT_SECRET: Secret.value("ephemeral-chat-secret") }
  },
  outputs: { allowedDirs: ["/workspace/out"], deniedDirs: [""] },
  includeBuiltinTools: false,
  outputMode: "stream",
  metadata: { suite: "chat-session-user-inputs" },
  runtime: "shared-0.06x-256mb",
  overrides: { timeout: "30m", idleTtl: "3m", maxSpendUsd: 2.5 },
  apiKeys: { anthropic: "sk-ant-chat" },
  idempotencyKey: "idem-chat-create"
});

strictEqual(session.id, "sess_user_1");
const create = onlyCall(h.calls, "POST", "/api/sessions");
strictEqual(create.headers["idempotency-key"], "idem-chat-create");
strictEqual(create.body.provider, "anthropic");
strictEqual(create.body.runtimeSize, "shared-0.06x-256mb");
strictEqual(create.body.timeout, "30m");
deepStrictEqual(create.body.limits, { maxSpendUsd: 2.5 });
deepStrictEqual(create.body.retention, { idleTtl: "3m" });
ok(!("input" in create.body));
ok(!("webhook" in create.body));
ok(!("postHook" in create.body));

const submission = create.body.submission;
strictEqual(submission.model, "claude-haiku-4-5");
strictEqual(submission.system, "System instructions for the whole session.");
ok(!("prompt" in submission));
ok(!("skills" in submission), "submission.skills is removed; skill-tools ride submission.tools");
strictEqual(submission.tools.length, 1);
deepStrictEqual(submission.tools[0], {
  kind: "skill",
  assetId: assetIdFromHash(skill.ref.contentHash),
  name: "chat-skill",
  description: "Follow instructions."
});
strictEqual(submission.agentsMd.length, 1);
strictEqual(submission.files.length, 1);
strictEqual(submission.includeBuiltinTools, false);
strictEqual(submission.outputMode, "stream");
deepStrictEqual(submission.outputs, { allowedDirs: ["/workspace/out"] });
deepStrictEqual(submission.metadata, { suite: "chat-session-user-inputs" });
deepStrictEqual(submission.environment, { envVars: { CHAT_MODE: "test" } });
deepStrictEqual(submission.secretEnv, { CHAT_SECRET: { ephemeral: true } });
deepStrictEqual(create.body.secrets.apiKeys, { anthropic: "sk-ant-chat" });
deepStrictEqual(create.body.secrets.envSecrets, { CHAT_SECRET: "ephemeral-chat-secret" });
ok(!("proxyEndpointAuth" in create.body.secrets));
ok(!JSON.stringify(submission).includes("ephemeral-chat-secret"));

await session.suspend({ idempotencyKey: "idem-suspend" });
await session.cancel({ idempotencyKey: "idem-cancel" });
await session.resume({ idempotencyKey: "idem-resume" });
await client.sessions.get(session.id);
await client.sessions.list({ status: "idle", limit: 5 });
await session.events().list();
await session.outputs().list({ filename: "answer.txt" });
await session.delete({ idempotencyKey: "idem-delete" });

strictEqual(onlyCall(h.calls, "POST", "/api/sessions/sess_user_1/suspend").headers["idempotency-key"], "idem-suspend");
strictEqual(onlyCall(h.calls, "POST", "/api/sessions/sess_user_1/cancel").headers["idempotency-key"], "idem-cancel");
strictEqual(onlyCall(h.calls, "POST", "/api/sessions/sess_user_1/resume").headers["idempotency-key"], "idem-resume");
strictEqual(onlyCall(h.calls, "DELETE", "/api/sessions/sess_user_1").headers["idempotency-key"], "idem-delete");
ok(h.calls.some((call) => call.method === "GET" && call.path === "/api/sessions" && call.search.includes("status=idle")));
ok(h.calls.some((call) => call.method === "GET" && call.path === "/api/sessions/sess_user_1/events"));
ok(h.calls.some((call) => call.method === "GET" && call.path === "/api/sessions/sess_user_1/outputs"));

console.log(JSON.stringify({ ok: true, createCalls: callsFor(h.calls, "POST", "/api/sessions").length }));
`;
    const result = await runChild(script, "sdk-chat-create-shape.mjs");
    expect(result).toMatchObject({ ok: true, createCalls: 1 });
  });

  it("sends a session turn over the session event stream and stops on idle", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex } = await import("@aexhq/sdk");

const h = makeHarness();
const client = new Aex({ apiKey: "aex_chat_token", baseUrl: "https://example.invalid", fetch: h.fetch });
const session = await client.sessions.create({
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: "sk-ant-chat" }
});
const result = await session.send(["hello", "again"], {
  idempotencyKey: "idem-message",
  webSocketFactory: h.webSocketFactory
}).done();

strictEqual(result.sessionId, "sess_user_1");
strictEqual(result.status, "idle");
strictEqual(result.turn.turnSeq, 1);
strictEqual(result.text, "hello from chat");
deepStrictEqual(result.outputs, [{ id: "out_1", filename: "answer.txt", sizeBytes: 12 }]);
deepStrictEqual(result.events.map((event) => event.type), ["TEXT_MESSAGE_CONTENT", "CUSTOM"]);
strictEqual(h.sockets.length, 1);
strictEqual(h.sockets[0].url, "wss://events.example.test/api/runs/sess_user_1/subscribe?ticket=ticket-1&from=10");

const message = onlyCall(h.calls, "POST", "/api/sessions/sess_user_1/messages");
strictEqual(message.headers["idempotency-key"], "idem-message");
deepStrictEqual(message.body, { input: ["hello", "again"] });
ok(h.calls.some((call) => call.method === "POST" && call.path === "/api/sessions/sess_user_1/events/ticket"));
ok(h.calls.some((call) => call.method === "GET" && call.path === "/api/sessions/sess_user_1"));
ok(h.calls.some((call) => call.method === "GET" && call.path === "/api/sessions/sess_user_1/outputs"));

console.log(JSON.stringify({ ok: true, events: result.events.length, sockets: h.sockets.length }));
`;
    const result = await runChild(script, "sdk-chat-send-stream.mjs");
    expect(result).toMatchObject({ ok: true, events: 2, sockets: 1 });
  });

  it("rejects removed session options before any HTTP call", async () => {
    const script = CHILD_HARNESS + String.raw`
const { Aex } = await import("@aexhq/sdk");
const h = makeHarness();
const client = new Aex({ apiKey: "aex_chat_token", baseUrl: "https://example.invalid", fetch: h.fetch });

await expectReject("missing create options", () => client.openSession(undefined), /options is required/);
await expectReject("removed postHook", () => client.openSession({
  model: "claude-haiku-4-5",
  postHook: { command: "bun test" },
  apiKeys: { anthropic: "sk-ant" }
}), /postHook is not a supported option/);
await expectReject("removed runtimeSize", () => client.openSession({
  model: "claude-haiku-4-5",
  runtimeSize: "shared-0.06x-256mb",
  apiKeys: { anthropic: "sk-ant" }
}), /runtimeSize is not a supported option/);
await expectReject("removed idleSuspendAfter override", () => client.openSession({
  model: "claude-haiku-4-5",
  overrides: { idleSuspendAfter: "3m" },
  apiKeys: { anthropic: "sk-ant" }
}), /overrides\.idleSuspendAfter is not a supported option/);
await expectReject("provider mismatch", () => client.openSession({
  provider: "anthropic",
  model: "gpt-4.1",
  apiKeys: { anthropic: "sk-ant" }
}), /provider "anthropic" is not available/);

strictEqual(h.calls.length, 0);
console.log(JSON.stringify({ ok: true, rejects: 5, calls: h.calls.length }));
`;
    const result = await runChild(script, "sdk-chat-invalid.mjs");
    expect(result).toMatchObject({ ok: true, rejects: 5, calls: 0 });
  });
});

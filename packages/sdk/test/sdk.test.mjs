import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  Aex,
  defineIntrinsicTool,
  OutputRefusalError,
  OutputSchemaError,
  OutputValidationError,
} from "../dist/index.js";
import { z } from "zod";

const snapshot = {
  id: "ses_01",
  object: "session",
  state: "idle",
  model: { provider: "anthropic", name: "claude-sonnet-5" },
  hand: { state: "ready", shape: "1gb" },
  storage: { workspace_bytes: 0, suspended_bytes: 0, artifact_bytes: 0 },
  created_at: "2026-08-19T10:00:00.000Z",
  updated_at: "2026-08-19T10:00:00.000Z",
  turns: 0,
  metadata: {},
};

test("errors use the Aex display name", () => {
  assert.throws(() => new Aex({ apiKey: "" }), /Aex apiKey cannot be empty/);
});

test("create uses the production origin and maps the small camelCase surface", async () => {
  let request;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init) => {
      request = { input: String(input), init };
      return Response.json(snapshot, { status: 201 });
    },
  });

  const session = await aex.sessions.create({
    model: {
      provider: "anthropic",
      name: "claude-sonnet-5",
      apiKey: "sk-ant-test",
      maxOutputTokens: 2048,
    },
  });

  assert.equal(request.input, "https://api.aex.dev/v1/sessions");
  assert.deepEqual(JSON.parse(request.init.body), {
    model: {
      provider: "anthropic",
      name: "claude-sonnet-5",
      api_key: "sk-ant-test",
      max_output_tokens: 2048,
    },
    tools: { items: [] },
  });
  assert.equal(request.init.headers.Authorization, "Bearer aex_sk_test");
  assert.ok(request.init.headers["Idempotency-Key"]);
  assert.equal(session.id, "ses_01");
  assert.equal("hand" in session, false);
  assert.equal(JSON.stringify(aex).includes("aex_sk_test"), false);
  assert.equal(JSON.stringify(session).includes("aex_sk_test"), false);
});

test("create preserves an explicit tool grant in order and rejects duplicates", async () => {
  const bodies = [];
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      bodies.push(JSON.parse(init.body));
      return Response.json(snapshot, { status: 201 });
    },
  });

  const task = intrinsicTool("task");
  const read = intrinsicTool("read");
  const write = intrinsicTool("write");
  await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [task, read, write],
  });

  assert.deepEqual(
    bodies[0].tools.items.map((item) => item.definition.name),
    ["task", "read", "write"],
  );
  assert.deepEqual(
    bodies[0].tools.items.map((item) => item.executor.kind),
    ["intrinsic", "intrinsic", "intrinsic"],
  );
  await assert.rejects(
    aex.sessions.create({
      model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
      tools: [read, read],
    }),
    /selected more than once/,
  );
  assert.equal(bodies.length, 1, "invalid tool selections fail before creating a session");
});

test("an explicit empty tool list is equivalent to omission", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json(snapshot, { status: 201 });
    },
  });

  await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [],
  });

  assert.deepEqual(body.tools, { items: [] });
});

test("create maps remote MCP configuration into Brain's sealed tool grant", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json(snapshot, { status: 201 });
    },
  });

  await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    mcp: [{
      name: "docs",
      url: "https://mcp.example.test/rpc",
      headers: { Authorization: "Bearer secret" },
      protocol: "2026-07",
      allowedTools: ["search"],
    }],
  });

  assert.deepEqual(body.tools, {
    items: [],
    mcp: [{
      name: "docs",
      url: "https://mcp.example.test/rpc",
      headers: { Authorization: "Bearer secret" },
      protocol: "2026-07",
      allowed_tools: ["search"],
    }],
  });
});

test("send output hashes the Zod schema, follows events, and resolves inferred data", async () => {
  let outputBody;
  const fetch = async (input, init) => {
    const url = String(input);
    if (url.endsWith("/messages")) {
      outputBody = JSON.parse(init.body);
      return Response.json(
        {
          session_id: "ses_01",
          turn_id: "turn_out",
          output_id: "out_01",
          schema_hash: outputBody.output.schema_hash,
          seq: 9,
        },
        { status: 202 },
      );
    }
    if (url.includes("/events?")) {
      const event = {
        type: "turn.completed",
        seq: 10,
        at: "2026-08-19T10:00:01.000Z",
        session_id: "ses_01",
        turn_id: "turn_out",
        stop_reason: "end_turn",
        rounds: 1,
        tool_calls: 1,
        result: {
          call_id: "call_01",
          name: "aex_submit_output",
          value: { answer: 42 },
          metadata: { output_id: "out_01", schema_hash: outputBody.output.schema_hash },
        },
      };
      return sse(event);
    }
    throw new Error(`unexpected request: ${url}`);
  };
  // Construct through create to keep the public API under test.
  const live = await new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init) => {
      if (String(input).endsWith("/v1/sessions")) return Response.json(snapshot, { status: 201 });
      return fetch(input, init);
    },
  }).sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  const result = await live.send("Give me the answer.", {
    output: z.object({ answer: z.number().int() }),
  });

  assert.deepEqual(result, { answer: 42 });
  assert.equal(outputBody.content, "Give me the answer.");
  const canonical = canonicalize(outputBody.output.schema);
  assert.equal(outputBody.output.schema_hash, createHash("sha256").update(canonical).digest("hex"));
  assert.equal(live.state, "idle");
});

test("send output maps terminal validation details to OutputValidationError", async () => {
  let schemaHash;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    baseUrl: "https://example.test",
    fetch: async (input, init) => {
      const url = String(input);
      if (url.endsWith("/v1/sessions")) return Response.json(snapshot, { status: 201 });
      if (url.endsWith("/messages")) {
        schemaHash = JSON.parse(init.body).output.schema_hash;
        return Response.json(
          { session_id: "ses_01", turn_id: "turn_bad", output_id: "out_bad", schema_hash: schemaHash, seq: 3 },
          { status: 202 },
        );
      }
      return sse({
        type: "turn.failed",
        seq: 4,
        at: "2026-08-19T10:00:01.000Z",
        session_id: "ses_01",
        turn_id: "turn_bad",
        error: {
          code: "output_validation_error",
          message: "output did not match",
          details: { issues: [{ path: "/answer", message: "must be number", keyword: "type" }] },
        },
      });
    },
  });
  const session = await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  await assert.rejects(
    session.send("answer", { output: z.object({ answer: z.number() }) }),
    (error) =>
      error instanceof OutputValidationError &&
      error.issues[0]?.path === "/answer" &&
      error.code === "output_validation_error",
  );
});

test("send output distinguishes a refusal from a missing submission", async () => {
  let stopReason = "end_turn";
  let turn = 0;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    baseUrl: "https://example.test",
    fetch: async (input, init) => {
      const url = String(input);
      if (url.endsWith("/v1/sessions")) return Response.json(snapshot, { status: 201 });
      if (url.endsWith("/messages")) {
        turn += 1;
        const schemaHash = JSON.parse(init.body).output.schema_hash;
        return Response.json(
          {
            session_id: "ses_01",
            turn_id: `turn_${turn}`,
            output_id: `out_${turn}`,
            schema_hash: schemaHash,
            seq: turn * 2,
          },
          { status: 202 },
        );
      }
      return sse({
        type: "turn.completed",
        seq: turn * 2 + 1,
        at: "2026-08-19T10:00:01.000Z",
        session_id: "ses_01",
        turn_id: `turn_${turn}`,
        stop_reason: stopReason,
        rounds: 1,
        tool_calls: 0,
      });
    },
  });
  const session = await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  await assert.rejects(
    session.send("answer", { output: z.object({ answer: z.number() }) }),
    (error) =>
      error instanceof OutputValidationError &&
      error.issues[0]?.keyword === "missing_output",
  );

  stopReason = "refusal";
  await assert.rejects(
    session.send("answer", { output: z.object({ answer: z.number() }) }),
    OutputRefusalError,
  );
});

test("send output rejects process-local Zod refinements before an API call", async () => {
  let requests = 0;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async () => {
      requests += 1;
      return Response.json(snapshot, { status: 201 });
    },
  });
  const session = await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });
  await assert.rejects(
    session.send("answer", { output: z.object({ answer: z.string().refine((value) => value === "yes") }) }),
    OutputSchemaError,
  );
  await assert.rejects(
    session.send("answer", { output: z.object({ answer: z.string().trim() }) }),
    OutputSchemaError,
  );
  await assert.rejects(
    session.send("answer", { output: z.object({ answer: z.string().regex(/^(a+)+$/) }) }),
    OutputSchemaError,
  );
  assert.equal(requests, 1, "schema rejection must not admit model work");
});

test("send returns the final root assistant message", async () => {
  const events = [
    {
      type: "assistant.message",
      seq: 12,
      at: "2026-08-19T10:00:01.000Z",
      session_id: "ses_01",
      turn_id: "turn_01",
      agent_id: "root",
      text: "The concise answer.",
    },
    {
      type: "turn.completed",
      seq: 13,
      at: "2026-08-19T10:00:02.000Z",
      session_id: "ses_01",
      turn_id: "turn_01",
      stop_reason: "end_turn",
      rounds: 1,
      tool_calls: 0,
    },
  ];
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input) => {
      const url = String(input);
      if (url.endsWith("/v1/sessions")) return Response.json(snapshot, { status: 201 });
      if (url.endsWith("/messages")) {
        return Response.json({ session_id: "ses_01", turn_id: "turn_01", seq: 11 }, { status: 202 });
      }
      return sse(...events);
    },
  });
  const session = await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });
  assert.equal(await session.send("Be concise."), "The concise answer.");
});

test("send output retries transport loss with one identity and reconnects event replay", async () => {
  let outputAttempts = 0;
  let eventAttempts = 0;
  const keys = [];
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init) => {
      const url = String(input);
      if (url.endsWith("/v1/sessions")) return Response.json(snapshot, { status: 201 });
      if (url.endsWith("/messages")) {
        outputAttempts += 1;
        keys.push(init.headers["Idempotency-Key"]);
        body = JSON.parse(init.body);
        if (outputAttempts === 1) throw new TypeError("connection reset after write");
        return Response.json(
          { session_id: "ses_01", turn_id: "turn_retry", output_id: "out_retry", schema_hash: body.output.schema_hash, seq: 20 },
          { status: 202 },
        );
      }
      eventAttempts += 1;
      if (eventAttempts === 1) throw new TypeError("stream disconnected");
      return sse({
        type: "turn.completed",
        seq: 21,
        at: "2026-08-19T10:00:01.000Z",
        session_id: "ses_01",
        turn_id: "turn_retry",
        stop_reason: "end_turn",
        rounds: 1,
        tool_calls: 1,
        result: {
          call_id: "call_retry",
          name: "aex_submit_output",
          value: { ok: true },
          metadata: { output_id: "out_retry", schema_hash: body.output.schema_hash },
        },
      });
    },
  });
  const session = await aex.sessions.create({
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  assert.deepEqual(
    await session.send("return ok", {
      output: z.object({ ok: z.boolean() }),
      idempotencyKey: "stable-output-request",
    }),
    { ok: true },
  );
  assert.deepEqual(keys, ["stable-output-request", "stable-output-request"]);
  assert.equal(eventAttempts, 2);
});

function sse(...events) {
  const body = events
    .map((event) => `id: ${event.seq}\nevent: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
    .join("");
  return new Response(body, {
    status: 200,
    headers: { "content-type": "text/event-stream" },
  });
}

function intrinsicTool(name) {
  return defineIntrinsicTool({
    name,
    description: `${name} test capability`,
    input: z.object({}),
    output: z.object({ ok: z.boolean() }),
    capability: `test.${name}.v1`,
  });
}

function canonicalize(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalize).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalize(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

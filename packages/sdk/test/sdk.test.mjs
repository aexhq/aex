import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";

import {
  Aex,
  OutputRefusalError,
  OutputSchemaError,
  OutputValidationError,
  tool,
} from "../dist/index.js";
import { parseEventStream, Transport } from "../dist/transport.js";
import { MAX_PUBLIC_EVENT_BYTES, component } from "@aexhq/brain";
import { app as appComponent } from "@aexhq/env-app";
import { z } from "zod";

const componentBytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
const testModel = component("model", componentBytes, {}, { metadata: { name: "fixture" } });
const testAgentloop = component("agentloop", componentBytes, {});

function testTool(name, grants = []) {
  return component("tool", componentBytes, {
    definition: {
      name,
      input_schema: { type: "object" },
      output_schema: {},
      contract_digest: createHash("sha256").update(name).digest("hex"),
    },
  }, { grants });
}

function create(aex, options, request) {
  return aex.sessions.create({
    ...options,
    model: { component: testModel, ...options.model },
    agentloop: testAgentloop,
  }, request);
}

const snapshot = {
  id: "ses_01",
  root_id: "ses_01",
  depth: 0,
  object: "session",
  state: "open",
  turn_state: "idle",
  shape: "1gb",
  model: {
    provider: "anthropic",
    name: "claude-sonnet-5",
    context_window_tokens: 200_000,
  },
  storage: { session_storage_bytes: 0, upload_reserved_bytes: 0 },
  created_at: "2026-08-19T10:00:00.000Z",
  retain_until: "2027-09-23T10:00:00.000Z",
  updated_at: "2026-08-19T10:00:00.000Z",
  turns: 0,
  last_seq: 0,
  metadata: {},
  environments: ["workspace"],
};

test("errors use the Aex display name", () => {
  assert.throws(() => new Aex({ apiKey: "" }), /Aex apiKey cannot be empty/);
});

test("tool infers a named function and has no placement finalizer", () => {
  const lookup = tool(
    z.object({ id: z.string() }),
    async function lookup({ id }) {
      return { id };
    },
  )
    .describe("Look up one record.")
    .returns(z.object({ id: z.string() }));

  assert.equal(lookup.name, "lookup");
  assert.equal(lookup.description, "Look up one record.");
  assert.ok(Object.isFrozen(lookup));
  assert.equal("client" in lookup, false);
  assert.equal("server" in lookup, false);
});

test("session creation fails before transport without an Agentloop component", async () => {
  let called = false;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async () => { called = true; return Response.json(snapshot); },
  });
  await assert.rejects(
    aex.sessions.create({
      model: { component: testModel, provider: "anthropic", name: "m", apiKey: "key" },
    }),
    /requires an imported Agentloop component/,
  );
  assert.equal(called, false);
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

  const session = await create(aex, {
    model: {
      provider: "anthropic",
      name: "claude-sonnet-5",
      apiKey: "sk-ant-test",
      maxOutputTokens: 2048,
    },
  });

  assert.equal(request.input, "https://api.aex.dev/v1/sessions");
  const body = JSON.parse(request.init.body);
  assert.equal(body.model.provider, "anthropic");
  assert.equal(body.model.name, "claude-sonnet-5");
  assert.equal(body.model.api_key, "sk-ant-test");
  assert.equal(body.model.max_output_tokens, 2048);
  assert.equal(body.model.world, "aex:model/model@1.0.0");
  assert.equal(body.agentloop.world, "aex:agentloop/agentloop@1.0.0");
  assert.deepEqual(body.tools, { items: [] });
  assert.equal(body.component_artifacts.length, 1);
  assert.equal(request.init.headers.Authorization, "Bearer aex_sk_test");
  assert.ok(request.init.headers["Idempotency-Key"]);
  assert.equal(session.id, "ses_01");
  assert.equal(session.rootId, "ses_01");
  assert.equal(session.parentId, undefined);
  assert.equal(session.depth, 0);
  assert.equal(session.model.contextWindowTokens, 200_000);
  assert.equal("hand" in session, false);
  assert.equal(JSON.stringify(aex).includes("aex_sk_test"), false);
  assert.equal(JSON.stringify(session).includes("aex_sk_test"), false);
});

test("create retries one server failure with the identical idempotency identity", async () => {
  const requests = [];
  const createOptions = {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    metadata: { phase: "before-dispatch" },
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      requests.push({
        key: init.headers["Idempotency-Key"],
        body: init.body,
      });
      if (requests.length === 1) {
        // A caller can mutate its input while the first fetch is pending. The retry identity owns
        // the originally serialized bytes, not a live object reference.
        createOptions.metadata.phase = "after-dispatch";
        return Response.json(
          { error: { code: "internal_error", message: "response outcome is unknown" } },
          { status: 503 },
        );
      }
      return Response.json(snapshot, { status: 201 });
    },
  });

  const session = await create(aex,
    createOptions,
    { idempotencyKey: "stable-create-request" },
  );

  assert.equal(session.id, "ses_01");
  assert.deepEqual(requests.map((request) => request.key), [
    "stable-create-request",
    "stable-create-request",
  ]);
  assert.equal(requests[0].body, requests[1].body);
  assert.equal(JSON.parse(requests[1].body).metadata.phase, "before-dispatch");
});

test("ordinary JSON responses are bounded before an advertised oversized body is polled", async () => {
  let pulls = 0;
  let cancels = 0;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async () => ({
      ok: true,
      status: 201,
      headers: new Headers({
        "content-length": String(2 * 1024 * 1024 + 1),
        "content-type": "application/json",
      }),
      body: {
        async cancel() {
          cancels += 1;
        },
        getReader() {
          pulls += 1;
          throw new Error("oversized body was polled");
        },
      },
    }),
  });

  await assert.rejects(
    create(aex, {
      model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    }),
    /Aex response exceeds 2097152 bytes/,
  );
  assert.equal(pulls, 0);
  assert.equal(cancels, 1, "the advertised oversized body is cancelled once without retrying it");
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

  const workspace = component("environment", componentBytes, {});
  const task = testTool("task", ["environment"]);
  const read = testTool("read", ["environment"]);
  const write = testTool("write", ["environment"]);
  await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [task, read, write],
    environments: { workspace },
  });

  assert.deepEqual(
    bodies[0].tools.items.map((item) => item.definition.name),
    ["task", "read", "write"],
  );
  assert.deepEqual(
    bodies[0].tools.items.map((item) => item.executor.kind),
    ["component", "component", "component"],
  );
  await assert.rejects(
    create(aex, {
      model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
      tools: [read, read],
      environments: { workspace },
    }),
    /selected twice/,
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

  await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [],
  });

  assert.deepEqual(body.tools, { items: [] });
});

test("component create composes ordinary Model, Agentloop, Tool, and Environment values", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json(snapshot, { status: 201 });
    },
  });
  const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
  const model = component("model", bytes, { dialect: "fixture" }, { metadata: { name: "fixture" } });
  const agentloop = component("agentloop", bytes, { policy: "sequential" });
  const environment = component("environment", bytes, { driver: "fixture" });
  const echo = component("tool", bytes, {
    definition: {
      name: "echo",
      input_schema: { type: "object" },
      output_schema: { type: "object" },
      contract_digest: "a".repeat(64),
    },
    descriptor: { action: "echo" },
  }, { grants: ["environment"] });
  const task = component("tool", bytes, {
    definition: {
      name: "subagents",
      input_schema: { type: "object" },
      output_schema: {},
      contract_digest: "b".repeat(64),
    },
  }, { grants: ["children"] });

  await aex.sessions.create({
    model: { component: model, provider: "fixture", name: "fixture", apiKey: "key" },
    agentloop,
    environments: { workspace: environment },
    tools: [echo, task],
  });

  assert.equal(body.component_artifacts.length, 1, "identical component bytes upload once");
  assert.equal(body.model.world, "aex:model/model@1.0.0");
  assert.equal(body.agentloop.world, "aex:agentloop/agentloop@1.0.0");
  assert.equal(body.tools.items[0].executor.kind, "component");
  assert.equal(body.tools.items[0].executor.environment, "workspace");
  assert.deepEqual(body.tools.items[1].executor.grants, ["children"]);
  assert.equal(body.tools.items[1].executor.environment, undefined);
  assert.equal(body.environments.workspace.world, "aex:environment/environment@1.0.0");
});

test("component create keeps application callback source local and binds its Tool to app()", async () => {
  let body;
  const socket = new FakeWebSocket({});
  const aex = new Aex({
    apiKey: "aex_sk_test",
    webSocketFactory() {
      queueMicrotask(() => socket.open());
      return socket;
    },
    fetch: async (input, init) => {
      if (String(input).endsWith("/v1/customer-environment/grants")) {
        return Response.json({
          url: "wss://customer-environment.example.test/connect",
          protocol: "aex.grant.component",
          expires_at: "2026-08-25T12:05:00Z",
          grant_id: "grant-component",
          observation_url:
            "https://api.aex.dev/v1/customer-environment/observations/grant-component",
          observation_token: "observation-component",
        }, { status: 201 });
      }
      body = JSON.parse(init.body);
      return Response.json(snapshot, { status: 201 });
    },
  });
  const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
  const lookup = tool(
    z.object({ id: z.string() }),
    async function lookup({ id }) {
      return { id };
    },
  ).returns(z.object({ id: z.string() }));

  await aex.sessions.create({
    model: {
      component: component("model", bytes, {}, { metadata: { name: "fixture" } }),
      provider: "fixture",
      name: "fixture",
      apiKey: "key",
    },
    agentloop: component("agentloop", bytes, {}),
    environments: { application: appComponent({ id: "component-app" }) },
    tools: [lookup],
  });

  assert.equal(socket.sent[1].registrations[0].name, "lookup");
  assert.equal(body.tools.items[0].executor.kind, "component");
  assert.equal(body.tools.items[0].executor.environment, "application");
  assert.deepEqual(body.tools.items[0].executor.grants, ["environment"]);
  assert.equal(body.environments.application.config.driver, "customer");
  assert.equal(JSON.stringify(body).includes("async function lookup"), false);
  aex.close();
});

test("create seals managed network and bounded recovery policy", async () => {
  let body;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (_input, init) => {
      body = JSON.parse(init.body);
      return Response.json(snapshot, { status: 201 });
    },
  });

  await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    network: {
      outbound: "allowlist",
      destinations: [
        { host: "datasets.example.com", ports: [443], protocol: "tls" },
        { cidr: "203.0.113.0/24", ports: [443], protocol: "tcp" },
      ],
    },
    providerRecoveryRetries: 0,
    secrets: { PROCESSOR_TOKEN: "write-only-secret" },
    children: { maxDepth: 2, maxDirectChildren: 8, maxDescendants: 32 },
  });

  assert.deepEqual(body.network, {
    outbound: "allowlist",
    destinations: [
      { host: "datasets.example.com", ports: [443], protocol: "tls" },
      { cidr: "203.0.113.0/24", ports: [443], protocol: "tcp" },
    ],
  });
  assert.equal(body.provider_recovery_retries, 0);
  assert.equal(body.client, undefined);
  assert.deepEqual(body.secrets, { PROCESSOR_TOKEN: "write-only-secret" });
  assert.deepEqual(body.children, {
    max_depth: 2,
    max_direct_children: 8,
    max_descendants: 32,
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
  const liveClient = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init) => {
      if (String(input).endsWith("/v1/sessions")) return Response.json(snapshot, { status: 201 });
      return fetch(input, init);
    },
  });
  const live = await create(liveClient, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  const result = await live.send("Give me the answer.", {
    output: z.object({ answer: z.number().int() }),
  });

  assert.deepEqual(result, { answer: 42 });
  assert.equal(outputBody.content, "Give me the answer.");
  const canonical = canonicalize(outputBody.output.schema);
  assert.equal(outputBody.output.schema_hash, createHash("sha256").update(canonical).digest("hex"));
  assert.equal(live.state, "open");
  assert.equal(live.turnState, "idle");
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
  const session = await create(aex, {
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
  const session = await create(aex, {
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
  const session = await create(aex, {
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
  const session = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });
  assert.equal(await session.send("Be concise."), "The concise answer.");
});

test("provisional retry frames never advance the durable reconnect cursor or contaminate the winner", async () => {
  const eventRequests = [];
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init = {}) => {
      const url = new URL(String(input));
      if (url.pathname === "/v1/sessions") return Response.json(snapshot, { status: 201 });
      if (url.pathname.endsWith("/messages")) {
        return Response.json(
          { session_id: "ses_01", turn_id: "turn_retry", seq: 1 },
          { status: 202 },
        );
      }
      eventRequests.push({
        after: url.searchParams.get("after"),
        lastEventId: init.headers["Last-Event-ID"],
      });
      if (eventRequests.length === 1) {
        // Provider attempt A became UNKNOWN. This frame is live-only: it deliberately has no SSE
        // id even though its payload carries an internal sequence beyond the durable high-water.
        // Reconnect must resume from the prior durable journal cursor.
        return new Response(
          "event: assistant.delta\n" +
          "data: {\"type\":\"assistant.delta\",\"seq\":99,\"session_id\":\"ses_01\",\"turn_id\":\"turn_retry\",\"agent_id\":\"root\",\"attempt_id\":\"attempt_a\",\"provisional\":true,\"text\":\"partial A\"}\n\n",
          { headers: { "content-type": "text/event-stream" } },
        );
      }
      return sse(
        {
          type: "model.attempt_superseded",
          seq: 2,
          at: "2026-08-19T10:00:01.000Z",
          session_id: "ses_01",
          turn_id: "turn_retry",
          logical_operation_id: "logical_retry",
          superseded_attempt_id: "attempt_a",
          replacement_attempt_id: "attempt_b",
          reason: "unknown",
        },
        {
          type: "assistant.message",
          seq: 3,
          at: "2026-08-19T10:00:02.000Z",
          session_id: "ses_01",
          turn_id: "turn_retry",
          agent_id: "root",
          attempt_id: "attempt_b",
          text: "replacement B",
        },
        {
          type: "turn.completed",
          seq: 4,
          at: "2026-08-19T10:00:03.000Z",
          session_id: "ses_01",
          turn_id: "turn_retry",
          stop_reason: "end_turn",
          rounds: 1,
          tool_calls: 0,
        },
      );
    },
  });
  const session = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  assert.equal(await session.send("Recover cleanly."), "replacement B");
  assert.deepEqual(eventRequests, [
    { after: "0", lastEventId: undefined },
    { after: "0", lastEventId: undefined },
  ]);
});

test("event decoder accepts exact payloads and bounds fragmented or unterminated frames", async () => {
  const fixed = JSON.stringify({ type: "model.usage", seq: 1, padding: "" });
  const exact = JSON.stringify({
    type: "model.usage",
    seq: 1,
    padding: "x".repeat(MAX_PUBLIC_EVENT_BYTES - fixed.length),
  });
  assert.equal(Buffer.byteLength(exact), MAX_PUBLIC_EVENT_BYTES);
  const frame = new TextEncoder().encode(`id:1\nevent: model.usage\ndata:${exact}\n\n`);
  const fragmented = new ReadableStream({
    start(controller) {
      controller.enqueue(frame.subarray(0, 1));
      for (let offset = 1; offset < frame.byteLength; offset += 37) {
        controller.enqueue(frame.subarray(offset, Math.min(frame.byteLength, offset + 37)));
      }
      controller.close();
    },
  });
  const events = [];
  for await (const event of parseEventStream(fragmented)) events.push(event);
  assert.equal(events.length, 1);
  assert.equal(events[0].padding.length, MAX_PUBLIC_EVENT_BYTES - fixed.length);

  for (const [wire, error] of [
    [`event:model.usage\ndata:${JSON.stringify({ type: "model.usage", seq: 1 })}\n\n`, /without an SSE id/],
    [`id:2\nevent:model.usage\ndata:${JSON.stringify({ type: "model.usage", seq: 1 })}\n\n`, /does not match/],
  ]) {
    await assert.rejects(
      async () => {
        const stream = new ReadableStream({
          start(controller) {
            controller.enqueue(new TextEncoder().encode(wire));
            controller.close();
          },
        });
        for await (const _event of parseEventStream(stream)) {
          // Durable cursor authority is the SSE id, not a JSON sequence alone.
        }
      },
      error,
    );
  }

  const over = JSON.stringify({
    type: "model.usage",
    seq: 1,
    padding: "x".repeat(MAX_PUBLIC_EVENT_BYTES + 1 - fixed.length),
  });
  await assert.rejects(
    async () => {
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode(`data:${over}\n\n`));
          controller.close();
        },
      });
      for await (const _event of parseEventStream(stream)) {
        // The +1 payload must fail before dispatch.
      }
    },
    /payload exceeds/,
  );

  for (const wire of [
    `data:${JSON.stringify({ type: "model.usage", seq: 1 })}\n`,
    "x".repeat(MAX_PUBLIC_EVENT_BYTES + 4 * 1024 + 1),
  ]) {
    await assert.rejects(
      async () => {
        const stream = new ReadableStream({
          start(controller) {
            controller.enqueue(new TextEncoder().encode(wire));
            controller.close();
          },
        });
        for await (const _event of parseEventStream(stream)) {
          // Unterminated streams never dispatch partial events.
        }
      },
      /truncated|line exceeds/,
    );
  }

  const framingOverflow = `${": keepalive\n".repeat(4 * 1024)}\n`;
  await assert.rejects(
    async () => {
      const encoded = new TextEncoder().encode(framingOverflow);
      const stream = new ReadableStream({
        start(controller) {
          for (let offset = 0; offset < encoded.byteLength; offset += 17) {
            controller.enqueue(encoded.subarray(offset, offset + 17));
          }
          controller.close();
        },
      });
      for await (const _event of parseEventStream(stream)) {
        // Aggregate non-data framing must stay inside the separate 4 KiB allowance.
      }
    },
    /frame exceeds/,
  );
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
  const session = await create(aex, {
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

test("one customer Environment socket registers client Tools before creating multiple sessions", async () => {
  const sockets = [];
  const calls = [];
  const lookup = tool(
    z.object({ id: z.string() }),
    async function lookup({ id }) {
      return { id };
    },
  );
  const update = tool(
    z.object({ id: z.string() }),
    async function update({ id }) {
      return { id };
    },
  );
  const app = appComponent({ id: "customer-backend" });
  const aex = new Aex({
    apiKey: "aex_sk_test",
    webSocketFactory(request) {
      const socket = new FakeWebSocket(request);
      sockets.push(socket);
      queueMicrotask(() => socket.open());
      return socket;
    },
    fetch: async (input, init) => {
      calls.push({ url: String(input), body: init.body === undefined ? undefined : JSON.parse(init.body) });
      if (String(input).endsWith("/v1/customer-environment/grants")) {
        return Response.json({
          url: "wss://customer-environment.example.test/connect",
          protocol: "aex.grant.short-lived",
          expires_at: "2026-08-20T12:05:00Z",
          grant_id: "grant-non-secret",
          observation_url:
            "https://api.aex.dev/v1/customer-environment/observations/grant-non-secret",
          observation_token: "observation-grant",
        }, { status: 201 });
      }
      return Response.json(snapshot, { status: 201 });
    },
  });

  await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [lookup],
    environments: { app },
  });
  await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [lookup, update],
    environments: { app },
  });

  assert.equal(sockets.length, 1);
  assert.deepEqual(sockets[0].request, {
    url: "wss://customer-environment.example.test/connect",
    protocol: "aex.grant.short-lived",
  });
  assert.equal(calls.filter((call) => call.url.endsWith("/customer-environment/grants")).length, 1);
  assert.deepEqual(calls[0].body, { client_id: "customer-backend" });
  assert.equal(sockets[0].sent[0].type, "register");
  assert.equal(sockets[0].sent[0].client_id, "customer-backend");
  assert.equal(sockets[0].sent[1].type, "register_tools");
  assert.deepEqual(sockets[0].sent[1].registrations.map((value) => value.name), ["lookup"]);
  assert.equal(sockets[0].sent[2].type, "register_tools");
  assert.deepEqual(sockets[0].sent[2].registrations.map((value) => value.name), ["update"]);
  const creates = calls.filter((call) => call.url.endsWith("/v1/sessions"));
  assert.equal(creates[0].body.client, undefined);
  assert.equal(creates[0].body.tools.items[0].executor.kind, "component");
  assert.equal(creates[0].body.tools.items[0].executor.environment, "app");
  assert.equal(creates[0].body.environments.app.config.driver, "customer");
  aex.close();
  assert.equal(sockets[0].closed, true);
});

test("customer Environment grants keep credentials out of URLs and pin observations to Aex", async () => {
  const canonical = {
    url: "wss://customer-environment.example.test/connect",
    protocol: "aex.grant.short-lived",
    expires_at: "2026-08-20T12:05:00Z",
    grant_id: "grant-non-secret",
    observation_url:
      "https://api.aex.dev/v1/customer-environment/observations/grant-non-secret",
    observation_token: "observation-secret",
  };
  const grantRequests = [];
  const grant = async (override) => new Transport(
    "aex_sk_test",
    "https://api.aex.dev",
    async (input, init) => {
      grantRequests.push({ input: String(input), redirect: init.redirect });
      return Response.json({ ...canonical, ...override }, { status: 201 });
    },
  ).customerEnvironmentGrant("customer-backend");

  assert.equal((await grant({})).observationUrl, canonical.observation_url);
  assert.deepEqual(grantRequests[0], {
    input: "https://api.aex.dev/v1/customer-environment/grants",
    redirect: "error",
  });
  await assert.rejects(
    grant({ observation_url: "https://attacker.example/collect" }),
    /unsafe customer Environment observation URL/,
  );
  await assert.rejects(
    grant({
      observation_url:
        "https://api.aex.dev/v1/customer-environment/observations/observation-secret",
    }),
    /unsafe customer Environment observation URL/,
  );
  await assert.rejects(
    grant({ url: "wss://customer-environment.example.test/connect?grant=secret" }),
    /credential-free WSS/,
  );
});

test("aborting callback readiness tears down the failed runner before a later create retries", async () => {
  const sockets = [];
  const calls = [];
  const lookup = tool(
    z.object({ id: z.string() }),
    async function lookup({ id }) {
      return { id };
    },
  );
  const app = appComponent({ id: "abort-safe-runner" });
  const aex = new Aex({
    apiKey: "aex_sk_test",
    webSocketFactory(request) {
      const socket = new FakeWebSocket(request);
      sockets.push(socket);
      return socket;
    },
    fetch: async (input, init) => {
      calls.push({ url: String(input), body: init.body });
      if (String(input).endsWith("/v1/customer-environment/grants")) {
        return Response.json({
          url: "wss://customer-environment.example.test/connect",
          protocol: "aex.grant.abort-safe",
          expires_at: "2026-08-20T12:05:00Z",
          grant_id: "grant-abort-safe",
          observation_url:
            "https://api.aex.dev/v1/customer-environment/observations/grant-abort-safe",
          observation_token: "observation-abort-safe",
        }, { status: 201 });
      }
      return Response.json(snapshot, { status: 201 });
    },
  });

  const controller = new AbortController();
  const first = create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [lookup],
    environments: { app },
  }, { signal: controller.signal });
  while (sockets.length === 0) await new Promise((resolve) => setImmediate(resolve));
  sockets[0].close();
  controller.abort(new Error("caller left"));
  await assert.rejects(first, (error) => error.name === "AbortError");
  assert.equal(sockets[0].closed, true, "the cancelled create must close its reconnecting runner");
  assert.equal(
    calls.filter((call) => call.url.endsWith("/v1/sessions")).length,
    0,
    "aborted readiness cannot create the session",
  );
  await new Promise((resolve) => setTimeout(resolve, 350));
  assert.equal(sockets.length, 1, "the failed ingress runner must not reconnect after cancellation");
  assert.equal(calls.filter((call) => call.url.endsWith("/customer-environment/grants")).length, 1);

  const second = create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [lookup],
    environments: { app },
  });
  while (sockets.length < 2) await new Promise((resolve) => setImmediate(resolve));
  sockets[1].open();
  await second;
  assert.equal(calls.filter((call) => call.url.endsWith("/customer-environment/grants")).length, 2);
  assert.equal(calls.filter((call) => call.url.endsWith("/v1/sessions")).length, 1);
  aex.close();
});

test("aborting one shared callback readiness waiter keeps the runner for another create", async () => {
  const sockets = [];
  let grants = 0;
  let sessionCreates = 0;
  const lookup = tool(z.object({ id: z.string() }), async function lookup({ id }) {
    return { id };
  });
  const app = appComponent({ id: "shared-readiness" });
  const aex = new Aex({
    apiKey: "aex_sk_test",
    webSocketFactory(request) {
      const socket = new FakeWebSocket(request);
      sockets.push(socket);
      return socket;
    },
    fetch: async (input) => {
      if (String(input).endsWith("/v1/customer-environment/grants")) {
        grants += 1;
        return Response.json({
          url: "wss://customer-environment.example.test/connect",
          protocol: "aex.grant.shared-readiness",
          expires_at: "2026-08-20T12:05:00Z",
          grant_id: "grant-shared-readiness",
          observation_url:
            "https://api.aex.dev/v1/customer-environment/observations/grant-shared-readiness",
          observation_token: "observation-shared-readiness",
        }, { status: 201 });
      }
      sessionCreates += 1;
      return Response.json(snapshot, { status: 201 });
    },
  });
  const firstController = new AbortController();
  const options = {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [lookup],
    environments: { app },
  };
  const first = create(aex, options, { signal: firstController.signal });
  const second = create(aex, options);
  while (sockets.length === 0) await new Promise((resolve) => setImmediate(resolve));
  firstController.abort(new Error("first caller left"));
  await assert.rejects(first, (error) => error.name === "AbortError");
  assert.equal(sockets[0].closed, false);
  sockets[0].open();
  await second;
  assert.equal(grants, 1);
  assert.equal(sockets.length, 1);
  assert.equal(sessionCreates, 1);
  aex.close();
});

test("closing Aex is terminal even while a customer Environment is still connecting", async () => {
  const sockets = [];
  let sessionCreates = 0;
  const lookup = tool(z.object({ id: z.string() }), async function lookup({ id }) {
    return { id };
  });
  const app = appComponent({ id: "closing-runner" });
  const aex = new Aex({
    apiKey: "aex_sk_test",
    webSocketFactory(request) {
      const socket = new FakeWebSocket(request);
      sockets.push(socket);
      return socket;
    },
    fetch: async (input) => {
      if (String(input).endsWith("/v1/customer-environment/grants")) {
        return Response.json({
          url: "wss://customer-environment.example.test/connect",
          protocol: "aex.grant.closing",
          expires_at: "2026-08-20T12:05:00Z",
          grant_id: "grant-closing",
          observation_url:
            "https://api.aex.dev/v1/customer-environment/observations/grant-closing",
          observation_token: "observation-closing",
        }, { status: 201 });
      }
      sessionCreates += 1;
      return Response.json(snapshot, { status: 201 });
    },
  });
  const creating = create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [lookup],
    environments: { app },
  });
  while (sockets.length === 0) await new Promise((resolve) => setImmediate(resolve));
  aex.close();
  await assert.rejects(creating, /Aex client is closed|closed/i);
  await assert.rejects(
    create(aex, {
      model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    }),
    /Aex client is closed/,
  );
  assert.equal(sessionCreates, 0);
});

test("one client registration can never silently alias a different closure", async () => {
  const sockets = [];
  const calls = [];
  const input = z.object({ id: z.string() });
  const first = tool(input, async ({ id }) => ({ source: "first", id }))
    .named("lookup");
  const second = tool(input, async ({ id }) => ({ source: "second", id }))
    .named("lookup");
  const app = appComponent({ id: "closure-collision" });

  const aex = new Aex({
    apiKey: "aex_sk_test",
    webSocketFactory(request) {
      const socket = new FakeWebSocket(request);
      sockets.push(socket);
      queueMicrotask(() => socket.open());
      return socket;
    },
    fetch: async (request) => {
      const url = String(request);
      calls.push(url);
      if (url.endsWith("/v1/customer-environment/grants")) {
        return Response.json({
          url: "wss://customer-environment.example.test/connect",
          protocol: "aex.grant.short-lived",
          expires_at: "2026-08-20T12:05:00Z",
          grant_id: "grant-non-secret",
          observation_url:
            "https://api.aex.dev/v1/customer-environment/observations/grant-non-secret",
          observation_token: "observation-secret",
        }, { status: 201 });
      }
      return Response.json(snapshot, { status: 201 });
    },
  });

  await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
    tools: [first],
    environments: { app },
  });
  await assert.rejects(
    create(aex, {
      model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
      tools: [second],
      environments: { app },
    }),
    /conflicts with its existing contract or handler/,
  );
  assert.equal(
    calls.filter((url) => url.endsWith("/v1/sessions")).length,
    1,
    "a colliding closure must fail before session creation",
  );
  aex.close();
});

test("storage and durable child resources keep wire details explicit", async () => {
  const requests = [];
  const file = {
    path: "/workspace/report.txt",
    kind: "file",
    bytes: 5,
    sha256: "a".repeat(64),
    modified_at_ms: Date.parse("2026-08-20T12:00:00Z"),
  };
  const object = {
    key: "outputs/report.txt",
    bytes: 5,
    sha256: "b".repeat(64),
    content_type: "text/plain",
    created_at: "2026-08-20T12:00:00Z",
    updated_at: "2026-08-20T12:00:01Z",
  };
  const child = {
    ...snapshot,
    id: "ses_child",
    parent_id: "ses_01",
    root_id: "ses_01",
    name: "research",
    depth: 1,
    state: "open",
    turn_state: "idle",
    last_seq: 1,
    created_at: "2026-08-20T12:00:00Z",
    updated_at: "2026-08-20T12:00:01Z",
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init = {}) => {
      const url = new URL(String(input));
      const body = typeof init.body === "string" ? JSON.parse(init.body) : undefined;
      requests.push({
        method: init.method,
        path: `${url.pathname}${url.search}`,
        body,
        idempotencyKey: init.headers?.["Idempotency-Key"],
      });
      if (url.pathname === "/v1/sessions" && init.method === "POST") {
        return Response.json(snapshot, { status: 201 });
      }
      if (url.pathname.endsWith("/environments/workspace") && init.method === "GET") {
        return Response.json({
          target: {
            kind: "default",
            session_id: "ses_01",
            root_id: "ses_01",
            binding_ref: "default",
          },
          state: "running",
          generation: "gen_01",
          changed_at_ms: Date.parse("2026-08-20T12:00:00Z"),
          expires_at_ms: Date.parse("2026-08-20T12:30:00Z"),
        });
      }
      if (url.pathname.endsWith("/environments/workspace/files/list")) {
        return Response.json({ data: [file], has_more: false, generation: "gen_01" });
      }
      if (url.pathname.endsWith("/environments/workspace/files/grep")) {
        return Response.json({ data: [file], has_more: false, generation: "gen_01" });
      }
      if (url.pathname.endsWith("/environments/workspace/files/stat")) return Response.json(file);
      if (url.pathname.endsWith("/environments/workspace/files/read-inline")) {
        return Response.json({ entry: file, content_base64: "aGVsbG8=" });
      }
      if (url.pathname.endsWith("/environments/workspace/files/write-inline")) return Response.json(file);
      if (url.pathname.endsWith("/storage/stat")) return Response.json(object);
      if (url.pathname.endsWith("/storage/write-inline")) return Response.json(object);
      if (url.pathname.endsWith("/storage/read-inline")) {
        return Response.json({ object, content_base64: "aGVsbG8=" });
      }
      if (url.pathname.endsWith("/storage/copy-from-environment/workspace")) {
        return Response.json(object);
      }
      if (url.pathname.endsWith("/storage/copy-to-environment/workspace")) {
        return Response.json(file);
      }
      if (url.pathname.endsWith("/children") && init.method === "POST") return Response.json(child, { status: 201 });
      if (url.pathname.endsWith("/children/ses_child") && init.method === "GET") return Response.json(child);
      if (url.pathname.endsWith("/messages")) {
        return Response.json({ session_id: "ses_child", turn_id: "turn_child", seq: 2 }, {
          status: 202,
        });
      }
      if (url.pathname.endsWith("/follow-up")) {
        return Response.json({ ...child, turn_state: "running" });
      }
      if (url.pathname.endsWith("/wait")) return Response.json(child);
      throw new Error(`unexpected request: ${init.method} ${url.pathname}`);
    },
  });
  const session = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  const sandbox = await session.sandbox.status();
  assert.equal(sandbox.state, "running");
  assert.equal(sandbox.generation, "gen_01");
  // Brain addresses Environments by the name the session declared; there is no unnamed default
  // resource, so the default sandbox must resolve to the session's one declared name.
  assert.deepEqual(session.environments, ["workspace"]);
  assert.equal(session.sandbox.environment, "workspace");
  assert.equal(
    requests.find((request) => request.method === "GET" && request.path.includes("/environments/"))
      .path,
    "/v1/sessions/ses_01/environments/workspace",
  );
  assert.throws(() => session.environment("missing"), TypeError);
  assert.equal((await session.sandbox.files.list("/workspace", {
    generation: sandbox.generation,
  })).data[0].path, file.path);
  assert.equal(new TextDecoder().decode(await session.sandbox.files.download(file.path, {
    generation: sandbox.generation,
  })), "hello");
  await session.sandbox.files.upload(file.path, "hello", {
    generation: sandbox.generation,
    overwrite: true,
  });
  await session.storage.upload(object.key, "hello", { contentType: "text/plain" });
  assert.equal(new TextDecoder().decode(await session.storage.download(object.key)), "hello");
  await session.storage.copyFromSandbox({
    environment: "workspace",
    key: object.key,
    path: file.path,
    sandboxGeneration: sandbox.generation,
    overwrite: true,
  });
  await session.storage.copyToSandbox({
    environment: "workspace",
    key: object.key,
    path: file.path,
    sandboxGeneration: sandbox.generation,
    overwrite: true,
  });
  const childHandle = await session.children.create(
    { prompt: "Research this.", name: "research", forkTurns: "3" },
    { idempotencyKey: "child-create" },
  );
  const childInfo = await childHandle.info();
  assert.equal(childInfo.parentId, "ses_01");
  assert.equal(childInfo.name, "research");
  assert.equal(childInfo.state, "open");
  assert.equal(childInfo.turnState, "idle");
  assert.equal(childInfo.shape, "1gb");
  await childHandle.send("One constraint.", { idempotencyKey: "child-message" });
  assert.equal(
    (await childHandle.followUp("Continue.", { idempotencyKey: "child-follow-up" })).turnState,
    "running",
  );
  await childHandle.wait({ timeoutMs: 250 });

  const childRequest = requests.find((request) => request.path.endsWith("/children") && request.method === "POST");
  assert.deepEqual(childRequest.body, { prompt: "Research this.", name: "research", fork_turns: "3" });
  assert.equal(childRequest.idempotencyKey, "child-create");
  assert.equal(
    requests.find((request) => request.path.endsWith("/children/ses_child/messages"))
      .idempotencyKey,
    "child-message",
  );
  assert.equal(
    requests.find((request) => request.path.endsWith("/follow-up")).idempotencyKey,
    "child-follow-up",
  );
  assert.deepEqual(
    requests.find((request) => request.path.endsWith("/storage/copy-from-environment/workspace"))
      .body,
    {
      key: object.key,
      path: file.path,
      environment_generation: "gen_01",
      overwrite: true,
    },
  );
});

test("large storage transfers bypass Brain and complete the scoped ticket", async () => {
  const content = new Uint8Array(1024 * 1024 + 1).fill(7);
  const requests = [];
  let completionAttempts = 0;
  const stored = {
    key: "large.bin",
    bytes: content.byteLength,
    sha256: "c".repeat(64),
    created_at: "2026-08-20T12:00:00Z",
    updated_at: "2026-08-20T12:00:01Z",
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init = {}) => {
      const url = new URL(String(input));
      requests.push({ method: init.method, url: String(input), body: init.body });
      if (url.pathname === "/v1/sessions") return Response.json(snapshot, { status: 201 });
      if (url.pathname.endsWith("/storage/uploads")) {
        return Response.json({
          transfer_id: "xfer_upload",
          method: "PUT",
          url: "https://objects.example.test/upload",
          headers: { "x-aex-transfer": "upload" },
          expires_at: "2026-08-20T12:05:00Z",
          max_bytes: content.byteLength,
        });
      }
      if (url.pathname.endsWith("/storage/uploads/xfer_upload/complete")) {
        completionAttempts += 1;
        if (completionAttempts === 1) {
          return Response.json({ error: { code: "unavailable", message: "retry completion" } }, {
            status: 503,
          });
        }
        return Response.json(stored);
      }
      if (url.pathname.endsWith("/storage/stat")) return Response.json(stored);
      if (url.pathname.endsWith("/storage/downloads")) {
        return Response.json({
          transfer_id: "xfer_download",
          method: "GET",
          url: "https://objects.example.test/download",
          headers: { "x-aex-transfer": "download" },
          expires_at: "2026-08-20T12:05:00Z",
          max_bytes: content.byteLength,
        });
      }
      if (url.hostname === "objects.example.test" && init.method === "PUT") return new Response(null, { status: 200 });
      if (url.hostname === "objects.example.test" && init.method === "GET") return new Response(content, { status: 200 });
      throw new Error(`unexpected request: ${init.method} ${url}`);
    },
  });
  const session = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  assert.equal((await session.storage.upload("large.bin", content)).bytes, content.byteLength);
  assert.deepEqual(await session.storage.download("large.bin"), content);
  const upload = requests.find((request) => request.url === "https://objects.example.test/upload");
  assert.equal(upload.method, "PUT");
  assert.equal(upload.body.buffer, content.buffer, "ordinary Uint8Array uploads share their input buffer");
  assert.equal(upload.body.byteOffset, content.byteOffset);
  assert.equal(upload.body.byteLength, content.byteLength);
  assert.equal(completionAttempts, 2, "the minted transfer identity makes completion retry-safe");
  assert.equal(requests.filter((request) => request.url.endsWith("/storage/uploads")).length, 1);
  assert.equal(requests.filter((request) => request.url === "https://objects.example.test/upload").length, 1);
  assert.equal(requests.some((request) => request.url.endsWith("/storage/write-inline")), false);
  assert.equal(requests.some((request) => request.url.endsWith("/storage/read-inline")), false);
});

test("streaming storage transfers keep O(1) heap and enforce length, ticket, and abort bounds", async () => {
  const bytes = 1024 * 1024 + 3;
  const first = new Uint8Array(700_000).fill(3);
  const second = new Uint8Array(bytes - first.byteLength).fill(5);
  const expected = new Uint8Array(bytes);
  expected.set(first);
  expected.set(second, first.byteLength);
  const digest = createHash("sha256").update(expected).digest("hex");
  const requests = [];
  let uploaded;
  let ticketMax = bytes;
  let downloadCancelled = false;
  let truncateDownload = false;
  const stored = {
    key: "stream.bin",
    bytes,
    sha256: digest,
    created_at: "2026-08-20T12:00:00Z",
    updated_at: "2026-08-20T12:00:01Z",
  };
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init = {}) => {
      const url = new URL(String(input));
      requests.push({ method: init.method, url: String(input), body: init.body, signal: init.signal });
      if (url.pathname === "/v1/sessions") return Response.json(snapshot, { status: 201 });
      if (url.pathname.endsWith("/storage/stat")) return Response.json(stored);
      if (url.pathname.endsWith("/storage/uploads")) {
        return Response.json({
          transfer_id: "xfer_stream_upload",
          method: "PUT",
          url: "https://objects.example.test/stream-upload",
          headers: { "content-length": String(bytes) },
          expires_at: "2026-08-20T12:05:00Z",
          max_bytes: ticketMax,
        });
      }
      if (url.pathname.endsWith("/storage/uploads/xfer_stream_upload/complete")) {
        return Response.json(stored);
      }
      if (url.pathname.endsWith("/storage/downloads")) {
        return Response.json({
          transfer_id: "xfer_stream_download",
          method: "GET",
          url: "https://objects.example.test/stream-download",
          headers: {},
          expires_at: "2026-08-20T12:05:00Z",
          max_bytes: bytes,
        });
      }
      if (url.pathname.endsWith("/stream-upload")) {
        uploaded = new Uint8Array(await new Response(init.body).arrayBuffer());
        return new Response(null, { status: 200 });
      }
      if (url.pathname.endsWith("/stream-download")) {
        return new Response(new ReadableStream({
          start(controller) {
            controller.enqueue(first);
            if (truncateDownload) controller.close();
          },
          cancel() {
            downloadCancelled = true;
          },
        }), { status: 200 });
      }
      throw new Error(`unexpected request: ${init.method} ${url}`);
    },
  });
  const session = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });

  let streamCalls = 0;
  const source = {
    bytes,
    sha256: digest,
    stream() {
      streamCalls += 1;
      return new ReadableStream({
        start(controller) {
          controller.enqueue(first);
          controller.enqueue(second);
          controller.close();
        },
      });
    },
  };
  assert.equal((await session.storage.upload("stream.bin", source)).bytes, bytes);
  assert.equal(streamCalls, 1);
  assert.deepEqual(uploaded, expected);
  const objectPut = requests.find((request) => request.url.endsWith("/stream-upload"));
  assert.ok(objectPut.body instanceof ReadableStream);

  ticketMax = bytes - 1;
  await assert.rejects(session.storage.upload("too-large.bin", source), /exceeds its transfer ticket/);
  assert.equal(streamCalls, 1, "a rejected ticket does not open the source stream");

  ticketMax = bytes;
  const oversized = {
    bytes,
    sha256: digest,
    stream() {
      return new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array(bytes + 1));
          controller.close();
        },
      });
    },
  };
  await assert.rejects(session.storage.upload("oversized.bin", oversized), /declared byte limit/);

  let abortedInlineStreams = 0;
  const preAborted = new AbortController();
  preAborted.abort("caller already left");
  await assert.rejects(
    session.storage.upload("aborted-inline.bin", {
      bytes: 1,
      sha256: "0".repeat(64),
      stream() {
        abortedInlineStreams += 1;
        return new ReadableStream();
      },
    }, { signal: preAborted.signal }),
    (error) => error?.name === "AbortError",
  );
  assert.equal(abortedInlineStreams, 0, "an already-aborted upload never opens its source");

  const abort = new AbortController();
  const download = await session.storage.downloadStream("stream.bin", { signal: abort.signal });
  const reader = download.getReader();
  assert.deepEqual((await reader.read()).value, first);
  abort.abort("stop download");
  await assert.rejects(reader.read(), (error) => error?.name === "AbortError");
  assert.equal(downloadCancelled, true);

  truncateDownload = true;
  await assert.rejects(
    session.storage.download("stream.bin"),
    /produced 700000 bytes; expected 1048579/,
    "a clean but truncated object response cannot be mistaken for a complete download",
  );
});

test("session capacity can suspend and resume before non-destructive end", async () => {
  const paths = [];
  let strictDeleteAttempts = 0;
  let deletionPolls = 0;
  const aex = new Aex({
    apiKey: "aex_sk_test",
    fetch: async (input, init) => {
      const url = new URL(String(input));
      paths.push(`${init.method} ${url.pathname}${url.search}`);
      if (url.pathname === "/v1/sessions") return Response.json(snapshot, { status: 201 });
      if (url.pathname.endsWith("/suspend")) {
        return Response.json({ ...snapshot, state: "suspended" });
      }
      if (url.pathname.endsWith("/resume")) {
        return Response.json({ ...snapshot, state: "open" });
      }
      if (url.pathname.endsWith("/retention")) {
        const body = JSON.parse(init.body);
        assert.deepEqual(body, {
          retain_until: "2028-01-01T00:00:00.000Z",
          allow_shorten: false,
        });
        return Response.json({ ...snapshot, retain_until: body.retain_until });
      }
      if (url.pathname.endsWith("/end")) {
        return Response.json({ ...snapshot, state: "ending" }, { status: 202 });
      }
      if (init.method === "DELETE") {
        strictDeleteAttempts += 1;
        if (strictDeleteAttempts === 2) throw new TypeError("response lost after delete");
        if (strictDeleteAttempts === 4) {
          return Response.json({ error: { code: "unavailable", message: "try again" } }, {
            status: 503,
          });
        }
        return new Response(null, {
          status: 202,
          headers: { Location: "/v1/sessions/ses_01/deletion", "Retry-After": "0" },
        });
      }
      if (url.pathname.endsWith("/deletion")) {
        deletionPolls += 1;
        if (deletionPolls === 1) {
          return Response.json({ error: { code: "unavailable", message: "try again" } }, {
            status: 503,
            headers: { "Retry-After": "0" },
          });
        }
        return Response.json({
          object: "session.deletion",
          session_id: "ses_01",
          state: deletionPolls === 2 ? "retrying" : "succeeded",
          requested_at_ms: 1,
          updated_at_ms: deletionPolls + 1,
          completed_at_ms: deletionPolls === 2 ? null : 3,
        }, { headers: deletionPolls === 2 ? { "Retry-After": "0" } : {} });
      }
      throw new Error(`unexpected request: ${init.method} ${url.pathname}`);
    },
  });

  const queued = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });
  assert.equal((await queued.suspend()).state, "suspended");
  assert.equal((await queued.resume()).state, "open");
  assert.equal((await queued.setRetention("2028-01-01T00:00:00Z")).retainUntil, "2028-01-01T00:00:00.000Z");
  assert.equal((await queued.end()).state, "ending");
  await queued.delete({ queue: true });
  assert.equal(queued.state, "deleting");

  const confirmed = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });
  await confirmed.delete();
  assert.equal(confirmed.state, "deleted");

  const serverRetried = await create(aex, {
    model: { provider: "anthropic", name: "claude-sonnet-5", apiKey: "sk-ant-test" },
  });
  await serverRetried.delete({ queue: true });
  assert.equal(serverRetried.state, "deleting");
  assert.ok(paths.includes("DELETE /v1/sessions/ses_01"));
  assert.equal(strictDeleteAttempts, 5, "network and 5xx failures retry the same acceptance");
  assert.equal(paths.filter((path) => path === "GET /v1/sessions/ses_01/deletion").length, 3);
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

class FakeWebSocket {
  constructor(request) {
    this.request = request;
    this.sent = [];
    this.listeners = new Map();
    this.closed = false;
  }

  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  send(value) {
    const frame = JSON.parse(String(value));
    this.sent.push(frame);
    if (frame.type === "register") {
      queueMicrotask(() => this.emit("message", { data: JSON.stringify({ type: "ready", epoch: 7 }) }));
    } else if (frame.type === "register_tools") {
      queueMicrotask(() => this.emit("message", {
        data: JSON.stringify({ type: "registered", epoch: 7, batch_id: frame.batch_id }),
      }));
    }
  }

  close() {
    this.closed = true;
    this.emit("close", {});
  }

  open() {
    this.emit("open", {});
  }

  emit(type, event) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
}

function intrinsicTool(name) {
  return officialTool({
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

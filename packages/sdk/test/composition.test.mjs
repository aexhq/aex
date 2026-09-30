import assert from "node:assert/strict";
import test from "node:test";
import { z } from "zod";
import { Aex, agentloop, brainEnv, clientBrowser, tool } from "../dist/index.js";

const loop = agentloop({ implementation: { type: "brain_component", entrypoint: "turn", id: "a".repeat(64) } });
const model = { provider: "openai", name: "model", apiKey: "private-model-key" };
const read = tool({ name: "read", description: "Read", input: z.object({}), options: z.object({ count: z.number().default(3) }),
  run: async (_, ctx) => ctx.finish(ctx.options.count) });

test("application placement compiles ordinary configured Tools without a host, catalog or binding", async () => {
  const calls = [];
  const aex = new Aex({ apiKey: "account-key", fetch: async (url, init) => {
    calls.push({ url, request: JSON.parse(init.body) });
    return Response.json({ session_id: "s", status: "idle", last_sequence: 1 });
  } });
  const env = aex.environments.application({ name: "app", endpoint: "https://customer.example/tools", credential: "application-credential" });
  await aex.sessions.create({ model, agentloop: loop({ env: brainEnv({ name: "brain" }) }), tools: [read({ env })] });
  assert.equal(calls.length, 2);
  assert.ok(calls.shift().url.endsWith("/v1/brain-env/prepare"));
  assert.ok(calls[0].url.endsWith("/v1/sessions"));
  assert.deepEqual(calls[0].request.environments.find(e => e.name === "app"), {
    name: "app", lifecycle: "automatic", driver: "http", url: "https://api.aex.dev/environments/application", credential: "application-credential",
    configuration: { type: "application", endpoint: "https://customer.example/tools", timeoutMs: 30000 },
  });
  assert.deepEqual(calls[0].request.tools[0].placements.app.implementation.options, { count: 3 });
  assert.equal(calls[0].request.tools[0].placements.app.implementation.type, "application_tool");
  assert.equal(JSON.stringify(calls).includes('"binding"'), false);
});

test("backend grants prepare the composition; the browser creates through the ordinary session API", async () => {
  const access = { id: "grant", token: "client-capability", expires_at: 2000000000, credentials: { hostId: "tab-host", token: "host-capability" } };
  let prepared;
  const backend = new Aex({ apiKey: "private-account-key", fetch: async (url, init) => {
    if (url.endsWith("/v1/brain-env/prepare")) return new Response(null, { status: 204 });
    assert.ok(url.endsWith("/v1/clients")); prepared = JSON.parse(init.body);
    return Response.json(access);
  } });
  const env = clientBrowser({ name: "editor" });
  const options = { model, agentloop: loop({ env: brainEnv({ name: "brain" }) }), tools: [read({ env })] };
  assert.deepEqual(await backend.clients.grant({ origin: "https://customer.example", session: options }), access);
  assert.equal(prepared.session.model.api_key, "private-model-key");
  assert.equal(prepared.session.environments.find(e => e.name === "editor").host_id, "authorized-client");
  const calls = [];
  const browser = new Aex({ clientAccess: access, fetch: async (url, init) => {
    const token = new Headers(init.headers).get("authorization");
    calls.push({ url, token, body: init.body });
    if (url.endsWith("/commands")) {
      assert.equal(token, "Bearer host-capability");
      return new Response(new ReadableStream({ start(controller) {
        init.signal.addEventListener("abort", () => controller.close(), { once: true });
      } }), { headers: { "content-type": "text/event-stream" } });
    }
    assert.equal(token, "Bearer client-capability");
    return Response.json({ session_id: "s", status: "idle", last_sequence: 1 });
  } });
  await assert.rejects(browser.sessions.create(options), /omit apiKey/);
  await browser.sessions.create({ ...options, model: { provider: "openai", name: "model" } });
  assert.equal(calls.length, 3);
  assert.ok(calls[0].url.endsWith("/v1/brain-env/prepare"));
  const created = JSON.parse(calls[2].body);
  assert.equal(created.model.api_key, access.token);
  assert.equal(created.environments.find(e => e.name === "editor").host_id, "tab-host");
  assert.equal(JSON.stringify(calls).includes("private-"), false);
  await browser.close();
  await backend.close();
});

test("failed Environment preparation prevents session creation", async () => {
  const calls = [];
  const aex = new Aex({ apiKey: "key", fetch: async (url) => {
    calls.push(new URL(url).pathname);
    return Response.json({ code: "preparation_failed", message: "runtime unavailable", retryable: false }, { status: 409 });
  } });
  await assert.rejects(aex.sessions.create({ model, agentloop: loop({ env: brainEnv({ name: "brain" }) }) }), error => error.code === "preparation_failed");
  assert.deepEqual(calls, ["/v1/brain-env/prepare"]);
  await aex.close();
});

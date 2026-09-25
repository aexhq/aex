import assert from "node:assert/strict";
import test from "node:test";
import { z } from "zod";
import { Aex, AexSessionHandle, Brain, SessionHandle } from "../dist/index.js";

test("wrapping preserves Environment services, live state and host cleanup", async () => {
  let cleanup = 0;
  const client = new Brain({ baseUrl: "https://brain.example", fetch: async (_, init) => init.method === "DELETE"
    ? new Response(null, { status: 204 })
    : Response.json({ session_id: "s", status: "ended", last_sequence: 7 }) });
  const raw = new SessionHandle(client, { id: "s", status: "idle", lastSequence: 0 }, () => cleanup++);
  const session = new AexSessionHandle(raw);
  assert.equal(session.environments, raw.environments);
  raw.state = { id: "s", status: "running", lastSequence: 4 };
  assert.equal(session.state, raw.state);
  await session.end();
  assert.equal(session.state, raw.state);
  assert.equal(session.state.status, "ended");
  assert.equal(cleanup, 1);
  await session.delete();
  assert.equal(cleanup, 2);
  await client.close();
});

test("Aex close aborts a typed answer read and blocks later sends", async () => {
  const reading = Promise.withResolvers();
  let messages = 0;
  const client = new Aex({ apiKey: "key", fetch: async (url, init) => {
    if (url.endsWith("/messages")) {
      messages++;
      return Response.json({ session_id: "s", status: "idle", last_sequence: 3 });
    }
    if (url.includes("/events?")) {
      reading.resolve();
      return new Promise((_, reject) => init.signal.addEventListener("abort", () => reject(init.signal.reason), { once: true }));
    }
    return Response.json({ session_id: "s", status: "idle", last_sequence: 0 });
  } });
  const session = await client.sessions.get("s");
  const pending = assert.rejects(session.send("Extract", { output: { type: z.string() } }), { name: "AbortError" });
  await reading.promise;
  await client.close();
  await pending;
  await assert.rejects(session.send("Later"), { name: "AbortError" });
  assert.equal(messages, 1);
});

test("typed sends exclude submit and release ownership after validation", async () => {
  const validating = Promise.withResolvers();
  const release = Promise.withResolvers();
  const client = new Aex({ apiKey: "key", fetch: async (url, init) => {
    if (url.endsWith("/messages")) return new Headers(init.headers).get("prefer") === "respond-async"
      ? Response.json(4, { status: 202 })
      : Response.json({ session_id: "s", status: "idle", last_sequence: 3 });
    if (url.includes("/events?")) return Response.json({ events: [
      { sequence: 1, recorded_at_ms: 1, event_type: "turn_started", data: {} },
      { sequence: 2, recorded_at_ms: 1, event_type: "output_emitted", origin: { kind: "agentloop", sequence: 1 }, data: { type: "assistant_message", message: '"answer"' } },
      { sequence: 3, recorded_at_ms: 1, event_type: "turn_ended", data: {} },
    ], next_cursor: 3 });
    return Response.json({ session_id: "s", status: "idle", last_sequence: 0 });
  } });
  const session = await client.sessions.get("s");
  const pending = session.send("Extract", { output: { type: z.string().refine(async () => { validating.resolve(); await release.promise; return true; }) } });
  await validating.promise;
  await assert.rejects(session.submit("Overlap"), /exclusive sends/);
  release.resolve();
  assert.equal(await pending, "answer");
  assert.equal(await session.submit("Later"), 4);
  await assert.rejects(session.submit("Typed", { output: { type: z.string() } }), /hosted Agentloop/);
  await client.close();
});

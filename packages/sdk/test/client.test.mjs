import assert from "node:assert/strict";
import test from "node:test";
import { Aex, Brain, tool, hostEnv, brainEnv } from "../dist/index.js";
import * as upstream from "@aexhq/brain";
import { z } from "zod";

test("attachment upload sends bytes and immutable expiry through Brain's transport", async () => {
  const bytes = new TextEncoder().encode("%PDF-1.7\nfixture");
  const attachment = { id: "att_one", media: { type: "file", media_type: "application/pdf", url: "https://api.aex.dev/v1/attachments/att_one/content?token=read_one" }, expires_at: 2000000000 };
  const requests = [];
  const client = new Aex({ apiKey: "customer-key", fetch: async (url, init) => {
    requests.push({ url, init });
    return init.method === "DELETE" ? new Response(null, { status: 204 }) : Response.json(attachment, { status: 201 });
  } });
  assert.deepEqual(await client.attachments.upload("session", bytes, { contentType: "application/pdf", expiresAt: attachment.expires_at, idempotencyKey: "publish-once" }), attachment);
  const { url, init } = requests[0];
  assert.equal(url, "https://api.aex.dev/v1/sessions/session/attachments");
  assert.deepEqual(new Uint8Array(init.body), bytes);
  const headers = new Headers(init.headers);
  assert.equal(headers.get("authorization"), "Bearer customer-key");
  assert.equal(headers.get("content-type"), "application/pdf");
  assert.equal(headers.get("x-aex-expires-at"), "2000000000");
  assert.equal(headers.get("idempotency-key"), "publish-once");
  assert.throws(() => client.attachments.upload("session", bytes, { contentType: "application/pdf", expiresAt: 1.5, idempotencyKey: "bad" }), /Unix seconds/);
  await client.attachments.delete("session", attachment.id);
  assert.equal(requests.at(-1).url, "https://api.aex.dev/v1/sessions/session/attachments/att_one");
});

test("Aex sessions inherit typed structured output from the pinned Brain SDK", async () => {
  const client = new Aex({ apiKey: "customer-key", fetch: async (url, init) => {
    if (url.endsWith("/messages")) {
      assert.match(JSON.parse(init.body).input.message, /For this response only/);
      return Response.json({ session_id: "s", status: "idle", last_sequence: 3 });
    }
    if (url.includes("/events?")) return Response.json({ events: [
      { sequence: 1, recorded_at_ms: 1, event_type: "turn_started", data: {} },
      { sequence: 2, recorded_at_ms: 1, event_type: "output_emitted", origin: { kind: "agentloop", sequence: 1 }, data: { type: "assistant_message", message: '{"age":37}' } },
      { sequence: 3, recorded_at_ms: 1, event_type: "turn_ended", data: { result: null } },
    ], next_cursor: 3 });
    return Response.json({ session_id: "s", status: "idle", last_sequence: 0 });
  } });
  const session = await client.sessions.get("s");
  assert.ok(session instanceof upstream.SessionHandle);
  assert.deepEqual(await session.send("Extract age", { output: { type: z.object({ age: z.number() }) } }), { age: 37 });
});

test("Aex preserves Brain and its extension identities and uses the account API", async () => {
  assert.equal(Brain, upstream.Brain);
  assert.equal(tool, upstream.tool);
  assert.equal(hostEnv, upstream.hostEnv);
  assert.equal(brainEnv, upstream.brainEnv);
  const requests=[];
  const client = new Aex({apiKey:"customer-key", fetch:async(url,init)=>{
    requests.push({url,init});
    return Response.json({id:"account"});
  }});
  assert.ok(client instanceof upstream.Brain);
  assert.equal((await client.account.get()).id,"account");
  assert.equal(requests[0].url,"https://api.aex.dev/v1/account");
  assert.equal(new Headers(requests[0].init.headers).get("authorization"),"Bearer customer-key");
});

test("account sessions and login exchange use the same HTTP API", async () => {
  const requests = [];
  const fetch = async (url, init) => { requests.push({ url, init }); return Response.json({ token: "account" }); };
  const client = new Aex({ accountToken: "account-session", fetch });
  await client.keys.list(); await client.keys.create({ name: "app" }); await client.keys.update("key_id", { name: "renamed" }); await client.keys.delete("key_id");
  await client.account.usage(); await client.account.authorizeLogin({ code_challenge: "challenge", redirect_uri: "http://127.0.0.1:1234/callback" }); await client.account.logout();
  for (const request of requests) assert.equal(new Headers(request.init.headers).get("authorization"), "Bearer account-session");
  await Aex.exchangeLogin({ code: "code", code_verifier: "verifier", redirect_uri: "http://127.0.0.1:1234/callback" }, { fetch });
  assert.equal(requests.at(-1).url, "https://api.aex.dev/v1/auth/exchange");
  assert.equal(new Headers(requests.at(-1).init.headers).has("authorization"), false);
});

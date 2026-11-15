import assert from "node:assert/strict";
import test from "node:test";
import { Pi, Aex, AexSessionHandle, Brain, Tool, ManagedTool, Model, ApiKey } from "../dist/index.js";
import * as upstream from "@aexhq/brain";
import { z } from "zod";

test("managed provider discovery preserves the account's installed operations and profiles",async()=>{
  const catalog={providers:{sandbox:{operations:["create","exec"],profiles:{analysis:300000}}}};
  const client=new Aex({apiKey:"key",fetch:async url=>{assert.equal(new URL(url).pathname,"/tools");return Response.json(catalog);}});assert.deepEqual(await client.tools(),catalog);
});

test("HTTP catalogs and scoped upload grants keep account authority on the backend", async () => {
  const calls = [];
  const client = new Aex({ apiKey: "backend-key", maxCostMicroUsd: 1000,
    fetch: async (url, init) => { calls.push({ url: String(url), init }); return Response.json({}); } });
  await client.tools();
  await client.attachments.limits();
  await client.attachments.grant("session/a", { content_type: "application/pdf", bytes: 20 * 1024 * 1024 },
    { idempotencyKey: "upload-once", downloadBudgetBytes: 40 * 1024 * 1024, maxCostMicroUsd: 2000 });
  await client.attachments.get("session/a", "attachment/b");
  assert.deepEqual(calls.map(({ url }) => new URL(url).pathname), [
    "/tools", "/v1/attachments/limits", "/v1/sessions/session%2Fa/attachment-grants",
    "/v1/sessions/session%2Fa/attachments/attachment%2Fb",
  ]);
  const grant = calls[2].init;
  assert.equal(grant.method, "POST");
  assert.deepEqual(JSON.parse(grant.body), { content_type: "application/pdf", bytes: 20971520 });
  const headers = new Headers(grant.headers);
  assert.equal(headers.get("authorization"), "Bearer backend-key");
  assert.equal(headers.get("idempotency-key"), "upload-once");
  assert.equal(headers.get("x-aex-max-cost-micro-usd"), "2000");
  assert.equal(headers.get("x-aex-download-budget-bytes"), "41943040");
  assert.throws(() => client.attachments.grant("s", { content_type: "application/pdf", bytes: 1 },
    { idempotencyKey: "invalid", downloadBudgetBytes: -1 }), /safe integer/);
});

test("prepaid ceilings and download allowances are explicit headers and billing mutations carry stable keys", async () => {
  const calls=[];
  const client=new Aex({apiKey:"key",maxCostMicroUsd:1000,fetch:async (url,init)=>{calls.push({url:String(url),init}); return Response.json({});}});
  await client.request("POST","/sessions/s/events",{type:"user.message",text:"run",key:"turn"},"turn");
  assert.equal(new Headers(calls[0].init.headers).get("x-aex-max-cost-micro-usd"),"1000");
  await client.attachments.upload("s",new Uint8Array([1]),{contentType:"image/png",idempotencyKey:"image",maxCostMicroUsd:2000,downloadBudgetBytes:8});
  assert.equal(new Headers(calls[1].init.headers).get("x-aex-max-cost-micro-usd"),"2000");
  assert.equal(new Headers(calls[1].init.headers).get("x-aex-download-budget-bytes"),"8");
  await client.billing.topup({amount_cents:1000},"checkout-once");
  assert.equal(new Headers(calls[2].init.headers).get("idempotency-key"),"checkout-once");
  assert.deepEqual(JSON.parse(calls[2].init.body),{amount_cents:1000});
  assert.throws(()=>new Aex({apiKey:"key",maxCostMicroUsd:0.1}),/safe integer/);
  assert.throws(()=>client.attachments.upload("s",new Uint8Array([1]),{contentType:"image/png",idempotencyKey:"image",downloadBudgetBytes:-1}),/safe integer/);
});

test("client close aborts an attachment upload and rejects later work", async () => {
  const entered = Promise.withResolvers();
  let requests = 0;
  const client = new Aex({ apiKey: "customer-key", fetch: async (_, { signal }) => {
    requests++;
    entered.resolve();
    return new Promise((_, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
  } });
  const upload = () => client.attachments.upload("session", new Uint8Array([1]), { contentType: "image/png", idempotencyKey: "image-once" });
  const rejected = assert.rejects(upload(), { name: "AbortError" });
  await entered.promise;
  await client.close();
  await rejected;
  await assert.rejects(upload(), { name: "AbortError" });
  await assert.rejects(client.account.get(), { name: "AbortError" });
  assert.equal(requests, 1);
  assert.equal(client.close(), client.close());
});

test("attachment upload sends bytes and immutable expiry through Brain's transport", async () => {
  const bytes = new TextEncoder().encode("%PDF-1.7\nfixture");
  const attachment = { id: "att_one", media: { type: "file", media_type: "application/pdf", url: "https://api.aex.dev/v1/attachments/att_one/content?token=read_one" }, expires_at: 2000000000 };
  const requests = [];
  const client = new Aex({ apiKey: "customer-key", fetch: async (url, init) => {
    requests.push({ url: String(url), init });
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

for(const method of ["create","get"])test(`Aex ${method} handles own typed corrections and preserve account headers`,async()=>{
  const records=[],messages=[];let inputId;
  const state={id:"s",harness:"pi",config:{model:"openai/gpt-4.1-mini",tools:[]},status:"idle",descriptors:[]};
  const add=(type,data)=>{const id=String(records.length+1);const event={id,cursor:`opaque/${id}`,type,time:"2026-10-20T00:00:00Z",data,input_id:inputId};records.push(event);return event;};
  const client=new Aex({apiKey:"customer-key",maxCostMicroUsd:1000,fetch:async(url,init)=>{
    url=String(url);assert.equal(new Headers(init.headers).get("authorization"),"Bearer customer-key");assert.equal(new Headers(init.headers).get("x-aex-max-cost-micro-usd"),"1000");
    if(url.endsWith("/events") && init.method==="POST"){
      const input=JSON.parse(init.body);messages.push(input.text);const accepted=add("user.message",input);inputId=accepted.id;add("assistant.message",{text:messages.length===1?"not JSON":'{"age":37}'});add("session.status",{status:"idle"});return Response.json(accepted);
    }
    if(url.includes("/events?")){const after=new URL(url).searchParams.get("after");const position=after?records.findIndex(event=>event.cursor===after)+1:0;const events=records.slice(position);return Response.json({events,next_cursor:events.at(-1)?.cursor??after});}
    return Response.json(state);
  }});
  const session=method==="get"?await client.sessions.get("s"):await client.sessions.create({harness: Pi(),model:Model("openai/gpt-4.1-mini",ApiKey("model-key"))});
  assert.ok(session instanceof AexSessionHandle);assert.deepEqual(await session.send("Extract age",{output:{type:z.object({age:z.number()})}}),{age:37});assert.match(messages[0],/Extract age[\s\S]*For this response only/);assert.match(messages[1],/Validation feedback/);assert.equal((await session.outcome("1")).output,"not JSON");assert.equal((await session.send("Chat")).session.status,"idle");assert.equal(messages[2],"Chat");await client.close();
});

test("Aex preserves Brain and its extension identities and uses the account API", async () => {
  assert.equal(Brain, upstream.Brain);
  assert.equal(Tool, upstream.Tool);
  assert.equal(ManagedTool, upstream.ManagedTool);
  const requests=[];
  const client = new Aex({apiKey:"customer-key", fetch:async(url,init)=>{
    requests.push({url:String(url),init});
    return Response.json({id:"account"});
  }});
  assert.equal(client instanceof upstream.Brain, false);
  assert.equal((await client.account.get()).id,"account");
  assert.equal(requests[0].url,"https://api.aex.dev/v1/account");
  assert.equal(new Headers(requests[0].init.headers).get("authorization"),"Bearer customer-key");
});

test("account sessions and login exchange use the same HTTP API", async () => {
  const requests = [];
  const fetch = async (url, init) => { requests.push({ url: String(url), init }); return Response.json({ token: "account" }); };
  const client = new Aex({ accountToken: "account-session", fetch });
  await client.keys.list(); await client.keys.create({ name: "app" }); await client.keys.update("key_id", { name: "renamed" }); await client.keys.delete("key_id");
  await client.account.usage(); await client.account.authorizeLogin({ code_challenge: "challenge", redirect_uri: "http://127.0.0.1:1234/callback" }); await client.account.logout();
  for (const request of requests) assert.equal(new Headers(request.init.headers).get("authorization"), "Bearer account-session");
  await Aex.exchangeLogin({ code: "code", code_verifier: "verifier", redirect_uri: "http://127.0.0.1:1234/callback" }, { fetch });
  assert.equal(requests.at(-1).url, "https://api.aex.dev/v1/auth/exchange");
  assert.equal(new Headers(requests.at(-1).init.headers).has("authorization"), false);
});

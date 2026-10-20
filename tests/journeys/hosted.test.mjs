import assert from "node:assert/strict";
import test from "node:test";
import { once } from "node:events";
import { toolServer } from "../../services/tools/server.mjs";
import { Applications } from "../../services/tools/application.mjs";
import { RemoteJournal,controller } from "../../services/tools/common.mjs";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";
import { z } from "zod";
import { fixture,sdk,until,listen } from "./fixture.mjs";

for(const harness of ["pi","codex","opencode"])test(`installed SDK and ${harness}: tenant credentials, callbacks, inspection, receipts and deletion`,{timeout:120000},async t=>{
  const f=await fixture(t),a=await f.account(),b=await f.account();let calls=0;
  const tools=[sdk.Tool({name:"lookup",description:"Look up",input_schema:{type:"object",properties:{id:{type:"string"}},required:["id"],additionalProperties:false},async run(args,ctx){calls++;assert.equal(ctx.scope.tenant_id,a.account);assert.notEqual(ctx.scope.session_id,args.session_id);await ctx.progress({ready:true});return {id:args.id,private:"owned callback"};}})];
  const session=await a.api.sessions.create({harness,model:sdk.Model("openai/gpt-4.1-mini",sdk.ApiKey(a.modelKey)),instructions:"tenant A private context",tools});
  const other=await b.api.sessions.create({harness,model:sdk.Model("openai/gpt-4.1-mini",sdk.ApiKey(b.modelKey)),instructions:"tenant B private context"});
  const [first,second]=await Promise.all([session.send('CALL runtime_lookup {"id":"42"}',{idempotencyKey:"same-key"}),other.send("hello from B",{idempotencyKey:"same-key"})]);assert.equal(first.output,"tool handled");assert.match(second.output,/hello from B/);assert.equal(calls,1);
  assert.ok(f.model.keys.includes(`Bearer ${a.modelKey}`));assert.ok(f.model.keys.includes(`Bearer ${b.modelKey}`));assert.ok(!JSON.stringify(f.model.requests).includes(a.modelKey));
  assert.equal((await fetch(`${f.url}/sessions/${session.id}`,{headers:{authorization:`Bearer ${b.token}`}})).status,404);
  assert.equal((await fetch(`${f.url}/sessions/${session.id}`,{headers:{authorization:`Bearer ${a.token}`,"x-aex-tenant":b.account}})).status,401);
  const count=f.model.requests.length;assert.equal((await session.send('CALL runtime_lookup {"id":"42"}',{idempotencyKey:"same-key"})).input_id,first.input_id);assert.equal(f.model.requests.length,count);assert.equal(calls,1);
  const transcript=await session.transcript();assert.ok(transcript.some(event=>event.type==="tool.result"));assert.ok(transcript.some(event=>event.type==="model.request"));assert.ok(!JSON.stringify(transcript).includes(a.modelKey));
  await until(()=>a.api.account.modelUsage(session.id),usage=>usage.reported_input_tokens>0,"durable usage receipts");
  const inspecting=await a.api.sessions.create({harness,model:sdk.Model("openai/gpt-4.1-mini",sdk.ApiKey(a.modelKey)),tools:[sdk.ManagedTool("session",{operations:["inspect","history"]})]});assert.equal((await inspecting.send("CALL session_inspect {}")).output,"tool handled");assert.ok(!JSON.stringify(f.model.requests).includes(a.modelKey));await inspecting.delete();
  await session.delete();await session.delete();assert.equal((await fetch(`${f.url}/sessions/${session.id}`,{headers:{authorization:`Bearer ${a.token}`}})).status,404);assert.equal(calls,1);assert.ok((await a.api.account.modelUsage(session.id)).reported_input_tokens>0);await other.delete();
});

test("native Pi: typed corrections, turn admission, stream revocation, restart and paired restore",{timeout:180000},async t=>{
  const f=await fixture(t),a=await f.account();const options={harness:"pi",model:sdk.Model("openai/gpt-4.1-mini",sdk.ApiKey(a.modelKey)),instructions:"preserved native conversation"};const session=await a.api.sessions.create(options);
  f.model.answers.push('"wrong"','{"age":37}');assert.deepEqual(await session.send("Extract",{output:{type:z.object({age:z.number()})},idempotencyKey:"typed-once"}),{age:37});
  const receipts=await until(()=>a.api.account.modelUsage(session.id),usage=>usage.reported_input_tokens>=24,"typed usage");
  const before=f.model.requests.length;assert.deepEqual(await session.send("Extract",{output:{type:z.object({age:z.number()})},idempotencyKey:"typed-once"}),{age:37});assert.equal(f.model.requests.length,before);
  const sweep=[];for(let index=0;index<8;index++)sweep.push(await a.api.sessions.create(options));
  for(const concurrency of [1,2,4,8]) {
    await f.meter();f.model.delay=250;
    const responses=await Promise.all(sweep.slice(0,concurrency).map((entry,index)=>fetch(`${f.url}/sessions/${entry.id}/events`,{method:"POST",headers:{authorization:`Bearer ${a.token}`,"content-type":"application/json","x-aex-max-cost-micro-usd":"1000000"},body:JSON.stringify({type:"user.message",text:"x".repeat(1024),key:`sweep-${concurrency}-${index}`})})));
    assert.equal(responses.filter(response=>response.status===202).length,Math.min(concurrency,2));assert.ok(responses.every(response=>[202,503].includes(response.status)),JSON.stringify(responses.map(response=>response.status)));
    for(const [index,response] of responses.entries())if(response.status===202){const accepted=await response.json();await sweep[index].outcome(accepted.id,{after:accepted.cursor});}
    await f.operate({action:"maintain"});
  }
  f.model.delay=0;for(const entry of sweep)await entry.delete();
  const stream=await fetch(`${f.url}/sessions/${session.id}/events`,{headers:{authorization:`Bearer ${a.token}`,accept:"text/event-stream"}});assert.equal(stream.status,200);const reader=stream.body.getReader();await reader.read();await f.operate({action:"revoke_key",key:a.key});await Promise.race([(async()=>{while(!(await reader.read()).done){}})(),new Promise((_,reject)=>setTimeout(()=>reject(new Error("revocation did not close stream")),3000))]);assert.equal((await fetch(`${f.url}/sessions/${session.id}`,{headers:{authorization:`Bearer ${a.token}`}})).status,401);
  const backup=await f.backup();await f.restore(backup);assert.equal((await fetch(`${f.url}/sessions/${session.id}`,{headers:{authorization:`Bearer ${a.token}`}})).status,401);const replacement=await f.operate({action:"issue_key",account:a.account});const reopened=f.client({apiKey:replacement.token,maxCostMicroUsd:1000000});const handle=await reopened.sessions.get(session.id);assert.equal((await handle.send("continue after paired restore")).session.status,"idle");assert.ok(f.model.requests.some(request=>JSON.stringify(request).includes("preserved native conversation")));
  const restored=await reopened.account.modelUsage(session.id);assert.ok(restored.charged_micro_usd>=receipts.charged_micro_usd);await handle.delete();
});

test("Chromium: explicit user gesture attaches DOM callbacks, scoped grants reconnect and expiry closes streams",{timeout:120000},async t=>{
  const f=await fixture(t),a=await f.account(),bundle=await readFile("artifacts/browser-journey.js");
  const frontend=await listen((req,res)=>{if(req.url==="/app.js")res.writeHead(200,{"content-type":"text/javascript"}).end(bundle);else res.writeHead(200,{"content-type":"text/html"}).end('<button id="attach">Attach</button><div id="selection">selected text</div><script type="module" src="/app.js"></script>');});t.after(()=>frontend.close());
  const browser=await chromium.launch({headless:true});t.after(()=>browser.close());const page=await browser.newPage();const errors=[];page.on("pageerror",error=>errors.push(error.message));await page.goto(frontend.url);await page.waitForFunction(()=>typeof window.prepareBrowserJourney==="function");
  const tools=[{name:"lookup",description:"Read the current selection",input_schema:{type:"object",properties:{id:{type:"string"}},required:["id"],additionalProperties:false}}];
  const access=await a.api.clients.grant({origin:frontend.url,expiresAt:Math.floor(Date.now()/1000)+60,session:{harness:"pi",config:{model:"openai/gpt-4.1-mini",tools:[]},model_credential:{api_key:a.modelKey}},tools});assert.ok(!JSON.stringify(access).includes(a.modelKey));
  await page.evaluate(input=>window.prepareBrowserJourney(input),{baseUrl:f.url,access});await page.click("#attach");await page.waitForFunction(()=>window.browserResult || window.browserFailure);assert.equal(await page.evaluate(()=>window.browserFailure),undefined);const result=await page.evaluate(()=>window.browserResult);assert.ok(result.transcript.some(event=>JSON.stringify(event).includes("selected text-42")),JSON.stringify(result));
  await page.evaluate(()=>window.disconnectBrowserJourney());const composition=await page.evaluate(()=>window.browserComposition());assert.equal(composition.session,result.id);await page.click("#attach");await page.waitForFunction(()=>window.browserReattached);assert.equal(await page.evaluate(()=>window.browserFailure),undefined);
  await f.db.client.query("UPDATE browser_grants SET expires_at=0 WHERE id=$1",[access.id]);await page.evaluate(()=>window.disconnectBrowserJourney());await page.click("#attach");await page.waitForFunction(()=>window.browserFailure);assert.equal(errors.length,0,JSON.stringify(errors));
  const handle=await a.api.sessions.get(result.id);await handle.delete();
});


test("native application tools commit a finish through the stateless controller, survive restart and erase their cached payloads on deletion",{timeout:120000},async t=>{
  const credential="application-private-credential-00000000000000",descriptor={name:"read",description:"Read",input_schema:{type:"object"},output_schema:{type:"integer"}};let runs=0,lastRequest;
  const f=await fixture(t,async({config,runtime,tokens,url,operator})=>{
    const control=controller(operator,tokens.tools),journal=new RemoteJournal(control);
    const handler=sdk.createApplicationHandler({credential,gatewayOrigin:"https://api.example",tools:[{...descriptor,async run(_args,context){runs++;await context.progress({ready:true});await context.finish(42);throw new Error("caller exited after its committed finish");}}],fetch:async(callback,options)=>{
      const target=new URL(callback);const response=await fetch(url+target.pathname,options);if(response.ok && JSON.parse(options.body).type==="finish")throw new Error("public acknowledgement was lost");return response;
    }});
    const apps=new Applications({journal,callbackOrigin:"https://api.example",outbound:async(endpoint,options)=>{assert.equal(endpoint,"https://application.example/tools");lastRequest=options.body;return (await handler(new Request(endpoint,{method:"POST",headers:{authorization:`Bearer ${options.token}`},body:JSON.stringify(options.body),signal:options.signal}))).json();}});
    const server=toolServer({token:tokens.sign,control,providers:{application:apps}});server.listen(0,"127.0.0.1");await once(server,"listening");t.after(()=>new Promise(resolve=>{server.closeAllConnections();server.close(resolve);}));
    config.application_tool_url=`http://127.0.0.1:${server.address().port}`;config.managed_providers.application={public:true,operations:[],active_per_account:10,profiles:{}};runtime.managed_providers.application={url:config.application_tool_url,tools:[]};return {apps,journal,control};
  });
  const account=await f.account();const session=await account.api.sessions.create({harness:"pi",model:sdk.Model("openai/gpt-4.1-mini",sdk.ApiKey(account.modelKey)),tools:[sdk.Application({endpoint:"https://application.example/tools",credential,tools:[descriptor]})]});assert.equal((await session.send("CALL application_read {}",{idempotencyKey:"application-once"})).output,"tool handled");assert.equal(runs,1);
  const grant=(await f.db.client.query("SELECT id FROM tool_grants WHERE session=$1 AND provider='application'",[session.id])).rows[0].id;
  const saved=(await f.db.client.query("SELECT value FROM tool_service_jobs WHERE value::jsonb->>'grant'=$1",[grant])).rows;assert.equal(saved.length,1);assert.ok(saved[0].value.includes("42"));assert.ok(!saved[0].value.includes(credential));
  const restarted=new Applications({journal:f.services.journal,callbackOrigin:"https://api.example",outbound:()=>{throw new Error("a committed action must not be repeated");}});
  assert.equal((await restarted.invoke("read",{tenant:account.account,session:session.id,parameters:{authorization:grant},credential:{token:credential}},lastRequest,{expires_ms:Date.now()+30000,parameters:{timeout_ms:10000}},new AbortController().signal)).structured_content,42);
  const before=await until(()=>account.api.account.modelUsage(session.id),usage=>usage.reported_input_tokens>=24 && usage.pending_estimate_micro_usd===0,"settled application usage");const backup=await f.backup();await f.restore(backup);assert.deepEqual(await account.api.account.modelUsage(session.id),before);
  await assert.rejects(session.delete(),error=>error.status===409);await f.services.apps.maintain(f.services.control);await session.delete();const remaining=await f.db.client.query("SELECT value FROM tool_service_jobs WHERE value::jsonb->>'grant'=$1",[grant]);assert.equal(remaining.rows.length,0);assert.equal(runs,1);
});

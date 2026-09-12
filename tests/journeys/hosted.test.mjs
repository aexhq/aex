import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer } from "node:http";
import { mkdtemp, readFile, writeFile, rm, readdir, stat, cp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { createHash, randomUUID } from "node:crypto";
import test from "node:test";
import { Aex, agentloop, brainEnv, component, hostEnv, tool } from "@aexhq/sdk";
import { database } from "./database.mjs";
import { pi } from "@aexhq/agentloop-pi";
import { codex } from "@aexhq/agentloop-codex";
import { z } from "zod";
import { measure } from "../../tools/benchmark.mjs";

const pause = (ms) => new Promise(r => setTimeout(r, ms));
async function port() { const s = createServer(); s.listen(0,"127.0.0.1"); await once(s,"listening"); const p=s.address().port; await new Promise(r=>s.close(r)); return p; }
async function size(path) { let bytes=0; for (const name of await readdir(path)) { const p=join(path,name), s=await stat(p); bytes+=s.isDirectory()?await size(p):s.size; } return bytes; }

test("published SDK: tenant isolation, host tool, replay, revocation, restart and backup restore", {timeout:180_000}, async t => {
  for (const name of ["AEX_TEST_SERVER","BRAIN_TEST_SERVER","BRAIN_TEST_WORKER","BRAIN_TEST_REFERENCE_AGENTLOOP"]) assert.ok(process.env[name], `${name} required`);
  const db=await database();
  const directory=await mkdtemp(join(tmpdir(),"aex-journey-"));
  const brainPort=await port(), aexPort=await port(), adminPort=await port();
  const brainUrl=`http://127.0.0.1:${brainPort}`, baseUrl=`http://127.0.0.1:${aexPort}`, adminUrl=`http://127.0.0.1:${adminPort}`;
  const internal=randomUUID(), operator=randomUUID();
  let modelCalls=0, toolCalls=0, modelDelay=0, running, brainChild, aexChild;
  let structuredAnswers;
  const processLogs=[];
  const children=new Set();
  const model=createServer(async(req,res)=>{
    const chunks=[]; for await(const c of req) chunks.push(c);
    const body=JSON.parse(Buffer.concat(chunks)); modelCalls++;
    assert.equal(req.headers.authorization,"Bearer test-model-secret");
    if(modelDelay) await pause(modelDelay);
    if (structuredAnswers) {
      assert.ok(structuredAnswers.length, "unexpected structured-output model call");
      assert.equal(body.response_format, undefined);
      res.writeHead(200,{"content-type":"text/event-stream"});
      res.end(`data: ${JSON.stringify({choices:[{index:0,delta:{content:structuredAnswers.shift()},finish_reason:"stop"}]})}\n\ndata: [DONE]\n\n`);
      return;
    }
    res.writeHead(200,{"content-type":"text/event-stream"});
    const delta=body.tools?.length && body.messages.at(-1).role!=="tool"
      ? {tool_calls:[{index:0,id:"lookup-1",type:"function",function:{name:"lookup",arguments:'{"id":"42"}'}}]} : {content:"answered"};
    res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:delta.tool_calls?"tool_calls":"stop"}]})}\n\ndata: [DONE]\n\n`);
  });
  model.listen(0,"127.0.0.1"); await once(model,"listening");
  const configuration=JSON.parse(await readFile(new URL("../../examples/config.json",import.meta.url)));
  Object.assign(configuration,{listen:`127.0.0.1:${aexPort}`,operator_listen:`127.0.0.1:${adminPort}`,data_dir:join(directory,"aex"),brain_url:brainUrl,
    models:["vercel-ai-gateway/test/journey"],agentloops:[createHash("sha256").update(await readFile(process.env.BRAIN_TEST_REFERENCE_AGENTLOOP)).digest("hex")]});
  configuration.limits.minimum_free_disk_bytes=1;
  await writeFile(join(directory,"config.json"),JSON.stringify(configuration));
  async function launch(executable,args,env,url){
    const child=spawn(executable,args,{env:{...process.env,...env},stdio:["ignore","pipe","pipe"],detached:true}); children.add(child);
    let logs="";const capture=c=>{logs+=c;processLogs.push(String(c));};child.stdout.on("data",capture);child.stderr.on("data",capture);
    for(let i=0;i<600;i++){ if(child.exitCode!==null)throw new Error(`process exited: ${logs}`);try{if((await fetch(url+"/health/live")).ok)return child;}catch{}await pause(50); }
    throw new Error(`readiness timeout: ${logs}`);
  }
  async function stop(child,signal="SIGTERM"){ if(child.exitCode===null && child.signalCode===null){const done=once(child,"exit");process.kill(-child.pid,signal);await done;}children.delete(child); }
  async function start(){
    brainChild=await launch(process.env.BRAIN_TEST_SERVER,[],{XDG_CACHE_HOME:join(directory,"compilation-cache"),BRAIN_LISTEN:`127.0.0.1:${brainPort}`,BRAIN_DATA_DIR:join(directory,"brain"),BRAIN_ENV_WORKER:process.env.BRAIN_TEST_WORKER,BRAIN_API_TOKEN:internal,BRAIN_MODEL_BASE_URL:`http://127.0.0.1:${model.address().port}/v1`},brainUrl);
    aexChild=await launch(process.env.AEX_TEST_SERVER,["serve","--config",join(directory,"config.json")],{AEX_BRAIN_TOKEN:internal,AEX_OPERATOR_TOKEN:operator,AEX_SITE_TOKEN:"s".repeat(32),AEX_DATABASE_URL:db.url,RUST_LOG:"info"},baseUrl);
    await meter();
    await operate({action:"resume"});
  }
  async function operate(body){const r=await fetch(adminUrl+"/operate",{method:"POST",headers:{authorization:`Bearer ${operator}`,"content-type":"application/json"},body:JSON.stringify(body)});assert.ok(r.ok,`operator ${body.action}: ${r.status} ${await (!r.ok?r.text():Promise.resolve(""))}`);return r.json();}
  async function meter(){const sessions={};for(const id of await readdir(join(directory,"brain/sessions")))sessions[id]=await size(join(directory,"brain/sessions",id));await operate({action:"report_usage",observed_at:Math.floor(Date.now()/1000),sessions});}
  async function account(){const {account}=await operate({action:"create_account"});return {account,...await operate({action:"issue_key",account})};}
  const raw=(path,token,options={})=>fetch(baseUrl+path,{...options,headers:{authorization:`Bearer ${token}`,...options.headers}});
  t.after(async()=>{running?.pump.stop();await running?.pump.closed;for(const child of children)await stop(child,"SIGKILL");model.closeAllConnections();await new Promise(r=>model.close(r));await db.close();await rm(directory,{recursive:true,force:true});});
  await start();
  for (const [executable,args,env,expected] of [
    [process.env.AEX_TEST_SERVER,["serve","--config",join(directory,"config.json")],{AEX_BRAIN_TOKEN:internal,AEX_OPERATOR_TOKEN:operator},/already has a writer/],
    [process.env.BRAIN_TEST_SERVER,[],{BRAIN_DATA_DIR:join(directory,"brain"),BRAIN_API_TOKEN:internal},/already in use or cannot be locked/],
  ]) {
    const child=spawn(executable,args,{env:{...process.env,...env},stdio:["ignore","pipe","pipe"]});
    let output="";child.stdout.on("data",c=>output+=c);child.stderr.on("data",c=>output+=c);
    const [code]=await once(child,"exit");assert.notEqual(code,0);assert.match(output,expected);
  }
  const a=await account(),b=await account();
  const client=new Aex({baseUrl,apiKey:a.token}),other=new Aex({baseUrl,apiKey:b.token});
  const options={model:{provider:"vercel-ai-gateway",name:"test/journey",apiKey:"test-model-secret"},agentloop:agentloop({implementation:component(pathToFileURL(process.env.BRAIN_TEST_REFERENCE_AGENTLOOP))})({env:brainEnv({name:"brain"})})};
  const lookup=tool({name:"lookup",description:"Lookup",input:z.object({id:z.string()}),run:({id})=>{toolCalls++;return `item-${id}`;}});
  const session=await client.sessions.create({...options,tools:[lookup({env:hostEnv({name:"app"})})]},{idempotencyKey:"shared-key"});
  running=await client.register();
  const otherSession=await other.sessions.create(options,{idempotencyKey:"shared-key"});
  const direct=await measure(`${brainUrl}/v1/sessions/${otherSession.id}`,internal);
  const hosted=await measure(`${baseUrl}/v1/sessions/${otherSession.id}`,b.token);
  t.diagnostic(JSON.stringify({operation:"warm_session_read",direct,hosted,added_p95_ms:hosted.p95_ms-direct.p95_ms}));
  assert.notEqual(session.id,otherSession.id);
  for(const [method,suffix] of [["GET",""],["GET","/transcript"],["GET","/events"],["POST","/messages"],["POST","/cancel"],["POST","/end"],["DELETE",""]]){
    const r=await raw(`/v1/sessions/${session.id}${suffix}`,b.token,{method,headers:{"idempotency-key":"x","content-type":"application/json"},...(method==="POST"?{body:'{"input":{"message":"x"}}'}:{})});assert.equal(r.status,404,`${method} ${suffix}`);
  }
  const listed=await (await raw("/v1/sessions",b.token)).json();assert.deepEqual(listed.sessions.map(s=>s.session_id),[otherSession.id]);
  assert.equal((await raw(`/v1/sessions/${session.id}/executions/1/call`,a.token,{method:"POST"})).status,404);
  assert.equal((await raw("/v1/tools",a.token,{method:"POST"})).status,400);
  assert.equal((await raw("/v1/agentloops",a.token,{method:"POST",body:"unapproved",headers:{"idempotency-key":"bad"}})).status,400);
  await session.send("look it up");assert.equal(toolCalls,1);assert.equal(modelCalls,2);
  const hostedTool=tool({name:"lookup",description:"Lookup",input:z.object({id:z.string()}),implementation:component(pathToFileURL(process.env.BRAIN_TEST_TOOL))});
  const hostedSession=await client.sessions.create({...options,agentloop:pi({env:brainEnv({name:"brain"})}),tools:[hostedTool({env:brainEnv({name:"brain"})})]});
  await hostedSession.send("look it up on the server");
  const hostedEvents=[];for await(const event of hostedSession.events())hostedEvents.push(event);
  assert.ok(hostedEvents.some(e=>e.type==="tool_call_ended"),"published Pi and uploaded Tool run in hosted Brain");
  assert.equal(toolCalls,1,"hosted Tool does not execute the application function");
  await hostedSession.end();await hostedSession.delete();

  for (const loop of [pi, codex]) {
    await meter();
    const structured = await client.sessions.create({ ...options, agentloop: loop({ env: brainEnv({ name: "brain" }) }) });
    await meter();
    const before = modelCalls;
    structuredAnswers = ["not JSON", '{"age":"wrong"}', '{"age":37}'];
    assert.deepEqual(await structured.send("Extract Ada's age", { output: { type: z.object({ age: z.number() }) } }), { age: 37 });
    assert.equal(modelCalls - before, 3);
    assert.equal(structuredAnswers.length, 0);
    const reopened = await client.sessions.get(structured.id);
    const history = []; for await (const event of reopened.events()) history.push(event);
    assert.equal(history.filter(e => e.type === "turn_ended").length, 3);
    structuredAnswers = undefined;
    await structured.end(); await structured.delete();

    let returned;
    let calls = 0;
    const terminal = tool({ name: "lookup", description: "Read a remote result", input: z.object({ id: z.string() }),
      output: z.string(), run: () => { calls++; return returned; } });
    const outcomes = await client.sessions.create({ ...options, agentloop: loop({ env: brainEnv({ name: "brain" }) }),
      tools: [terminal({ env: hostEnv({ name: "app" }) })] });
    for (const outcome of [
      { status: "error", error: { code: "rate_limited", message: "Wait", retryable: true, details: { retry_after_ms: 1000 } } },
      { status: "unknown", message: "Connection closed after dispatch" }, { status: "timeout" }, { status: "cancelled" },
    ]) {
      returned = outcome;
      await outcomes.send("look it up once");
      const history = []; for await (const event of outcomes.events()) history.push(event);
      const result = history.filter(event => event.type === "tool_call_ended").at(-1).data.result;
      assert.equal(result.is_error, true);
      assert.equal(result.output.code, outcome.status === "error" ? "rate_limited" : outcome.status);
      if (outcome.status === "error") assert.deepEqual(result.output, outcome.error);
      assert.equal(history.some(event => event.type === "environment_unreachable"), false);
    }
    assert.equal(calls, 4);
    await outcomes.end(); await outcomes.delete();
  }
  await meter();

  const events=[];for await(const event of session.events())events.push(event);assert.ok(events.some(e=>e.type==="tool_call_ended"));
  let last=events.at(-1).sequence;const page=await (await raw(`/v1/sessions/${session.id}/events?after=${last}`,a.token)).json();assert.equal(page.events.length,0);
  await session.send("look it up again");assert.equal(toolCalls,2,"terminal turns release admission for the next message");
  const sweep=[];
  for(let i=0;i<8;i++) sweep.push(await other.sessions.create(options,{idempotencyKey:`sweep-${i}`}));
  modelDelay=250;
  for(const concurrency of [1,2,4,8]) {
    await meter();
    const started=performance.now();
    const results=await Promise.all(sweep.slice(0,concurrency).map(async(s,i)=>{
      const r=await raw(`/v1/sessions/${s.id}/messages`,b.token,{method:"POST",headers:{"content-type":"application/json","idempotency-key":`sweep-${concurrency}-${i}`},body:JSON.stringify({input:{message:"x".repeat(concurrency===8?65536:1024)}})});
      await r.arrayBuffer();return r.status;
    }));
    assert.equal(results.filter(s=>s===200).length,Math.min(concurrency,configuration.limits.active_turns_per_account));
    assert.ok(results.every(s=>s===200||s===503));
    t.diagnostic(JSON.stringify({operation:"turn_admission_sweep",concurrency,statuses:results,elapsed_ms:performance.now()-started,brain_bytes:await size(join(directory,"brain"))}));
  }
  modelDelay=0;
  const subscribers=[];
  for(let i=0;i<configuration.limits.streams_per_account;i++) {
    const abort=new AbortController();
    const r=await raw(`/v1/sessions/${otherSession.id}/events`,b.token,{headers:{accept:"text/event-stream"},signal:abort.signal});assert.equal(r.status,200);
    subscribers.push({abort,body:r.body});
  }
  assert.equal((await raw(`/v1/sessions/${otherSession.id}/events`,b.token,{headers:{accept:"text/event-stream"}})).status,503,"unread subscribers stay within the account limit");
  for(const subscriber of subscribers){subscriber.abort.abort();await subscriber.body.cancel().catch(()=>{});}
  const premature=await raw(`/v1/sessions/${sweep[0].id}`,b.token,{method:"DELETE",headers:{"idempotency-key":"premature-delete"}});
  assert.equal(premature.status,400);
  assert.equal((await raw(`/v1/sessions/${sweep[0].id}`,b.token)).status,200,"rejected deletion must not hide a live session");
  for(const s of sweep){await s.end();await s.delete();}
  last=(await (await raw(`/v1/sessions/${session.id}/events`,a.token)).json()).next_cursor;
  await meter();
  const controller=new AbortController();const feed=await raw(`/v1/sessions/${session.id}/events?after=${last}`,a.token,{headers:{accept:"text/event-stream"},signal:controller.signal});const reader=feed.body.getReader();await reader.read();
  await operate({action:"revoke_key",key:a.key});
  const closed=await Promise.race([(async()=>{while(!(await reader.read()).done){}return true;})(),pause(3000).then(()=>false)]);assert.ok(closed,"revocation closes open SSE");controller.abort();
  assert.equal((await raw(`/v1/sessions/${session.id}`,a.token)).status,401);
  running.pump.stop();await running.pump.closed;running=null;
  await stop(aexChild,"SIGKILL");await stop(brainChild,"SIGKILL");
  const productBackup=await db.snapshot();await cp(join(directory,"aex"),join(directory,"backup-aex"),{recursive:true});await cp(join(directory,"brain"),join(directory,"backup-brain"),{recursive:true,filter:source=>source!==join(directory,"brain/run")});
  await start();assert.equal((await raw(`/v1/sessions/${session.id}`,a.token)).status,401);
  const replacement=await operate({action:"issue_key",account:a.account});
  const history=await (await raw(`/v1/sessions/${session.id}/events`,replacement.token)).json();assert.equal(history.next_cursor,last);
  await otherSession.end();await otherSession.delete();
  assert.equal((await raw(`/v1/sessions/${otherSession.id}`,b.token)).status,404);
  await stop(aexChild);await stop(brainChild);
  await rm(join(directory,"aex"),{recursive:true});await rm(join(directory,"brain"),{recursive:true});
  await db.restore(productBackup);await cp(join(directory,"backup-aex"),join(directory,"aex"),{recursive:true});await cp(join(directory,"backup-brain"),join(directory,"brain"),{recursive:true});
  await start();assert.equal((await raw(`/v1/sessions/${session.id}`,a.token)).status,401,"restored key remains revoked");
  assert.equal((await raw(`/v1/sessions/${session.id}`,replacement.token)).status,401,"post-backup credentials do not exist in restore");
  assert.equal((await raw(`/v1/sessions/${otherSession.id}`,b.token)).status,200,"restore requires post-backup deletion reconciliation before reopening");
  await db.client.query("UPDATE sessions SET created=0 WHERE id=$1",[otherSession.id]);
  assert.equal((await operate({action:"maintain"})).deleted,1,"retention ends and removes expired sessions");
  assert.equal((await raw(`/v1/sessions/${otherSession.id}`,b.token)).status,404);
  const logs=processLogs.join("");
  for(const secret of ["test-model-secret",internal,operator,a.token,b.token,replacement.token]) assert.equal(logs.includes(secret),false,"service logs must not contain credentials");
  assert.ok(logs.includes('"request_id"'),"redacted request correlation is emitted");
});

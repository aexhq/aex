import assert from "node:assert/strict";
import test from "node:test";
import { Aex, Application, Model, ApiKey, createApplicationHandler } from "../dist/index.js";
const descriptor={name:"read",description:"Read",input_schema:{type:"object",properties:{id:{type:"string"}},required:["id"],additionalProperties:false},output_schema:{type:"integer"}};
test("application placement composes one managed provider without installing customer code or preparing a host",async()=>{
  const calls=[];const aex=new Aex({apiKey:"account-key",fetch:async(url,init)=>{calls.push({url:String(url),init});return Response.json({id:"s",status:"idle"});}});
  await aex.sessions.create({harness:"pi",model:Model("openai/gpt-4.1-mini",ApiKey("private-model-key")),tools:[Application({endpoint:"https://app.example/tools",credential:"private-application-key".repeat(2),tools:[descriptor]})]},{idempotencyKey:"create-once"});
  assert.equal(calls.length,1);assert.equal(new URL(calls[0].url).pathname,"/sessions");assert.equal(new Headers(calls[0].init.headers).get("idempotency-key"),"create-once");const body=JSON.parse(calls[0].init.body);assert.equal(body.model_credential.api_key,"private-model-key");assert.equal(body.config.bindings[0].provider,"application");assert.deepEqual(body.config.bindings[0].operations,["read"]);assert.equal(body.credentials[0].binding,"application");assert.equal(body.config.bindings[0].parameters.credential,undefined);await aex.close();
});
test("backend browser grants carry private composition only to the backend; the browser reads its bound session explicitly",async()=>{
  const create={harness:"pi",config:{model:"openai/gpt-4.1-mini",tools:[]},model_credential:{api_key:"private-model-key"}};
  const access={id:"grant",token:"browser-key",expires_at:2000000000,session:{harness:"pi",config:create.config},tools:[]};const calls=[];
  const backend=new Aex({apiKey:"private-account-key",fetch:async(url,init)=>{calls.push({url:String(url),init});return Response.json(access);}});
  assert.deepEqual(await backend.clients.grant({session:create,origin:"https://customer.example",tools:[],expiresAt:access.expires_at}),access);assert.equal(JSON.parse(calls[0].init.body).session.model_credential.api_key,"private-model-key");
  const browser=new Aex({clientAccess:access,fetch:async(url,init)=>{calls.push({url:String(url),init});return Response.json({create:access.session,session:"existing",tools:[]});}});assert.equal((await browser.clients.composition()).session,"existing");assert.equal(new Headers(calls.at(-1).init.headers).get("authorization"),"Bearer browser-key");assert.throws(()=>browser.clients.grant({session:create,origin:"https://customer.example"}),/backend API key/);await backend.close();await browser.close();
});
test("invalid application placement and unsupported hosted transports fail before effects",async()=>{
  assert.throws(()=>Application({endpoint:"http://169.254.169.254/tools",credential:"x".repeat(32),tools:[descriptor]}),/HTTPS/);
  assert.throws(()=>Application({endpoint:"https://app.example/tools",credential:"short",tools:[descriptor]}),/credential/);
  let calls=0;const aex=new Aex({apiKey:"key",fetch:async()=>{calls++;throw new Error("unexpected effect");}});await assert.rejects(aex.sessions.create({harness:"pi",config:{model:"openai/gpt-4.1-mini",tools:[]},runtime_transport:"rpc"}),/SSE/);assert.equal(calls,0);await aex.close();
});
test("application handlers validate inputs and output, scope callbacks, and wait for durable finish acknowledgements",async()=>{
  const credential="x".repeat(32),callbacks=[];let runs=0;
  const handler=createApplicationHandler({credential,gatewayOrigin:"https://api.example",tools:[{...descriptor,async run(input,context){runs++;assert.equal(context.scope.tenant_id,"tenant");await context.progress({ready:true});await context.finish(42);return 99;}}],fetch:async(url,init)=>{assert.equal(new URL(url).origin,"https://api.example");assert.equal(new Headers(init.headers).has("authorization"),false);callbacks.push(JSON.parse(init.body));return Response.json({committed:true});}});
  const invocation={tenant_id:"tenant",session_id:"session",call_id:"call",tool:descriptor,arguments:{id:"42"},deadline_at_ms:Date.now()+20000,completion:{url:`https://api.example/v1/tool-completions/${"a".repeat(64)}`}};
  const request=(body,token=credential)=>new Request("https://app.example/tools",{method:"POST",headers:{authorization:`Bearer ${token}`},body:JSON.stringify(body)});
  assert.equal((await handler(request(invocation,"wrong"))).status,401);
  assert.equal((await(await handler(request({...invocation,completion:{url:`https://evil.example/v1/tool-completions/${"a".repeat(64)}`}}))).json()).is_error,true);assert.equal(runs,0);
  assert.equal((await(await handler(request({...invocation,arguments:{id:1}}))).json()).is_error,true);assert.equal(runs,0);
  const response=await(await handler(request(invocation))).json();assert.equal(response.structured_content,42);assert.equal(runs,1);assert.deepEqual(callbacks.map(value=>value.type),["progress","finish"]);assert.equal(callbacks[1].result.structured_content,42);
});

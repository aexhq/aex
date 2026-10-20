import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtemp,rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHmac } from 'node:crypto';
import { Journal,authenticate } from './common.mjs';
import { Sandboxes } from './sandbox.mjs';
import { Applications } from './application.mjs';
import { publicAddress,publicJSON } from './outbound.mjs';
import { Browsers } from './browser.mjs';
import { toolServer } from './server.mjs';
import { once } from 'node:events';

async function journal(t){const directory=await mkdtemp(join(tmpdir(),'aex-tools-test-'));const journal=new Journal(directory);t.after(async()=>{journal.close();await rm(directory,{recursive:true,force:true});});return journal;}
test('paired capture waits for callbacks and maintenance, rejects new writes, and can resume',async t=>{
  let enter,finish;const entered=new Promise(resolve=>enter=resolve),finished=new Promise(resolve=>finish=resolve),token='private-control-token-000000000000';
  const server=toolServer({token,control:()=>{},providers:{application:{async complete(){enter();await finished;return {};}}}});
  server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>new Promise(resolve=>{server.closeAllConnections();server.close(resolve);}));
  const url=`http://127.0.0.1:${server.address().port}`,post=(path,auth=false)=>fetch(url+path,{method:'POST',headers:{...(auth?{authorization:`Bearer ${token}`}:{})},body:'{}'});
  const callback=post('/complete/'+'a'.repeat(64));await entered;
  assert.equal((await post('/control/quiesce')).status,401);
  let quiesced=false;const capture=post('/control/quiesce',true).then(response=>{quiesced=true;return response;});
  while(!(await(await fetch(url+'/health')).json()).paused)await new Promise(resolve=>setTimeout(resolve,5));
  assert.equal(quiesced,false);assert.equal((await post('/complete/'+'b'.repeat(64))).status,503);
  let maintained=false;await server.maintain(()=>{maintained=true;});assert.equal(maintained,false);
  finish();assert.equal((await callback).status,200);assert.deepEqual(await(await capture).json(),{paused:true,active:0});
  assert.equal((await post('/control/resume',true)).status,200);await server.maintain(()=>{maintained=true;});assert.equal(maintained,true);
});
test('signed tool credentials bind identity and reject tampering',()=>{
  const key='secret'.repeat(8),body=Buffer.from(JSON.stringify({tenant:'a',session:'s',provider:'sandbox',alias:'compute',parameters:{authorization:'grant'}})).toString('base64url');
  const signature=createHmac('sha256',key).update(body).digest('base64url');
  assert.equal(authenticate(`Bearer ${body}.${signature}`,key).tenant,'a');
  assert.throws(()=>authenticate(`Bearer ${body}a.${signature}`,key));assert.throws(()=>authenticate(`Bearer ${body}.${signature}`,'wrong'));
});
test('application destinations deny private, reserved and mapped addresses before dispatch',()=>{
  for(const address of ['127.0.0.1','10.0.0.1','169.254.169.254','192.168.1.1','::1','::ffff:127.0.0.1','2001:db8::1','224.0.0.1'])assert.equal(publicAddress(address),false,address);
  assert.equal(publicAddress('8.8.8.8'),true);assert.equal(publicAddress('2606:4700:4700::1111'),true);
  for(const url of ['https://127.0.0.1/action','http://example.com/action','https://example.com:8443/action','https://a:b@example.com/action'])assert.throws(()=>publicJSON(url,{token:'x',body:{},signal:AbortSignal.timeout(1000)}));
});
test('sandbox dispatch and money remain unresolved across a lost allocation acknowledgement and restart',async t=>{
  const state=await journal(t);let creates=0,terminated=0;const reports=[];
  const provider={create:async()=>{creates++;throw new Error('acknowledgement lost');},find:async()=>['original-resource'],terminate:async()=>{terminated++;}};
  const control=async(path,body)=>{if(path==='pending')return {grants:[{authorization:'g',tenant:'a',session:'s'}]};reports.push(body);};
  const context={tenant:'a',session:'s',parameters:{authorization:'g'}},request={call_id:'call',arguments:{}};
  const first=new Sandboxes({journal:state,provider,control,now:()=>1000});
  assert.equal((await first.invoke('create',context,request,{expires_ms:10000,profile:{}})).unknown_outcome,true);
  const restarted=new Sandboxes({journal:state,provider,control,now:()=>2000});
  assert.equal((await restarted.invoke('create',context,request,{expires_ms:10000,profile:{}})).unknown_outcome,true);
  await restarted.maintain();assert.equal(creates,1);assert.equal(terminated,1);assert.equal(reports.length,0);assert.equal(state.get('g').phase,'usage_unknown');
});
test('sandbox actions are owned, dispatched once and settled cumulatively only after confirmed termination',async t=>{
  const state=await journal(t),reports=[];let now=1000,commands=0;
  const provider={create:async()=> 'resource',exec:async()=>{commands++;throw new Error('lost output');},terminate:async()=>{},find:async()=>[]};
  const control=async(path,body)=>{if(path==='pending')return {grants:[]};reports.push(body);};
  const sandboxes=new Sandboxes({journal:state,provider,control,now:()=>now});
  const context={tenant:'a',session:'s',parameters:{authorization:'g'}},request={call_id:'call',arguments:{sandbox:'g',command:'effect'}};
  assert.deepEqual((await sandboxes.invoke('create',context,request,{expires_ms:10000,profile:{}})).structured_content,{sandbox:'g'});
  assert.equal((await sandboxes.invoke('exec',{...context,session:'other'},request,{})).is_error,true);
  for(let i=0;i<2;i++)assert.equal((await sandboxes.invoke('exec',context,request,{})).unknown_outcome,true);
  assert.equal(commands,1);now=3500;await sandboxes.invoke('terminate',context,request,{});
  assert.equal(reports.at(-1).units_ms,2500);assert.equal(reports.at(-1).terminal,true);assert.equal(state.get('g').phase,'settled');
});
test('application finish acknowledgement is durable before a caller exits and cannot change on replay',async t=>{
  const state=await journal(t);let outbound,started;const called=new Promise(resolve=>started=resolve),progress=[];
  const apps=new Applications({journal:state,callbackOrigin:'https://api.example',outbound:async(url,options)=>{outbound=options;started();return new Promise((resolve,reject)=>options.signal.addEventListener('abort',()=>reject(new Error('caller exited')),{once:true}));}});
  const context={tenant:'a',session:'s',parameters:{authorization:'g'},credential:{token:'private-application-key-00000000000000'}};
  const request={call_id:'call',arguments:{}};
  const admitted={expires_ms:Date.now()+30000,parameters:{endpoint:'https://application.example/tools',timeout_ms:10000,tools:[{name:'read'}]}};
  const pending=apps.invoke('read',context,request,admitted,new AbortController().signal,value=>progress.push(value));await called;
  const token=new URL(outbound.body.completion.url).pathname.split('/').at(-1),value={content:[{type:'text',text:'committed'}]};
  await apps.complete(token,{type:'progress',value:{step:1}});assert.deepEqual(progress,[{step:1}]);
  assert.deepEqual(await apps.complete(token,{type:'finish',result:value}),{committed:true});assert.deepEqual(await pending,value);
  const restarted=new Applications({journal:state,callbackOrigin:'https://api.example',outbound:()=>{throw new Error('must not redispatch');}});
  assert.deepEqual(await restarted.invoke('read',context,request,admitted,new AbortController().signal),value);
  await assert.rejects(restarted.complete(token,{type:'finish',result:{content:[]}}),/already committed/);
  assert.equal(JSON.stringify(state.values()).includes('private-application-key'),false);
});

test('browser deletion waits for confirmed context cleanup and retries outages without another action',async t=>{
  const state=await journal(t),calls=[],reports=[];let unavailable=false;
  const control=async(path,body)=>{if(path==='pending')return {grants:[{provider:'browser',authorization:'g',tenant:'a',session:'s'}]};reports.push(body);};
  const browser=new Browsers({journal:state,url:'http://127.0.0.1:3005',token:'b'.repeat(32),control,fetcher:async(url,init)=>{assert.equal(url,'http://127.0.0.1:3005/browser');const input=JSON.parse(init.body);calls.push(input);if(unavailable)throw new Error('outage');return Response.json({content:[{type:'text',text:'ok'}]});}});
  await browser.invoke('browse',{tenant:'a',session:'s',parameters:{authorization:'g'}},{tenant_id:'a',session_id:'s',call_id:'c',arguments:{action:'read'}},{},new AbortController().signal);
  unavailable=true;await assert.rejects(browser.maintain());assert.equal(reports.length,0);assert.ok(state.get('g'));
  unavailable=false;await browser.maintain();assert.equal(calls.at(-1).arguments.action,'close');assert.equal(reports.length,1);assert.equal(state.get('g'),undefined);
});

test('application deletion removes cached results before confirming cleanup',async t=>{
  const state=await journal(t);state.set('application:call',{id:'application:call',grant:'g',result:{content:[{type:'text',text:'private output'}]}});
  const apps=new Applications({journal:state,callbackOrigin:'https://api.example'});let confirmed=false;
  await apps.maintain(async(path,body)=>{if(path==='pending')return {grants:[{provider:'application',authorization:'g',tenant:'a',session:'s'}]};assert.equal(body.authorization,'g');assert.ok(!JSON.stringify(state.values()).includes('private output'));confirmed=true;});assert.equal(confirmed,true);
});

for(const provider of ['sandbox','browser','application'])test(`${provider} sends response headers while its result is still pending`,async t=>{
  let finish;const finished=new Promise(resolve=>finish=resolve),token='private-control-token-000000000000';
  const scope={tenant:'a',session:'s',provider,alias:'tool',parameters:{authorization:'grant'}};
  const body=Buffer.from(JSON.stringify(scope)).toString('base64url'),signature=createHmac('sha256',token).update(body).digest('base64url');
  const value={content:[{type:'text',text:'confirmed'}]};
  const server=toolServer({token,control:async()=>({}),providers:{[provider]:{invoke:async()=>{await finished;return value;}}}});
  server.listen(0,'127.0.0.1');await once(server,'listening');t.after(()=>new Promise(resolve=>{finish();server.closeAllConnections();server.close(resolve);}));
  const response=await fetch(`http://127.0.0.1:${server.address().port}/${provider}/run`,{method:'POST',headers:{authorization:`Bearer ${body}.${signature}`,'content-type':'application/json'},body:JSON.stringify({tenant_id:'a',session_id:'s',call_id:'call',arguments:{}}),signal:AbortSignal.timeout(1000)});
  assert.equal(response.status,200);assert.equal(response.headers.get('content-type'),'text/event-stream');
  finish();assert.equal(await response.text(),`event: result\ndata: ${JSON.stringify(value)}\n\n`);
});
